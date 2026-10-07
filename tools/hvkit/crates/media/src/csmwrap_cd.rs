//! A CD that boots CSMWrap on Hyper-V Generation 2: an ISO whose only El Torito entry is for UEFI
//! (platform 0xEF), with a FAT image holding \EFI\BOOT\BOOTX64.EFI, csmwrap.ini next to it and the
//! DSDT (or whatever else CSMWrap is told to load). CSMWrap reads its files from the device it was
//! loaded from; SeaBIOS then passes over this CD, whose catalog has no BIOS entry, and boots the next
//! CD or the disk. The VM's disk needs no ESP: setup installs onto an empty disk and XP gets C:. The
//! CD has to stay in its drive, as it is the VM's firmware; put it at a SCSI location after the
//! install CD's, which then keeps the lower drive letter.

use crate::{Error, Result};
use disk::Image;
use disk::fat::{self, FormatOptions};
use std::path::{Path, PathBuf};

pub struct CsmwrapCd {
    /// csmwrap.efi, as \EFI\BOOT\BOOTX64.EFI.
    pub efi: PathBuf,
    /// csmwrap.ini, as \EFI\BOOT\csmwrap.ini.
    pub ini: Option<PathBuf>,
    /// A DSDT, as \dsdt.aml (csmwrap.ini's acpi_dsdt names it).
    pub dsdt: Option<PathBuf>,
    /// More files: (host file, path in the image).
    pub put: Vec<(PathBuf, String)>,
    pub volume_id: String,
    /// The ISO to write (an existing one is rewritten in place, keeping its ACL).
    pub out: PathBuf,
}

/// The FAT image is at least this big (FAT12; CSMWrap and its files take well under 1 MiB).
const MIN_IMAGE: u64 = 4 << 20;

pub fn build(c: &CsmwrapCd, log: &mut dyn FnMut(String)) -> Result<()> {
    let mut files: Vec<(PathBuf, String)> = vec![(c.efi.clone(), "/EFI/BOOT/BOOTX64.EFI".into())];
    if let Some(i) = &c.ini {
        files.push((i.clone(), "/EFI/BOOT/csmwrap.ini".into()));
    }
    if let Some(d) = &c.dsdt {
        files.push((d.clone(), "/dsdt.aml".into()));
    }
    files.extend(c.put.iter().cloned());
    let mut data = Vec::new();
    for (src, dest) in &files {
        data.push(std::fs::read(src).map_err(|e| Error(format!("{}: {e}", src.display())))?);
        log(format!("{dest} <- {}", src.display()));
    }
    // Room for the files with a margin for clusters and directories, in whole MiB.
    let total: u64 = data.iter().map(|d| d.len() as u64).sum();
    let size = (total + total / 4 + (1 << 20)).div_ceil(1 << 20) * (1 << 20);
    let size = size.max(MIN_IMAGE);

    let work = std::env::temp_dir().join(format!("hvkit-csmwrap-cd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| Error(format!("{}: {e}", work.display())))?;
    let r = (|| {
        let img_path = work.join("efiboot.img");
        let mut img = Image::create(&img_path, size, 1 << 20).map_err(|e| Error(e.0))?;
        let o = FormatOptions {
            label: Some("CSMWRAP".into()),
            ..Default::default()
        };
        fat::format(img.window(0, size), &o).map_err(|e| Error(e.0))?;
        let fs = fat::open(img.window(0, size)).map_err(|e| Error(e.0))?;
        for ((src, dest), d) in files.iter().zip(&data) {
            if let Some((dir, _)) = dest.trim_matches('/').rsplit_once('/') {
                fat::mkdir_p(&fs, dir).map_err(|e| Error(e.0))?;
            }
            let mtime = std::fs::metadata(src).and_then(|m| m.modified()).ok();
            fat::write(&fs, dest, d, mtime).map_err(|e| Error(e.0))?;
        }
        fs.unmount()
            .map_err(|e| Error(format!("efiboot.img: {e}")))?;
        img.flush().map_err(|e| Error(e.0))?;
        drop(img);
        let o = iso::Options {
            volume_id: c.volume_id.clone(),
            bios_boot: None,
            efi_boot: Some("/efiboot.img".into()),
            catalog: "/boot.catalog".into(),
            hide: vec!["/efiboot.img".into()],
            iso_level: 2,
            rock_ridge: false,
            joliet: true,
            force_dots: false,
        };
        iso::build(&work, &c.out, &o).map_err(|e| Error(format!("{}: {e}", c.out.display())))
    })();
    let _ = std::fs::remove_dir_all(&work);
    r?;
    log(format!(
        "wrote {} (FAT image of {} MiB)",
        c.out.display(),
        size >> 20
    ));
    Ok(())
}

/// Parses SRC=/DEST.
pub fn parse_put(s: &str) -> Result<(PathBuf, String)> {
    match s.rsplit_once('=') {
        Some((src, dest)) if !src.is_empty() && dest.starts_with(['/', '\\']) => {
            Ok((Path::new(src).to_path_buf(), dest.replace('\\', "/")))
        }
        _ => Err(Error(format!("--put {s}: expected SRC=/DEST"))),
    }
}
