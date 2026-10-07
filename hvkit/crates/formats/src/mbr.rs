//! Master boot records: the boot code, the disk signature and the four primary partition entries.

use crate::{Error, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    pub active: bool,
    pub kind: u8,
    /// First sector.
    pub start: u32,
    /// Number of sectors.
    pub sectors: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mbr {
    /// Bytes 0..440.
    pub boot_code: Vec<u8>,
    pub signature: u32,
    pub partitions: [Option<Partition>; 4],
}

/// The CHS address of an LBA as partitioning tools write it (255 heads, 63 sectors per track;
/// FE FF FF past cylinder 1023).
pub fn chs(lba: u32) -> [u8; 3] {
    let track = lba / 63;
    let sector = lba % 63 + 1;
    let (cyl, head) = (track / 255, track % 255);
    if cyl > 1023 {
        return [0xfe, 0xff, 0xff];
    }
    [
        head as u8,
        (sector as u8) | (((cyl >> 8) as u8) << 6),
        cyl as u8,
    ]
}

impl Mbr {
    pub fn new(signature: u32) -> Mbr {
        Mbr {
            boot_code: vec![0; 440],
            signature,
            partitions: [None; 4],
        }
    }

    pub fn parse(sector: &[u8]) -> Result<Mbr> {
        if sector.len() < 512 || sector[510..512] != [0x55, 0xaa] {
            bail!("no MBR (no 55 AA at the end of the first sector)");
        }
        let mut partitions = [None; 4];
        for (i, p) in partitions.iter_mut().enumerate() {
            let e = &sector[446 + 16 * i..462 + 16 * i];
            if e[4] != 0 {
                *p = Some(Partition {
                    active: e[0] & 0x80 != 0,
                    kind: e[4],
                    start: u32::from_le_bytes(e[8..12].try_into().unwrap()),
                    sectors: u32::from_le_bytes(e[12..16].try_into().unwrap()),
                });
            }
        }
        Ok(Mbr {
            boot_code: sector[..440].to_vec(),
            signature: u32::from_le_bytes(sector[440..444].try_into().unwrap()),
            partitions,
        })
    }

    pub fn to_bytes(&self) -> Result<[u8; 512]> {
        if self.boot_code.len() > 440 {
            return Err(Error(format!(
                "boot code of {} bytes does not fit 440",
                self.boot_code.len()
            )));
        }
        let mut s = [0u8; 512];
        s[..self.boot_code.len()].copy_from_slice(&self.boot_code);
        s[440..444].copy_from_slice(&self.signature.to_le_bytes());
        for (i, p) in self.partitions.iter().enumerate() {
            let Some(p) = p else { continue };
            let e = &mut s[446 + 16 * i..462 + 16 * i];
            e[0] = if p.active { 0x80 } else { 0 };
            e[1..4].copy_from_slice(&chs(p.start));
            e[4] = p.kind;
            e[5..8].copy_from_slice(&chs(p.start + p.sectors - 1));
            e[8..12].copy_from_slice(&p.start.to_le_bytes());
            e[12..16].copy_from_slice(&p.sectors.to_le_bytes());
        }
        s[510] = 0x55;
        s[511] = 0xaa;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_entries_as_sfdisk() {
        // sfdisk on an 8 GiB disk: "start=2048, size=64MiB, type=ef, bootable" and "start=133120, type=7".
        let mut m = Mbr::new(0x1234_5678);
        m.partitions[0] = Some(Partition {
            active: true,
            kind: 0xef,
            start: 2048,
            sectors: 131072,
        });
        m.partitions[1] = Some(Partition {
            active: false,
            kind: 0x07,
            start: 133120,
            sectors: 16_644_096,
        });
        let b = m.to_bytes().unwrap();
        let want: [u8; 36] = [
            0x78, 0x56, 0x34, 0x12, 0x00, 0x00, //
            0x80, 0x20, 0x21, 0x00, 0xef, 0x49, 0x01, 0x08, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00,
            0x02, 0x00, //
            0x00, 0x49, 0x02, 0x08, 0x07, 0xfe, 0xff, 0xff, 0x00, 0x08, 0x02, 0x00, 0x00, 0xf8,
        ];
        assert_eq!(b[440..476], want);
        assert_eq!(Mbr::parse(&b).unwrap(), m);
    }
}
