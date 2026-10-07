//! FAT12/16/32 file systems on a partition of an image, through fatfs (with long names).

use crate::image::Window;
use crate::{Error, Result};
use fatfs::{
    Date, DateTime, FatType, FileAttributes, FormatVolumeOptions, FsOptions, StdIoWrapper, Time,
};
use std::io::{Read, Write};
use std::path::Path;
use std::time::SystemTime;

pub type Fs<'a> = fatfs::FileSystem<StdIoWrapper<Window<'a>>>;
type Dir<'a, 'b> = fatfs::Dir<
    'b,
    StdIoWrapper<Window<'a>>,
    fatfs::DefaultTimeProvider,
    fatfs::LossyOemCpConverter,
>;

fn fat_err(what: &str) -> impl Fn(fatfs::Error<std::io::Error>) -> Error + '_ {
    move |e| Error(format!("{what}: {e}"))
}

#[derive(Debug, Clone, Default)]
pub struct FormatOptions {
    /// 12, 16 or 32; default: by size, as fatfs chooses.
    pub fat: Option<u8>,
    pub label: Option<String>,
    /// The partition's first sector (BPB hidden sectors), for BIOS boot code.
    pub hidden_sectors: u32,
    pub cluster_size: Option<u32>,
    pub drive_num: Option<u8>,
    pub volume_id: Option<u32>,
}

pub fn format(w: Window<'_>, o: &FormatOptions) -> Result<()> {
    let mut f = FormatVolumeOptions::new().hidden_sectors(o.hidden_sectors);
    if let Some(t) = o.fat {
        f = f.fat_type(match t {
            12 => FatType::Fat12,
            16 => FatType::Fat16,
            32 => FatType::Fat32,
            _ => return Err(Error(format!("FAT{t}: expected 12, 16 or 32"))),
        });
    }
    if let Some(l) = &o.label {
        let up = l.to_uppercase();
        if up.len() > 11 || !up.is_ascii() {
            return Err(Error(format!("label {l:?}: up to 11 ASCII characters")));
        }
        let mut b = [b' '; 11];
        b[..up.len()].copy_from_slice(up.as_bytes());
        f = f.volume_label(b);
    }
    if let Some(c) = o.cluster_size {
        f = f.bytes_per_cluster(c);
    }
    if let Some(d) = o.drive_num {
        f = f.drive_num(d);
    }
    let id = o.volume_id.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| (d.as_secs() as u32) ^ d.subsec_nanos())
            .unwrap_or(0)
    });
    f = f.volume_id(id);
    let mut s = StdIoWrapper::from(w);
    fatfs::format_volume(&mut s, f).map_err(fat_err("format"))?;
    Ok(())
}

pub fn open(w: Window<'_>) -> Result<Fs<'_>> {
    Fs::new(w, FsOptions::new()).map_err(fat_err("not a FAT file system"))
}

/// Replaces the boot code of the volume's boot sector with that of another FAT boot sector (e.g. a
/// DOS boot floppy's), keeping this volume's BPB: bytes 0-10 (jump and OEM name) and the code after
/// the BPB (from 62 on FAT12/16, from 90 on FAT32; the source must be the same kind).
pub fn set_boot_code(w: &mut Window<'_>, from: &[u8]) -> Result<()> {
    use std::io::Seek;
    let io = |e: std::io::Error| Error(format!("boot sector: {e}"));
    if from.len() < 512 || from[510..512] != [0x55, 0xaa] {
        return Err(Error(
            "the boot code source is not a boot sector (no 55 AA)".into(),
        ));
    }
    let mut s = [0u8; 512];
    w.seek(std::io::SeekFrom::Start(0)).map_err(io)?;
    w.read_exact(&mut s).map_err(io)?;
    let fat32 = |b: &[u8]| u16::from_le_bytes([b[22], b[23]]) == 0;
    if fat32(&s) != fat32(from) {
        return Err(Error(
            "the boot code is for another FAT type (FAT32 vs FAT12/16)".into(),
        ));
    }
    let code = if fat32(&s) { 90 } else { 62 };
    s[..11].copy_from_slice(&from[..11]);
    s[code..512].copy_from_slice(&from[code..512]);
    w.seek(std::io::SeekFrom::Start(0)).map_err(io)?;
    w.write_all(&s).map_err(io)?;
    Ok(())
}

/// A directory entry, for listings.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub short_name: String,
    pub dir: bool,
    pub size: u64,
    pub attributes: FileAttributes,
    pub modified: DateTime,
}

/// Splits "a/b/c" into ("a/b", "c"); paths may use / or \ and start with either.
fn split(path: &str) -> (String, String) {
    let p = path.replace('\\', "/");
    let p = p.trim_matches('/');
    match p.rsplit_once('/') {
        Some((d, f)) => (d.to_string(), f.to_string()),
        None => (String::new(), p.to_string()),
    }
}

fn norm(path: &str) -> String {
    path.replace('\\', "/").trim_matches('/').to_string()
}

fn dir<'a, 'b>(fs: &'b Fs<'a>, path: &str) -> Result<Dir<'a, 'b>> {
    let p = norm(path);
    if p.is_empty() {
        return Ok(fs.root_dir());
    }
    fs.root_dir()
        .open_dir(&p)
        .map_err(fat_err(&format!("/{p}")))
}

pub fn list(fs: &Fs<'_>, path: &str) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in dir(fs, path)?.iter() {
        let e = e.map_err(fat_err(path))?;
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        out.push(Entry {
            short_name: e.short_file_name(),
            dir: e.is_dir(),
            size: e.len(),
            attributes: e.attributes(),
            modified: e.modified(),
            name,
        });
    }
    Ok(out)
}

pub fn read(fs: &Fs<'_>, path: &str) -> Result<Vec<u8>> {
    let p = norm(path);
    let mut f = fs
        .root_dir()
        .open_file(&p)
        .map_err(fat_err(&format!("/{p}")))?;
    let mut data = Vec::new();
    f.read_to_end(&mut data)
        .map_err(|e| Error(format!("/{p}: {e}")))?;
    Ok(data)
}

pub fn exists(fs: &Fs<'_>, path: &str) -> bool {
    let (d, f) = split(path);
    dir(fs, &d).is_ok_and(|d| {
        d.iter()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().eq_ignore_ascii_case(&f))
    })
}

/// Creates the directory and its missing parents.
pub fn mkdir_p(fs: &Fs<'_>, path: &str) -> Result<()> {
    let mut cur = fs.root_dir();
    let mut done = String::new();
    for part in norm(path).split('/').filter(|p| !p.is_empty()) {
        done.push('/');
        done.push_str(part);
        cur = match cur.open_dir(part) {
            Ok(d) => d,
            Err(_) => cur.create_dir(part).map_err(fat_err(&done))?,
        };
    }
    Ok(())
}

fn to_fat_time(t: SystemTime) -> DateTime {
    // FAT times are local times, two-second resolution, years 1980-2107.
    let l = chrono::DateTime::<chrono::Local>::from(t).naive_local();
    use chrono::{Datelike, Timelike};
    let y = l.year().clamp(1980, 2107) as u16;
    DateTime::new(
        Date::new(y, l.month() as u16, l.day() as u16),
        Time::new(
            l.hour() as u16,
            l.minute() as u16,
            (l.second() & !1) as u16,
            0,
        ),
    )
}

/// Writes a file (replacing one that exists; its attributes are kept), with the modification time
/// `mtime` (default: now). The parent directories must exist.
pub fn write(fs: &Fs<'_>, path: &str, data: &[u8], mtime: Option<SystemTime>) -> Result<()> {
    let (d, name) = split(path);
    let p = format!("/{}", norm(path));
    let dir = dir(fs, &d)?;
    let mut f = dir.create_file(&name).map_err(fat_err(&p))?;
    f.truncate().map_err(fat_err(&p))?;
    f.write_all(data).map_err(|e| Error(format!("{p}: {e}")))?;
    if let Some(t) = mtime {
        f.set_modified(to_fat_time(t));
    }
    f.flush().map_err(|e| Error(format!("{p}: {e}")))?;
    Ok(())
}

fn io_err(p: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error(format!("{}: {e}", p.display()))
}

/// Copies a host file or directory tree to `dest` (a directory path, created; the host name is kept),
/// with the host's modification times. Returns the number of files.
pub fn put(fs: &Fs<'_>, host: &Path, dest: &str) -> Result<usize> {
    let name = host
        .file_name()
        .ok_or_else(|| Error(format!("{}: no file name", host.display())))?
        .to_string_lossy()
        .into_owned();
    let target = if norm(dest).is_empty() {
        name
    } else {
        format!("{}/{name}", norm(dest))
    };
    let meta = std::fs::metadata(host).map_err(io_err(host))?;
    if meta.is_dir() {
        mkdir_p(fs, &target)?;
        let mut n = 0;
        let mut entries: Vec<_> = std::fs::read_dir(host)
            .map_err(io_err(host))?
            .collect::<std::io::Result<_>>()
            .map_err(io_err(host))?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            n += put(fs, &e.path(), &target)?;
        }
        Ok(n)
    } else {
        write(
            fs,
            &target,
            &std::fs::read(host).map_err(io_err(host))?,
            meta.modified().ok(),
        )?;
        Ok(1)
    }
}

/// Removes a file, or a directory with everything in it.
pub fn remove(fs: &Fs<'_>, path: &str) -> Result<()> {
    let p = norm(path);
    let is_dir = fs.root_dir().open_dir(&p).is_ok();
    if is_dir {
        for e in list(fs, &p)? {
            remove(fs, &format!("{p}/{}", e.name))?;
        }
    }
    fs.root_dir().remove(&p).map_err(fat_err(&format!("/{p}")))
}

/// Sets and clears read-only, hidden, system and archive bits of a file.
pub fn set_attributes(
    fs: &Fs<'_>,
    path: &str,
    set: FileAttributes,
    clear: FileAttributes,
) -> Result<FileAttributes> {
    let p = norm(path);
    let mut f = fs
        .root_dir()
        .open_file(&p)
        .map_err(fat_err(&format!("/{p}")))?;
    let a = (f.attributes() | set) & !clear;
    f.set_attributes(a);
    f.flush().map_err(|e| Error(format!("/{p}: {e}")))?;
    Ok(f.attributes())
}
