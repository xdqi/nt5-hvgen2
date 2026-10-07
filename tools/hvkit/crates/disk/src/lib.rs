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
