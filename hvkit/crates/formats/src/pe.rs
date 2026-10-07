//! 32-bit PE images, read and patched in place in the file's bytes.
//!
//! A [`Pe`] holds only the parsed layout (offsets and header values); the bytes stay with the
//! caller, so the same layout can be used to read one buffer and write another. The image may start
//! anywhere in the file (NTLDR and SETUPLDR.BIN carry it behind a 16-bit startup module), so every
//! file offset here is absolute: `start` plus the offset inside the image.

use crate::{Result, bail, u16_at, u32_at};

pub const IMAGE_FILE_MACHINE_I386: u16 = 0x14c;
pub const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
pub const IMAGE_DIRECTORY_ENTRY_IMPORT: usize = 1;
pub const IMAGE_DIRECTORY_ENTRY_RESOURCE: usize = 2;
pub const IMAGE_DIRECTORY_ENTRY_BASERELOC: usize = 5;
const RT_VERSION: u32 = 16;

#[derive(Debug, Clone)]
pub struct Section {
    pub index: usize,
    /// File offset of the section header.
    pub header: usize,
    pub name: String,
    pub virtual_size: u32,
    pub rva: u32,
    pub raw_size: u32,
    /// PointerToRawData, relative to the image start.
    pub raw: u32,
    pub characteristics: u32,
}

#[derive(Debug, Clone)]
pub struct Pe {
    /// File offset of the image ('MZ').
    pub start: usize,
    /// File offset of the 'PE\0\0' signature.
    pub nt: usize,
    pub number_of_sections: u16,
    /// File offset of the optional header.
    pub opt: usize,
    pub opt_size: u16,
    pub image_base: u32,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub sections: Vec<Section>,
}

/// One IMAGE_IMPORT_DESCRIPTOR.
#[derive(Debug, Clone)]
pub struct ImportDescriptor {
    /// File offset of the 20-byte descriptor.
    pub offset: usize,
    pub original_first_thunk: u32,
    pub name_rva: u32,
    pub first_thunk: u32,
    pub dll: String,
}

impl Pe {
    /// Parses a PE32 image whose 'MZ' header is at file offset `start`.
    pub fn parse(b: &[u8], start: usize) -> Result<Pe> {
        if b.get(start..start + 2) != Some(b"MZ") {
            bail!("no 'MZ' header at 0x{start:x}");
        }
        let nt = start + u32_at(b, start + 0x3c)? as usize;
        if u32_at(b, nt)? != 0x4550 {
            bail!("no PE header");
        }
        let number_of_sections = u16_at(b, nt + 6)?;
        let opt_size = u16_at(b, nt + 20)?;
        let opt = nt + 24;
        if u16_at(b, opt)? != 0x10b {
            bail!("not a PE32 image");
        }
        let mut pe = Pe {
            start,
            nt,
            number_of_sections,
            opt,
            opt_size,
            image_base: u32_at(b, opt + 28)?,
            section_alignment: u32_at(b, opt + 32)?,
            file_alignment: u32_at(b, opt + 36)?,
            size_of_image: u32_at(b, opt + 56)?,
            size_of_headers: u32_at(b, opt + 60)?,
            sections: Vec::new(),
        };
        let sec0 = opt + opt_size as usize;
        for index in 0..number_of_sections as usize {
            let h = sec0 + 40 * index;
            let Some(raw_name) = b.get(h..h + 8) else {
                bail!("section table past the end of the file")
            };
            pe.sections.push(Section {
                index,
                header: h,
                name: String::from_utf8_lossy(raw_name)
                    .trim_end_matches('\0')
                    .to_string(),
                virtual_size: u32_at(b, h + 8)?,
                rva: u32_at(b, h + 12)?,
                raw_size: u32_at(b, h + 16)?,
                raw: u32_at(b, h + 20)?,
                characteristics: u32_at(b, h + 36)?,
            });
        }
        Ok(pe)
    }

    pub fn machine(&self, b: &[u8]) -> Result<u16> {
        u16_at(b, self.nt + 4)
    }

    /// File offset of OptionalHeader.CheckSum.
    pub fn checksum_offset(&self) -> usize {
        self.opt + 64
    }

    /// File offset of the section table's end, where a new header would go.
    pub fn section_table_end(&self) -> usize {
        self.opt + self.opt_size as usize + 40 * self.sections.len()
    }

    /// File offset of data directory entry `i` (RVA, then size).
    pub fn data_directory_offset(&self, i: usize) -> usize {
        self.opt + 96 + 8 * i
    }

    pub fn data_directory(&self, b: &[u8], i: usize) -> Result<(u32, u32)> {
        let o = self.data_directory_offset(i);
        Ok((u32_at(b, o)?, u32_at(b, o + 4)?))
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// File offset of an RVA that has file data (inside some section's raw data).
    pub fn rva_to_offset(&self, rva: u32) -> Result<usize> {
        for s in &self.sections {
            if s.raw != 0 && rva >= s.rva && rva - s.rva < s.raw_size {
                return Ok(self.start + (s.raw + rva - s.rva) as usize);
            }
        }
        bail!("RVA 0x{rva:x} is in no section's file data")
    }

    /// File offset of a virtual address.
    pub fn va_to_offset(&self, va: u32) -> Result<usize> {
        self.rva_to_offset(va.wrapping_sub(self.image_base))
    }

    /// The NUL-terminated ASCII string at an RVA.
    pub fn str_at(&self, b: &[u8], rva: u32) -> Result<String> {
        let o = self.rva_to_offset(rva)?;
        let Some(len) = b[o..].iter().position(|&c| c == 0) else {
            bail!("unterminated string at RVA 0x{rva:x}")
        };
        Ok(String::from_utf8_lossy(&b[o..o + len]).into_owned())
    }

    /// The import descriptors, up to the all-zero terminator.
    pub fn imports(&self, b: &[u8]) -> Result<Vec<ImportDescriptor>> {
        let (rva, _) = self.data_directory(b, IMAGE_DIRECTORY_ENTRY_IMPORT)?;
        let mut out = Vec::new();
        let mut o = self.rva_to_offset(rva)?;
        loop {
            let (oft, name_rva, ft) = (u32_at(b, o)?, u32_at(b, o + 12)?, u32_at(b, o + 16)?);
            if oft == 0 && name_rva == 0 && ft == 0 {
                return Ok(out);
            }
            out.push(ImportDescriptor {
                offset: o,
                original_first_thunk: oft,
                name_rva,
                first_thunk: ft,
                dll: self.str_at(b, name_rva)?,
            });
            o += 20;
        }
    }

    /// The function a thunk value imports by name (`None` for an ordinal import).
    pub fn thunk_name(&self, b: &[u8], thunk: u32) -> Result<Option<String>> {
        if thunk & 0x8000_0000 != 0 {
            return Ok(None);
        }
        Ok(Some(self.str_at(b, thunk + 2)?))
    }

    /// File offsets of the base relocation entries (type << 12 | page offset) that apply to `rva`.
    pub fn base_relocations_at(&self, b: &[u8], rva: u32) -> Result<Vec<usize>> {
        let (dir, size) = self.data_directory(b, IMAGE_DIRECTORY_ENTRY_BASERELOC)?;
        let mut out = Vec::new();
        if size == 0 {
            return Ok(out);
        }
        let mut p = self.rva_to_offset(dir)?;
        let end = p + size as usize;
        while p < end {
            let page = u32_at(b, p)?;
            let block = u32_at(b, p + 4)? as usize;
            if block < 8 {
                bail!("bad base relocation block at 0x{p:x}");
            }
            if page == rva & !0xfff {
                for q in (p + 8..p + block).step_by(2) {
                    let e = u16_at(b, q)?;
                    if e >> 12 != 0 && u32::from(e & 0xfff) == rva & 0xfff {
                        out.push(q);
                    }
                }
            }
            p += block;
        }
        Ok(out)
    }

    /// The `FileVersion` string of the version resource (first string table), if there is one.
    pub fn file_version_string(&self, b: &[u8]) -> Result<Option<String>> {
        let Some(vi) = self.version_resource(b)? else {
            return Ok(None);
        };
        Ok(version_strings(vi)
            .into_iter()
            .find(|(k, _)| k == "FileVersion")
            .map(|(_, v)| v))
    }

    /// The bytes of the first RT_VERSION resource.
    fn version_resource<'a>(&self, b: &'a [u8]) -> Result<Option<&'a [u8]>> {
        let (rva, size) = self.data_directory(b, IMAGE_DIRECTORY_ENTRY_RESOURCE)?;
        if size == 0 {
            return Ok(None);
        }
        let root = self.rva_to_offset(rva)?;
        // Resource directory: type -> name -> language -> data entry. Offsets are relative to root;
        // bit 31 marks a subdirectory.
        let entries = |dir: usize| -> Result<Vec<(u32, u32)>> {
            let n = u16_at(b, dir + 12)? as usize + u16_at(b, dir + 14)? as usize;
            (0..n)
                .map(|i| Ok((u32_at(b, dir + 16 + 8 * i)?, u32_at(b, dir + 20 + 8 * i)?)))
                .collect()
        };
        let Some(&(_, ty)) = entries(root)?.iter().find(|(id, _)| *id == RT_VERSION) else {
            return Ok(None);
        };
        let mut node = ty;
        while node & 0x8000_0000 != 0 {
            let Some(&(_, next)) = entries(root + (node & 0x7fff_ffff) as usize)?.first() else {
                return Ok(None);
            };
            node = next;
        }
        let entry = root + node as usize;
        let (data_rva, data_size) = (u32_at(b, entry)?, u32_at(b, entry + 4)? as usize);
        let o = self.rva_to_offset(data_rva)?;
        match b.get(o..o + data_size) {
            Some(s) => Ok(Some(s)),
            None => bail!("version resource past the end of the file"),
        }
    }
}

/// (key, value) pairs of the first StringTable of a VS_VERSIONINFO block.
fn version_strings(vi: &[u8]) -> Vec<(String, String)> {
    // Every node: wLength, wValueLength, wType, szKey (UTF-16, NUL), padding to 4, value, children.
    struct Node<'a> {
        key: String,
        value: &'a [u8],
        children: &'a [u8],
    }
    fn node(b: &[u8]) -> Option<(Node<'_>, usize)> {
        let len = u16::from_le_bytes([*b.first()?, *b.get(1)?]) as usize;
        let vlen = u16::from_le_bytes([*b.get(2)?, *b.get(3)?]) as usize;
        let text = u16::from_le_bytes([*b.get(4)?, *b.get(5)?]) == 1;
        let b = b.get(..len)?;
        let mut p = 6;
        let mut key = Vec::new();
        loop {
            let c = u16::from_le_bytes([*b.get(p)?, *b.get(p + 1)?]);
            p += 2;
            if c == 0 {
                break;
            }
            key.push(c);
        }
        p = (p + 3) & !3;
        let vbytes = if text { vlen * 2 } else { vlen };
        let value = b.get(p..(p + vbytes).min(len))?;
        let children = b.get(((p + vbytes + 3) & !3).min(len)..)?;
        Some((
            Node {
                key: String::from_utf16_lossy(&key),
                value,
                children,
            },
            (len + 3) & !3,
        ))
    }
    fn kids(mut b: &[u8]) -> Vec<Node<'_>> {
        let mut out = Vec::new();
        while let Some((n, step)) = node(b) {
            out.push(n);
            if step == 0 || step >= b.len() {
                break;
            }
            b = &b[step..];
        }
        out
    }
    let utf16 = |v: &[u8]| {
        let w: Vec<u16> = v
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&w)
            .trim_end_matches('\0')
            .to_string()
    };
    let Some((root, _)) = node(vi) else {
        return Vec::new();
    };
    for sfi in kids(root.children)
        .into_iter()
        .filter(|n| n.key == "StringFileInfo")
    {
        if let Some(table) = kids(sfi.children).into_iter().next() {
            return kids(table.children)
                .into_iter()
                .map(|s| (s.key, utf16(s.value)))
                .collect();
        }
    }
    Vec::new()
}

/// Finds the PE image NTLDR and SETUPLDR.BIN carry behind their 16-bit startup module: an 'MZ' header
/// at a multiple of 16 between 4 KB and 64 KB with an i386 PE32 header (standard optional header size).
pub fn find_embedded_image(b: &[u8]) -> Option<usize> {
    let end = b.len().saturating_sub(0x400).min(0x10000);
    (0x1000..end).step_by(0x10).find(|&o| {
        if &b[o..o + 2] != b"MZ" {
            return false;
        }
        let lf = i32::from_le_bytes([b[o + 0x3c], b[o + 0x3d], b[o + 0x3e], b[o + 0x3f]]);
        if !(0x40..=0x400).contains(&lf) {
            return false;
        }
        let p = o + lf as usize;
        p + 24 + 0xe0 + 40 <= b.len()
            && u32_at(b, p) == Ok(0x4550)
            && u16_at(b, p + 4) == Ok(IMAGE_FILE_MACHINE_I386)
            && u16_at(b, p + 20) == Ok(0xe0)
    })
}

/// The PE checksum (as imagehlp's CheckSumMappedFile) of `b[start..]`, with the 4 bytes at
/// `field` (the CheckSum field itself) counted as zero.
pub fn checksum(b: &[u8], start: usize, field: usize) -> u32 {
    let img = &b[start..];
    let skip = field - start;
    let mut sum: u64 = 0;
    for (i, w) in img.chunks(2).enumerate() {
        let o = 2 * i;
        if o >= skip && o < skip + 4 {
            continue;
        }
        sum += if w.len() == 2 {
            u64::from(u16::from_le_bytes([w[0], w[1]]))
        } else {
            u64::from(w[0])
        };
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum = (sum & 0xffff) + (sum >> 16);
    (sum as u32).wrapping_add(img.len() as u32)
}

/// Recomputes and stores the checksum of the image at `pe.start`, which runs to the end of the file.
pub fn update_checksum(b: &mut [u8], pe: &Pe) {
    let field = pe.checksum_offset();
    let sum = checksum(b, pe.start, field);
    crate::put_u32(b, field, sum);
}
