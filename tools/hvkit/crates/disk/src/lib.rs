//! Disk images (raw or VHDX), their MBR partitions and FAT file systems.

pub mod fat;
pub mod image;

pub use image::{Image, SECTOR, Window};

use formats::mbr::Mbr;
use std::fmt;

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
