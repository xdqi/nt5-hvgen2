//! `hvkit setup-dvd`: a Windows NT 6.x (Vista and later) setup DVD for Hyper-V Generation 2
//! (see media::setup_dvd).

use clap::Args;
use media::setup_dvd::{SetupDvd, build};
use std::path::PathBuf;

#[derive(Args)]
pub struct SetupDvdArgs {
    /// The original DVD (only read)
    source: PathBuf,
    /// The ISO to write (an existing one is rewritten in place, keeping its ACL)
    out: PathBuf,
    /// A folder of driver packages (repeatable; every INF below it is one). They go in
    /// $WinPEDriver$ on the DVD, where Windows Setup loads them into Windows PE and adds them to the
    /// installed system, except the boot-critical ones, which Setup does not install unsigned: those
    /// need --replace-inbox. Pass every package of one Integration Services build together.
    #[arg(long = "driver")]
    drivers: Vec<PathBuf>,
    /// Where an image has its own package of a --driver package's name (Windows\inf\NAME.inf),
    /// make it that package: both copies of the INF, the files of its driver store folder and its
    /// installed copies in System32\drivers and System32. The images' own signed packages outrank
    /// the staged ones at device install, so without this Windows 7 runs its own vmbus.sys beside
    /// the package's hyperkbd.sys (keyboard code 10), and Windows PE its own VMBus and storage
    /// drivers. Not a standard interface: the image's own package is no longer what its catalog
    /// signed
    #[arg(long)]
    replace_inbox: bool,
    /// A file placed in both images, SRC=DEST (DEST below the Windows volume root, e.g.
    /// Windows/System32/drivers/mdlex.sys). Repeatable.
    #[arg(long = "copy")]
    copies: Vec<String>,
    /// A .reg file applied to one hive of both images, [HIVE:]FILE (default hive SYSTEM). The
    /// file's first key names the root: HKEY_LOCAL_MACHINE\W7\... Repeatable.
    #[arg(long = "reg")]
    regs: Vec<String>,
    /// hvfb.sys: the display for a machine with no VGA (Hyper-V Generation 2). It gets the registry
    /// a legacy VideoPort miniport needs, and is installed as the boot display of both images.
    #[arg(long)]
    hvfb: Option<PathBuf>,
    /// The mode the installed system starts in, WIDTHxHEIGHTxBPP (default 1024x768x32, Generation
    /// 2's frame buffer)
    #[arg(long)]
    mode: Option<String>,
    /// Keep SynthVid from being installed (device installation policy), so hvfb keeps the frame
    /// buffer; the alternative is to leave it out and let PnP take SynthVid instead of hvfb
    #[arg(long)]
    deny_synthvid: bool,
    /// PollBootPartitionTimeout in milliseconds: the boot disk is a VMBus child that storvsc reports
    /// late, and without this value nt!PnpBootDeviceWait does not wait at all. 0 leaves it alone
    #[arg(long, default_value_t = 30000)]
    poll_boot_partition: u32,
    /// The image of sources/boot.wim that is Windows Setup (default 2)
    #[arg(long, default_value_t = 2)]
    boot_index: u32,
    /// The image of sources/install.wim to change (default 1)
    #[arg(long, default_value_t = 1)]
    install_index: u32,
    /// Leave sources/boot.wim alone
    #[arg(long)]
    no_boot_wim: bool,
    /// Leave sources/install.wim alone
    #[arg(long)]
    no_install_wim: bool,
    /// autounattend.xml: answer the pages of the install (one partition over disk 0, Administrator
    /// with an empty password, auto-logon, OOBE skipped)
    #[arg(long)]
    unattend: bool,
    /// A file with the product key (one line; never printed)
    #[arg(long)]
    product_key_file: Option<PathBuf>,
    /// The name autounattend.xml gives the machine
    #[arg(long)]
    computer_name: Option<String>,
    /// autounattend.xml's processorArchitecture, if the image is not x86 (e.g. "amd64")
    #[arg(long)]
    arch: Option<String>,
    /// Where extracted DVDs are kept between runs (default ~/.cache/hvkit/dvd; one folder per
    /// DVD)
    #[arg(long)]
    cache: Option<PathBuf>,
    /// Where the DVD is assembled (default: OUT with .d appended; deleted first). Its files are
    /// hard links to the cache where both are on one filesystem; what the images are given waits in
    /// the same name with .tmp appended
    #[arg(long)]
    work: Option<PathBuf>,
    /// A bash script run in the tree just before mastering (as its working directory)
    #[arg(long)]
    hook: Option<PathBuf>,
}

/// "HOST=DEST" -> the two halves, with DEST kept in the forward-slash form the CLI takes.
fn pair(s: &str, what: &str) -> Result<(PathBuf, String), String> {
    let (a, b) = s
        .split_once('=')
        .ok_or_else(|| format!("{what} needs SRC=DEST, not {s:?}"))?;
    if a.is_empty() || b.is_empty() {
        return Err(format!("{what} needs SRC=DEST, not {s:?}"));
    }
    Ok((PathBuf::from(a), b.to_string()))
}

/// "HIVE:FILE" or "FILE" -> the hive it goes to and the file.
fn hive_and_file(s: &str) -> (String, PathBuf) {
    match s.split_once(':') {
        // A drive letter or a URL is not a hive name.
        Some((h, f))
            if h.len() > 1
                && h.chars().all(|c| c.is_ascii_alphabetic())
                && !h.eq_ignore_ascii_case("http") =>
        {
            (h.to_uppercase(), PathBuf::from(f))
        }
        _ => ("SYSTEM".to_string(), PathBuf::from(s)),
    }
}

pub fn run(a: SetupDvdArgs) -> Result<(), String> {
    let copies = a
        .copies
        .iter()
        .map(|s| pair(s, "--copy"))
        .collect::<Result<Vec<_>, _>>()?;
    let regs = a.regs.iter().map(|s| hive_and_file(s)).collect::<Vec<_>>();
    let work = a.work.clone().unwrap_or_else(|| {
        let mut w = a.out.as_os_str().to_owned();
        w.push(".d");
        PathBuf::from(w)
    });
    let cache = match &a.cache {
        Some(c) => c.clone(),
        None => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set; give --cache")?)
            .join(".cache/hvkit/dvd"),
    }
    // One directory per DVD: its file name without the extension (as setup-cd).
    .join(a.source.file_stem().unwrap_or_default());
    let c = SetupDvd {
        source: a.source,
        out: a.out,
        drivers: a.drivers,
        replace_inbox: a.replace_inbox,
        copies,
        regs,
        hvfb: a.hvfb,
        hvfb_mode: a.mode.unwrap_or_else(|| "1024x768x32".to_string()),
        deny_synthvid: a.deny_synthvid,
        poll_boot_partition_ms: a.poll_boot_partition,
        boot_index: a.boot_index,
        install_index: a.install_index,
        no_boot_wim: a.no_boot_wim,
        no_install_wim: a.no_install_wim,
        unattend: a.unattend,
        product_key_file: a.product_key_file,
        computer_name: a.computer_name,
        arch: a.arch,
        cache,
        work,
        hook: a.hook,
    };
    build(&c, &mut |l| println!("{l}")).map_err(|e| e.0)
}
