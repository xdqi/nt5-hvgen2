//! `hvkit iso`: ISO 9660 images.

use clap::Subcommand;
use formats::iso9660::Iso;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum IsoCommand {
    /// Print the volume id, the El Torito boot entry and, for a Windows setup CD, where SETUPLDR.BIN's
    /// record is in \I386 (the CD boot sector only reads its first 128 sectors)
    Info { iso: PathBuf },
    /// Extract every file (Joliet names when the image has them) with its recorded time
    Extract {
        iso: PathBuf,
        dir: PathBuf,
        /// Also write the El Torito boot image here
        #[arg(long)]
        boot_image: Option<PathBuf>,
    },
    /// List the files with their first block and size (to map a disk trace's LBAs to files)
    Ls { iso: PathBuf },
    /// Master a directory into an ISO image (an existing OUT is rewritten in place, keeping its ACL)
    #[cfg(feature = "iso")]
    Build {
        dir: PathBuf,
        out: PathBuf,
        /// Volume id (e.g. the original CD's, from hvkit iso info)
        #[arg(long)]
        volume_id: String,
        /// A Windows NT 5.x setup CD booting this no-emulation image (path in DIR, e.g. /boot.img):
        /// names as on Microsoft's CDs, Joliet, no Rock Ridge, the image hidden from the ISO 9660 tree
        #[arg(long)]
        nt5_setup: Option<String>,
        /// An EFI System Partition image in DIR for an El Torito UEFI entry
        #[arg(long)]
        efi: Option<String>,
        /// Hide a file of DIR from the ISO 9660 tree (Joliet keeps it)
        #[arg(long)]
        hide: Vec<String>,
        /// Add Rock Ridge
        #[arg(long)]
        rock_ridge: bool,
    },
}

pub fn run(cmd: IsoCommand) -> Result<(), String> {
    #[cfg(feature = "iso")]
    if let IsoCommand::Build {
        dir,
        out,
        volume_id,
        nt5_setup,
        efi,
        hide,
        rock_ridge,
    } = cmd
    {
        let mut o = match &nt5_setup {
            Some(boot) => iso::Options::nt5_setup(&volume_id, boot),
            None => iso::Options {
                volume_id,
                bios_boot: None,
                efi_boot: None,
                catalog: "/boot.catalog".into(),
                hide: Vec::new(),
                iso_level: 2,
                rock_ridge: false,
                joliet: true,
                force_dots: false,
            },
        };
        o.efi_boot = efi;
        o.hide.extend(hide);
        o.rock_ridge |= rock_ridge;
        iso::build(&dir, &out, &o).map_err(|e| format!("{}: {e}", out.display()))?;
        if nt5_setup.is_some() {
            let mut i = Iso::open(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            match i
                .primary_record_offset("I386", "SETUPLDR.BIN")
                .map_err(|e| format!("{}: {e}", out.display()))?
            {
                Some(off) if off >= 128 * 2048 => {
                    return Err(format!(
                        "{}: SETUPLDR.BIN's record is at {off} in \\I386, past the 128 sectors the CD boot sector reads",
                        out.display()
                    ));
                }
                Some(off) => println!(
                    "{}: \\I386 record of SETUPLDR.BIN at {off} (sector {})",
                    out.display(),
                    off / 2048
                ),
                None => {}
            }
        }
        return Ok(());
    }
    let open = |p: &PathBuf| Iso::open(p).map_err(|e| format!("{}: {e}", p.display()));
    match cmd {
        IsoCommand::Info { iso } => {
            let mut i = open(&iso)?;
            let e = |err: formats::Error| format!("{}: {err}", iso.display());
            println!("volume id   {}", i.volume_id);
            println!("size        {} blocks of 2048 bytes", i.blocks);
            println!("joliet      {}", if i.has_joliet() { "yes" } else { "no" });
            match i.boot_entry().map_err(e)? {
                Some(b) => println!(
                    "boot        El Torito, media {}, load segment 0x{:x}, {} sectors at block {}",
                    b.media, b.load_segment, b.sector_count, b.lba
                ),
                None => println!("boot        none"),
            }
            if let Some(off) = i.primary_record_offset("I386", "SETUPLDR.BIN").map_err(e)? {
                println!(
                    "setupldr    \\I386 record at {off} (sector {}{})",
                    off / 2048,
                    if off >= 128 * 2048 {
                        ", PAST the 128 sectors etfsboot reads"
                    } else {
                        ""
                    }
                );
            }
            Ok(())
        }
        IsoCommand::Extract {
            iso,
            dir,
            boot_image,
        } => {
            let mut i = open(&iso)?;
            let e = |err: formats::Error| format!("{}: {err}", iso.display());
            let n = i.extract_all(&dir).map_err(e)?;
            println!(
                "{}: {n} files extracted to {}",
                iso.display(),
                dir.display()
            );
            if let Some(b) = boot_image {
                let img = i
                    .boot_image()
                    .map_err(e)?
                    .ok_or_else(|| format!("{}: no El Torito boot entry", iso.display()))?;
                std::fs::write(&b, img).map_err(|err| format!("{}: {err}", b.display()))?;
            }
            Ok(())
        }
        IsoCommand::Ls { iso } => {
            let mut i = open(&iso)?;
            for e in i
                .entries()
                .map_err(|err| format!("{}: {err}", iso.display()))?
                .iter()
                .filter(|e| !e.dir)
            {
                println!(
                    "{:>8} {:>6}  {}",
                    e.lba,
                    (u64::from(e.size)).div_ceil(2048),
                    e.path
                );
            }
            Ok(())
        }
        #[cfg(feature = "iso")]
        IsoCommand::Build { .. } => unreachable!(),
    }
}
