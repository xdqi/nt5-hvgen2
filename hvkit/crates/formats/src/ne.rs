//! 16-bit NE executables (Windows 3.x drivers and DLLs): the segment table.

use crate::{Result, bail, u16_at};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// 1-based, as Windows numbers them.
    pub number: usize,
    /// File offset of the segment's data (0: no data in the file).
    pub offset: usize,
    /// Bytes of data in the file.
    pub len: usize,
    pub flags: u16,
}

impl Segment {
    /// Code segments have bit 0 of the flags clear.
    pub fn is_code(&self) -> bool {
        self.flags & 1 == 0
    }
}

/// The segments of an NE file.
pub fn segments(b: &[u8]) -> Result<Vec<Segment>> {
    if b.get(..2) != Some(b"MZ") {
        bail!("no 'MZ' header");
    }
    let ne = usize::from(u16_at(b, 0x3c)?);
    if b.get(ne..ne + 2) != Some(b"NE") {
        bail!("not an NE file");
    }
    let table = ne + usize::from(u16_at(b, ne + 0x22)?);
    let count = usize::from(u16_at(b, ne + 0x1c)?);
    let shift = u16_at(b, ne + 0x32)?;
    if shift > 15 {
        bail!("bad segment alignment shift {shift}");
    }
    (0..count)
        .map(|i| {
            let e = table + 8 * i;
            let sector = usize::from(u16_at(b, e)?);
            let len = u16_at(b, e + 2)?;
            Ok(Segment {
                number: i + 1,
                offset: sector << shift,
                // A length of 0 means 64 KiB when the segment has data.
                len: if len == 0 && sector != 0 {
                    0x10000
                } else {
                    usize::from(len)
                },
                flags: u16_at(b, e + 4)?,
            })
        })
        .collect()
}
