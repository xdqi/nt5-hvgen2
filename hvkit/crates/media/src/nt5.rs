//! The changes that make a Windows NT 5.x setup CD (XP SP3, Server 2003 SP2, XP Professional x64
//! SP2) install and boot on Hyper-V Generation 2 through CSMWrap, as data: `setup_cd` makes them in
//! an extracted CD, and each belongs to a `Part`, which is what an nLite addon would carry. The
//! reasons for each change are at the change.
//!
//! Text-mode setup gets (from `files`): hvfb.sys as the display miniport and the frame buffer
//! bootvid.dll; KMDF (wdf01000.sys, wdfldr.sys), the VMBus (vmbus.sys, winhv.sys, vmbkmcl.sys),
//! storvsc.sys with the KB943295 storport.sys, bootwait.sys; hyperkbd.sys, mapped as the keyboard;
//! NTLDR and SETUPLDR.BIN with the mode 12h highlight patch; the multiprocessor HAL for "ACPI
//! Multiprocessor PC" (on XP x64 its hal.dll with the `hal-clock` recipe). The installed system
//! gets, through HIVESYS.INF, the KMDF library key, hvfb as its boot display driver and critical
//! device database entries. The Integration Services' packages for GUI-mode Plug and Play come
//! from `packages`, changed so that they install Dynamic Memory, the Guest Service Interface and
//! SynthVid at 32 bpp with patched files on both versions, and on XP the VSS service too (see
//! `media::components`).

use crate::components::{self, Components, NtVersion, find_file};
use crate::{Error, Result, copy, copy_tree, fmt_err, io, read_text, remove_if_exists, write_text};
use formats::inf::Text;
use std::path::Path;

/// Drivers copied to \I386, or \AMD64 on an x64 CD (upper-case names; a compressed X.SY_ there
/// would be taken instead).
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
pub(crate) const PREDEV_VMBAUD: (&str, &str, &str) = (
    "{2a7f3e10-9c4d-4b8a-a6e5-7d1c0f3b8e62}",
    "{8b57f4e3-2a3c-4f6e-9c8d-1e5a70b9c4d2}",
    r"vmbaud\vmbaud.inf",
);
pub(crate) const PREDEV_GSI: (&str, &str, &str) = (
    "{eb765408-105f-49b6-b4aa-c123b64d17d4}",
    "{34d14be3-dee4-41c8-9ae7-6b174977c192}",
    r"vmic\vmic.inf",
);
/// hvfb's fixed VideoID, as in nt5-hvgen2's hvfb.inf.
const HVFB_VIDEO_ID: &str = "{449ECA2B-4408-4A8C-979B-72B866C035D8}";

/// How text-mode setup gets the partition it installs to, through WINNT.SIF (the same on XP and
/// Server 2003, whose setupdd.sys handle these keys alike). The new system gets C: in each case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Partition {
    /// The disk's FAT32 partition, as it is: [Data] AutoPartition=1 takes the first formatted
    /// partition with room and no Windows on it, [Unattended] FileSystem=LeaveAlone keeps its file
    /// system. Setup cannot make a FAT32 partition without asking (unattended too, it confirms a FAT
    /// format over 2 GB on screen), so the disk comes with one (`hvkit disk create D.vhdx --size 8G
    /// --part type=c,fat=32,ntldr`; with `ntldr` setup takes the boot sector for its own and keeps
    /// no bootsect.dos). The partition must not be active: with an active partition the CD asks
    /// "Press any key to boot from CD" and boots the disk, which has no system yet; setup makes it
    /// active. On an empty disk setup asks as with `None`.
    Fat,
    /// [Unattended] Repartition=Yes: setup deletes every partition on the first disk, makes one over
    /// all of it and quick-formats it NTFS, without asking.
    Ntfs,
    /// Setup asks for the partition and how to format it.
    None,
}

/// The part (an nLite addon) a change belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The drivers text mode boots from, their TXTSETUP.SIF and HIVESYS.INF lines, the keyboard,
    /// the kernel options.
    Core,
    /// The multiprocessor HAL for text mode (XP x64: its hal.dll).
    MpHal,
    /// hvfb and the frame buffer bootvid.dll.
    Display,
    /// The loaders' mode 12h highlight patch.
    Ntldr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A file in the CD's system directory (\I386, or \AMD64 on an x64 CD), or with `loader` in
    /// \I386 (NTLDR, SETUPLDR.BIN), replacing the CD's own in either form (X.SYS or X.SY_: SETUPLDR
    /// and setup's file copy take a compressed one over an uncompressed one). `sdf` is the
    /// [SourceDisksFiles] line of a file the CD does not list.
    File {
        name: String,
        bytes: Vec<u8>,
        loader: bool,
        sdf: Option<String>,
    },
    /// TXTSETUP.SIF: the line `old` of `section`, as the CD has it, becomes `new`.
    Replace {
        section: String,
        old: String,
        new: String,
    },
    /// TXTSETUP.SIF: lines appended to `section` (created at the end when the CD has none).
    Add { section: String, lines: Vec<String> },
    /// HIVESYS.INF, which builds the new system's SYSTEM hive: lines for [AddReg].
    HiveSys(Vec<String>),
}

pub struct Options<'a> {
    /// Our drivers and the user's Microsoft files (`setup_cd::SetupCd::files`).
    pub files: &'a Path,
    /// The kernel debugger on COM2 for text mode and the installed system.
    pub kd: bool,
    /// More kernel options for both.
    pub load_options: Option<&'a str>,
    /// Patch NTLDR and SETUPLDR.BIN (recipe ntldr).
    pub patch_ntldr: bool,
}

pub struct Plan {
    pub version: NtVersion,
    pub amd64: bool,
    pub changes: Vec<(Part, Change)>,
}

/// The NT version of a CD, from its TXTSETUP.SIF.
pub fn version(sif: &Text) -> Result<NtVersion> {
    let number = |k: &str| {
        sif.get("SetupData", k)
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| Error(format!("TXTSETUP.SIF: no [SetupData] {k}")))
    };
    NtVersion::from_numbers(number("MajorVersion")?, number("MinorVersion")?)
}

/// The line with key `k` in section `name` of `sif`; there must be exactly one.
fn line_by_key(sif: &Text, name: &str, k: &str) -> Result<String> {
    let fail = |n: usize| Error(format!("TXTSETUP.SIF: [{name}] {k}: {n} lines"));
    let (s, e) = sif.section(name).ok_or_else(|| fail(0))?;
    let hits: Vec<&String> = sif.lines[s..e]
        .iter()
        .filter(|l| formats::inf::key(l).as_deref() == Some(k))
        .collect();
    match hits[..] {
        [l] => Ok(l.clone()),
        _ => Err(fail(hits.len())),
    }
}

fn state(s: recipes::State) -> String {
    match s {
        recipes::State::Known(name) => format!("patched ({name})"),
        recipes::State::Untested => "patched (not one of the tested files)".into(),
        recipes::State::Patched => "already patched".into(),
    }
}

impl Plan {
    /// The changes for the extracted CD at `root` (only read), in the order `apply` makes them. Its
    /// TXTSETUP.SIF must list the multiprocessor HAL (an nLite'd CD may need them put back first).
    pub fn new(root: &Path, o: &Options, log: &mut dyn FnMut(String)) -> Result<Plan> {
        let i386 = root.join("I386");
        // XP Professional x64: the loaders stay in \I386, TXTSETUP.SIF, the hives and the drivers
        // are in \AMD64.
        let amd64 = root.join("AMD64/TXTSETUP.SIF").exists();
        let sys = if amd64 {
            root.join("AMD64")
        } else {
            i386.clone()
        };
        let sif = read_text(&sys.join("TXTSETUP.SIF"))?;
        let mut plan = Plan {
            version: version(&sif)?,
            amd64,
            changes: Vec::new(),
        };
        let read = |p: &Path| std::fs::read(p).map_err(io(p));
        let file = |name: &str, bytes: Vec<u8>, loader: bool, sdf: bool| Change::File {
            name: name.to_string(),
            bytes,
            loader,
            // Copied to system32\drivers on every install (same fields as vga.sys, source disk 1 =
            // \i386, or \amd64 on an x64 CD).
            sdf: sdf.then(|| format!("{name:<12} = 1,,,,,,4_,4,0,0,,1,4")),
        };
        let mut push = |part: Part, change: Change| plan.changes.push((part, change));

        // Server 2003 SP2 has STORPORT.SY_ (5.2.3790.3959), older than the KB943295 storport
        // (5.2.3790.4163) the IC's storvsc needs, whose DriverEntry fails on it (no \Driver\storvsc,
        // the SCSI controller never starts, 0x7B): the file replaces the CD's.
        for f in DRIVERS {
            let part = if f == "hvfb.sys" {
                Part::Display
            } else {
                Part::Core
            };
            push(part, file(f, read(&o.files.join(f))?, false, true));
        }
        // SETUPLDR and setup's file copy read bootvid.dll uncompressed when there is no BOOTVID.DL_.
        // There is no x64 build of ours yet; the CD's own then stays (it draws on VGA hardware, so
        // the boot screen and bug checks are not seen).
        if amd64 && !o.files.join("bootvid.dll").exists() {
            log("bootvid.dll: none in the files, the x64 CD's own stays".into());
        } else {
            let b = read(&o.files.join("bootvid.dll"))?;
            push(Part::Display, file("bootvid.dll", b, false, false));
        }
        // The x64 HAL sends the system clock to every local APIC (physical destination 0xff), which
        // a Generation 2 VM never delivers: the tick count stops once the application processors
        // start. The recipe sends it to the boot processor. The HAL goes uncompressed, like the
        // drivers.
        if amd64 {
            let cab = sys.join("HAL.DL_");
            let stock = if cab.exists() {
                formats::cab::extract(&cab, "hal.dll").map_err(fmt_err)?
            } else {
                read(&sys.join("HAL.DLL"))?
            };
            let p =
                recipes::hal_clock::apply(&stock).map_err(|e| Error(format!("hal.dll: {e}")))?;
            log(format!("hal.dll: {}", state(p.state)));
            push(Part::MpHal, file("hal.dll", p.bytes, false, false));
        }
        // The x64 CD's loaders are not among the files the recipe was made with, and its English
        // loaders draw no mode 12h menu anyway.
        if o.patch_ntldr && amd64 {
            log("NTLDR, SETUPLDR.BIN: not patched on an x64 CD".into());
        }
        if o.patch_ntldr && !amd64 {
            for f in ["NTLDR", "SETUPLDR.BIN"] {
                match recipes::ntldr::apply(&read(&i386.join(f))?) {
                    Ok(p) => {
                        log(format!("{f}: {}", state(p.state)));
                        push(Part::Ntldr, file(f, p.bytes, true, false));
                    }
                    // A loader the recipe does not know stays as it is.
                    Err(e) => log(format!("{f}: not patched: {e}")),
                }
            }
        }

        let replace = |section: &str, k: &str, new: &str| -> Result<Change> {
            Ok(Change::Replace {
                section: section.to_string(),
                old: line_by_key(&sif, section, k)?,
                new: new.to_string(),
            })
        };
        let add = |section: &str, lines: &[&str]| Change::Add {
            section: section.to_string(),
            lines: lines.iter().map(|l| l.to_string()).collect(),
        };
        let mut opts = String::from("/fastdetect /noguiboot");
        opts.push_str(if o.kd {
            " /debug /debugport=com2 /baudrate=115200"
        } else {
            " /nodebug"
        });
        if let Some(l) = o.load_options {
            opts.push(' ');
            opts.push_str(l);
        }
        push(
            Part::Core,
            replace(
                "SetupData",
                "osloadoptions",
                &format!("OsLoadOptions = \"{opts}\""),
            )?,
        );
        push(
            Part::Display,
            replace("Display.Load", "vga", "vga      = hvfb.sys")?,
        );
        // Text mode runs the MP kernel (ntkrnlmp) but [Hal.Load] gives "ACPI Multiprocessor PC" the
        // UP HAL halaacpi, whose KfAcquire/KfReleaseSpinLock only change the IRQL. vmbkmcl takes a
        // channel lock with the kernel's KefAcquireSpinLockAtDpcLevel, which sets the lock bit, so
        // with that pair the bit stays set and the next VMBus interrupt DPC spins forever. The MP HAL
        // matches the kernel (needs csmwrap.ini madt_pcat_compat = true, like the installed system).
        // (x64 has one HAL, hal.dll, for both.)
        if !amd64 {
            push(
                Part::MpHal,
                replace("Hal.Load", "acpiapic_mp", "acpiapic_mp    = halmacpi.dll")?,
            );
        }
        // setupldr loads the drivers of the .Load sections; setupdd installs each descriptor
        // section's drivers as boot drivers in that section's group: KMDF in Boot Bus Extender, its
        // client vmbus (and bootwait, whose group does not matter) in System Bus Extender, as the IC
        // install them.
        for c in [
            add("BootBusExtenders.Load", &["Wdf01000 = wdf01000.sys"]),
            add(
                "BootBusExtenders",
                &["Wdf01000 = \"Kernel Mode Driver Framework\",files.Wdf01000,Wdf01000"],
            ),
            add("files.Wdf01000", &["wdf01000.sys,4", "wdfldr.sys,4"]),
            add(
                "BusExtenders.Load",
                &["vmbus    = vmbus.sys", "bootwait = bootwait.sys"],
            ),
            add(
                "BusExtenders",
                &[
                    "vmbus    = \"Virtual Machine Bus\",files.vmbus,vmbus",
                    "bootwait = \"Boot Device Wait\",files.bootwait,bootwait",
                ],
            ),
            add(
                "files.vmbus",
                &["vmbus.sys,4", "winhv.sys,4", "vmbkmcl.sys,4"],
            ),
            add("files.bootwait", &["bootwait.sys,4"]),
            add("InputDevicesSupport.Load", &["hyperkbd = hyperkbd.sys"]),
            add(
                "InputDevicesSupport",
                &["hyperkbd = \"Hyper-V Keyboard\",files.hyperkbd,hyperkbd"],
            ),
            add("files.hyperkbd", &["hyperkbd.sys,4"]),
            add(
                "Keyboard",
                &["hyperkbd = \"Hyper-V Keyboard\",files.hyperkbd,hyperkbd"],
            ),
        ] {
            push(Part::Core, c);
        }
        push(Part::Core, map_unknown_keyboard(&sif)?);
        for c in [
            add("SCSI.Load", &["storvsc  = storvsc.sys,4"]),
            add(
                "SCSI",
                &["storvsc  = \"Microsoft Hyper-V SCSI Controller\""],
            ),
            add(
                "HardwareIdsDatabase",
                &[
                    r#"ACPI\VMBus = "vmbus",{4D36E97D-E325-11CE-BFC1-08002BE10318}"#,
                    r#"VMBUS\{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f} = "storvsc",{4D36E97B-E325-11CE-BFC1-08002BE10318}"#,
                    r#"VMBUS\{f912ad6d-2b17-48ea-bd65-f927a61c7684} = "hyperkbd",{4D36E96B-E325-11CE-BFC1-08002BE10318}"#,
                ],
            ),
        ] {
            push(Part::Core, c);
        }
        let (core, display) = hivesys_lines();
        push(Part::Core, Change::HiveSys(core));
        push(Part::Display, Change::HiveSys(display));
        if o.kd {
            push(
                Part::Core,
                Change::HiveSys(vec![
                    r#"HKLM,"SYSTEM\CurrentControlSet\Control\CrashControl","AutoReboot",0x00010001,0"#
                        .to_string(),
                ]),
            );
        }
        Ok(plan)
    }

    /// Makes the changes in the extracted CD at `root`.
    pub fn apply(&self, root: &Path, log: &mut dyn FnMut(String)) -> Result<()> {
        let i386 = root.join("I386");
        let sys = if self.amd64 {
            root.join("AMD64")
        } else {
            i386.clone()
        };
        let sif_path = sys.join("TXTSETUP.SIF");
        // ANSI, GBK on the zh-hans CDs; held byte for byte.
        let mut sif = read_text(&sif_path)?;
        let fail = |e: formats::Error| Error(format!("TXTSETUP.SIF: {e}"));
        let mut hivesys = Vec::new();
        for (_, change) in &self.changes {
            match change {
                Change::File {
                    name,
                    bytes,
                    loader,
                    sdf,
                } => {
                    let dir = if *loader { &i386 } else { &sys };
                    let up = name.to_uppercase();
                    let p = dir.join(&up);
                    std::fs::write(&p, bytes).map_err(io(&p))?;
                    if let Some((stem, ext)) = up.rsplit_once('.') {
                        let cab = format!("{stem}.{}_", &ext[..ext.len().min(3) - 1]);
                        remove_if_exists(&dir.join(cab))?;
                    }
                    if let Some(l) = sdf {
                        sif.add("SourceDisksFiles", std::slice::from_ref(l));
                    }
                }
                Change::Replace { section, old, new } => {
                    let (s, e) = sif
                        .section(section)
                        .ok_or_else(|| fail(formats::Error(format!("[{section}] not found"))))?;
                    let hits: Vec<usize> = (s..e).filter(|&i| sif.lines[i] == *old).collect();
                    let [i] = hits[..] else {
                        return Err(Error(format!(
                            "TXTSETUP.SIF: [{section}] {old:?}: {} lines",
                            hits.len()
                        )));
                    };
                    sif.lines[i] = new.clone();
                    log(format!("TXTSETUP.SIF: [{section}] {new}"));
                }
                Change::Add { section, lines } => sif.add(section, lines),
                Change::HiveSys(lines) => hivesys.extend(lines.iter().cloned()),
            }
        }
        write_text(&sif_path, &sif)?;
        insert_hivesys(&sys.join("HIVESYS.INF"), hivesys, log)
    }
}

/// HIVESYS.INF [AddReg] lines: (core, display).
fn hivesys_lines() -> (Vec<String>, Vec<String>) {
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
    let core = vec![
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
    ];
    // hvfb: legacy VideoPort miniport, boot started in group Video, with a fixed VideoID and the keys
    // videoprt would otherwise create on the first boot (like VgaSave's).
    let mut display = vec![
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
        display.extend(dev.iter().map(|(n, v)| format!(r#"{ccs}{key}","{n}",{v}"#)));
        display.extend(
            mode.iter()
                .map(|(n, v)| format!(r#"{ccs}{key}","DefaultSettings.{n}",0x00010001,{v}"#)),
        );
    }
    (core, display)
}

/// HIVESYS.INF: `add` into [AddReg] after VgaSave's lines. It is UTF-16 on the CD; nLite writes it
/// back as ANSI. Either is kept as it is.
fn insert_hivesys(path: &Path, add: Vec<String>, log: &mut dyn FnMut(String)) -> Result<()> {
    let mut t = read_text(path)?;
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

/// The keyboard type: setupdd takes it from PnP matches of i8042prt and kbdhid only, otherwise from
/// the identifier NTDETECT reports, through [Map.Keyboard]. On Generation 2 that is UNKNOWN_KEYBOARD:
/// SETUPLDR passes NTDETECT NOLEGACY because the FADT has no 8042. XP's map lacks it (text mode shows
/// "keyboard: unknown" and refuses to go on), Server 2003's and XP x64's map it to "No Keyboard".
/// Mapped to the [Keyboard] entry hyperkbd, setup installs hyperkbd as the keyboard driver. setupdd
/// takes the first line whose value matches, so a stock line for it (5.2's `none`) is replaced in
/// place; XP has none and gets the line appended.
fn map_unknown_keyboard(sif: &Text) -> Result<Change> {
    const ID: &str = "\"UNKNOWN_KEYBOARD\"";
    let line = format!("hyperkbd = {ID}");
    let Some((s, e)) = sif.section("Map.Keyboard") else {
        return Err(Error("TXTSETUP.SIF: [Map.Keyboard] not found".into()));
    };
    let section = "Map.Keyboard".to_string();
    Ok(
        match sif.lines[s..e]
            .iter()
            .find(|l| l.split_once('=').is_some_and(|(_, v)| v.trim() == ID))
        {
            Some(old) => Change::Replace {
                section,
                old: old.clone(),
                new: line,
            },
            None => Change::Add {
                section,
                lines: vec![line],
            },
        },
    )
}

/// The Integration Services packages for GUI-mode Plug and Play, one directory each in `dest`: the
/// IC's (from `ic`), storvsc with the KB943295 storport (the IC's storvsc.inf installs a NULL driver
/// on XP), the components' patched files and INFs, and SynthVid held until setup's reboot.
pub fn packages(
    dest: &Path,
    ic: &Path,
    files: &Path,
    comps: Components,
    amd64: bool,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    for p in IC_PACKAGES {
        copy_tree(&ic.join(p), &dest.join(p))?;
    }
    std::fs::create_dir_all(dest.join("storvsc")).map_err(io(dest))?;
    for f in ["storvsc-xp.inf", "storvsc.sys", "storport.sys"] {
        copy(&files.join(f), &dest.join("storvsc").join(f))?;
    }
    patch_packages(dest, files, comps, amd64, log)?;
    // Synthetic video (the IC's SynthVid) must not start before setup's reboot. GUI-mode PnP installs
    // vmbusvideo.inf and would start SynthVid at once; it takes the synthetic video channel and the
    // VRAM, so hvfb's frame buffer goes dead: drawing crawls and the display watchdog stops setup (XP:
    // 0xEA in framebuf), or win32k's MMX reads of it fault (Server 2003: 0x7F/0xD). A Reboot directive
    // in the DDInstall section makes setupapi leave the device for the next boot, when SynthVid takes
    // over from hvfb, which desk.cpl disables during the install.
    let vv = dest.join("vmbusvideo/vmbusvideo.inf");
    let mut inf = read_text(&vv)?;
    let Some(at) = inf.lines.iter().position(|l| l == "[SynthVid_Install]") else {
        return Err(Error("vmbusvideo.inf: [SynthVid_Install] not found".into()));
    };
    inf.lines.insert(at + 1, "Reboot".into());
    write_text(&vv, &inf)
}

/// A device that first turns up after setup is installed by Plug and Play's non-interactive server
/// side, which refuses every unsigned file whatever the signing policy (only GUI-mode setup and the
/// Found New Hardware wizard honour Ignore), so it would get the wizard. predev.exe, run near the end
/// of GUI-mode setup, creates its device node ahead of time and installs the driver on it then; when
/// the device appears it is an installed one. A node that is there already with a driver (the
/// device was present during setup) is left alone. On x64 it must be the x64 build: SetupAPI does
/// not let a 32-bit process install devices there (ERROR_IN_WOW64). Returns predev.exe and its
/// devices (instance, interface type, the INF in its package), if a component needs it.
#[allow(clippy::type_complexity)]
pub fn predev(
    files: &Path,
    comps: Components,
    amd64: bool,
) -> Result<Option<(Vec<u8>, Vec<(&'static str, &'static str, &'static str)>)>> {
    let devices: Vec<_> = [(comps.vmbaud, PREDEV_VMBAUD), (comps.gsi, PREDEV_GSI)]
        .into_iter()
        .filter(|d| d.0)
        .map(|d| d.1)
        .collect();
    if devices.is_empty() {
        return Ok(None);
    }
    let p = files.join("predev.exe");
    let b = std::fs::read(&p).map_err(io(&p))?;
    let machine = formats::pe::Pe::parse(&b, 0)
        .and_then(|pe| pe.machine(&b))
        .map_err(|e| Error(format!("{}: {e}", p.display())))?;
    let want = if amd64 {
        formats::pe::IMAGE_FILE_MACHINE_AMD64
    } else {
        formats::pe::IMAGE_FILE_MACHINE_I386
    };
    if machine != want {
        return Err(Error(format!(
            "{}: machine 0x{machine:x}, this CD needs the {} build",
            p.display(),
            if amd64 { "x64 (predev64.exe)" } else { "x86" }
        )));
    }
    Ok(Some((b, devices)))
}

/// The components that need patched files (`media::components`), in the packages under `hv`. On
/// XP the Integration Services' INFs give the Dynamic Memory and VSS devices NULL drivers, which they
/// install for real on Server 2003 ([Standard.NT.5.2]); their copies here are changed to do on XP
/// what they do there, and on both to take the patched files, so that GUI-mode Plug and Play
/// installs everything itself and nothing has to be repaired on later boots. The [Standard] models
/// are XP's alone; the install sections are shared, and a CD is one version. On an x64 CD (`amd64`,
/// Dynamic Memory only) it is XP Professional x64's models section that gets the change. The
/// packages' catalogs no longer match: setup needs DriverSigningPolicy=Ignore.
fn patch_packages(
    hv: &Path,
    files: &Path,
    comps: Components,
    amd64: bool,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    let read = |p: &Path| std::fs::read(p).map_err(io(p));
    if comps.dynamic_memory {
        let dir = hv.join("dmvsc");
        let sys = find_file(&dir, "dmvsc.sys")?;
        let b = components::dmvsc(&read(&sys)?, log)?;
        std::fs::write(&sys, b).map_err(io(&sys))?;
        copy(&files.join("mdlex.sys"), &dir.join("mdlex.sys"))?;
        let edit = if amd64 { dmvsc_inf_x64 } else { dmvsc_inf };
        edit_inf(&find_file(&dir, "dmvsc.inf")?, edit)?;
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
    Ok(())
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
    dmvsc_inf_models(t, "Standard")
}

/// dmvsc.inf on XP Professional x64 (the same INF as x86's): the models section of a 5.2 x64
/// workstation (ProductType 1), which the INF gives the NULL driver on purpose ("Block installation
/// of 5.2 Workstation"), gets the DynMemDriver install of the x64 server models. setupapi takes the
/// most specific models section, so this is the one XP x64 reads.
fn dmvsc_inf_x64(t: &mut Text) -> formats::Result<()> {
    dmvsc_inf_models(t, "Standard.NTamd64.5.2.0x0000001")
}

fn dmvsc_inf_models(t: &mut Text, models: &str) -> formats::Result<()> {
    edit_line(t, models, DMVSC_HWID, |_| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// XP's map has no line for UNKNOWN_KEYBOARD (appended), 5.2's maps it to none (replaced there).
    #[test]
    fn unknown_keyboard_map() {
        let parse = |s: &str| Text::parse(s.as_bytes()).unwrap();
        let line = "hyperkbd = \"UNKNOWN_KEYBOARD\"".to_string();
        let xp = parse("[Map.Keyboard]\r\nSTANDARD = \"101-KEY\"\r\n\r\n[Map.PROM]\r\n");
        assert_eq!(
            map_unknown_keyboard(&xp).unwrap(),
            Change::Add {
                section: "Map.Keyboard".into(),
                lines: vec![line.clone()]
            }
        );
        let k3 = parse(
            "[Map.Keyboard]\r\nnone     = \"NO KEYBOARD\"\r\nnone     = \"UNKNOWN_KEYBOARD\"\r\n",
        );
        assert_eq!(
            map_unknown_keyboard(&k3).unwrap(),
            Change::Replace {
                section: "Map.Keyboard".into(),
                old: "none     = \"UNKNOWN_KEYBOARD\"".into(),
                new: line
            }
        );
    }

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
        // The x64 package has the same vmic.inf, whose NTamd64 model (XP x64) installs the Guest
        // Service Interface with the same section as XP's: the edit needs no x64 variant.
        assert!(
            line_with(&ic, "Standard.NTamd64", PREDEV_GSI.0).contains("VmIcGuestInterface_NT5,")
        );
    }

    /// The x64 package's dmvsc.inf is the x86 one byte for byte, so the same test file serves.
    #[test]
    fn x64_inf_edit() {
        let Some(stock) = ic_inf("dmvsc.inf") else {
            eprintln!("HVKIT_TESTDATA not set; skipped");
            return;
        };
        let mut dm = stock.clone();
        dmvsc_inf_x64(&mut dm).unwrap();
        let xp64 = "Standard.NTamd64.5.2.0x0000001";
        assert!(line_with(&stock, xp64, DMVSC_HWID).contains("=DynMemDriver_NULL,"));
        assert!(line_with(&dm, xp64, DMVSC_HWID).contains("=DynMemDriver,"));
        assert!(line_with(&dm, "Drivers_Dir", "mdlex.sys").trim() == "mdlex.sys");
        assert!(dm.get("SourceDisksFiles", "mdlex.sys").as_deref() == Some("1"));
        // The other models are not touched.
        for sec in [
            "Standard",
            "Standard.NT.5.2",
            "Standard.NTamd64.5.2.0x0000002",
            "Standard.NTamd64.5.2.0x0000003",
            "Standard.NTamd64.6.0",
        ] {
            assert_eq!(
                line_with(&dm, sec, DMVSC_HWID),
                line_with(&stock, sec, DMVSC_HWID)
            );
        }
    }

    /// An x64 CD's SynthVid, from the x64 package (lower-case names there).
    #[test]
    fn x64_packages() {
        let Some(dir) = std::env::var_os("HVKIT_TESTDATA").map(PathBuf::from) else {
            eprintln!("HVKIT_TESTDATA not set; skipped");
            return;
        };
        let t = crate::offline::TempDir::new("nt5-x64").unwrap();
        let hv = t.0.join("$OEM$/$1/Drivers/HV");
        let vv = hv.join("vmbusvideo");
        std::fs::create_dir_all(&vv).unwrap();
        let files = [
            ("VMBusVideoM-x64.sys", "vmbusvideom.sys"),
            ("VMBusVideoD-x64.dll", "vmbusvideod.dll"),
        ];
        for (name, ours) in files {
            copy(&dir.join("in").join(name), &vv.join(ours)).unwrap();
        }
        let comps = Components {
            synthvid: true,
            ..Components::default()
        };
        patch_packages(&hv, &t.0, comps, true, &mut |_| {}).unwrap();
        for (name, ours) in files {
            let expected = std::fs::read(dir.join("expected").join(name)).unwrap();
            assert!(std::fs::read(vv.join(ours)).unwrap() == expected, "{ours}");
        }
    }
}
