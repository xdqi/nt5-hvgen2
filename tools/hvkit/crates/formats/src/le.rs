//! LE (linear executable) files: Windows 3.x/9x VxDs, read and patched in place. Port of the CSMWrap
//! testbed's w98/le.py.
//!
//! The LE header may sit inside a bigger file (VMM in a W3 VMM32.VXD); its data pages offset is
//! counted from the start of that file either way.

use crate::{Error, Result, bail, u16_at, u32_at};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Object {
    /// 1-based.
    pub number: usize,
    pub virtual_size: u32,
    pub base: u32,
    pub flags: u32,
    /// First page (1-based) and number of pages.
    pub first_page: u32,
    pub pages: u32,
}

impl Object {
    pub fn is_code(&self) -> bool {
        self.flags & 0x0004 != 0
    }

    pub fn is_32bit(&self) -> bool {
        self.flags & 0x2000 != 0
    }
}

/// A fixup of an internal reference: where it applies, and its raw record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixup {
    pub object: usize,
    /// Offset in the object of the field that is fixed up.
    pub offset: u32,
    /// Source type (low 4 bits of the record's first byte): 7 = 32-bit offset, 8 = 48-bit pointer...
    pub source: u8,
    pub record: Vec<u8>,
}

impl Fixup {
    /// Bytes of the field the fixup writes.
    pub fn field_len(&self) -> u32 {
        match self.source {
            8 => 6,
            _ => 4,
        }
    }
}

/// An exported entry of type 3 (32-bit offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub ordinal: u32,
    pub object: u16,
    pub offset: u32,
    pub flags: u8,
}

/// The device descriptor block of a VxD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ddb {
    pub object: u16,
    pub offset: u32,
    pub sdk_version: u16,
    pub device_id: u16,
    pub version: (u8, u8),
    pub flags: u16,
    pub name: String,
    pub init_order: u32,
    pub control_proc: u32,
    pub service_table: u32,
    pub service_count: u32,
}

#[derive(Debug, Clone)]
pub struct Le {
    /// File offset of the 'LE' header.
    pub header: usize,
    pub page_size: u32,
    pub last_page: u32,
    pub page_count: u32,
    pub objects: Vec<Object>,
    page_map: usize,
    resident_names: usize,
    entry_table: usize,
    fixup_pages: usize,
    fixup_records: usize,
    /// File offset of the data pages.
    data_pages: u32,
}

impl Le {
    /// Parses the LE header at `header`, or (None) where the MZ header's e_lfanew points.
    pub fn parse(b: &[u8], header: Option<usize>) -> Result<Le> {
        let h = match header {
            Some(h) => h,
            None => u32_at(b, 0x3c)? as usize,
        };
        if b.get(h..h + 2) != Some(b"LE") {
            bail!("not an LE file (no 'LE' at 0x{h:x})");
        }
        let at = |o: usize| u32_at(b, h + o);
        let mut le = Le {
            header: h,
            page_count: at(0x14)?,
            page_size: at(0x28)?,
            last_page: at(0x2c)?,
            objects: Vec::new(),
            page_map: h + at(0x48)? as usize,
            resident_names: h + at(0x58)? as usize,
            entry_table: h + at(0x5c)? as usize,
            fixup_pages: h + at(0x68)? as usize,
            fixup_records: h + at(0x6c)? as usize,
            data_pages: at(0x80)?,
        };
        if le.page_size == 0 {
            bail!("page size 0");
        }
        let table = h + at(0x40)? as usize;
        for i in 0..at(0x44)? as usize {
            let o = table + 24 * i;
            le.objects.push(Object {
                number: i + 1,
                virtual_size: u32_at(b, o)?,
                base: u32_at(b, o + 4)?,
                flags: u32_at(b, o + 8)?,
                first_page: u32_at(b, o + 12)?,
                pages: u32_at(b, o + 16)?,
            });
        }
        Ok(le)
    }

    /// Header field at `off` (32-bit).
    pub fn field(&self, b: &[u8], off: usize) -> Result<u32> {
        u32_at(b, self.header + off)
    }

    fn object(&self, n: usize) -> Result<&Object> {
        self.objects
            .get(n.wrapping_sub(1))
            .ok_or_else(|| Error(format!("no object {n}")))
    }

    /// File offset of logical page `page` (1-based).
    pub fn page_offset(&self, b: &[u8], page: u32) -> Result<usize> {
        let e = self.page_map + 4 * (page as usize - 1);
        let Some(m) = b.get(e..e + 3) else {
            bail!("page {page} past the page map")
        };
        // LE: a 3-byte big-endian page number, then flags.
        let num = (u32::from(m[0]) << 16) | (u32::from(m[1]) << 8) | u32::from(m[2]);
        Ok(self.data_pages as usize + (num as usize - 1) * self.page_size as usize)
    }

    fn page_len(&self, page: u32) -> u32 {
        if page == self.page_count {
            self.last_page
        } else {
            self.page_size
        }
    }

    /// (object offset, file offset, length) of each page of an object.
    pub fn object_ranges(&self, b: &[u8], n: usize) -> Result<Vec<(u32, usize, u32)>> {
        let o = self.object(n)?;
        (0..o.pages)
            .map(|k| {
                let p = o.first_page + k;
                Ok((
                    k * self.page_size,
                    self.page_offset(b, p)?,
                    self.page_len(p),
                ))
            })
            .collect()
    }

    /// The object's bytes (virtual size; past the file data zeros).
    pub fn object_bytes(&self, b: &[u8], n: usize) -> Result<Vec<u8>> {
        let mut out = vec![0u8; self.object(n)?.virtual_size as usize];
        for (ooff, foff, len) in self.object_ranges(b, n)? {
            let ooff = ooff as usize;
            if ooff >= out.len() {
                break;
            }
            let len = (len as usize).min(out.len() - ooff);
            let Some(src) = b.get(foff..foff + len) else {
                bail!("object {n}: page data past the end of the file")
            };
            out[ooff..ooff + len].copy_from_slice(src);
        }
        Ok(out)
    }

    /// File offset of an object offset, if it has file data.
    pub fn file_offset(&self, b: &[u8], n: usize, off: u32) -> Result<Option<usize>> {
        for (o, f, len) in self.object_ranges(b, n)? {
            if o <= off && off < o + len {
                return Ok(Some(f + (off - o) as usize));
            }
        }
        Ok(None)
    }

    /// The fixups (internal references only; other target types are an error).
    pub fn fixups(&self, b: &[u8]) -> Result<Vec<Fixup>> {
        let mut page_obj = std::collections::HashMap::new();
        for o in &self.objects {
            for k in 0..o.pages {
                page_obj.insert(o.first_page + k, (o.number, k * self.page_size));
            }
        }
        let byte = |p: usize| {
            b.get(p)
                .copied()
                .ok_or_else(|| Error("fixup record past the end of the file".into()))
        };
        let mut out = Vec::new();
        for p in 1..=self.page_count {
            let start = u32_at(b, self.fixup_pages + 4 * (p as usize - 1))? as usize;
            let end = u32_at(b, self.fixup_pages + 4 * p as usize)? as usize;
            let mut pos = self.fixup_records + start;
            while pos < self.fixup_records + end {
                let (src, flg) = (byte(pos)?, byte(pos + 1)?);
                let mut q = pos + 2;
                let mut count = 0;
                let mut srcoffs = Vec::new();
                if src & 0x20 != 0 {
                    count = usize::from(byte(q)?); // source list
                    q += 1;
                } else {
                    srcoffs.push(u16_at(b, q)? as i16);
                    q += 2;
                }
                if flg & 3 != 0 {
                    bail!(
                        "fixup target type {} (only internal references are supported)",
                        flg & 3
                    );
                }
                q += if flg & 0x40 != 0 { 2 } else { 1 }; // object number
                if src & 0xf != 2 {
                    q += if flg & 0x10 != 0 { 4 } else { 2 }; // target offset (not for selector fixups)
                }
                if flg & 0x04 != 0 {
                    q += if flg & 0x20 != 0 { 4 } else { 2 }; // additive value
                }
                if src & 0x20 != 0 {
                    for i in 0..count {
                        srcoffs.push(u16_at(b, q + 2 * i)? as i16);
                    }
                    q += 2 * count;
                }
                let &(obj, base) = page_obj
                    .get(&p)
                    .ok_or_else(|| Error(format!("page {p} belongs to no object")))?;
                for s in srcoffs {
                    out.push(Fixup {
                        object: obj,
                        offset: (base as i64 + i64::from(s)) as u32,
                        source: src & 0xf,
                        record: b[pos..q].to_vec(),
                    });
                }
                pos = q;
            }
        }
        Ok(out)
    }

    pub fn entries(&self, b: &[u8]) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        let (mut ordinal, mut pos) = (1u32, self.entry_table);
        loop {
            let count = *b
                .get(pos)
                .ok_or_else(|| Error("entry table past the end".into()))?;
            if count == 0 {
                return Ok(out);
            }
            let kind = b[pos + 1];
            pos += 2;
            if kind == 0 {
                ordinal += u32::from(count);
                continue;
            }
            let object = u16_at(b, pos)?;
            pos += 2;
            for _ in 0..count {
                if kind != 3 {
                    bail!("entry type {kind} (only type 3, 32-bit offsets, is supported)");
                }
                out.push(Entry {
                    ordinal,
                    object,
                    flags: b[pos],
                    offset: u32_at(b, pos + 1)?,
                });
                pos += 5;
                ordinal += 1;
            }
        }
    }

    /// File offset of the first entry bundle of the entry table.
    pub fn entry_table_offset(&self) -> usize {
        self.entry_table
    }

    pub fn resident_names(&self, b: &[u8]) -> Result<Vec<(String, u16)>> {
        let mut out = Vec::new();
        let mut pos = self.resident_names;
        while let Some(&n) = b.get(pos).filter(|&&n| n != 0) {
            let n = usize::from(n);
            let Some(name) = b.get(pos + 1..pos + 1 + n) else {
                bail!("resident name past the end")
            };
            out.push((
                String::from_utf8_lossy(name).into_owned(),
                u16_at(b, pos + 1 + n)?,
            ));
            pos += n + 3;
        }
        Ok(out)
    }

    /// The VxD's device descriptor block, which entry 1 points at.
    pub fn ddb(&self, b: &[u8]) -> Result<Ddb> {
        let e = self
            .entries(b)?
            .into_iter()
            .find(|e| e.ordinal == 1)
            .ok_or_else(|| Error("no entry 1 (the DDB)".into()))?;
        let obj = self.object_bytes(b, usize::from(e.object))?;
        let d = obj
            .get(e.offset as usize..e.offset as usize + 0x50)
            .ok_or_else(|| Error("DDB past the end of its object".into()))?;
        let u32le = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
        let u16le = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
        Ok(Ddb {
            object: e.object,
            offset: e.offset,
            sdk_version: u16le(4),
            device_id: u16le(6),
            version: (d[8], d[9]),
            flags: u16le(10),
            name: String::from_utf8_lossy(&d[12..20]).trim_end().to_string(),
            init_order: u32le(20),
            control_proc: u32le(24),
            service_table: u32le(0x30),
            service_count: u32le(0x34),
        })
    }
}

/// Rewrites the DDB export of a VxD linked by Open Watcom's wlink (2.0 beta, Oct 2026) from a type 2
/// entry (286 call gate, 16-bit offset), which Windows 98's loader rejects as "LoadFailed (Damaged)",
/// to the type 3 entry (32-bit offset) other VxDs have. Both entries are 5 bytes: type 2 = flags,
/// offset16, gate16; type 3 = flags, offset32. Returns whether anything changed. Port of the CSMWrap
/// testbed's w98/gen2leg-ow/fixentry.py.
pub fn fix_ddb_entry(b: &mut [u8]) -> Result<bool> {
    let h = u32_at(b, 0x3c)? as usize;
    if b.get(h..h + 2) != Some(b"LE") {
        bail!("not an LE file");
    }
    let et = h + u32_at(b, h + 0x5c)? as usize;
    if b.get(et) != Some(&1) {
        bail!("expected a single exported entry");
    }
    match b.get(et + 1) {
        Some(2) => {
            let off = u16_at(b, et + 5)?;
            b[et + 1] = 3;
            b[et + 4] = 0x01;
            b[et + 5..et + 9].copy_from_slice(&u32::from(off).to_le_bytes());
            Ok(true)
        }
        Some(3) => Ok(false),
        t => bail!("entry type {t:?}"),
    }
}
