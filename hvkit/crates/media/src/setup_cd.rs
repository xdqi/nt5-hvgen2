//! A Windows NT 5.x setup CD that installs and boots on Hyper-V Generation 2 through CSMWrap: the
//! XP SP3 and Server 2003 SP2 CDs (also nLite'd ones). The source ISO is only read.
//!
//! Text-mode setup gets (from `files`): hvfb.sys as the display miniport and the frame buffer
//! bootvid.dll; KMDF (wdf01000.sys, wdfldr.sys), the VMBus (vmbus.sys, winhv.sys, vmbkmcl.sys),
//! storvsc.sys with the KB943295 storport.sys, bootwait.sys; hyperkbd.sys; NTLDR and SETUPLDR.BIN with
//! the mode 12h highlight patch; the multiprocessor HAL for "ACPI Multiprocessor PC", and for a CD that
//! nLite stripped of them the MP HALs, ntkrpamp.exe and their TXTSETUP.SIF lines from `mp_source`.
//! SETUPREG.HIV gets KMDF and the VMBus in their groups. The installed system gets, through HIVESYS.INF,
//! the KMDF library key, hvfb as its boot display driver and critical device database entries, and
//! through $OEM$\$1\Drivers\HV the Integration Services' INFs for GUI-mode Plug and Play (WINNT.SIF
//! OemPnPDriversPath), merged into the CD's own WINNT.SIF. Those INFs are changed so that they install
//! Dynamic Memory, the Guest Service Interface and SynthVid at 32 bpp with patched files on both
//! versions, and on XP the VSS service too (see `media::components`).
//!
//! The reasons for each change are in the comments at each step.

use crate::components::{self, Components, NtVersion, find_file};
use crate::{Error, Result, copy_tree};
use formats::inf::{Ini, Text};
use formats::iso9660::Iso;
use std::path::{Path, PathBuf};

pub struct SetupCd {
    /// The CD to start from.
    pub source: PathBuf,
    /// Its extracted copy, made if missing (".done" marks a complete one; same layout as `7z x`).
    pub cache: PathBuf,
    /// A CD of the same build with the multiprocessor HALs and kernel, and its cache, for a CD
    /// without them (nLite's "Multi-Processor Support" removal).
    pub mp_source: Option<(PathBuf, PathBuf)>,
    /// hvfb.sys bootwait.sys wdf01000.sys wdfldr.sys vmbus.sys winhv.sys vmbkmcl.sys storvsc.sys
    /// storport.sys hyperkbd.sys bootvid.dll storvsc-xp.inf; mdlex.sys for Dynamic Memory, predev.exe
    /// for the Guest Service Interface and vmbaud, vmbaud.inf and vmbaud.sys
    pub files: PathBuf,
    /// The Integration Services 6.3 driver packages (vmbus, synthkbd, vmbushid, vmbusvideo, vmic,
    /// netvsc, dmvsc), from an XP installation's Program Files.
    pub ic: PathBuf,
    /// Components to leave out of the version's defaults, and ones to add that are off by default
    /// (`Components::select`).
    pub leave_out: Components,
    pub opt_in: Components,
    /// The tree to assemble the CD in (deleted first).
    pub work: PathBuf,
    /// The ISO to write (an existing one is rewritten in place, which keeps its ACL).
    pub out: PathBuf,
    /// The kernel debugger on COM2 for text mode and the installed system.
    pub kd: bool,
    /// More kernel options for both.
    pub load_options: Option<String>,
    /// Answer the GUI-mode pages (text mode still asks for the partition).
    pub unattend: bool,
    pub product_key: Option<String>,
    /// bootwait's TimeoutSeconds in text mode.
    pub bootwait_timeout: u32,
    /// Drop I386\BOOTFIX.BIN ("Press any key to boot from CD"), for testing text mode.
    pub no_bootfix: bool,
    /// Patch NTLDR and SETUPLDR.BIN (recipe ntldr).
    pub patch_ntldr: bool,
    /// A bash script run in the tree just before mastering.
    pub hook: Option<PathBuf>,
}

/// Drivers copied to \I386 (upper-case names; a compressed X.SY_ there would be taken instead).
const DRIVERS: [&str; 10] = [
    "hvfb.sys",
    "bootwait.sys",
    "wdf01000.sys",
    "wdfldr.sys",
    "vmbus.sys",
    "winhv.sys",
    "vmbkmcl.sys",
    "storvsc.sys",
    "storport.sys",
    "hyperkbd.sys",
];
/// Integration Services packages for GUI-mode Plug and Play.
const IC_PACKAGES: [&str; 7] = [
    "vmbus",
    "synthkbd",
    "vmbushid",
    "vmbusvideo",
    "vmic",
    "netvsc",
    "dmvsc",
];
/// First hardware IDs of the Dynamic Memory and VSS devices (VMBus device classes).
const DMVSC_HWID: &str = r"vmbus\{525074DC-8985-46e2-8057-A307DC18A502}";
const VSS_HWID: &str = r"vmbus\{2450ee40-33bf-4fbd-892e-9fb06e9214cf}";
/// VMBus devices that may first turn up after setup, which predev.exe pre-installs: (instance,
/// interface type, package). Their instances are fixed: vmbaud-host.ps1's and vmbaudtray's default,
/// and the Integration Services' Guest Service Interface (off on a new VM unless enabled).
const PREDEV_VMBAUD: (&str, &str, &str) = (
    "{2a7f3e10-9c4d-4b8a-a6e5-7d1c0f3b8e62}",
    "{8b57f4e3-2a3c-4f6e-9c8d-1e5a70b9c4d2}",
    r"vmbaud\vmbaud.inf",
);
const PREDEV_GSI: (&str, &str, &str) = (
    "{eb765408-105f-49b6-b4aa-c123b64d17d4}",
    "{34d14be3-dee4-41c8-9ae7-6b174977c192}",
    r"vmic\vmic.inf",
);
/// hvfb's fixed VideoID, as in nt5-hvgen2's hvfb.inf.
const HVFB_VIDEO_ID: &str = "{449ECA2B-4408-4A8C-979B-72B866C035D8}";

pub(crate) fn io(p: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error(format!("{}: {e}", p.display()))
}

pub(crate) fn fmt_err(e: formats::Error) -> Error {
    Error(e.0)
}

/// Extracts `iso` into `dir` unless a complete copy is there, in the layout of `7z x`: the files,
/// and the boot image as [BOOT]/Boot-NoEmul.img.
pub fn extract_cached(iso: &Path, dir: &Path, log: &mut dyn FnMut(String)) -> Result<()> {
    if dir.join(".done").exists() {
        return Ok(());
    }
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(io(dir))?;
    }
    let mut i = Iso::open(iso).map_err(fmt_err)?;
    let n = i.extract_all(dir).map_err(fmt_err)?;
    let boot = i
        .boot_image()
        .map_err(fmt_err)?
        .ok_or_else(|| Error(format!("{}: not bootable", iso.display())))?;
    let b = dir.join("[BOOT]");
    std::fs::create_dir_all(&b).map_err(io(&b))?;
    std::fs::write(b.join("Boot-NoEmul.img"), boot).map_err(io(&b))?;
    std::fs::write(dir.join(".done"), b"").map_err(io(dir))?;
    log(format!(
        "extracted {} ({n} files) to {}",
        iso.display(),
        dir.display()
    ));
    Ok(())
}

pub(crate) fn read_text(p: &Path) -> Result<Text> {
    Text::parse(&std::fs::read(p).map_err(io(p))?)
        .map_err(|e| Error(format!("{}: {e}", p.display())))
}

pub(crate) fn write_text(p: &Path, t: &Text) -> Result<()> {
    std::fs::write(
        p,
        t.to_bytes()
            .map_err(|e| Error(format!("{}: {e}", p.display())))?,
    )
    .map_err(io(p))
}

pub(crate) fn copy(from: &Path, to: &Path) -> Result<()> {
    std::fs::copy(from, to)
        .map_err(|e| Error(format!("{} -> {}: {e}", from.display(), to.display())))?;
    Ok(())
}

pub(crate) fn remove_if_exists(p: &Path) -> Result<()> {
    match std::fs::remove_file(p) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(p)(e)),
        _ => Ok(()),
    }
}

/// Whether some line of the file is `key = ...` (any section, any case; spaces before the key only
/// with `indent`), as `grep -i '^ *key *='` finds.
fn has_key_line(t: &Text, k: &str, indent: bool) -> bool {
    t.lines.iter().any(|l| {
        let l = l.to_lowercase();
        let l = if indent {
            l.trim_start_matches(' ')
        } else {
            l.as_str()
        };
        l.strip_prefix(k)
            .is_some_and(|rest| rest.trim_start_matches(' ').starts_with('='))
    })
}

/// Copies the extracted CD (cached) to `root`, with the boot image as /boot.img.
pub(crate) fn stage(
    source: &Path,
    cache: &Path,
    root: &Path,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    extract_cached(source, cache, log)?;
    if root.exists() {
        std::fs::remove_dir_all(root).map_err(io(root))?;
    }
    copy_tree(cache, root)?;
    remove_if_exists(&root.join(".done"))?;
    std::fs::rename(root.join("[BOOT]/Boot-NoEmul.img"), root.join("boot.img"))
        .map_err(io(root))?;
    std::fs::remove_dir(root.join("[BOOT]")).map_err(io(root))?;
    Ok(())
}

/// Masters `root` as an NT 5.x setup CD into `out` (rewritten in place, keeping its ACL), checking
/// that the CD boot sector will find SETUPLDR.BIN.
pub(crate) fn master(
    root: &Path,
    volume_id: &str,
    out: &Path,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    let tmp = root.with_extension("iso");
    iso::build(root, &tmp, &iso::Options::nt5_setup(volume_id, "/boot.img"))
        .map_err(|e| Error(format!("{}: {e}", tmp.display())))?;
    // etfsboot reads at most 128 sectors of \I386 looking for SETUPLDR.BIN (and BOOTFIX.BIN).
    let off = Iso::open(&tmp)
        .and_then(|mut i| i.primary_record_offset("I386", "SETUPLDR.BIN"))
        .map_err(fmt_err)?
        .unwrap_or(0);
    log(format!("I386 directory: SETUPLDR.BIN record at {off}"));
    if off >= 128 * 2048 {
        return Err(Error(format!(
            "SETUPLDR.BIN's record is past the 128 sectors of \\I386 the CD boot sector reads ({off})"
        )));
    }
    // Overwrite in place: a new file would lose the ACL entry Hyper-V adds for the VM.
    let mut src = std::fs::File::open(&tmp).map_err(io(&tmp))?;
    let mut dst = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(out)
        .map_err(io(out))?;
    std::io::copy(&mut src, &mut dst).map_err(io(out))?;
    std::fs::remove_file(&tmp).map_err(io(&tmp))?;
    log(format!("wrote {}", out.display()));
    Ok(())
}

pub fn build(c: &SetupCd, log: &mut dyn FnMut(String)) -> Result<()> {
    let root = &c.work;
    stage(&c.source, &c.cache, root, log)?;
    let i386 = root.join("I386");
    let sif_path = i386.join("TXTSETUP.SIF");
    let mut sif = read_text(&sif_path)?;
    let number = |k: &str| {
        sif.get("SetupData", k)
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| Error(format!("TXTSETUP.SIF: no [SetupData] {k}")))
    };
    let version = NtVersion::from_numbers(number("MajorVersion")?, number("MinorVersion")?)?;
    let comps = Components::select(version, c.leave_out, c.opt_in);
    log(format!("{version}: components {}", comps.describe()));
    let winnt_path = i386.join("WINNT.SIF");
    if c.unattend
        && c.product_key.is_none()
        && !(winnt_path.exists() && has_key_line(&read_text(&winnt_path)?, "productkey", true))
    {
        return Err(Error(
            "unattended setup needs a product key (the CD's WINNT.SIF has none)".into(),
        ));
    }

    // Multiprocessor HALs and the PAE MP kernel from a CD of the same build, for a CD that nLite
    // stripped of them, with the TXTSETUP.SIF lines of the MP computer types.
    if !has_key_line(&sif, "acpiapic_mp", false) {
        let Some((mp_iso, mp_cache)) = &c.mp_source else {
            return Err(Error("this CD has no multiprocessor HAL (nLite'd?); give a CD of the same build that has them".into()));
        };
        extract_cached(mp_iso, mp_cache, log)?;
        for f in ["HALMACPI.DL_", "HALMPS.DL_", "HALSP.DL_"] {
            copy(&mp_cache.join("I386").join(f), &i386.join(f))?;
        }
        let k = formats::cab::extract(&mp_cache.join("I386/SP3.CAB"), "ntkrpamp.exe")
            .map_err(fmt_err)?;
        std::fs::write(i386.join("NTKRPAMP.EXE"), k).map_err(io(&i386))?;
        let src = read_text(&mp_cache.join("I386/TXTSETUP.SIF"))?;
        let mp = [
            "halmacpi.dll",
            "halmps.dll",
            "halsp.dll",
            "ntkrpamp.exe",
            "acpiapic_mp",
            "mps_mp",
            "syspro_mp",
            "mpkrnlpa",
        ];
        for (name, keys) in sif
            .merge_keys(&src, |k| mp.contains(&k))
            .map_err(|e| Error(format!("TXTSETUP.SIF: {e}")))?
        {
            log(format!("TXTSETUP.SIF: [{name}] +{}", keys.join(", ")));
        }
    }

    // SETUPLDR and setup's file copy take a compressed X.SY_ over an uncompressed X.SYS, so a CD's
    // own copy of one of these goes: Server 2003 SP2 has STORPORT.SY_ (5.2.3790.3959), older than the
    // KB943295 storport (5.2.3790.4163) the IC's storvsc needs, whose DriverEntry fails on it (no
    // \Driver\storvsc, the SCSI controller never starts, 0x7B).
    for f in DRIVERS {
        let up = f.to_uppercase();
        copy(&c.files.join(f), &i386.join(&up))?;
        remove_if_exists(&i386.join(format!("{}_", &up[..up.len() - 1])))?;
    }
    // SETUPLDR and setup's file copy read I386\bootvid.dll uncompressed when there is no BOOTVID.DL_.
    remove_if_exists(&i386.join("BOOTVID.DL_"))?;
    copy(&c.files.join("bootvid.dll"), &i386.join("BOOTVID.DLL"))?;
    if c.no_bootfix {
        remove_if_exists(&i386.join("BOOTFIX.BIN"))?;
    }
    if c.patch_ntldr {
        for f in ["NTLDR", "SETUPLDR.BIN"] {
            let p = i386.join(f);
            let data = std::fs::read(&p).map_err(io(&p))?;
            match recipes::ntldr::apply(&data) {
                Ok(o) => {
                    std::fs::write(&p, &o.bytes).map_err(io(&p))?;
                    log(format!(
                        "{f}: {}",
                        match o.state {
                            recipes::State::Known(name) => format!("patched ({name})"),
                            recipes::State::Untested =>
                                "patched (not one of the tested files)".into(),
                            recipes::State::Patched => "already patched".into(),
                        }
                    ));
                }
                // As before: a loader the recipe does not know stays as it is.
                Err(e) => log(format!("{f}: not patched: {e}")),
            }
        }
    }

    let mut opts = String::from("/fastdetect /noguiboot");
    opts.push_str(if c.kd {
        " /debug /debugport=com2 /baudrate=115200"
    } else {
        " /nodebug"
    });
    if let Some(o) = &c.load_options {
        opts.push(' ');
        opts.push_str(o);
    }

    // TXTSETUP.SIF (ANSI, GBK on the zh-hans CDs; held byte for byte).
    let fail = |e: formats::Error| Error(format!("TXTSETUP.SIF: {e}"));
    let lines = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    sif.replace(
        "SetupData",
        "osloadoptions",
        &format!("OsLoadOptions = \"{opts}\""),
    )
    .map_err(fail)?;
    sif.replace("Display.Load", "vga", "vga      = hvfb.sys")
        .map_err(fail)?;
    // Text mode runs the MP kernel (ntkrnlmp) but [Hal.Load] gives "ACPI Multiprocessor PC" the UP
    // HAL halaacpi, whose KfAcquire/KfReleaseSpinLock only change the IRQL. vmbkmcl takes a channel
    // lock with the kernel's KefAcquireSpinLockAtDpcLevel, which sets the lock bit, so with that pair
    // the bit stays set and the next VMBus interrupt DPC spins forever. The MP HAL matches the kernel
    // (needs csmwrap.ini madt_pcat_compat = true, like the installed system).
    sif.replace("Hal.Load", "acpiapic_mp", "acpiapic_mp    = halmacpi.dll")
        .map_err(fail)?;
    // Copied to system32\drivers on every install (same fields as vga.sys, source disk 1 = \i386).
    sif.add(
        "SourceDisksFiles",
        &DRIVERS
            .iter()
            .map(|f| format!("{f:<12} = 1,,,,,,4_,4,0,0,,1,4"))
            .collect::<Vec<_>>(),
    );
    // setupldr loads the drivers of the .Load sections; setupdd installs each descriptor section's
    // drivers as boot drivers in that section's group: KMDF in Boot Bus Extender, its client vmbus
    // (and bootwait, whose group does not matter) in System Bus Extender, as the IC install them.
    sif.add(
        "BootBusExtenders.Load",
        &lines(&["Wdf01000 = wdf01000.sys"]),
    );
    sif.add(
        "BootBusExtenders",
        &lines(&["Wdf01000 = \"Kernel Mode Driver Framework\",files.Wdf01000,Wdf01000"]),
    );
    sif.add(
        "files.Wdf01000",
        &lines(&["wdf01000.sys,4", "wdfldr.sys,4"]),
    );
    sif.add(
        "BusExtenders.Load",
        &lines(&["vmbus    = vmbus.sys", "bootwait = bootwait.sys"]),
    );
    sif.add(
        "BusExtenders",
        &lines(&[
            "vmbus    = \"Virtual Machine Bus\",files.vmbus,vmbus",
            "bootwait = \"Boot Device Wait\",files.bootwait,bootwait",
        ]),
    );
    sif.add(
        "files.vmbus",
        &lines(&["vmbus.sys,4", "winhv.sys,4", "vmbkmcl.sys,4"]),
    );
    sif.add("files.bootwait", &lines(&["bootwait.sys,4"]));
    sif.add(
        "InputDevicesSupport.Load",
        &lines(&["hyperkbd = hyperkbd.sys"]),
    );
    sif.add(
        "InputDevicesSupport",
        &lines(&["hyperkbd = \"Hyper-V Keyboard\",files.hyperkbd,hyperkbd"]),
    );
    sif.add("files.hyperkbd", &lines(&["hyperkbd.sys,4"]));
    // Keyboard type: NTDETECT finds no i8042, and setupdd only derives the type from PnP matches of
    // i8042prt, kbdhid and a few others, so text mode shows "keyboard: unknown" and refuses to go on.
    // WINNT.SIF [KeyboardDrivers] (below) picks this entry instead.
    sif.add(
        "Keyboard",
        &lines(&["hyperkbd = \"Hyper-V Keyboard\",files.hyperkbd,hyperkbd"]),
    );
    sif.add("SCSI.Load", &lines(&["storvsc  = storvsc.sys,4"]));
    sif.add(
        "SCSI",
        &lines(&["storvsc  = \"Microsoft Hyper-V SCSI Controller\""]),
    );
    sif.add(
        "HardwareIdsDatabase",
        &lines(&[
            r#"ACPI\VMBus = "vmbus",{4D36E97D-E325-11CE-BFC1-08002BE10318}"#,
            r#"VMBUS\{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f} = "storvsc",{4D36E97B-E325-11CE-BFC1-08002BE10318}"#,
            r#"VMBUS\{f912ad6d-2b17-48ea-bd65-f927a61c7684} = "hyperkbd",{4D36E96B-E325-11CE-BFC1-08002BE10318}"#,
        ]),
    );
    write_text(&sif_path, &sif)?;
    log(format!("TXTSETUP.SIF: OsLoadOptions = \"{opts}\""));

    hivesys(&i386.join("HIVESYS.INF"), c.kd, log)?;

    // $OEM$ at the CD root (CD installs look for it there): $1 is copied to the system drive in text
    // mode, and GUI-mode PnP searches the OemPnPDriversPath directories.
    let hv = root.join("$OEM$/$1/Drivers/HV");
    for p in IC_PACKAGES {
        copy_tree(&c.ic.join(p), &hv.join(p))?;
    }
    std::fs::create_dir_all(hv.join("storvsc")).map_err(io(&hv))?;
    for f in ["storvsc-xp.inf", "storvsc.sys", "storport.sys"] {
        copy(&c.files.join(f), &hv.join("storvsc").join(f))?;
    }
    extras(&root.join("$OEM$"), &hv, &c.files, comps, log)?;
    let mut dirs: Vec<String> = std::fs::read_dir(&hv)
        .map_err(io(&hv))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    dirs.sort();
    let pnp = dirs
        .iter()
        .map(|d| format!("Drivers\\HV\\{d}"))
        .collect::<Vec<_>>()
        .join(";");
    // Synthetic video (the IC's SynthVid) must not start before setup's reboot. GUI-mode PnP installs
    // vmbusvideo.inf and would start SynthVid at once; it takes the synthetic video channel and the
    // VRAM, so hvfb's frame buffer goes dead: drawing crawls and the display watchdog stops setup (XP:
    // 0xEA in framebuf), or win32k's MMX reads of it fault (Server 2003: 0x7F/0xD). A Reboot directive
    // in the DDInstall section makes setupapi leave the device for the next boot, when SynthVid takes
    // over from hvfb, which desk.cpl disables during the install.
    let vv = hv.join("vmbusvideo/vmbusvideo.inf");
    let mut inf = read_text(&vv)?;
    let Some(at) = inf.lines.iter().position(|l| l == "[SynthVid_Install]") else {
        return Err(Error("vmbusvideo.inf: [SynthVid_Install] not found".into()));
    };
    inf.lines.insert(at + 1, "Reboot".into());
    write_text(&vv, &inf)?;

    winnt_sif(c, &winnt_path, &sif, &pnp, log)?;
    setupreg(&i386.join("SETUPREG.HIV"), c.bootwait_timeout, log)?;

    if let Some(hook) = &c.hook {
        log(format!("hook: {}", hook.display()));
        let st = std::process::Command::new("bash")
            .arg(hook)
            .current_dir(root)
            .status()
            .map_err(io(hook))?;
        if !st.success() {
            return Err(Error(format!("{}: {st}", hook.display())));
        }
    }

    let volume_id = Iso::open(&c.source).map_err(fmt_err)?.volume_id;
    master(root, &volume_id, &c.out, log)
}

/// The components that need patched files (`media::components`), in the packages under `hv`. On
/// XP the Integration Services' INFs give the Dynamic Memory and VSS devices NULL drivers, which they
/// install for real on Server 2003 ([Standard.NT.5.2]); their copies here are changed to do on XP
/// what they do there, and on both to take the patched files, so that GUI-mode Plug and Play
/// installs everything itself and nothing has to be repaired on later boots. The [Standard] models
/// are XP's alone; the install sections are shared, and a CD is one version. The packages'
/// catalogs no longer match; WINNT.SIF has DriverSigningPolicy=Ignore.
fn extras(
    oem: &Path,
    hv: &Path,
    files: &Path,
    comps: Components,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    let read = |p: &Path| std::fs::read(p).map_err(io(p));
    if comps.dynamic_memory {
        let dir = hv.join("dmvsc");
        let sys = find_file(&dir, "dmvsc.sys")?;
        let b = components::dmvsc(&read(&sys)?, log)?;
        std::fs::write(&sys, b).map_err(io(&sys))?;
        copy(&files.join("mdlex.sys"), &dir.join("mdlex.sys"))?;
        edit_inf(&find_file(&dir, "dmvsc.inf")?, dmvsc_inf)?;
        log(
            "dmvsc.inf: the patched dmvsc.sys with mdlex.sys (on XP instead of the NULL driver)"
                .into(),
        );
    }
    if comps.vss || comps.gsi {
        let dir = hv.join("vmic");
        let icsvc = read(&find_file(&dir, "icsvc.dll")?)?;
        let inf = find_file(&dir, "vmic.inf")?;
        if comps.vss {
            let p = dir.join("icsvcvss.dll");
            std::fs::write(&p, components::icsvc_vss(&icsvc, log)?).map_err(io(&p))?;
            edit_inf(&inf, vmic_inf_vss)?;
            log("vmic.inf: VSS service from icsvcvss.dll instead of the NULL driver".into());
        }
        if comps.gsi {
            let p = dir.join("icsvcgsi.dll");
            std::fs::write(&p, components::icsvc_gsi(&icsvc, log)?).map_err(io(&p))?;
            edit_inf(&inf, vmic_inf_gsi)?;
            log("vmic.inf: Guest Service Interface from icsvcgsi.dll".into());
        }
    }
    if comps.synthvid {
        let dir = hv.join("vmbusvideo");
        for name in ["VMBusVideoM.sys", "VMBusVideoD.dll"] {
            let p = find_file(&dir, name)?;
            let b = components::synthvid(&read(&p)?, name, log)?;
            std::fs::write(&p, b).map_err(io(&p))?;
        }
    }
    // The sound card's device turns up only when the host offers it, after setup; its package in
    // OemPnPDriversPath ends up in the installed system's DevicePath, where PnP finds it then.
    if comps.vmbaud {
        let dir = hv.join("vmbaud");
        std::fs::create_dir_all(&dir).map_err(io(&dir))?;
        for f in ["vmbaud.inf", "vmbaud.sys"] {
            copy(&files.join(f), &dir.join(f))?;
        }
        log("vmbaud: Drivers\\HV\\vmbaud, for when the host offers the sound device".into());
    }
    // A device that first turns up after setup is installed by Plug and Play's non-interactive
    // server side, which refuses every unsigned file whatever the signing policy (only GUI-mode setup
    // and the Found New Hardware wizard honour Ignore), so it would get the wizard. predev.exe, run
    // from cmdlines.txt near the end of GUI-mode setup, creates its device node ahead of time and
    // installs the driver on it then; when the device appears it is an installed one. A node that is
    // there already with a driver (the device was present during setup) is left alone.
    let devices: Vec<_> = [(comps.vmbaud, PREDEV_VMBAUD), (comps.gsi, PREDEV_GSI)]
        .into_iter()
        .filter(|d| d.0)
        .map(|d| d.1)
        .collect();
    if !devices.is_empty() {
        copy(&files.join("predev.exe"), &hv.join("predev.exe"))?;
        let mut line = String::from(r#""cmd /c %SystemDrive%\Drivers\HV\predev.exe"#);
        for (inst, ty, inf) in &devices {
            line.push_str(&format!(r" {inst} {ty} %SystemDrive%\Drivers\HV\{inf}"));
        }
        line.push('"');
        add_cmdline(oem, &line)?;
        log(format!(
            "cmdlines.txt: predev.exe pre-installs {}",
            devices
                .iter()
                .map(|d| d.2.split('\\').next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(())
}

/// Adds a command to $OEM$\cmdlines.txt ([Commands]; GUI-mode setup runs them near its end, as
/// SYSTEM, with OemPreinstall=Yes), keeping a file the CD has already.
fn add_cmdline(oem: &Path, line: &str) -> Result<()> {
    let p = oem.join("cmdlines.txt");
    let mut t = if p.exists() {
        read_text(&p)?
    } else {
        Text::parse(b"[Commands]\r\n").map_err(fmt_err)?
    };
    t.add("Commands", &[line.to_string()]);
    write_text(&p, &t)
}

fn edit_inf(p: &Path, f: fn(&mut Text) -> formats::Result<()>) -> Result<()> {
    let mut t = read_text(p)?;
    let name = p.file_name().unwrap_or_default().to_string_lossy();
    f(&mut t).map_err(|e| Error(format!("{name}: {e}")))?;
    write_text(p, &t)
}

/// The one line of section `sec` that contains `needle` (any case), changed by `f`.
fn edit_line(
    t: &mut Text,
    sec: &str,
    needle: &str,
    f: impl Fn(&str) -> String,
) -> formats::Result<()> {
    let Some((s, e)) = t.section(sec) else {
        return Err(formats::Error(format!("section [{sec}] not found")));
    };
    let n = needle.to_lowercase();
    let hits: Vec<usize> = (s..e)
        .filter(|&i| t.lines[i].to_lowercase().contains(&n))
        .collect();
    let [i] = hits[..] else {
        return Err(formats::Error(format!(
            "[{sec}]: {} lines with {needle}",
            hits.len()
        )));
    };
    t.lines[i] = f(&t.lines[i]);
    Ok(())
}

/// `line` with its first `from` (any case) replaced by `to`.
fn replace_ci(line: &str, from: &str, to: &str) -> String {
    match line.to_lowercase().find(&from.to_lowercase()) {
        Some(i) => format!("{}{to}{}", &line[..i], &line[i + from.len()..]),
        None => line.to_string(),
    }
}

/// dmvsc.inf: XP gets the DynMemDriver install of 2003, which on both copies mdlex.sys too.
fn dmvsc_inf(t: &mut Text) -> formats::Result<()> {
    edit_line(t, "Standard", DMVSC_HWID, |_| {
        format!("%DynMemVsc.DeviceDesc%=DynMemDriver, {DMVSC_HWID}")
    })?;
    t.add("Drivers_Dir", &["mdlex.sys".to_string()]);
    t.add("SourceDisksFiles", &["mdlex.sys = 1".to_string()]);
    Ok(())
}

/// vmic.inf: `service` (VSS, GuestInterface) runs from `dll`, which its install section copies.
fn vmic_service_dll(t: &mut Text, service: &str, install: &str, dll: &str) -> formats::Result<()> {
    edit_line(
        t,
        &format!("{service}_AddReg_Common"),
        "\"ServiceDll\"",
        |l| replace_ci(l, "ICSvc.dll", dll),
    )?;
    let copy = format!("{service}_Dll_Copy");
    edit_line(t, install, "CopyFiles", |l| format!("{l},{copy}"))?;
    t.add(&copy, &[dll.to_string()]);
    t.add("DestinationDirs", &[format!("{copy} = 11")]);
    t.add("SourceDisksFiles", &[format!("{dll} = 1")]);
    Ok(())
}

/// vmic.inf on XP: the VSS service of 2003 (VmIcVss_NT5), from icsvcvss.dll.
fn vmic_inf_vss(t: &mut Text) -> formats::Result<()> {
    edit_line(t, "Standard", VSS_HWID, |_| {
        format!("%VSS.DeviceDesc% = VmIcVss_NT5, {VSS_HWID}")
    })?;
    vmic_service_dll(t, "VSS", "VmIcVss_NT5.NT", "icsvcvss.dll")
}

/// vmic.inf on XP: the Guest Service Interface (installed there already) from icsvcgsi.dll.
fn vmic_inf_gsi(t: &mut Text) -> formats::Result<()> {
    vmic_service_dll(
        t,
        "GuestInterface",
        "VmIcGuestInterface_NT5.NT",
        "icsvcgsi.dll",
    )
}

/// HIVESYS.INF builds the new system's SYSTEM hive; the lines go to [AddReg] after VgaSave's. It is
/// UTF-16 on the CD; nLite writes it back as ANSI. Either is kept as it is.
fn hivesys(path: &Path, kd: bool, log: &mut dyn FnMut(String)) -> Result<()> {
    let mut t = read_text(path)?;
    let ccs = r#"HKLM,"SYSTEM\CurrentControlSet"#;
    let vid = HVFB_VIDEO_ID;
    let dev = [
        ("InstalledDisplayDrivers", r#"0x00010000,"framebuf""#),
        ("VgaCompatible", "0x00010001,0"),
        (
            "Device Description",
            r#"0x00000000,"Linear frame buffer display (VBE / Hyper-V Gen2)""#,
        ),
    ];
    let mode = [
        ("BitsPerPel", 32),
        ("XResolution", 1024),
        ("YResolution", 768),
        ("VRefresh", 60),
        ("Flags", 0),
        ("XPanning", 0),
        ("YPanning", 0),
    ];
    let mut add: Vec<String> = vec![
        // WdfLdr finds the library for KMDF major version 1 here (the KMDF co-installer writes it).
        format!(
            r#"{ccs}\Control\Wdf\Kmdf\KmdfLibrary\Versions\1","Service",0x00000000,"Wdf01000""#
        ),
        format!(
            r#"{ccs}\Services\Wdf01000","ImagePath",0x00020000,"system32\DRIVERS\wdf01000.sys""#
        ),
        // Fallback bindings for the boot path and the keyboard until GUI-mode PnP installs their INFs.
        format!(r#"{ccs}\Control\CriticalDeviceDatabase\acpi#vmbus","Service",0x00000000,"vmbus""#),
        format!(
            r#"{ccs}\Control\CriticalDeviceDatabase\acpi#vmbus","ClassGUID",0x00000000,"{{4D36E97D-E325-11CE-BFC1-08002BE10318}}""#
        ),
        format!(
            r#"{ccs}\Control\CriticalDeviceDatabase\vmbus#{{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f}}","Service",0x00000000,"storvsc""#
        ),
        format!(
            r#"{ccs}\Control\CriticalDeviceDatabase\vmbus#{{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f}}","ClassGUID",0x00000000,"{{4D36E97B-E325-11CE-BFC1-08002BE10318}}""#
        ),
        format!(
            r#"{ccs}\Control\CriticalDeviceDatabase\vmbus#{{f912ad6d-2b17-48ea-bd65-f927a61c7684}}","Service",0x00000000,"hyperkbd""#
        ),
        format!(
            r#"{ccs}\Control\CriticalDeviceDatabase\vmbus#{{f912ad6d-2b17-48ea-bd65-f927a61c7684}}","ClassGUID",0x00000000,"{{4D36E96B-E325-11CE-BFC1-08002BE10318}}""#
        ),
        // hvfb: legacy VideoPort miniport, boot started in group Video, with a fixed VideoID and the
        // keys videoprt would otherwise create on the first boot (like VgaSave above).
        format!(r#"{ccs}\Services\hvfb","ErrorControl",0x00010001,0"#),
        format!(r#"{ccs}\Services\hvfb","Group",0x00000000,"Video""#),
        format!(
            r#"{ccs}\Services\hvfb","ImagePath",0x00020000,"\SystemRoot\System32\drivers\hvfb.sys""#
        ),
        format!(r#"{ccs}\Services\hvfb","Start",0x00010001,1"#),
        format!(r#"{ccs}\Services\hvfb","Type",0x00010001,1"#),
        format!(r#"{ccs}\Services\hvfb\Video","VideoID",0x00000000,"{vid}""#),
        format!(r#"{ccs}\Services\hvfb\Video","Service",0x00000000,"hvfb""#),
        format!(r#"{ccs}\Control\Video\{vid}\Video","Service",0x00000000,"hvfb""#),
    ];
    for key in [
        r"\Services\hvfb\Device0".to_string(),
        format!(r"\Control\Video\{vid}\0000"),
    ] {
        add.extend(dev.iter().map(|(n, v)| format!(r#"{ccs}{key}","{n}",{v}"#)));
        add.extend(
            mode.iter()
                .map(|(n, v)| format!(r#"{ccs}{key}","DefaultSettings.{n}",0x00010001,{v}"#)),
        );
    }
    if kd {
        add.push(format!(
            r#"{ccs}\Control\CrashControl","AutoReboot",0x00010001,0"#
        ));
    }
    let s = t
        .lines
        .iter()
        .position(|l| l.trim().to_lowercase() == "[addreg]")
        .ok_or_else(|| Error("HIVESYS.INF: no [AddReg]".into()))?;
    let e = (s + 1..t.lines.len())
        .find(|&i| t.lines[i].trim_start().starts_with('['))
        .unwrap_or(t.lines.len());
    let last = (s..e)
        .filter(|&i| {
            t.lines[i].contains(r"Services\VgaSave") || t.lines[i].contains(r"Video\{23A77BF7")
        })
        .max()
        .ok_or_else(|| Error("HIVESYS.INF: VgaSave's lines not found in [AddReg]".into()))?;
    let n = add.len();
    t.lines.splice(last + 1..last + 1, add);
    write_text(path, &t)?;
    log(format!("HIVESYS.INF: +{n} lines"));
    Ok(())
}

/// WINNT.SIF. [KeyboardDrivers] names the [Keyboard] entry of TXTSETUP.SIF: setupdd reads the
/// unattended hardware sections only with OemPreinstall=Yes (RETAIL entries need no TXTSETUP.OEM).
/// Text mode always asks for the partition (AutoPartition=0). These values are merged into the CD's
/// own WINNT.SIF if it has one (nLite's, with its product key and regional settings), ours winning,
/// except that the CD's [SetupData] OsLoadOptionsVar options stay in front of ours.
fn winnt_sif(
    c: &SetupCd,
    path: &Path,
    sif: &Text,
    pnp: &str,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    let mut ours = vec![
        "[Data]".to_string(),
        "MsDosInitiated=\"0\"".into(),
        "UnattendedInstall=\"Yes\"".into(),
        "AutoPartition=0".into(),
        String::new(),
        "[Unattended]".into(),
        "OemPreinstall=\"Yes\"".into(),
        format!("OemPnPDriversPath=\"{pnp}\""),
        "DriverSigningPolicy=Ignore".into(),
        "NonDriverSigningPolicy=Ignore".into(),
    ];
    let push = |v: &mut Vec<String>, l: &[&str]| v.extend(l.iter().map(|s| s.to_string()));
    if c.unattend {
        push(
            &mut ours,
            &[
                "UnattendMode=FullUnattended",
                "OemSkipEula=\"Yes\"",
                "UnattendSwitch=\"Yes\"",
                "WaitForReboot=\"No\"",
            ],
        );
        push(
            &mut ours,
            &[
                "",
                "[GuiUnattended]",
                "AdminPassword=*",
                "TimeZone=210",
                "OemSkipWelcome=1",
                "OemSkipRegional=1",
            ],
        );
        push(
            &mut ours,
            &[
                "",
                "[UserData]",
                "FullName=\"User\"",
                "OrgName=\"\"",
                "ComputerName=*",
            ],
        );
        push(
            &mut ours,
            &["", "[Networking]", "InstallDefaultComponents=\"Yes\""],
        );
        // Server CDs ([SetupData] ProductType 1-3) also ask for the licensing mode.
        let server = sif.lines.iter().any(|l| {
            let l = l.to_lowercase();
            let v = l
                .strip_prefix("producttype")
                .and_then(|r| r.trim_start_matches(' ').strip_prefix('='));
            v.is_some_and(|v| {
                v.trim_start_matches(' ')
                    .starts_with(|ch: char| ('1'..='9').contains(&ch))
            })
        });
        if server {
            push(
                &mut ours,
                &[
                    "",
                    "[LicenseFilePrintData]",
                    "AutoMode=PerServer",
                    "AutoUsers=5",
                ],
            );
        }
        if let Some(k) = &c.product_key {
            let k: String = k
                .chars()
                .filter(|ch| !matches!(ch, '\r' | '\n' | ' '))
                .collect();
            ours.extend([
                "".into(),
                "[UserData]".into(),
                format!("ProductKey=\"{k}\""),
            ]);
        }
    } else {
        push(
            &mut ours,
            &["UnattendMode=ProvideDefault", "OemSkipEula=\"No\""],
        );
    }
    push(
        &mut ours,
        &["", "[KeyboardDrivers]", "\"Hyper-V Keyboard\"=\"RETAIL\""],
    );
    if c.kd || c.load_options.is_some() {
        let mut o = String::new();
        if c.kd {
            o.push_str("/debug /debugport=com2 /baudrate=115200");
        }
        if let Some(l) = &c.load_options {
            o.push(' ');
            o.push_str(l);
        }
        ours.extend([
            "".into(),
            "[SetupData]".into(),
            format!("OsLoadOptionsVar=\"{o}\""),
        ]);
    }
    let exists = path.exists();
    let mut cd = if exists {
        let bytes = std::fs::read(path).map_err(io(path))?;
        Ini::parse(&bytes.iter().map(|&b| char::from(b)).collect::<String>())
    } else {
        Ini::default()
    };
    let new = Ini::parse(&ours.join("\n"));
    let old = cd.get("SetupData", "OsLoadOptionsVar").map(str::to_string);
    for (title, keys) in &new.sections {
        for (k, v) in keys {
            let v = match &old {
                Some(o)
                    if title.eq_ignore_ascii_case("SetupData")
                        && k.eq_ignore_ascii_case("OsLoadOptionsVar") =>
                {
                    format!("\"{} {}\"", o.trim_matches('"'), v.trim_matches('"'))
                }
                _ => v.clone(),
            };
            cd.set(title, k, &v);
        }
    }
    let mut out = vec![format!(
        "; hvkit setup-cd{}",
        if exists {
            ", merged into the CD's WINNT.SIF"
        } else {
            ""
        }
    )];
    out.extend(cd.to_lines());
    let text = out.join("\r\n") + "\r\n";
    let bytes: Vec<u8> = text
        .chars()
        .map(|ch| u8::try_from(u32::from(ch)).unwrap_or(b'?'))
        .collect();
    std::fs::write(path, bytes).map_err(io(path))?;
    for l in out
        .iter()
        .filter(|l| !l.to_lowercase().contains("productkey"))
    {
        log(format!("  WINNT.SIF: {l}"));
    }
    Ok(())
}

/// SETUPREG.HIV, the SYSTEM hive of text-mode setup: Wdf01000 in Boot Bus Extender and vmbus in
/// System Bus Extender (KMDF must be initialized before its clients), WdfLdr's library lookup key, and
/// bootwait's TimeoutSeconds.
fn setupreg(path: &Path, timeout: u32, log: &mut dyn FnMut(String)) -> Result<()> {
    let r = r"HKEY_LOCAL_MACHINE\SETUPREG\ControlSet001";
    let text = [
        "Windows Registry Editor Version 5.00".to_string(),
        String::new(),
        format!(r"[{r}\Services\Wdf01000]"),
        "\"Type\"=dword:00000001".into(),
        "\"Start\"=dword:00000000".into(),
        "\"ErrorControl\"=dword:00000000".into(),
        "\"Group\"=\"Boot Bus Extender\"".into(),
        r#""ImagePath"="system32\\DRIVERS\\wdf01000.sys""#.into(),
        String::new(),
        format!(r"[{r}\Control\Wdf\Kmdf\KmdfLibrary\Versions\1]"),
        "\"Service\"=\"Wdf01000\"".into(),
        String::new(),
        format!(r"[{r}\Services\vmbus]"),
        "\"Type\"=dword:00000001".into(),
        "\"Start\"=dword:00000000".into(),
        "\"ErrorControl\"=dword:00000001".into(),
        "\"Group\"=\"System Bus Extender\"".into(),
        r#""ImagePath"="system32\\DRIVERS\\vmbus.sys""#.into(),
        String::new(),
        format!(r"[{r}\Services\bootwait\Parameters]"),
        format!("\"TimeoutSeconds\"=dword:{timeout:08x}"),
    ]
    .join("\r\n");
    let mut h =
        hive::Hive::open(path, true).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    hive::reg::import(&mut h, &text, Some(r"HKEY_LOCAL_MACHINE\SETUPREG"))
        .map_err(|e| Error(format!("SETUPREG.HIV: {e}")))?;
    h.commit(None)
        .map_err(|e| Error(format!("{}: {e}", path.display())))?;
    drop(h);
    let hd = hive::Header::read(&std::fs::read(path).map_err(io(path))?)
        .map_err(|e| Error(format!("{}: {e}", path.display())))?;
    log(format!(
        "SETUPREG.HIV: seq {}/{} version {}.{}{}",
        hd.sequence.0,
        hd.sequence.1,
        hd.version.0,
        hd.version.1,
        if hd.dirty() { "  !! DIRTY" } else { "" }
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Integration Services' INFs are Microsoft files: HVKIT_TESTDATA/in has them (see the
    /// README); without it this test does nothing.
    fn ic_inf(name: &str) -> Option<Text> {
        let dir = PathBuf::from(std::env::var_os("HVKIT_TESTDATA")?);
        Some(read_text(&dir.join("in").join(name)).expect("INF"))
    }

    fn line_with<'a>(t: &'a Text, sec: &str, needle: &str) -> &'a str {
        let (s, e) = t.section(sec).expect("section");
        t.lines[s..e]
            .iter()
            .find(|l| l.to_lowercase().contains(&needle.to_lowercase()))
            .expect("line")
    }

    #[test]
    fn xp_inf_edits() {
        let (Some(mut dm), Some(mut ic)) = (ic_inf("dmvsc.inf"), ic_inf("vmic.inf")) else {
            eprintln!("HVKIT_TESTDATA not set; skipped");
            return;
        };
        let ic_2k3 = line_with(&ic, "Standard.NT.5.2", VSS_HWID).to_string();
        dmvsc_inf(&mut dm).unwrap();
        assert!(line_with(&dm, "Standard", DMVSC_HWID).contains("=DynMemDriver,"));
        assert!(line_with(&dm, "Drivers_Dir", "mdlex.sys").trim() == "mdlex.sys");
        assert!(dm.get("SourceDisksFiles", "mdlex.sys").as_deref() == Some("1"));

        vmic_inf_vss(&mut ic).unwrap();
        vmic_inf_gsi(&mut ic).unwrap();
        assert!(line_with(&ic, "Standard", VSS_HWID).contains("= VmIcVss_NT5,"));
        assert!(
            line_with(&ic, "VSS_AddReg_Common", "ServiceDll").contains(r"\System32\icsvcvss.dll")
        );
        assert!(
            line_with(&ic, "GuestInterface_AddReg_Common", "ServiceDll")
                .contains(r"\System32\icsvcgsi.dll")
        );
        assert_eq!(
            line_with(&ic, "VmIcVss_NT5.NT", "CopyFiles"),
            "CopyFiles=System_Dir,VSS_Dll_Copy"
        );
        assert_eq!(line_with(&ic, "VSS_Dll_Copy", "icsvcvss"), "icsvcvss.dll");
        assert_eq!(
            ic.get("DestinationDirs", "VSS_Dll_Copy").as_deref(),
            Some("11")
        );
        assert_eq!(
            ic.get("SourceDisksFiles", "icsvcgsi.dll").as_deref(),
            Some("1")
        );
        // Server 2003's models are not touched.
        assert_eq!(line_with(&ic, "Standard.NT.5.2", VSS_HWID), ic_2k3);
    }
}
