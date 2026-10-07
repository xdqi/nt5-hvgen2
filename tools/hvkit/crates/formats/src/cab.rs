//! Microsoft cabinet files: reading single cabinets and cabinet sets (folders continued across
//! cabinets, with blocks split between them), MSZIP, LZX and stored data; writing MSZIP cabinets,
//! alone or as a set of two laid out like Windows 98 setup's MINI.CAB/MINI1.CAB.

use crate::{Error, Result, bail};
use std::path::{Path, PathBuf};

const FLAG_PREV: u16 = 1;
const FLAG_NEXT: u16 = 2;
const FLAG_RESERVE: u16 = 4;
const CONT_FROM_PREV: u16 = 0xfffd;
const CONT_TO_NEXT: u16 = 0xfffe;
const CONT_BOTH: u16 = 0xffff;
const BLOCK: usize = 32768;

fn u16_at(b: &[u8], o: usize) -> Result<u16> {
    crate::u16_at(b, o)
}

fn u32_at(b: &[u8], o: usize) -> Result<u32> {
    crate::u32_at(b, o)
}

fn cstr(b: &[u8], o: &mut usize, utf8: bool) -> Result<String> {
    let Some(len) = b.get(*o..).and_then(|s| s.iter().position(|&c| c == 0)) else {
        bail!("unterminated name")
    };
    let raw = &b[*o..*o + len];
    *o += len + 1;
    Ok(if utf8 {
        String::from_utf8_lossy(raw).into_owned()
    } else {
        raw.iter().map(|&c| char::from(c)).collect()
    })
}

/// One file of a cabinet (as listed in the cabinet that has its start).
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub size: u32,
    /// Offset in the (set-wide) folder's uncompressed data.
    pub offset: u32,
    /// Set-wide folder number.
    pub folder: usize,
    pub date: u16,
    pub time: u16,
    pub attributes: u16,
}

struct Block {
    cab: usize,
    offset: usize,
    compressed: usize,
    uncompressed: usize,
}

struct Folder {
    compression: u16,
    blocks: Vec<Block>,
    /// The folder started in a cabinet that is not there, so it cannot be decompressed.
    incomplete: bool,
}

struct Cabinet {
    path: PathBuf,
    data: Vec<u8>,
    flags: u16,
    set_id: u16,
    index: u16,
    prev: Option<String>,
    next: Option<String>,
}

/// A cabinet, with the other cabinets of its set that are next to it.
pub struct CabSet {
    cabs: Vec<Cabinet>,
    folders: Vec<Folder>,
    pub files: Vec<FileEntry>,
}

fn read_cabinet(path: &Path) -> Result<Cabinet> {
    let data = std::fs::read(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    if data.get(..4) != Some(b"MSCF") {
        bail!("{}: not a cabinet", path.display());
    }
    let flags = u16_at(&data, 30)?;
    let mut o = 36;
    if flags & FLAG_RESERVE != 0 {
        o += 4 + usize::from(u16_at(&data, 36)?);
    }
    let mut prev = None;
    let mut next = None;
    if flags & FLAG_PREV != 0 {
        prev = Some(cstr(&data, &mut o, false)?);
        cstr(&data, &mut o, false)?;
    }
    if flags & FLAG_NEXT != 0 {
        next = Some(cstr(&data, &mut o, false)?);
        cstr(&data, &mut o, false)?;
    }
    Ok(Cabinet {
        path: path.to_path_buf(),
        set_id: u16_at(&data, 32)?,
        index: u16_at(&data, 34)?,
        data,
        flags,
        prev,
        next,
    })
}

/// The file `name` next to `near`, compared without case (cabinet names are upper case, files on a
/// Linux copy of a CD often lower case).
fn sibling(near: &Path, name: &str) -> Option<PathBuf> {
    let dir = near.parent().unwrap_or(Path::new("."));
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(name))
        })
}

impl CabSet {
    /// Opens a cabinet and follows its links to the previous and next cabinets of the set, as far as
    /// they exist next to it.
    pub fn open(path: &Path) -> Result<CabSet> {
        let first = read_cabinet(path)?;
        let mut cabs = vec![first];
        while let Some(p) = cabs[0]
            .prev
            .clone()
            .and_then(|n| sibling(&cabs[0].path, &n))
        {
            let c = read_cabinet(&p)?;
            if c.set_id != cabs[0].set_id {
                break;
            }
            cabs.insert(0, c);
        }
        while let Some(p) = cabs
            .last()
            .unwrap()
            .next
            .clone()
            .and_then(|n| sibling(&cabs.last().unwrap().path, &n))
        {
            let c = read_cabinet(&p)?;
            if c.set_id != cabs[0].set_id || cabs.iter().any(|x| x.index == c.index) {
                break;
            }
            cabs.push(c);
        }
        let mut set = CabSet {
            cabs,
            folders: Vec::new(),
            files: Vec::new(),
        };
        set.index()?;
        Ok(set)
    }

    fn index(&mut self) -> Result<()> {
        // Set-wide folder of local folder j of each cabinet.
        for ci in 0..self.cabs.len() {
            let c = &self.cabs[ci];
            let d = &c.data;
            let name = c.path.display().to_string();
            let fail = |e: Error| Error(format!("{name}: {e}"));
            let nfolders = usize::from(u16_at(d, 26).map_err(fail)?);
            let nfiles = usize::from(u16_at(d, 28).map_err(fail)?);
            let files_at = u32_at(d, 16).map_err(fail)? as usize;
            let (folder_reserve, data_reserve) = if c.flags & FLAG_RESERVE != 0 {
                (usize::from(d[38]), usize::from(d[39]))
            } else {
                (0, 0)
            };
            let mut o = 36;
            if c.flags & FLAG_RESERVE != 0 {
                o += 4 + usize::from(u16_at(d, 36).map_err(fail)?);
            }
            for present in [c.flags & FLAG_PREV != 0, c.flags & FLAG_NEXT != 0] {
                if present {
                    cstr(d, &mut o, false).map_err(fail)?;
                    cstr(d, &mut o, false).map_err(fail)?;
                }
            }
            // Files: which local folders continue from the previous cabinet.
            let mut files = Vec::new();
            let mut fo = files_at;
            for _ in 0..nfiles {
                let (size, offset, ifolder) = (
                    u32_at(d, fo).map_err(fail)?,
                    u32_at(d, fo + 4).map_err(fail)?,
                    u16_at(d, fo + 8).map_err(fail)?,
                );
                let (date, time, attributes) = (
                    u16_at(d, fo + 10).map_err(fail)?,
                    u16_at(d, fo + 12).map_err(fail)?,
                    u16_at(d, fo + 14).map_err(fail)?,
                );
                fo += 16;
                let name = cstr(d, &mut fo, attributes & 0x80 != 0).map_err(fail)?;
                files.push((name, size, offset, ifolder, date, time, attributes));
            }
            let continued = ci > 0
                && files
                    .iter()
                    .any(|f| f.3 == CONT_FROM_PREV || f.3 == CONT_BOTH);
            let mut global = Vec::new();
            for j in 0..nfolders {
                let at = o + j * (8 + folder_reserve);
                let start = u32_at(d, at).map_err(fail)? as usize;
                let nblocks = usize::from(u16_at(d, at + 4).map_err(fail)?);
                let compression = u16_at(d, at + 6).map_err(fail)?;
                let mut blocks = Vec::new();
                let mut p = start;
                for _ in 0..nblocks {
                    let compressed = usize::from(u16_at(d, p + 4).map_err(fail)?);
                    let uncompressed = usize::from(u16_at(d, p + 6).map_err(fail)?);
                    let offset = p + 8 + data_reserve;
                    if offset + compressed > d.len() {
                        return Err(fail(Error("data block past the end".into())));
                    }
                    blocks.push(Block {
                        cab: ci,
                        offset,
                        compressed,
                        uncompressed,
                    });
                    p = offset + compressed;
                }
                if j == 0 && continued {
                    let g = self.folders.len() - 1;
                    self.folders[g].blocks.extend(blocks);
                    global.push(g);
                } else {
                    let incomplete = j == 0
                        && ci == 0
                        && files
                            .iter()
                            .any(|f| f.3 == CONT_FROM_PREV || f.3 == CONT_BOTH);
                    self.folders.push(Folder {
                        compression,
                        blocks,
                        incomplete,
                    });
                    global.push(self.folders.len() - 1);
                }
            }
            for (name, size, offset, ifolder, date, time, attributes) in files {
                let folder = match ifolder {
                    CONT_FROM_PREV | CONT_BOTH => global[0],
                    CONT_TO_NEXT => *global
                        .last()
                        .ok_or_else(|| fail(Error("no folder".into())))?,
                    j => *global
                        .get(usize::from(j))
                        .ok_or_else(|| fail(Error(format!("{name}: folder {j} out of range"))))?,
                };
                // A file continued from the previous cabinet is listed there too.
                if !self.files.iter().any(|f| {
                    f.folder == folder && f.offset == offset && f.name.eq_ignore_ascii_case(&name)
                }) {
                    self.files.push(FileEntry {
                        name,
                        size,
                        offset,
                        folder,
                        date,
                        time,
                        attributes,
                    });
                }
            }
        }
        Ok(())
    }

    /// The paths of the cabinets found, in set order.
    pub fn cabinets(&self) -> Vec<&Path> {
        self.cabs.iter().map(|c| c.path.as_path()).collect()
    }

    /// The uncompressed data of a set-wide folder.
    pub fn folder_data(&self, n: usize) -> Result<Vec<u8>> {
        let f = &self.folders[n];
        if f.incomplete {
            bail!(
                "folder {n} starts in an earlier cabinet that is not there ({})",
                self.cabs[0].prev.as_deref().unwrap_or("?")
            );
        }
        // Join blocks split between cabinets (the first part has no uncompressed size).
        let mut blocks: Vec<(Vec<u8>, usize)> = Vec::new();
        let mut pending: Vec<u8> = Vec::new();
        for b in &f.blocks {
            pending.extend_from_slice(&self.cabs[b.cab].data[b.offset..b.offset + b.compressed]);
            if b.uncompressed != 0 {
                blocks.push((std::mem::take(&mut pending), b.uncompressed));
            }
        }
        let mut out = Vec::new();
        match f.compression & 0x0f {
            0 => blocks
                .into_iter()
                .for_each(|(b, _)| out.extend_from_slice(&b)),
            1 => {
                for (i, (b, ulen)) in blocks.into_iter().enumerate() {
                    if b.get(..2) != Some(b"CK") {
                        bail!("MSZIP block {i} without its CK signature");
                    }
                    // Each block is a raw deflate stream whose dictionary is the 32 KiB before it.
                    let mut z = flate2::Decompress::new(false);
                    let hist = &out[out.len().saturating_sub(BLOCK)..];
                    if !hist.is_empty() {
                        z.set_dictionary(hist)
                            .map_err(|e| Error(format!("MSZIP block {i}: {e}")))?;
                    }
                    let mut buf = Vec::with_capacity(ulen);
                    z.decompress_vec(&b[2..], &mut buf, flate2::FlushDecompress::Finish)
                        .map_err(|e| Error(format!("MSZIP block {i}: {e}")))?;
                    if buf.len() != ulen {
                        bail!("MSZIP block {i}: {} bytes instead of {ulen}", buf.len());
                    }
                    out.extend_from_slice(&buf);
                }
            }
            3 => {
                use lzxd::WindowSize::*;
                let window = match (f.compression >> 8) & 0x1f {
                    15 => KB32,
                    16 => KB64,
                    17 => KB128,
                    18 => KB256,
                    19 => KB512,
                    20 => MB1,
                    21 => MB2,
                    w => bail!("LZX window of 2^{w} bytes"),
                };
                let mut lzx = lzxd::Lzxd::new(window);
                for (i, (b, ulen)) in blocks.into_iter().enumerate() {
                    out.extend_from_slice(
                        lzx.decompress_next(&b, ulen)
                            .map_err(|e| Error(format!("LZX block {i}: {e}")))?,
                    );
                }
            }
            t => bail!("compression type {t} (Quantum?) is not supported"),
        }
        Ok(out)
    }

    /// The file `name` (compared without case).
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let f = self
            .files
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error(format!("no {name} in the cabinet set")))?;
        let data = self.folder_data(f.folder)?;
        let (s, e) = (f.offset as usize, f.offset as usize + f.size as usize);
        data.get(s..e)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| Error(format!("{name}: past the end of its folder")))
    }
}

/// The names of the files in a cabinet (and the rest of its set next to it), as stored.
pub fn list(path: &Path) -> Result<Vec<String>> {
    Ok(CabSet::open(path)?
        .files
        .into_iter()
        .map(|f| f.name)
        .collect())
}

/// The contents of `name` (compared without case) in a cabinet (or its set).
pub fn extract(path: &Path, name: &str) -> Result<Vec<u8>> {
    CabSet::open(path)?
        .read(name)
        .map_err(|e| Error(format!("{}: {e}", path.display())))
}

/// A file to put into a cabinet: its name as stored (upper-cased ASCII), contents and DOS date/time.
#[derive(Debug, Clone)]
pub struct NewFile {
    pub name: String,
    pub data: Vec<u8>,
    pub date: u16,
    pub time: u16,
}

pub fn dos_date_time(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> (u16, u16) {
    (
        (((y - 1980) as u16) << 9) | ((mo as u16) << 5) | d as u16,
        ((h as u16) << 11) | ((mi as u16) << 5) | (s / 2) as u16,
    )
}

/// (name, size, offset in the folder, date, time) of a file written into a folder.
type FolderEntry = (String, u32, u32, u16, u16);
/// A compressed data block and its uncompressed length.
type DataBlock = (Vec<u8>, usize);

/// A folder's entries (name, size, offset, date, time) and its MSZIP blocks (compressed, uncompressed
/// length): each 32 KiB is an independent raw deflate stream (level 9), which every MSZIP decoder
/// accepts.
fn mszip_folder(files: &[NewFile]) -> Result<(Vec<FolderEntry>, Vec<DataBlock>)> {
    let mut entries = Vec::new();
    let mut stream = Vec::new();
    for f in files {
        entries.push((
            f.name.to_uppercase(),
            f.data.len() as u32,
            stream.len() as u32,
            f.date,
            f.time,
        ));
        stream.extend_from_slice(&f.data);
    }
    let mut blocks = Vec::new();
    for raw in stream.chunks(BLOCK) {
        let mut z = flate2::Compress::new(flate2::Compression::new(9), false);
        let mut out = Vec::with_capacity(raw.len() + 64);
        out.extend_from_slice(b"CK");
        loop {
            out.reserve(4096);
            let st = z
                .compress_vec(
                    &raw[z.total_in() as usize..],
                    &mut out,
                    flate2::FlushCompress::Finish,
                )
                .map_err(|e| Error(format!("deflate: {e}")))?;
            if st == flate2::Status::StreamEnd {
                break;
            }
        }
        blocks.push((out, raw.len()));
    }
    Ok((entries, blocks))
}

type Entry = (String, u32, u32, u16, u16, u16);

fn cabinet(
    entries: &[Entry],
    folders: &[&[(Vec<u8>, usize)]],
    flags: u16,
    links: &[u8],
    set_id: u16,
    index: u16,
) -> Result<Vec<u8>> {
    for e in entries {
        if !e.0.is_ascii() {
            bail!("{}: cabinet names here are ASCII", e.0);
        }
    }
    let hdr_len = 36 + links.len();
    let files_at = hdr_len + 8 * folders.len();
    let data_at = files_at + entries.iter().map(|e| 16 + e.0.len() + 1).sum::<usize>();
    let total = data_at
        + folders
            .iter()
            .flat_map(|b| b.iter())
            .map(|(c, _)| 8 + c.len())
            .sum::<usize>();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"MSCF");
    for v in [0u32, total as u32, 0, files_at as u32, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[3, 1]);
    for v in [
        folders.len() as u16,
        entries.len() as u16,
        flags,
        set_id,
        index,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(links);
    let mut pos = data_at;
    for blocks in folders {
        out.extend_from_slice(&(pos as u32).to_le_bytes());
        out.extend_from_slice(&(blocks.len() as u16).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        pos += blocks.iter().map(|(c, _)| 8 + c.len()).sum::<usize>();
    }
    for (name, size, off, date, time, ifolder) in entries {
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
        for v in [*ifolder, *date, *time, 0x20] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    for blocks in folders {
        for (c, ulen) in blocks.iter() {
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&(c.len() as u16).to_le_bytes());
            out.extend_from_slice(&(*ulen as u16).to_le_bytes());
            out.extend_from_slice(c);
        }
    }
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

/// One MSZIP cabinet with one folder.
pub fn create(files: &[NewFile], set_id: u16) -> Result<Vec<u8>> {
    let (entries, blocks) = mszip_folder(files)?;
    let entries: Vec<Entry> = entries
        .into_iter()
        .map(|(n, s, o, d, t)| (n, s, o, d, t, 0))
        .collect();
    cabinet(&entries, &[&blocks], 0, &[], set_id, 0)
}

/// A set of two MSZIP cabinets laid out like Windows 98 setup's MINI.CAB/MINI1.CAB, whose extractor
/// only moves on to the next cabinet when a folder continues there: `first` form folder 0, split at
/// the 32 KiB block boundary nearest the middle of its last file (marked as continued in both
/// cabinets); the second cabinet holds the rest of folder 0 (that file's tail) and folder 1 with
/// `second`, so that it can be extracted on its own. `names` are the two cabinets' file names and
/// disk names as recorded in the links.
pub fn create_span(
    first: &[NewFile],
    second: &[NewFile],
    set_id: u16,
    names: [(&str, &str); 2],
) -> Result<(Vec<u8>, Vec<u8>)> {
    let (ea, ba) = mszip_folder(first)?;
    let (eb, bb) = mszip_folder(second)?;
    let Some(&(ref last, size, start, d, t)) = ea.last() else {
        bail!("the first cabinet has no files")
    };
    let (size, start) = (size as usize, start as usize);
    let k = (start + size / 2 + BLOCK / 2) / BLOCK;
    if !(start < k * BLOCK && k * BLOCK < start + size) {
        bail!("{last}: the last file of the first cabinet must span a block boundary");
    }
    let mut e1: Vec<Entry> = ea[..ea.len() - 1]
        .iter()
        .map(|(n, s, o, d, t)| (n.clone(), *s, *o, *d, *t, 0))
        .collect();
    e1.push((last.clone(), size as u32, start as u32, d, t, CONT_TO_NEXT));
    let mut e2: Vec<Entry> = vec![(
        last.clone(),
        size as u32,
        start as u32,
        d,
        t,
        CONT_FROM_PREV,
    )];
    e2.extend(
        eb.iter()
            .map(|(n, s, o, d, t)| (n.clone(), *s, *o, *d, *t, 1)),
    );
    let link = |(cab, disk): (&str, &str)| {
        let mut v = cab.to_uppercase().into_bytes();
        v.push(0);
        v.extend_from_slice(disk.as_bytes());
        v.push(0);
        v
    };
    let c1 = cabinet(&e1, &[&ba[..k]], FLAG_NEXT, &link(names[1]), set_id, 0)?;
    let c2 = cabinet(&e2, &[&ba[k..], &bb], FLAG_PREV, &link(names[0]), set_id, 1)?;
    Ok((c1, c2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, len: usize, seed: u8) -> NewFile {
        // Compressible but not trivial data.
        let data = (0..len)
            .map(|i| (i as u8).wrapping_mul(seed).wrapping_add((i / 300) as u8))
            .collect();
        NewFile {
            name: name.into(),
            data,
            date: 0x2ca5,
            time: 0xb2c0,
        }
    }

    #[test]
    fn single_and_spanned_round_trip() {
        let dir = std::env::temp_dir().join(format!("hvkit-cab-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = [file("A.TXT", 1000, 3), file("BIG.BIN", 200_000, 7)];
        let second = [file("C.DRV", 70_000, 11), file("D.FON", 5, 13)];

        std::fs::write(dir.join("ONE.CAB"), create(&first, 0x1234).unwrap()).unwrap();
        let set = CabSet::open(&dir.join("ONE.CAB")).unwrap();
        assert_eq!(set.read("big.bin").unwrap(), first[1].data);

        let (c1, c2) = create_span(
            &first,
            &second,
            0x6101,
            [("MINI.CAB", "Disk 1"), ("MINI1.CAB", "Disk 2")],
        )
        .unwrap();
        std::fs::write(dir.join("MINI.CAB"), c1).unwrap();
        std::fs::write(dir.join("mini1.cab"), c2).unwrap(); // found without regard to case
        let set = CabSet::open(&dir.join("MINI.CAB")).unwrap();
        assert_eq!(set.cabinets().len(), 2);
        assert_eq!(set.files.len(), 4, "the continued file is listed once");
        for f in first.iter().chain(&second) {
            assert_eq!(set.read(&f.name).unwrap(), f.data, "{}", f.name);
        }
        // The second cabinet alone: its own folder can be read, the continued file cannot.
        let alone = dir.join("alone");
        std::fs::create_dir_all(&alone).unwrap();
        std::fs::copy(dir.join("mini1.cab"), alone.join("MINI1.CAB")).unwrap();
        let set = CabSet::open(&alone.join("MINI1.CAB")).unwrap();
        assert_eq!(set.read("C.DRV").unwrap(), second[0].data);
        assert!(set.read("BIG.BIN").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
