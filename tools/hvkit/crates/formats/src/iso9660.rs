//! ISO 9660 images, read only: the volume descriptors, the directory tree (Joliet names when the image
//! has a Joliet tree, as the Windows CD drivers use them) and the El Torito boot image.

use crate::{Error, Result, bail};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, SystemTime};

pub const SECTOR: u64 = 2048;

/// An entry of the directory tree.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Path from the root, components separated by '/', as named in the tree that was read.
    pub path: String,
    pub dir: bool,
    pub lba: u32,
    pub size: u32,
    pub mtime: SystemTime,
}

/// The El Torito default (initial) boot entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootEntry {
    /// 0 = no emulation, 1-3 = floppy, 4 = hard disk.
    pub media: u8,
    pub load_segment: u16,
    /// In 512-byte virtual sectors.
    pub sector_count: u16,
    pub lba: u32,
}

pub struct Iso {
    file: File,
    pub volume_id: String,
    /// Size of the volume, in 2048-byte blocks.
    pub blocks: u32,
    root: (u32, u32),
    joliet: Option<(u32, u32)>,
    pub boot_catalog: Option<u32>,
}

fn read_at(f: &mut File, lba: u64, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0; len];
    f.seek(SeekFrom::Start(lba * SECTOR))
        .map_err(|e| Error(format!("seek: {e}")))?;
    f.read_exact(&mut buf)
        .map_err(|e| Error(format!("read at block {lba}: {e}")))?;
    Ok(buf)
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// The 7-byte recording date of a directory record.
fn record_time(d: &[u8]) -> SystemTime {
    let (y, mo, day, h, mi, s) = (
        1900 + i64::from(d[0]),
        i64::from(d[1]),
        i64::from(d[2]),
        i64::from(d[3]),
        i64::from(d[4]),
        i64::from(d[5]),
    );
    let gmt_offset_min = i64::from(d[6] as i8) * 15;
    if !(1..=12).contains(&mo) || day == 0 {
        return SystemTime::UNIX_EPOCH;
    }
    // Days from 1970-01-01 to y-mo-day (proleptic Gregorian).
    let (yy, mm) = if mo <= 2 {
        (y - 1, mo + 9)
    } else {
        (y, mo - 3)
    };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + h * 3600 + mi * 60 + s - gmt_offset_min * 60;
    if secs >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs as u64)
    } else {
        SystemTime::UNIX_EPOCH
    }
}

impl Iso {
    pub fn open(path: &Path) -> Result<Iso> {
        let mut file = File::open(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
        let (mut pvd, mut joliet, mut boot_catalog) = (None, None, None);
        for n in 16..64 {
            let v = read_at(&mut file, n, SECTOR as usize)?;
            if &v[1..6] != b"CD001" {
                bail!("no volume descriptor at block {n}");
            }
            match v[0] {
                0 if v[7..30] == *b"EL TORITO SPECIFICATION" => boot_catalog = Some(le32(&v, 0x47)),
                1 => pvd = Some(v),
                2 if v[88] == b'%' && v[89] == b'/' && matches!(v[90], b'@' | b'C' | b'E') => {
                    joliet = Some((le32(&v, 156 + 2), le32(&v, 156 + 10)));
                }
                255 => break,
                _ => {}
            }
        }
        let Some(pvd) = pvd else {
            bail!("no primary volume descriptor")
        };
        Ok(Iso {
            file,
            volume_id: String::from_utf8_lossy(&pvd[40..72]).trim_end().to_string(),
            blocks: le32(&pvd, 80),
            root: (le32(&pvd, 156 + 2), le32(&pvd, 156 + 10)),
            joliet,
            boot_catalog,
        })
    }

    pub fn has_joliet(&self) -> bool {
        self.joliet.is_some()
    }

    /// The records of one directory: (raw name, record), skipping "." and "..".
    fn records(&mut self, lba: u32, size: u32) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let d = read_at(&mut self.file, u64::from(lba), size as usize)?;
        let mut out = Vec::new();
        let mut i = 0;
        while i < d.len() {
            let len = d[i] as usize;
            if len == 0 {
                i = (i / SECTOR as usize + 1) * SECTOR as usize; // records do not cross sectors
                continue;
            }
            if i + len > d.len() || len < 34 {
                bail!("bad directory record at block {lba} + {i}");
            }
            let r = &d[i..i + len];
            let name = r[33..33 + r[32] as usize].to_vec();
            if name != [0] && name != [1] {
                out.push((name, r.to_vec()));
            }
            i += len;
        }
        Ok(out)
    }

    /// Every file and directory, depth first, from the Joliet tree when there is one.
    pub fn entries(&mut self) -> Result<Vec<Entry>> {
        let (joliet, (lba, size)) = match self.joliet {
            Some(j) => (true, j),
            None => (false, self.root),
        };
        let mut out = Vec::new();
        self.walk(lba, size, "", joliet, &mut out, 0)?;
        Ok(out)
    }

    fn walk(
        &mut self,
        lba: u32,
        size: u32,
        prefix: &str,
        joliet: bool,
        out: &mut Vec<Entry>,
        depth: usize,
    ) -> Result<()> {
        if depth > 64 {
            bail!("directory tree too deep (loop?)");
        }
        for (raw, r) in self.records(lba, size)? {
            if r[25] & 0x80 != 0 {
                bail!("multi-extent files are not supported");
            }
            let name = if joliet {
                let w: Vec<u16> = raw
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&w)
            } else {
                String::from_utf8_lossy(&raw).into_owned()
            };
            let dir = r[25] & 2 != 0;
            // File names end in ";1" and, without an extension, may end in ".".
            let name = if dir {
                name
            } else {
                name.split(';')
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('.')
                    .to_string()
            };
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let e = Entry {
                path: path.clone(),
                dir,
                lba: le32(&r, 2),
                size: le32(&r, 10),
                mtime: record_time(&r[18..25]),
            };
            out.push(e.clone());
            if dir {
                self.walk(e.lba, e.size, &path, joliet, out, depth + 1)?;
            }
        }
        Ok(())
    }

    pub fn read(&mut self, e: &Entry) -> Result<Vec<u8>> {
        read_at(&mut self.file, u64::from(e.lba), e.size as usize)
    }

    /// The default entry of the El Torito boot catalog.
    pub fn boot_entry(&mut self) -> Result<Option<BootEntry>> {
        let Some(cat) = self.boot_catalog else {
            return Ok(None);
        };
        let c = read_at(&mut self.file, u64::from(cat), SECTOR as usize)?;
        if c[0] != 1 || c[0x1e] != 0x55 || c[0x1f] != 0xaa {
            bail!("bad El Torito validation entry");
        }
        let e = &c[32..64];
        Ok(Some(BootEntry {
            media: e[1],
            load_segment: u16::from_le_bytes([e[2], e[3]]),
            sector_count: u16::from_le_bytes([e[6], e[7]]),
            lba: le32(e, 8),
        }))
    }

    /// The bytes the BIOS loads for the default boot entry (sector_count * 512).
    pub fn boot_image(&mut self) -> Result<Option<Vec<u8>>> {
        let Some(b) = self.boot_entry()? else {
            return Ok(None);
        };
        let mut data = read_at(
            &mut self.file,
            u64::from(b.lba),
            SECTOR as usize * usize::from(b.sector_count).div_ceil(4),
        )?;
        data.truncate(usize::from(b.sector_count) * 512);
        Ok(Some(data))
    }

    /// Offset of `name`'s record in the ISO 9660 (not Joliet) directory `dir` (e.g. "I386"; "" for the
    /// root), as the XP CD boot sector sees it: etfsboot reads only the first 128 sectors of \I386.
    pub fn primary_record_offset(&mut self, dir: &str, name: &str) -> Result<Option<usize>> {
        let (mut lba, mut size) = self.root;
        if !dir.is_empty() {
            let Some((_, d)) = self
                .records(lba, size)?
                .into_iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(dir.as_bytes()))
            else {
                return Ok(None);
            };
            (lba, size) = (le32(&d, 2), le32(&d, 10));
        }
        let data = read_at(&mut self.file, u64::from(lba), size as usize)?;
        let mut i = 0;
        while i < data.len() {
            let len = data[i] as usize;
            if len == 0 {
                i = (i / SECTOR as usize + 1) * SECTOR as usize;
                continue;
            }
            let raw = &data[i + 33..i + 33 + data[i + 32] as usize];
            let base = raw.split(|&c| c == b';').next().unwrap_or(raw);
            if base.eq_ignore_ascii_case(name.as_bytes()) {
                return Ok(Some(i));
            }
            i += len;
        }
        Ok(None)
    }
}

impl Iso {
    /// Writes every file under `dest` (created), with the times recorded in the image; returns the
    /// number of files.
    pub fn extract_all(&mut self, dest: &Path) -> Result<usize> {
        let io = |p: &Path, e: std::io::Error| Error(format!("{}: {e}", p.display()));
        std::fs::create_dir_all(dest).map_err(|e| io(dest, e))?;
        let entries = self.entries()?;
        let mut files = 0;
        for e in entries.iter().filter(|e| !e.dir) {
            let p = dest.join(&e.path);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(|err| io(parent, err))?;
            }
            let data = self.read(e)?;
            std::fs::write(&p, data).map_err(|err| io(&p, err))?;
            File::options()
                .write(true)
                .open(&p)
                .and_then(|f| f.set_modified(e.mtime))
                .map_err(|err| io(&p, err))?;
            files += 1;
        }
        // Directory times last, after their contents were written.
        for e in entries.iter().rev().filter(|e| e.dir) {
            let p = dest.join(&e.path);
            std::fs::create_dir_all(&p).map_err(|err| io(&p, err))?;
            File::open(&p)
                .and_then(|f| f.set_modified(e.mtime))
                .map_err(|err| io(&p, err))?;
        }
        Ok(files)
    }
}
