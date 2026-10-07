//! Disk images (raw or VHDX), their MBR partitions and FAT file systems.

pub mod fat;
pub mod image;

pub use image::{Image, SECTOR, Window};

use formats::mbr::Mbr;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The MBR of an image.
pub fn mbr(img: &mut Image) -> Result<Mbr> {
    let mut s = [0u8; 512];
    img.read_at(0, &mut s)?;
    Mbr::parse(&s).map_err(|e| Error(format!("{}: {e}", img.path().display())))
}

/// Byte range (start, length) of partition `n` (1-4) of the image's MBR, or of the whole image for
/// 0 (a partition image or a floppy).
pub fn partition(img: &mut Image, n: usize) -> Result<(u64, u64)> {
    if n == 0 {
        return Ok((0, img.size()));
    }
    let m = mbr(img)?;
    match m.partitions.get(n - 1).copied().flatten() {
        Some(p) => Ok((u64::from(p.start) * SECTOR, u64::from(p.sectors) * SECTOR)),
        None => Err(Error(format!("{}: no partition {n}", img.path().display()))),
    }
}

/// Whether a first sector is a FAT boot sector (a floppy or a partition image) rather than an MBR: a
/// plausible BPB (sector size, cluster size, number of FATs, media byte).
pub fn is_fat_boot_sector(s: &[u8]) -> bool {
    if s.len() < 512 || s[510..512] != [0x55, 0xaa] || !(s[0] == 0xeb || s[0] == 0xe9) {
        return false;
    }
    let bps = u16::from_le_bytes([s[11], s[12]]);
    matches!(bps, 512 | 1024 | 2048 | 4096)
        && s[13].is_power_of_two()
        && (1..=2).contains(&s[16])
        && s[21] >= 0xf0
}

/// The partition to use when none is given: 0 (the whole image) for a FAT volume without an MBR,
/// else the MBR's first partition.
pub fn default_partition(img: &mut Image) -> Result<usize> {
    let mut s = [0u8; 512];
    img.read_at(0, &mut s)?;
    if is_fat_boot_sector(&s) {
        return Ok(0);
    }
    Ok(usize::from(
        Mbr::parse(&s).is_ok_and(|m| m.partitions.iter().any(Option::is_some)),
    ))
}

/// A file inside a FAT volume of an image, written `IMAGE:N:PATH` (partition N, 0 for an image
/// without an MBR) or `IMAGE::PATH` (the one FAT partition that has PATH). PATH starts with `\` or
/// `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageFile {
    pub image: PathBuf,
    pub partition: Option<usize>,
    pub path: String,
}

impl ImageFile {
    /// Splits at the first `:N:` or `::` that is followed by `\` or `/`; None if there is none.
    pub fn parse(s: &str) -> Option<ImageFile> {
        let b = s.as_bytes();
        for (i, _) in s.match_indices(':').filter(|&(i, _)| i > 0) {
            let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
            let j = i + 1 + digits;
            if b.get(j) == Some(&b':') && matches!(b.get(j + 1), Some(b'\\' | b'/')) {
                return Some(ImageFile {
                    image: PathBuf::from(&s[..i]),
                    partition: (digits > 0).then(|| s[i + 1..j].parse().ok()).flatten(),
                    path: s[j + 1..].to_string(),
                });
            }
        }
        None
    }

    /// The partition: the one given, else the only FAT partition (or FAT image) that has the path.
    pub fn resolve_partition(&self, img: &mut Image) -> Result<usize> {
        match self.partition {
            Some(n) => Ok(n),
            None => find_partition(img, &self.path),
        }
    }

    /// Reads the file.
    pub fn read(&self) -> Result<Vec<u8>> {
        let mut img = Image::open(&self.image, false)?;
        let n = self.resolve_partition(&mut img)?;
        let (start, len) = partition(&mut img, n)?;
        let fs = fat::open(img.window(start, len))?;
        fat::read(&fs, &self.path)
    }

    /// Replaces the file's contents; its attributes are kept, its modification time becomes now.
    pub fn write(&self, data: &[u8]) -> Result<()> {
        let mut img = Image::open(&self.image, true)?;
        let n = self.resolve_partition(&mut img)?;
        let (start, len) = partition(&mut img, n)?;
        let fs = fat::open(img.window(start, len))?;
        fat::write(&fs, &self.path, data, Some(std::time::SystemTime::now()))?;
        fs.unmount().map_err(|e| Error(format!("{self}: {e}")))?;
        img.flush()
    }
}

impl fmt::Display for ImageFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.partition {
            Some(n) => write!(f, "{}:{n}:{}", self.image.display(), self.path),
            None => write!(f, "{}::{}", self.image.display(), self.path),
        }
    }
}

/// The partition (0 for a FAT image without an MBR, else 1-4) whose FAT file system has `path`; an
/// error if none or more than one has it.
pub fn find_partition(img: &mut Image, path: &str) -> Result<usize> {
    let mut s = [0u8; 512];
    img.read_at(0, &mut s)?;
    let candidates: Vec<usize> = if is_fat_boot_sector(&s) {
        vec![0]
    } else {
        let m = mbr(img)?;
        (1..=4).filter(|&n| m.partitions[n - 1].is_some()).collect()
    };
    let mut found = Vec::new();
    for n in candidates {
        let (start, len) = partition(img, n)?;
        if fat::open(img.window(start, len)).is_ok_and(|fs| fat::exists(&fs, path)) {
            found.push(n);
        }
    }
    match found[..] {
        [n] => Ok(n),
        [] => Err(Error(format!(
            "{}: no FAT partition has {path}",
            img.path().display()
        ))),
        _ => Err(Error(format!(
            "{}: partitions {found:?} all have {path}; name one (IMAGE:N:PATH)",
            img.path().display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::ImageFile;
    use std::path::PathBuf;

    #[test]
    fn image_file_specs() {
        let f = ImageFile::parse(r"xp.vhdx:2:\WINDOWS\system32\config\system").unwrap();
        assert_eq!(f.image, PathBuf::from("xp.vhdx"));
        assert_eq!(f.partition, Some(2));
        assert_eq!(f.path, r"\WINDOWS\system32\config\system");
        let f = ImageFile::parse("/a/b.vhdx::/x").unwrap();
        assert_eq!((f.image.to_str(), f.partition), (Some("/a/b.vhdx"), None));
        assert_eq!(f.to_string(), "/a/b.vhdx::/x");
        assert!(ImageFile::parse("system.hiv").is_none());
        assert!(ImageFile::parse("xp.vhdx:2").is_none());
        assert!(ImageFile::parse(":1:/x").is_none());
    }
}
