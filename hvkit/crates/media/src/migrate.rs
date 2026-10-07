//! A Windows XP SP3 x86 from a Hyper-V Generation 1 VM with the Integration Services 6.3, made to
//! boot on Generation 2 through CSMWrap: changed offline on a copy of its disk (raw or VHDX), whose
//! system volume must be FAT32 because CSMWrap goes onto it and the firmware reads only FAT. Then the
//! components of `inject`. Nothing is written before every file and the system have been checked.
//!
//! The registry changes go into the current control set only (LastKnownGood stays the Generation 1
//! configuration). The reasons for each are in the comments at each step.

use crate::components::{Components, NtVersion};
use crate::inject::{DeviceFix, bootwait_device, bootwait_patch, components};
use crate::offline::{Keys, SYSTEM32, Session, disk_err, find_in, host, ic_file, open_image};
use crate::{Error, Result};
use disk::SECTOR;
use disk::fat::{self, Fs};
use fatfs::{FatType, FileAttributes};
use formats::pe::Pe;
use std::path::{Path, PathBuf};

pub struct Migrate {
    /// The disk image; None checks the files alone.
    pub image: Option<PathBuf>,
    /// The partition (default: the FAT partition that has \WINDOWS\system32\config\system).
    pub partition: Option<usize>,
    /// Where the files to install are, searched in this order: csmwrap.efi dsdt.aml hvfb.sys
    /// bootwait.sys bootvid.dll storport.sys diskdump.sys, mdlex.sys for Dynamic Memory, vmbaud.inf
    /// vmbaud.sys for vmbaud; storvsc.sys dmvsc.sys dmvscres.dll if not from the Integration
    /// Services on the volume.
    pub files: Vec<PathBuf>,
    /// CSMWrap (default: csmwrap.efi from `files`).
    pub efi: Option<PathBuf>,
    /// The Integration Services packages (default: the copy below Program Files on the volume).
    pub ic: Option<PathBuf>,
    /// Of inject's components, as there; SynthVid is always left out (Plug and Play puts the stock
    /// files back when it installs SynthVid on the first Generation 2 boot).
    pub leave_out: Components,
    pub opt_in: Components,
    /// CSMWrap's log on COM1, a boot.ini entry with the kernel debugger on COM2 as the default, no
    /// automatic restart after a bug check.
    pub debug: bool,
    /// Leave Memory Management's DisablePagingExecutive as it is.
    pub keep_paging_executive: bool,
    /// Accept other versions of the Microsoft files whose version is checked, and dirty hives.
    pub force: bool,
    /// Check everything, write nothing.
    pub check: bool,
}

/// SHA-256 of the builds this was tested with (KB943295 ENU); other languages of the same build
/// differ.
const TESTED: [(&str, &str); 3] = [
    (
        "storvsc.sys",
        "ECD0071B7229BEB1CEC80A1F302A9864E35958AB7EF659780695E80A14B9E647",
    ),
    (
        "storport.sys",
        "F4349AA615559618D6AB5F1A98505BD37C14F9C955503D4581A6FA3D46F5D20C",
    ),
    (
        "diskdump.sys",
        "2784AE321240287915A36F25FB032839DAB203433E963086CCCA12A7F905BDE1",
    ),
];
/// FileVersion of the Integration Services 6.3 files (storvsc.sys, dmvscres.dll).
const IC_VERSION: &str = "6.3.9600.16384 ";
/// FileVersion of KB943295's storport.sys and diskdump.sys from the SP2 QFE branch (the SP2 RTM
/// storport rejects the Integration Services' storvsc with STATUS_REVISION_MISMATCH).
const KB943295_VERSION: &str = "5.2.3790.4163 (srv03_sp2_qfe.";
const CDDB: &str = r"Control\CriticalDeviceDatabase";
const SYSTEM_CLASS: &str = "{4D36E97D-E325-11CE-BFC1-08002BE10318}";
/// VMBus devices XP's Integration Services have no driver for (their INF installs a NULL driver):
/// bootwait gives them the names and the class of Windows 8's INF (wvmic.inf). An INF of our own
/// would need a signature, or XP shows the Found New Hardware wizard for it.
const NAMELESS: [(&str, &str); 3] = [
    (
        r"VMBUS\{3375baf4-9e15-4b30-b765-67acb10d607b}",
        "Microsoft Hyper-V Activation Component",
    ),
    (
        r"VMBUS\{f8e65716-3cb3-4a06-9a60-1889c5cccab5}",
        "Microsoft Hyper-V Remote Desktop Control Channel",
    ),
    (
        r"VMBUS\{f9e9c0d3-b511-4a48-8046-d38079a8830c}",
        "Microsoft Hyper-V Remote Desktop Data Channel",
    ),
];
/// hvfb's fixed VideoID, as in hvfb.inf.
const HVFB_VIDEO_ID: &str = "{449ECA2B-4408-4A8C-979B-72B866C035D8}";

/// The host files, found and checked.
struct HostFiles {
    efi: PathBuf,
    dsdt: PathBuf,
    hvfb: PathBuf,
    bootwait: PathBuf,
    bootvid: PathBuf,
    storport: PathBuf,
    diskdump: PathBuf,
}

/// Logs a Microsoft file's version and hash, and refuses a FileVersion that does not start with
/// `want` unless `force`.
fn check_version(
    name: &str,
    from: &dyn std::fmt::Display,
    data: &[u8],
    want: &str,
    force: bool,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    let v = Pe::parse(data, 0)
        .and_then(|pe| pe.file_version_string(data))
        .map_err(|e| Error(format!("{from}: {e}")))?
        .unwrap_or_default();
    let h = recipes::sha256_hex(data);
    let tested = match TESTED.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        Some((_, k)) if *k == h => " (the tested build)",
        Some(_) => " (not the tested ENU build)",
        None => "",
    };
    log(format!("{name}: {from}"));
    log(format!("  version {v}, SHA-256 {h}{tested}"));
    if !v.starts_with(want) {
        let m = format!("{from} has version {v:?}, expected {}*", want.trim_end());
        if !force {
            return Err(Error(format!("{m} (--force accepts it anyway)")));
        }
        log(format!("WARNING: {m}; accepted because of --force"));
    }
    Ok(())
}

/// An x64 EFI application: PE32+, AMD64, subsystem 10.
fn check_efi(p: &Path, log: &mut dyn FnMut(String)) -> Result<()> {
    let b = host(p)?;
    let u16_at = |o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32_at = |o: usize| {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    };
    let pe = u32_at(0x3c).unwrap_or(0) as usize;
    let ok = b.starts_with(b"MZ")
        && u32_at(pe) == Some(0x4550)
        && u16_at(pe + 4) == Some(0x8664)
        && u16_at(pe + 24) == Some(0x20b)
        && u16_at(pe + 24 + 68) == Some(10);
    if !ok {
        return Err(Error(format!(
            "{}: not an x64 EFI application",
            p.display()
        )));
    }
    log(format!("csmwrap.efi: {} ({} bytes)", p.display(), b.len()));
    Ok(())
}

/// A DSDT: signature, length and checksum.
fn check_dsdt(p: &Path, log: &mut dyn FnMut(String)) -> Result<()> {
    let b = host(p)?;
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    if b.len() < 36
        || &b[..4] != b"DSDT"
        || u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize != b.len()
        || sum != 0
    {
        return Err(Error(format!("{}: not a valid DSDT", p.display())));
    }
    log(format!(
        "dsdt.aml: {} (revision {}, {} bytes)",
        p.display(),
        b[8],
        b.len()
    ));
    Ok(())
}

/// Finds and checks the host files, reporting all missing ones at once.
fn host_files(c: &Migrate, comps: Components, log: &mut dyn FnMut(String)) -> Result<HostFiles> {
    let mut missing = Vec::new();
    let mut find = |name: &str, what: &str| {
        let p = find_in(&c.files, name);
        if p.is_none() {
            missing.push(format!("{name} ({what})"));
        }
        p.unwrap_or_default()
    };
    let efi = match &c.efi {
        Some(p) => p.clone(),
        None => find(
            "csmwrap.efi",
            "a release build of CSMWrap with Hyper-V Generation 2 support",
        ),
    };
    let dsdt = find("dsdt.aml", "nt5-hvgen2: acpi/build.sh");
    let hvfb = find("hvfb.sys", "nt5-hvgen2: make");
    let bootwait = find("bootwait.sys", "nt5-hvgen2: make");
    let bootvid = find("bootvid.dll", "nt5-hvgen2: make");
    let storport = find("storport.sys", "KB943295, SP2QFE");
    let diskdump = find("diskdump.sys", "KB943295, SP2QFE");
    if comps.dynamic_memory {
        find("mdlex.sys", "nt5-hvgen2: make");
    }
    if comps.vmbaud {
        find("vmbaud.inf", "nt5-hvgen2: make");
        find("vmbaud.sys", "nt5-hvgen2: make");
    }
    if !missing.is_empty() {
        let dirs: Vec<String> = c.files.iter().map(|d| d.display().to_string()).collect();
        return Err(Error(format!(
            "missing files (looked in {}):\n  {}",
            if dirs.is_empty() {
                "no --files".to_string()
            } else {
                dirs.join(", ")
            },
            missing.join("\n  ")
        )));
    }
    check_efi(&efi, log)?;
    check_dsdt(&dsdt, log)?;
    for (n, p) in [
        ("hvfb.sys", &hvfb),
        ("bootwait.sys", &bootwait),
        ("bootvid.dll", &bootvid),
    ] {
        log(format!("{n}: {}", p.display()));
    }
    for (n, p) in [("storport.sys", &storport), ("diskdump.sys", &diskdump)] {
        check_version(n, &p.display(), &host(p)?, KB943295_VERSION, c.force, log)?;
    }
    for n in ["storvsc.sys", "dmvsc.sys", "dmvscres.dll"] {
        if n != "storvsc.sys" && !comps.dynamic_memory {
            continue;
        }
        match find_in(&c.files, n) {
            Some(p) => check_version(n, &p.display(), &host(&p)?, IC_VERSION, c.force, log)?,
            None => log(format!("{n}: from the Integration Services on the disk")),
        }
    }
    Ok(HostFiles {
        efi,
        dsdt,
        hvfb,
        bootwait,
        bootvid,
        storport,
        diskdump,
    })
}

/// XP SP3 with the Integration Services 6.3 on FAT32.
fn check_system(fs: &Fs<'_>, log: &mut dyn FnMut(String)) -> Result<()> {
    let t = fs.fat_type();
    if t != FatType::Fat32 {
        return Err(Error(format!(
            "the system volume is {t:?}; only FAT32 is supported (CSMWrap goes onto it)"
        )));
    }
    let k = format!(r"{SYSTEM32}\ntoskrnl.exe");
    let b = fat::read(fs, &k).map_err(disk_err)?;
    let v = Pe::parse(&b, 0)
        .and_then(|pe| pe.file_version_string(&b))
        .map_err(|e| Error(format!("{k}: {e}")))?
        .unwrap_or_default();
    log(format!("ntoskrnl.exe {v}"));
    let n: Vec<u32> = v
        .split_whitespace()
        .next()
        .unwrap_or("")
        .split('.')
        .map(|x| x.parse().unwrap_or(0))
        .collect();
    if n.len() != 4 || n[..3] != [5, 1, 2600] || n[3] < 5512 {
        return Err(Error(
            "this is not Windows XP SP3 (ntoskrnl.exe 5.1.2600.5512 or later)".into(),
        ));
    }
    let p = format!(r"{SYSTEM32}\drivers\vmbus.sys");
    let b = fat::read(fs, &p).map_err(|_| {
        Error("no vmbus.sys: install the Hyper-V Integration Services (6.3.9600) on the Generation 1 VM first".into())
    })?;
    let v = Pe::parse(&b, 0)
        .and_then(|pe| pe.file_version_string(&b))
        .map_err(|e| Error(format!("{p}: {e}")))?
        .unwrap_or_default();
    log(format!("vmbus.sys {v}"));
    if !v.starts_with("6.3.9600.") {
        return Err(Error(format!(
            "vmbus.sys is {v}; the Integration Services of Windows Server 2012 R2 (6.3.9600) are required"
        )));
    }
    Ok(())
}

/// CSMWrap's settings: XP's ACPI HALs need the PC-AT compatibility flag, and XP's ACPI driver a
/// DSDT it can parse.
fn csmwrap_ini(debug: bool) -> Vec<u8> {
    let mut lines = vec![
        "; CSMWrap configuration for Windows XP on Hyper-V Generation 2 (written by hvkit migrate)",
        "; XP's ACPI HALs need the PC-AT compatibility flag, and XP's ACPI driver needs a DSDT it can parse.",
        "madt_pcat_compat = true",
        r"acpi_dsdt = \EFI\CSMWrap\dsdt.aml",
    ];
    if debug {
        lines.extend([
            "serial = true",
            "serial_port = 0x3f8",
            "serial_baud = 115200",
            "verbose = true",
        ]);
    }
    lines
        .iter()
        .map(|l| format!("{l}\r\n"))
        .collect::<String>()
        .into_bytes()
}

/// boot.ini with a copy of the default entry, with the kernel debugger on COM2, put first (NTLDR
/// boots the first entry whose ARC path matches default=); None if the default entry has /debug.
/// Latin-1 maps every byte to one character and back, so localised descriptions stay as they are.
fn debug_boot_entry(text: &[u8]) -> Result<Option<Vec<u8>>> {
    let s: String = text.iter().map(|&b| b as char).collect();
    let mut lines: Vec<String> = s
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    let default = lines
        .iter()
        .find_map(|l| {
            let t = l.trim_start();
            let (k, v) = t.split_once('=')?;
            k.trim_end()
                .eq_ignore_ascii_case("default")
                .then(|| v.trim().to_string())
        })
        .ok_or_else(|| Error("boot.ini has no default= line".into()))?;
    let i = lines
        .iter()
        .position(|l| {
            let t = l.trim();
            t.len() > default.len()
                && t[..default.len()].eq_ignore_ascii_case(&default)
                && t[default.len()..].starts_with('=')
        })
        .ok_or_else(|| Error(format!("boot.ini has no entry for the default {default}")))?;
    let tokens =
        |s: &str| -> Vec<String> { s.split_whitespace().map(str::to_ascii_lowercase).collect() };
    if tokens(&lines[i]).iter().any(|t| t == "/debug") {
        return Ok(None);
    }
    let l = &lines[i];
    let parse = || {
        let (arc, rest) = l.split_once('=')?;
        let rest = rest.strip_prefix('"')?;
        let (desc, opts) = rest.split_once('"')?;
        Some((arc, desc, opts))
    };
    let (arc, desc, opts) =
        parse().ok_or_else(|| Error(format!("cannot parse boot.ini entry: {l}")))?;
    // Drop the options the new entry sets: a whitespace character and the option.
    let mut kept = String::new();
    let mut rest = opts;
    while !rest.is_empty() {
        let ws = rest.chars().next().unwrap();
        let tail = &rest[ws.len_utf8()..];
        if ws.is_whitespace() && tail.starts_with('/') {
            let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
            let o = tail[..end].to_ascii_lowercase();
            if o == "/sos"
                || o == "/bootlog"
                || o.starts_with("/debug")
                || o.starts_with("/baudrate=")
            {
                rest = &tail[end..];
                continue;
            }
        }
        kept.push(ws);
        rest = tail;
    }
    let entry = format!(
        "{arc}=\"{desc} (kernel debugger on COM2)\"{} /debug /debugport=com2 /baudrate=115200 /sos /bootlog",
        kept.trim_end()
    );
    lines.insert(i, entry);
    let out = lines.join("\r\n");
    let out = format!("{}\r\n", out.trim_end());
    Ok(Some(out.chars().map(|c| c as u8).collect()))
}

/// The Generation 2 changes: files and registry.
fn gen2(
    c: &Migrate,
    hf: &HostFiles,
    s: &mut Session,
    fs: &Fs<'_>,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    // CSMWrap on the XP partition, which the firmware boots as \EFI\BOOT\BOOTX64.EFI.
    s.copy_host(r"\EFI\BOOT\BOOTX64.EFI", &hf.efi)?;
    s.put(r"\EFI\BOOT\csmwrap.ini", csmwrap_ini(c.debug));
    s.copy_host(r"\EFI\CSMWrap\dsdt.aml", &hf.dsdt)?;

    // Drivers: storvsc.sys (the Integration Services' storport miniport for the VMBus SCSI
    // controller), storport.sys and diskdump.sys of KB943295, hvfb.sys (frame buffer display),
    // bootwait.sys (holds the boot until the boot disk has appeared).
    let drv = format!(r"{SYSTEM32}\drivers");
    let (_, from) = ic_file(fs, &c.files, c.ic.as_deref(), "storvsc", "storvsc.sys")?;
    s.copy_from(fs, &from, &format!(r"{drv}\storvsc.sys"))?;
    s.copy_host(&format!(r"{drv}\storport.sys"), &hf.storport)?;
    s.copy_host(&format!(r"{drv}\diskdump.sys"), &hf.diskdump)?;
    s.copy_host(&format!(r"{drv}\hvfb.sys"), &hf.hvfb)?;
    s.copy_host(&format!(r"{drv}\bootwait.sys"), &hf.bootwait)?;

    // The kernel imports bootvid.dll from system32; Windows File Protection would put XP's copy
    // back from dllcache, so dllcache gets ours too. XP's VGA bootvid.dll is kept once as
    // bootvid.xp.
    let bootvid = format!(r"{SYSTEM32}\bootvid.dll");
    let xp = format!(r"{SYSTEM32}\bootvid.xp");
    if !fat::exists(fs, &xp) {
        let b = fat::read(fs, &bootvid).map_err(disk_err)?;
        let company = Pe::parse(&b, 0)
            .and_then(|pe| pe.version_string(&b, "CompanyName"))
            .ok()
            .flatten()
            .unwrap_or_default();
        if company.to_lowercase().contains("microsoft") {
            s.copy_within(fs, &bootvid, &xp)?;
            log(format!("{xp}: XP's bootvid.dll"));
        }
    }
    s.copy_host(&bootvid, &hf.bootvid)?;
    if fat::exists(fs, &format!(r"{SYSTEM32}\dllcache")) {
        s.copy_host(&format!(r"{SYSTEM32}\dllcache\bootvid.dll"), &hf.bootvid)?;
    }

    if c.debug {
        let b = fat::read(fs, r"\boot.ini").map_err(disk_err)?;
        match debug_boot_entry(&b)? {
            Some(new) => {
                let t = fat::fat_time(std::time::SystemTime::now());
                s.write(
                    r"\boot.ini",
                    new,
                    t,
                    Some(FileAttributes::HIDDEN | FileAttributes::SYSTEM),
                );
                log("boot.ini: a default entry with the kernel debugger on COM2".into());
            }
            None => log("boot.ini: the default entry has /debug already; left alone".into()),
        }
    }

    let mut k = s.keys()?;
    storage(&mut k)?;
    hvfb(&mut k, log)?;
    bootwait(&mut k)?;
    if !c.keep_paging_executive {
        // Stock XP races in Msfs.sys: with no mailslot open, Msfs pages its whole image
        // (MmPageEntireDriver), including the .data page with its spinlock, and the first
        // NtCreateMailslotFile writes that lock with interrupts disabled before it locks the driver
        // back in. If the page was written to the pagefile and read back clean, the dirty-bit fault
        // comes with IF=0 and XP stops with 0xD3 in Msfs. The first Generation 2 boot (Plug and
        // Play busy, the first mailslot late) hit it every time, and each crash left FAT32 dirty
        // for chkdsk. With DisablePagingExecutive, MmPageEntireDriver does nothing.
        k.dword(
            r"Control\Session Manager\Memory Management",
            "DisablePagingExecutive",
            1,
        )?;
    }
    if c.debug {
        // A bug check stays on the screen (and in the debugger) instead of a restart loop.
        k.dword(r"Control\CrashControl", "AutoReboot", 0)?;
    }
    log(format!(
        "SYSTEM ({}): storvsc and its Critical Device Database entries, storflt off, hvfb at 1024x768x32, bootwait{}{}",
        s.cs,
        if c.keep_paging_executive {
            ""
        } else {
            ", DisablePagingExecutive"
        },
        if c.debug { ", AutoReboot 0" } else { "" }
    ));
    Ok(())
}

/// Boot storage, the Critical Device Database entries for the VMBus devices XP needs before its
/// first Generation 2 logon, SynthVid held back, storflt off.
fn storage(k: &mut Keys<'_>) -> Result<()> {
    // VMBus SCSI controller -> storvsc (storport miniport). vmbus and Wdf01000 are boot start on
    // the Generation 1 install already; winhv, vmbkmcl, WdfLdr and storport are export drivers that
    // NTLDR loads as imports of the boot drivers.
    let s = r"Services\storvsc";
    k.dword(s, "Type", 1)?;
    k.dword(s, "Start", 0)?;
    k.dword(s, "ErrorControl", 1)?;
    k.sz(s, "Group", "SCSI miniport")?;
    k.expand(s, "ImagePath", r"system32\DRIVERS\storvsc.sys")?;
    k.sz(s, "DisplayName", "Microsoft Hyper-V SCSI Controller")?;
    k.dword(r"Services\storvsc\Parameters", "BusType", 10)?; // storvsc.inf bus_type_sas
    // storvsc.inf pnpsafe_pci_addreg
    k.dword(
        r"Services\storvsc\Parameters\Device",
        "EnableQueryAccessAlignment",
        1,
    )?;
    let cddb = |k: &mut Keys<'_>, dev: &str, service: &str, class: &str| -> Result<()> {
        let p = format!(r"{CDDB}\{dev}");
        k.sz(&p, "Service", service)?;
        k.sz(&p, "ClassGUID", class)
    };
    cddb(
        k,
        "vmbus#{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f}",
        "storvsc",
        "{4D36E97B-E325-11CE-BFC1-08002BE10318}",
    )?;
    // ACPI\VMBus -> vmbus is there from the Generation 1 install; written again for an install
    // that never saw the device.
    cddb(k, "acpi#vmbus", "vmbus", SYSTEM_CLASS)?;
    // The keyboard (there from the Generation 1 install too) and the synthetic mouse (HID over
    // VMBus).
    cddb(
        k,
        "vmbus#{f912ad6d-2b17-48ea-bd65-f927a61c7684}",
        "hyperkbd",
        "{4D36E96B-E325-11CE-BFC1-08002BE10318}",
    )?;
    cddb(
        k,
        "vmbus#{cfa8b69e-5b4a-4cc0-b98b-8ba1a1f3f95a}",
        "VMBusHID",
        "{745A17A0-74D3-11D0-B6FE-00A0C90F57DA}",
    )?;
    // Synthetic video (SynthVid). The Generation 2 VMBus is a new parent, so the video channel is
    // a new device node on the first boot, and hvfb has to draw that boot. Left alone, user-mode
    // Plug and Play installs and starts SynthVid in the middle of the first session, hvfb's drawing
    // then crawls and the display watchdog stops the system (0xEA in framebuf); bound through this
    // entry alone, SynthVid becomes \Device\Video0 before its installation is finished and win32k
    // enables no display at all. Bound through the entry but with the service disabled, the first
    // boot runs on hvfb, Plug and Play installs SynthVid (its INF sets Start back to 3) and asks for
    // a restart, and from the second boot on SynthVid is the primary display.
    if k.exists(r"Services\SynthVid")? {
        cddb(
            k,
            "vmbus#{da0a7802-e377-4aac-8e77-0558eb1073f8}",
            "SynthVid",
            "{4D36E968-E325-11CE-BFC1-08002BE10318}",
        )?;
        k.dword(r"Services\SynthVid", "Start", 4)?;
    }
    // storflt (Hyper-V's IDE "storage accelerator") is a class lower filter below every disk on the
    // Generation 1 install; Generation 2 has no emulated IDE, so it stays out of the boot disk's
    // stack.
    k.delete_value(
        r"Control\Class\{4D36E967-E325-11CE-BFC1-08002BE10318}",
        "LowerFilters",
    )?;
    k.dword(r"Services\storflt", "Start", 4)
}

/// hvfb: a legacy VideoPort miniport, as hvfb.inf installs it, starting at 1024x768x32.
fn hvfb(k: &mut Keys<'_>, log: &mut dyn FnMut(String)) -> Result<()> {
    let s = r"Services\hvfb";
    k.dword(s, "Type", 1)?;
    k.dword(s, "Start", 1)?;
    k.dword(s, "ErrorControl", 0)?;
    k.sz(s, "Group", "Video")?;
    k.expand(s, "ImagePath", r"system32\DRIVERS\hvfb.sys")?;
    k.sz(s, "DisplayName", "hvfb frame buffer display miniport")?;
    let device = |k: &mut Keys<'_>, p: &str| -> Result<()> {
        k.set(
            p,
            hive::Value::multi_string("InstalledDisplayDrivers", &["framebuf"]),
        )?;
        k.dword(p, "VgaCompatible", 0)?;
        k.sz(
            p,
            "Device Description",
            "Linear frame buffer display (VBE / Hyper-V Gen2)",
        )
    };
    let mode = |k: &mut Keys<'_>, p: &str| -> Result<()> {
        for (n, v) in [
            ("BitsPerPel", 32),
            ("XResolution", 1024),
            ("YResolution", 768),
            ("VRefresh", 60),
            ("Flags", 0),
            ("XPanning", 0),
            ("YPanning", 0),
        ] {
            k.dword(p, &format!("DefaultSettings.{n}"), v)?;
        }
        Ok(())
    };
    device(k, r"Services\hvfb\Device0")?;
    mode(k, r"Services\hvfb\Device0")?;

    // Display settings. On its first boot videoprt gives a legacy miniport a VideoID
    // (Services\<svc>\Video\VideoID, a new GUID unless one is there), copies Device0 to
    // Control\Video\{VideoID}\0000 and uses that copy from then on. The mode, however, comes from
    // the hardware profile's copy, Hardware Profiles\<n>\System\CurrentControlSet\Control\Video\
    // {VideoID}\0000, where Display Properties stores it. win32k used the device key's
    // DefaultSettings on the first boot, when the profile key did not exist yet; that boot created
    // the profile key with Attach.ToDesktop only, and every later boot started at 640x480, hvfb's
    // mode 0. So, as HIVESYS.INF does for VgaSave, hvfb gets a fixed VideoID up front, the keys
    // videoprt would create, and the mode in the profile key unless one was chosen there already.
    // An existing VideoID is kept.
    let id = k
        .get(r"Services\hvfb\Video", "VideoID")?
        .and_then(|v| v.as_strings().into_iter().next())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| HVFB_VIDEO_ID.to_string());
    k.sz(r"Services\hvfb\Video", "VideoID", &id)?;
    k.sz(r"Services\hvfb\Video", "Service", "hvfb")?;
    k.sz(&format!(r"Control\Video\{id}\Video"), "Service", "hvfb")?;
    device(k, &format!(r"Control\Video\{id}\0000"))?;
    mode(k, &format!(r"Control\Video\{id}\0000"))?;
    let profile = k
        .get(r"Control\IDConfigDB", "CurrentConfig")?
        .and_then(|v| v.as_dword())
        .ok_or_else(|| Error("SYSTEM: no Control\\IDConfigDB\\CurrentConfig".into()))?;
    let p =
        format!(r"Hardware Profiles\{profile:04}\System\CurrentControlSet\Control\VIDEO\{id}\0000");
    let mut chosen = Vec::new();
    for n in ["XResolution", "YResolution", "BitsPerPel"] {
        if let Some(v) = k.get(&p, &format!("DefaultSettings.{n}"))? {
            chosen.push(v.as_dword().map_or("?".to_string(), |d| d.to_string()));
        }
    }
    if chosen.is_empty() {
        k.dword(&p, "Attach.ToDesktop", 1)?;
        mode(k, &p)?;
    } else {
        log(format!(
            "SYSTEM: hardware profile {profile:04} keeps the mode chosen there ({})",
            chosen.join("x")
        ));
    }
    Ok(())
}

/// bootwait: a boot-start helper whose boot driver reinitialization routine waits (up to
/// TimeoutSeconds) for the boot partition: vmbus.sys reports the SCSI controller from a work item
/// bound to CPU 0, which the boot thread (priority 31) holds until IopMarkBootPartition.
/// RepairStorvsc gives the SCSI controller its service back after Plug and Play has installed the
/// Integration Services' NULL driver on it, and its name; the Devices table names the devices XP
/// has no driver for.  The Patches table keeps Microsoft's drivers that we change from being undone
/// when Plug and Play copies their stock file back (see `inject::bootwait_patch`).
fn bootwait(k: &mut Keys<'_>) -> Result<()> {
    let s = r"Services\bootwait";
    k.dword(s, "Type", 1)?;
    k.dword(s, "Start", 0)?;
    k.dword(s, "ErrorControl", 0)?;
    k.expand(s, "ImagePath", r"system32\DRIVERS\bootwait.sys")?;
    k.sz(s, "DisplayName", "Wait for the boot disk")?;
    let p = r"Services\bootwait\Parameters";
    k.dword(p, "TimeoutSeconds", 30)?;
    k.dword(p, "RepairStorvsc", 1)?;
    for (hwid, name) in NAMELESS {
        bootwait_device(
            k,
            &DeviceFix {
                hwid,
                service: None,
                name: Some(name),
                class: Some((SYSTEM_CLASS, "System")),
            },
        )?;
    }
    bootwait_patch(k)
}

pub fn migrate(c: &Migrate, log: &mut dyn FnMut(String)) -> Result<()> {
    let comps = Components::select(
        NtVersion::Xp,
        Components {
            synthvid: true,
            ..c.leave_out
        },
        c.opt_in,
    );
    let hf = host_files(c, comps, log)?;
    let Some(image) = &c.image else {
        log("the files are complete".into());
        return Ok(());
    };
    let (mut img, part, start, len) = open_image(image, c.partition, !c.check)?;
    log(format!(
        "{}: partition {part} (at sector {})",
        image.display(),
        start / SECTOR
    ));
    let fs = fat::open(img.window(start, len))
        .map_err(|e| Error(format!("partition {part}: {e} (only FAT32 is supported)")))?;
    check_system(&fs, log)?;
    // The Integration Services' files from the volume, checked as those from --files were
    // (inject's dmvsc recipe takes only the tested dmvsc.sys).
    let mut ic = vec![("storvsc", "storvsc.sys")];
    if comps.dynamic_memory {
        ic.push(("dmvsc", "dmvscres.dll"));
    }
    for (pkg, name) in ic {
        let (data, from) = ic_file(&fs, &c.files, c.ic.as_deref(), pkg, name)?;
        if find_in(&c.files, name).is_none() {
            check_version(name, &from, &data, IC_VERSION, c.force, log)?;
        }
    }
    let mut s = Session::load(&fs, c.force)?;
    if s.version != NtVersion::Xp {
        return Err(Error(format!(
            "{}: only Windows XP is supported",
            s.version
        )));
    }
    gen2(c, &hf, &mut s, &fs, log)?;
    components(&mut s, &fs, comps, &c.files, c.ic.as_deref(), log)?;
    if c.check {
        log("all checks passed; nothing written (--check)".into());
        return Ok(());
    }
    s.store(&fs, log)?;
    fs.unmount()
        .map_err(|e| Error(format!("partition {part}: {e}")))?;
    img.flush().map_err(disk_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offline::{TempDir, string_value};
    use hive::Hive;

    /// The registry part on the SYSTEM hive of a Generation 1 XP (HVKIT_TESTDATA/in/system-xpv1.hiv,
    /// a Microsoft file not in the repository); without it this test does nothing.
    #[test]
    fn gen2_registry() {
        let Some(dir) = std::env::var_os("HVKIT_TESTDATA").map(PathBuf::from) else {
            eprintln!("HVKIT_TESTDATA not set; skipped");
            return;
        };
        let tmp = TempDir::new("migrate-test").unwrap();
        let p = tmp.0.join("system");
        std::fs::copy(dir.join("in/system-xpv1.hiv"), &p).unwrap();
        let mut h = Hive::open(&p, true).unwrap();
        let cs = h.current_control_set().unwrap();
        let root = h.find(h.root(), &cs).unwrap().unwrap();
        let mut k = Keys {
            h: &mut h,
            root,
            what: "SYSTEM",
        };
        let dword =
            |k: &Keys<'_>, p: &str, n: &str| k.get(p, n).unwrap().and_then(|v| v.as_dword());
        assert_eq!(dword(&k, r"Services\SynthVid", "Start"), Some(3));
        for _ in 0..2 {
            storage(&mut k).unwrap();
            hvfb(&mut k, &mut |_| {}).unwrap();
            bootwait(&mut k).unwrap();
        }
        assert_eq!(dword(&k, r"Services\storvsc", "Start"), Some(0));
        let ip = k.get(r"Services\storvsc", "ImagePath").unwrap().unwrap();
        assert_eq!(ip.ty, hive::REG_EXPAND_SZ);
        let cddb = |k: &Keys<'_>, dev: &str| {
            k.get(&format!(r"{CDDB}\{dev}"), "Service")
                .unwrap()
                .unwrap()
                .as_strings()
        };
        assert_eq!(
            cddb(&k, "vmbus#{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f}"),
            ["storvsc"]
        );
        assert_eq!(
            cddb(&k, "vmbus#{da0a7802-e377-4aac-8e77-0558eb1073f8}"),
            ["SynthVid"]
        );
        assert_eq!(dword(&k, r"Services\SynthVid", "Start"), Some(4));
        assert_eq!(dword(&k, r"Services\storflt", "Start"), Some(4));
        let disk = r"Control\Class\{4D36E967-E325-11CE-BFC1-08002BE10318}";
        assert!(k.get(disk, "LowerFilters").unwrap().is_none());
        assert!(k.get(disk, "UpperFilters").unwrap().is_some());
        let id = k.get(r"Services\hvfb\Video", "VideoID").unwrap().unwrap();
        assert_eq!(id.as_strings(), [HVFB_VIDEO_ID]);
        let mode = format!(r"Control\Video\{HVFB_VIDEO_ID}\0000");
        assert_eq!(dword(&k, &mode, "DefaultSettings.XResolution"), Some(1024));
        let profile = format!(
            r"Hardware Profiles\0001\System\CurrentControlSet\Control\VIDEO\{HVFB_VIDEO_ID}\0000"
        );
        assert_eq!(dword(&k, &profile, "Attach.ToDesktop"), Some(1));
        // A mode chosen in Display Properties stays.
        k.dword(&profile, "DefaultSettings.XResolution", 800)
            .unwrap();
        hvfb(&mut k, &mut |_| {}).unwrap();
        assert_eq!(
            dword(&k, &profile, "DefaultSettings.XResolution"),
            Some(800)
        );
        // The three nameless devices, once each.
        let devices = k.key(r"Services\bootwait\Parameters\Devices").unwrap();
        let ids: Vec<String> =
            k.h.children(devices)
                .unwrap()
                .into_iter()
                .map(|d| string_value(k.h, d, "HardwareID"))
                .collect();
        assert_eq!(ids, NAMELESS.map(|n| n.0));
        assert_eq!(
            dword(&k, r"Services\bootwait\Parameters", "RepairStorvsc"),
            Some(1)
        );
    }

    #[test]
    fn boot_ini_debug_entry() {
        let ini = b"[boot loader]\r\ntimeout=30\r\ndefault=multi(0)disk(0)rdisk(0)partition(1)\\WINDOWS\r\n[operating systems]\r\nmulti(0)disk(0)rdisk(0)partition(1)\\WINDOWS=\"Microsoft Windows XP Professional \xb0\" /noexecute=optin /fastdetect /sos  /baudrate=9600\r\n\r\n";
        let out = debug_boot_entry(ini).unwrap().unwrap();
        let want = b"[boot loader]\r\ntimeout=30\r\ndefault=multi(0)disk(0)rdisk(0)partition(1)\\WINDOWS\r\n[operating systems]\r\nmulti(0)disk(0)rdisk(0)partition(1)\\WINDOWS=\"Microsoft Windows XP Professional \xb0 (kernel debugger on COM2)\" /noexecute=optin /fastdetect /debug /debugport=com2 /baudrate=115200 /sos /bootlog\r\nmulti(0)disk(0)rdisk(0)partition(1)\\WINDOWS=\"Microsoft Windows XP Professional \xb0\" /noexecute=optin /fastdetect /sos  /baudrate=9600\r\n";
        assert_eq!(String::from_utf8_lossy(&out), String::from_utf8_lossy(want));
        assert!(debug_boot_entry(&out).unwrap().is_none());
        assert!(debug_boot_entry(b"[boot loader]\r\n").is_err());
    }

    #[test]
    fn csmwrap_ini_lines() {
        let s = String::from_utf8(csmwrap_ini(true)).unwrap();
        assert!(s.ends_with("verbose = true\r\n") && s.contains("madt_pcat_compat = true\r\n"));
        assert!(
            !String::from_utf8(csmwrap_ini(false))
                .unwrap()
                .contains("serial")
        );
    }
}
