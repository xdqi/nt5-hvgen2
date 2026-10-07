//! File formats used by hvkit.

pub mod pattern;
pub mod pe;

use std::fmt;

/// An error in the input: what was expected and what was found, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Returns early with a formatted [`Error`].
#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => { return Err($crate::Error(format!($($arg)*))) };
}

pub fn u16_at(b: &[u8], o: usize) -> Result<u16> {
    match b.get(o..o + 2) {
        Some(s) => Ok(u16::from_le_bytes([s[0], s[1]])),
        None => bail!("offset 0x{o:x} is past the end of the file"),
    }
}

pub fn u32_at(b: &[u8], o: usize) -> Result<u32> {
    match b.get(o..o + 4) {
        Some(s) => Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]])),
        None => bail!("offset 0x{o:x} is past the end of the file"),
    }
}

pub fn put_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

pub fn put_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

pub fn align_up(v: u32, a: u32) -> u32 {
    v.div_ceil(a) * a
}
