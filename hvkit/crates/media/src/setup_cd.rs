//! A Windows NT 5.x setup CD that installs and boots on Hyper-V Generation 2 through CSMWrap: the
//! XP SP3 and Server 2003 SP2 CDs (also nLite'd ones). The source ISO is only read.
//!
//! The changes to text mode, TXTSETUP.SIF, HIVESYS.INF and the Integration Services packages are
//! `media::nt5`'s. A CD build adds the multiprocessor HALs, ntkrpamp.exe and their TXTSETUP.SIF lines
//! from `mp_source` for a CD that nLite stripped of them, puts the packages in $OEM$\$1\Drivers\HV
//! for GUI-mode Plug and Play (WINNT.SIF OemPnPDriversPath) with predev.exe in cmdlines.txt, merges
//! its WINNT.SIF into the CD's own, and masters the ISO.
//!
//! On an XP Professional x64 CD (NT 5.2 x64) the loaders stay in \I386 but TXTSETUP.SIF, the hives
//! and the drivers are in \AMD64, so that is where the drivers (x64 builds, from `files`) go and what
//! is changed. Its hal.dll gets the `hal-clock` recipe. The Integration Services extras are those of
//! Server 2003 (both 5.2): the `dmvsc`, `icsvc-gsi` and `synthvid` recipes know the x64 files too.
//! The bootvid.dll is left out unless `files` has one. `files` has the x64 builds under the same
//! names: mdlex.sys is `make`'s mdlex64.sys, predev.exe its predev64.exe, vmbaud.sys vmbaud64.sys.

use crate::components::Components;
use crate::nt5;
pub use crate::nt5::Partition;
use crate::{
    Error, Result, copy, copy_tree, extract_cached, fmt_err, io, read_text, remove_if_exists,
    write_text,
};
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
    /// for the Guest Service Interface and vmbaud, vmbaud.inf and vmbaud.sys (x64 builds for an x64
    /// CD)
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
    /// How text mode gets the partition it installs to.
    pub partition: Partition,
    /// Answer the GUI-mode pages.
    pub unattend: bool,
    pub product_key: Option<String>,
    /// Drop I386\BOOTFIX.BIN ("Press any key to boot from CD"), for testing text mode.
    pub no_bootfix: bool,
    /// Patch NTLDR and SETUPLDR.BIN (recipe ntldr).
    pub patch_ntldr: bool,
    /// A bash script run in the tree just before mastering.
    pub hook: Option<PathBuf>,
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
    // XP Professional x64: the loaders stay in \I386, TXTSETUP.SIF, the hives and the drivers are
    // in \AMD64.
    let amd64 = root.join("AMD64/TXTSETUP.SIF").exists();
    let sys = if amd64 {
        root.join("AMD64")
    } else {
        i386.clone()
    };
    let sif_path = sys.join("TXTSETUP.SIF");
    let mut sif = read_text(&sif_path)?;
    let version = nt5::version(&sif)?;
    let comps = Components::select(version, c.leave_out, c.opt_in);
    log(format!(
        "{version}{}: components {}",
        if amd64 { " x64" } else { "" },
        comps.describe()
    ));
    let winnt_path = sys.join("WINNT.SIF");
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
        write_text(&sif_path, &sif)?;
    }
    if c.no_bootfix {
        remove_if_exists(&i386.join("BOOTFIX.BIN"))?;
    }

    let options = nt5::Options {
        files: &c.files,
        kd: c.kd,
        load_options: c.load_options.as_deref(),
        patch_ntldr: c.patch_ntldr,
    };
    nt5::Plan::new(root, &options, log)?.apply(root, log)?;

    // $OEM$ at the CD root (CD installs look for it there): $1 is copied to the system drive in text
    // mode, and GUI-mode PnP searches the OemPnPDriversPath directories.
    let hv = root.join("$OEM$/$1/Drivers/HV");
    nt5::packages(&hv, &c.ic, &c.files, comps, amd64, log)?;
    // The packages' directories only: predev.exe sits next to them.
    let mut dirs: Vec<String> = std::fs::read_dir(&hv)
        .map_err(io(&hv))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    dirs.sort();
    let pnp = dirs
        .iter()
        .map(|d| format!("Drivers\\HV\\{d}"))
        .collect::<Vec<_>>()
        .join(";");
    // predev.exe from cmdlines.txt, near the end of GUI-mode setup (see `nt5::predev`).
    if let Some((exe, devices)) = nt5::predev(&c.files, comps, amd64)? {
        let p = hv.join("predev.exe");
        std::fs::write(&p, exe).map_err(io(&p))?;
        let mut line = String::from(r#""cmd /c %SystemDrive%\Drivers\HV\predev.exe"#);
        for (inst, ty, inf) in &devices {
            line.push_str(&format!(r" {inst} {ty} %SystemDrive%\Drivers\HV\{inf}"));
        }
        line.push('"');
        add_cmdline(&root.join("$OEM$"), &line)?;
        log(format!(
            "cmdlines.txt: predev.exe pre-installs {}",
            devices
                .iter()
                .map(|d| d.2.split('\\').next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let sif = read_text(&sif_path)?;
    winnt_sif(c, &winnt_path, &sif, &pnp, log)?;

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

/// WINNT.SIF. OemPreinstall=Yes makes setup copy $OEM$ and add OemPnPDriversPath to DevicePath.
/// The partition keys are `c.partition`'s; Repartition is always written, so that a Repartition=Yes
/// of the CD's own cannot wipe the disk behind `Fat` or `None`. These values are merged into the
/// CD's own WINNT.SIF if it has one (nLite's, with its product key and regional settings), ours
/// winning, except that the CD's [SetupData] OsLoadOptionsVar options stay in front of ours.
fn winnt_sif(
    c: &SetupCd,
    path: &Path,
    sif: &Text,
    pnp: &str,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    let (auto, repartition, file_system) = match c.partition {
        Partition::Fat => ("1", "No", Some("LeaveAlone")),
        Partition::Ntfs => ("0", "Yes", None),
        Partition::None => ("0", "No", None),
    };
    let mut ours = vec![
        "[Data]".to_string(),
        "MsDosInitiated=\"0\"".into(),
        "UnattendedInstall=\"Yes\"".into(),
        format!("AutoPartition={auto}"),
        String::new(),
        "[Unattended]".into(),
        "OemPreinstall=\"Yes\"".into(),
        format!("OemPnPDriversPath=\"{pnp}\""),
        "DriverSigningPolicy=Ignore".into(),
        "NonDriverSigningPolicy=Ignore".into(),
        format!("Repartition={repartition}"),
    ];
    // nLite writes FileSystem=*, which makes setup ask for the file system.
    ours.extend(file_system.map(|f| format!("FileSystem={f}")));
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
