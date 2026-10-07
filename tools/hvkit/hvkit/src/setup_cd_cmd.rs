//! `hvkit setup-cd`: a Windows NT 5.x setup CD for Hyper-V Generation 2 (see media::setup_cd).

use clap::Args;
use media::hvfb_cd::HvfbCd;
use media::setup_cd::{SetupCd, build};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct SetupCdArgs {
    /// The original CD (only read)
    source: PathBuf,
    /// The ISO to write (an existing one is rewritten in place, keeping its ACL)
    out: PathBuf,
    /// The drivers: hvfb.sys bootwait.sys wdf01000.sys wdfldr.sys vmbus.sys winhv.sys vmbkmcl.sys
    /// storvsc.sys storport.sys hyperkbd.sys bootvid.dll storvsc-xp.inf, and mdlex.sys for Dynamic
    /// Memory
    #[arg(long)]
    files: PathBuf,
    /// The Integration Services 6.3 driver packages (vmbus, synthkbd, vmbushid, vmbusvideo, vmic,
    /// netvsc, dmvsc), e.g. an XP installation's Program Files\Hyper-V Integration Services
    #[arg(long)]
    ic: PathBuf,
    /// XP: leave out Dynamic Memory (patched dmvsc.sys with mdlex.sys)
    #[arg(long)]
    no_dynamic_memory: bool,
    /// XP: leave out the VSS service (production checkpoints; icsvcvss.dll)
    #[arg(long)]
    no_vss: bool,
    /// XP: leave out the Guest Service Interface (Copy-VMFile; icsvcgsi.dll)
    #[arg(long)]
    no_gsi: bool,
    /// Keep SynthVid at 16 bpp and its six modes (default: 32 bpp, 56 modes, on XP and 2003)
    #[arg(long)]
    no_synthvid: bool,
    /// Add the vmbaud sound card (vmbaud.inf and vmbaud.sys from --files) for when the host runs
    /// vmbaud-host.ps1
    #[arg(long)]
    vmbaud: bool,
    /// A CD of the same build with the multiprocessor HALs and kernel, for a CD that nLite stripped
    /// of them
    #[arg(long)]
    mp_source: Option<PathBuf>,
    /// Where extracted CDs are kept between runs (default ~/.cache/hvkit/cd)
    #[arg(long)]
    cache: Option<PathBuf>,
    /// Where the CD is assembled (default: OUT with .d appended; deleted first)
    #[arg(long)]
    work: Option<PathBuf>,
    /// The kernel debugger on COM2 for text mode and the installed system, which also keeps bug
    /// checks on the screen
    #[arg(long)]
    kd: bool,
    /// More kernel options for both (e.g. "/sos")
    #[arg(long)]
    load_options: Option<String>,
    /// Answer the GUI-mode pages (needs --product-key-file unless the CD's WINNT.SIF has a key)
    #[arg(long)]
    unattend: bool,
    /// A file with the product key (one line; never printed)
    #[arg(long)]
    product_key_file: Option<PathBuf>,
    /// bootwait's TimeoutSeconds in text mode
    #[arg(long, default_value_t = 60)]
    bootwait_timeout: u32,
    /// Drop I386\BOOTFIX.BIN (no "Press any key to boot from CD"; only for testing text mode)
    #[arg(long)]
    no_bootfix: bool,
    /// Keep the CD's NTLDR and SETUPLDR.BIN
    #[arg(long)]
    no_patch_ntldr: bool,
    /// A bash script run in the tree (as its working directory) just before mastering
    #[arg(long)]
    hook: Option<PathBuf>,
}

#[derive(Args)]
pub struct HvfbCdArgs {
    /// The original XP CD (only read)
    source: PathBuf,
    /// The ISO to write (an existing one is rewritten in place, keeping its ACL)
    out: PathBuf,
    /// hvfb.sys
    #[arg(long)]
    hvfb: PathBuf,
    /// The frame buffer bootvid.dll to use instead of the CD's (for machines without VGA)
    #[arg(long)]
    bootvid: Option<PathBuf>,
    /// Text-mode setup only: do not install hvfb into the new system
    #[arg(long)]
    no_install: bool,
    /// The mode the installed system starts in, WIDTHxHEIGHTxBPP (e.g. 1024x768x32, Hyper-V Gen2's
    /// native mode); default: the mode setup ran in (hvfb's mode 0, normally 640x480x32)
    #[arg(long)]
    default_mode: Option<String>,
    /// Volume id (default: the source's)
    #[arg(long)]
    volume_id: Option<String>,
    /// A file with a product key (one line) for WINNT.SIF [UserData]; never printed
    #[arg(long)]
    product_key_file: Option<PathBuf>,
    /// Where extracted CDs are kept between runs (default ~/.cache/hvkit/cd)
    #[arg(long)]
    cache: Option<PathBuf>,
    /// Where the CD is assembled (default: OUT with .d appended; deleted first)
    #[arg(long)]
    work: Option<PathBuf>,
}

fn default_cache(cache: Option<PathBuf>) -> Result<PathBuf, String> {
    match cache {
        Some(c) => Ok(c),
        None => Ok(
            PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set; give --cache")?)
                .join(".cache/hvkit/cd"),
        ),
    }
}

fn default_work(out: &Path) -> PathBuf {
    let mut w = out.as_os_str().to_owned();
    w.push(".d");
    PathBuf::from(w)
}

fn read_key(p: &Option<PathBuf>) -> Result<Option<String>, String> {
    p.as_ref()
        .map(|p| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display())))
        .transpose()
}

pub fn run_hvfb(a: HvfbCdArgs) -> Result<(), String> {
    let default_mode = match &a.default_mode {
        None => None,
        Some(m) => {
            let v: Vec<u32> = m
                .split('x')
                .map(|x| {
                    x.parse()
                        .map_err(|_| format!("--default-mode {m}: expected e.g. 1024x768x32"))
                })
                .collect::<Result<_, _>>()?;
            let [w, h, bpp] = v[..] else {
                return Err(format!("--default-mode {m}: expected e.g. 1024x768x32"));
            };
            Some((w, h, bpp))
        }
    };
    let c = HvfbCd {
        cache: cache_dir(&default_cache(a.cache)?, &a.source),
        source: a.source,
        hvfb: a.hvfb,
        bootvid: a.bootvid,
        install: !a.no_install,
        default_mode,
        volume_id: a.volume_id,
        product_key: read_key(&a.product_key_file)?,
        work: a.work.unwrap_or_else(|| default_work(&a.out)),
        out: a.out,
    };
    media::hvfb_cd::build(&c, &mut |l| println!("{l}")).map_err(|e| e.to_string())
}

/// The cache directory of one CD: its file name without the extension.
fn cache_dir(base: &Path, iso: &Path) -> PathBuf {
    base.join(iso.file_stem().unwrap_or_default())
}

pub fn run(a: SetupCdArgs) -> Result<(), String> {
    let cache = default_cache(a.cache)?;
    let product_key = read_key(&a.product_key_file)?;
    let c = SetupCd {
        cache: cache_dir(&cache, &a.source),
        mp_source: a
            .mp_source
            .as_ref()
            .map(|m| (m.clone(), cache_dir(&cache, m))),
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
        work: a.work.unwrap_or_else(|| default_work(&a.out)),
        out: a.out,
        kd: a.kd,
        load_options: a.load_options,
        unattend: a.unattend,
        product_key,
        bootwait_timeout: a.bootwait_timeout,
        no_bootfix: a.no_bootfix,
        patch_ntldr: !a.no_patch_ntldr,
        hook: a.hook,
    };
    build(&c, &mut |l| println!("{l}")).map_err(|e| e.to_string())
}
