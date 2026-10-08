//! `hvkit nlite`: nLite addons and driver folders for a Windows NT 5.x CD (see media::nlite).

use clap::{Args, ValueEnum};
use media::nlite::Nlite;
use media::nt5::Partition;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum PartitionArg {
    Fat,
    Ntfs,
    None,
}

impl From<PartitionArg> for Partition {
    fn from(p: PartitionArg) -> Partition {
        match p {
            PartitionArg::Fat => Partition::Fat,
            PartitionArg::Ntfs => Partition::Ntfs,
            PartitionArg::None => Partition::None,
        }
    }
}

#[derive(Args)]
pub struct NliteArgs {
    /// The original CD (only read)
    source: PathBuf,
    /// The directory to write addons\, drivers\ and README.txt to
    out: PathBuf,
    /// As for setup-cd (for an XP x64 CD the x64 builds)
    #[arg(long)]
    files: PathBuf,
    /// As for setup-cd
    #[arg(long)]
    ic: PathBuf,
    /// Leave out Dynamic Memory (patched dmvsc.sys with mdlex.sys)
    #[arg(long)]
    no_dynamic_memory: bool,
    /// XP: leave out the VSS service (production checkpoints; icsvcvss.dll)
    #[arg(long)]
    no_vss: bool,
    /// Leave out the Guest Service Interface (Copy-VMFile; icsvcgsi.dll)
    #[arg(long)]
    no_gsi: bool,
    /// Keep SynthVid at 16 bpp and its six modes
    #[arg(long)]
    no_synthvid: bool,
    /// Add the vmbaud sound card (vmbaud.inf and vmbaud.sys from --files)
    #[arg(long)]
    vmbaud: bool,
    /// Where extracted CDs are kept between runs (default ~/.cache/hvkit/cd, on Windows
    /// %LOCALAPPDATA%\hvkit\cd)
    #[arg(long)]
    cache: Option<PathBuf>,
    /// The kernel debugger on COM2, in text mode
    #[arg(long)]
    kd: bool,
    /// More kernel options for text mode
    #[arg(long)]
    load_options: Option<String>,
    /// The partition addon: fat or ntfs as setup-cd's --partition, through the WINNT.SIF that
    /// nLite's Unattended page writes; none = no partition addon
    #[arg(long, value_enum, default_value_t = PartitionArg::Fat)]
    partition: PartitionArg,
    /// No NTLDR addon (the loaders stay as they are)
    #[arg(long)]
    no_patch_ntldr: bool,
    /// A file with the product key (one line; never printed), for the preset
    #[arg(long)]
    product_key_file: Option<PathBuf>,
    /// The time zone of the preset, by its index (WINNT.SIF TimeZone; 210 = Beijing)
    #[arg(long, default_value_t = 210)]
    time_zone: u32,
}

/// The cache directory of one CD: its file name without the extension, under `base`.
pub(crate) fn cd_cache(base: Option<PathBuf>, iso: &Path) -> Result<PathBuf, String> {
    let base = match base {
        Some(c) => c,
        None => match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(h).join(".cache/hvkit/cd"),
            None => PathBuf::from(
                std::env::var_os("LOCALAPPDATA")
                    .ok_or("neither HOME nor LOCALAPPDATA is set; give --cache")?,
            )
            .join("hvkit")
            .join("cd"),
        },
    };
    Ok(base.join(iso.file_stem().unwrap_or_default()))
}

pub fn run(a: NliteArgs) -> Result<(), String> {
    let n = Nlite {
        cache: cd_cache(a.cache, &a.source)?,
        source: a.source,
        files: a.files,
        ic: a.ic,
        leave_out: media::components::Components {
            dynamic_memory: a.no_dynamic_memory,
            vss: a.no_vss,
            gsi: a.no_gsi,
            synthvid: a.no_synthvid,
            ..Default::default()
        },
        opt_in: media::components::Components {
            vmbaud: a.vmbaud,
            ..Default::default()
        },
        kd: a.kd,
        load_options: a.load_options,
        patch_ntldr: !a.no_patch_ntldr,
        partition: a.partition.into(),
        product_key: a
            .product_key_file
            .as_ref()
            .map(|p| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display())))
            .transpose()?,
        time_zone: a.time_zone,
        out: a.out,
    };
    media::nlite::build(&n, &mut |l| println!("{l}")).map_err(|e| e.to_string())
}
