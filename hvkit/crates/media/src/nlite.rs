//! nLite addons and driver folders that give a Windows NT 5.x CD processed with nLite 1.4.9.3 what
//! `setup_cd` gives an extracted CD (the changes are `media::nt5`'s): one addon per `nt5::Part`, one
//! for the partition, one for predev, and the Integration Services packages as folders for nLite's
//! Drivers page.
//! The user's CD is only read, for its version and for the text of the lines the addons replace;
//! the addons carry the user's Microsoft files, so they are made for that user only.
//!
//! What nLite 1.4.9.3 does with an addon (read in its code): it unpacks the archive with 7-Zip and
//! reads the first `*entries*.ini` in its root. A [txtsetup_files] line goes to TXTSETUP.SIF
//! [SourceDisksFiles], and the file of that name in the archive's root is compressed into \I386
//! (\AMD64). Every other file in the root replaces the CD's file of that name in either form (X.SYS
//! or X.SY_), unless the CD's has a newer version (`flags=IgnoreFileVersions` in [general] turns that
//! check off). [AddDirective] `FILE,Section` adds an empty section at the end of a file. [EditFile]
//! `FILE,Section,Lines` appends the lines of [Lines] to the section, leaving out one it has already
//! and making ` = ` the separator. [ExtraFileEdits] `FILE|find|replace` replaces the text in every
//! line of the file that has it, as nLite loaded the file (trimmed, without comments), and does
//! nothing when no line has it. [NeededComponents] keeps components from nLite's removal by number.
//! FILE is matched by its end. A line of the entries file loses a `;` comment after its last quote.
//! As in an update pack, the files of a `svcpack\` folder go to \I386\SVCPACK, and the
//! [SetupHotfixesToRun] lines of the svcpack.inf in a cabinet SVCPACK.IN_ go to the CD's SVCPACK.INF
//! (which nLite gives the version lines svcpack.dll wants).

use crate::components::Components;
use crate::nt5::{self, Change, Part, Partition, Plan};
use crate::{Error, Result, extract_cached, io, read_text};
use formats::cab::{self, NewFile};
use formats::inf::Text;
use std::path::{Path, PathBuf};

pub struct Nlite {
    /// The CD to start from (only read).
    pub source: PathBuf,
    /// Its extracted copy, made if missing.
    pub cache: PathBuf,
    /// As `setup_cd::SetupCd::files`.
    pub files: PathBuf,
    /// As `setup_cd::SetupCd::ic`.
    pub ic: PathBuf,
    pub leave_out: Components,
    pub opt_in: Components,
    /// The kernel debugger on COM2, in text mode (nLite's WINNT.SIF has no place for the installed
    /// system's options).
    pub kd: bool,
    pub load_options: Option<String>,
    pub patch_ntldr: bool,
    pub partition: Partition,
    /// For the preset: the product key, and the time zone's index (WINNT.SIF TimeZone, e.g. 210).
    pub product_key: Option<String>,
    pub time_zone: u32,
    /// The directory to write (addons\, drivers\ and README.txt are made anew).
    pub out: PathBuf,
}

/// nLite's component number of "Multi-Processor Support".
const MULTI_PROCESSOR_SUPPORT: u32 = 606;

#[derive(Default)]
struct Addon {
    /// Files in the archive's root, by name as nLite looks for them.
    files: Vec<(String, Vec<u8>)>,
    /// Files for \I386\SVCPACK, and the lines they add to SVCPACK.INF [SetupHotfixesToRun].
    svcpack_files: Vec<(String, Vec<u8>)>,
    svcpack_lines: Vec<String>,
    txtsetup_files: Vec<String>,
    add_directive: Vec<String>,
    /// (`FILE,Section`, lines).
    edit_file: Vec<(String, Vec<String>)>,
    extra_edits: Vec<String>,
    needed: Vec<u32>,
    ignore_versions: bool,
}

/// A line of a file as nLite loads it: trimmed, without a `;` comment after its last quote. None
/// for a blank or comment line.
fn as_nlite_loads(l: &str) -> Option<String> {
    let t = l.trim();
    if t.is_empty() || t.starts_with(';') {
        return None;
    }
    let pos = |c: char| t.rfind(c).map_or(-1, |i| i as isize);
    Some(if pos(';') > pos('"') {
        t[..t.rfind(';').unwrap()].trim().to_string()
    } else {
        t.to_string()
    })
}

/// A line the entries file must keep as it is: no comment nLite would cut off, no section header.
fn plain(l: &str) -> Result<&str> {
    if as_nlite_loads(l).as_deref() != Some(l.trim()) || l.trim_start().starts_with('[') {
        return Err(Error(format!(
            "nLite would not keep this line as it is: {l}"
        )));
    }
    Ok(l)
}

impl Addon {
    fn is_empty(&self) -> bool {
        self.files.is_empty()
            && self.svcpack_files.is_empty()
            && self.txtsetup_files.is_empty()
            && self.edit_file.is_empty()
            && self.extra_edits.is_empty()
    }

    fn edit(&mut self, target: &str, lines: &[String]) {
        match self.edit_file.last_mut() {
            Some((t, l)) if t == target => l.extend(lines.iter().cloned()),
            _ => self.edit_file.push((target.to_string(), lines.to_vec())),
        }
    }

    /// The entries file.
    fn entries(&self, title: &str, description: &str, date: &str) -> Result<String> {
        let mut o = vec![
            "[general]".to_string(),
            format!("builddate={date}"),
            format!("description={description}"),
            format!("title={title}"),
            format!("version={}", env!("CARGO_PKG_VERSION")),
            "website=https://github.com/xdqi/nt5-hvgen2".to_string(),
        ];
        if self.ignore_versions {
            o.push("flags=IgnoreFileVersions".into());
        }
        let section = |o: &mut Vec<String>, name: &str, lines: &[String]| -> Result<()> {
            if !lines.is_empty() {
                o.push(String::new());
                o.push(format!("[{name}]"));
                for l in lines {
                    o.push(plain(l)?.to_string());
                }
            }
            Ok(())
        };
        section(&mut o, "txtsetup_files", &self.txtsetup_files)?;
        section(&mut o, "AddDirective", &self.add_directive)?;
        let directives: Vec<String> = (0..self.edit_file.len())
            .map(|i| format!("{},hvgen2_{}", self.edit_file[i].0, i + 1))
            .collect();
        section(&mut o, "EditFile", &directives)?;
        section(&mut o, "ExtraFileEdits", &self.extra_edits)?;
        let needed: Vec<String> = self.needed.iter().map(|n| n.to_string()).collect();
        section(&mut o, "NeededComponents", &needed)?;
        for (i, (_, lines)) in self.edit_file.iter().enumerate() {
            section(&mut o, &format!("hvgen2_{}", i + 1), lines)?;
        }
        Ok(o.join("\r\n") + "\r\n")
    }

    /// The addon as a cabinet: the entries file, the root files, and the svcpack\ folder with
    /// SVCPACK.IN_ (a cabinet with a svcpack.inf that has the lines).
    fn cab(&self, name: &str, title: &str, description: &str) -> Result<Vec<u8>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let (y, mo, d, h, mi, s) = civil(now);
        let (date, time) = cab::dos_date_time(y, mo, d, h, mi, s);
        let entries = self.entries(title, description, &format!("{y}/{mo:02}/{d:02}"))?;
        let mut files = vec![NewFile {
            name: format!("ENTRIES_{}.INI", name.to_uppercase().replace('-', "_")),
            data: entries.into_bytes(),
            date,
            time,
        }];
        let svcpack = self
            .svcpack_files
            .iter()
            .map(|(n, b)| (format!("SVCPACK\\{n}"), b.clone()));
        for (n, b) in self.files.iter().cloned().chain(svcpack) {
            files.push(NewFile {
                name: n.to_uppercase(),
                data: b,
                date,
                time,
            });
        }
        if !self.svcpack_lines.is_empty() {
            let inf = [
                "[Version]",
                "Signature=\"$Windows NT$\"",
                "",
                "[SetupHotfixesToRun]",
            ]
            .into_iter()
            .map(String::from)
            .chain(self.svcpack_lines.iter().cloned())
            .collect::<Vec<_>>()
            .join("\r\n")
                + "\r\n";
            let inner = NewFile {
                name: "SVCPACK.INF".into(),
                data: inf.into_bytes(),
                date,
                time,
            };
            files.push(NewFile {
                name: "SVCPACK.IN_".into(),
                data: cab::create(&[inner], 0)
                    .map_err(|e| Error(format!("{name}.cab: SVCPACK.IN_: {e}")))?,
                date,
                time,
            });
        }
        cab::create(&files, 0).map_err(|e| Error(format!("{name}.cab: {e}")))
    }
}

/// UTC date and time of a Unix time.
fn civil(t: i64) -> (i32, u32, u32, u32, u32, u32) {
    let (days, secs) = (t.div_euclid(86400), t.rem_euclid(86400) as u32);
    // Howard Hinnant's days_from_civil, inverted.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m, d, secs / 3600, secs / 60 % 60, secs % 60)
}

/// The addons for `plan`, with `sif` the CD's TXTSETUP.SIF: (file name, title, description, addon).
fn addons(
    plan: &Plan,
    sif: &Text,
) -> Result<Vec<(&'static str, &'static str, &'static str, Addon)>> {
    let loaded: Vec<String> = sif.lines.iter().filter_map(|l| as_nlite_loads(l)).collect();
    let mut out = [
        (
            Part::Core,
            "hvgen2-core",
            "Hyper-V Gen2: core",
            "Hyper-V Generation 2 text-mode drivers (KMDF, VMBus, storvsc, bootwait, keyboard) and their setup lines",
        ),
        (
            Part::MpHal,
            "hvgen2-mphal",
            "Hyper-V Gen2: multiprocessor HAL",
            "Hyper-V Generation 2: text mode with the multiprocessor HAL (keeps Multi-Processor Support)",
        ),
        (
            Part::Display,
            "hvgen2-display",
            "Hyper-V Gen2: display",
            "Hyper-V Generation 2: hvfb display driver and frame buffer bootvid.dll",
        ),
        (
            Part::Ntldr,
            "hvgen2-ntldr",
            "Hyper-V Gen2: NTLDR menu highlight",
            "Hyper-V Generation 2: NTLDR and SETUPLDR.BIN patched for the mode 12h menu highlight",
        ),
    ]
    .map(|(part, file, title, description)| (part, file, title, description, Addon::default()));
    for (part, change) in &plan.changes {
        let a = &mut out.iter_mut().find(|o| o.0 == *part).unwrap().4;
        match change {
            Change::File {
                name, bytes, sdf, ..
            } => {
                a.files.push((name.clone(), bytes.clone()));
                match sdf {
                    Some(l) => a.txtsetup_files.push(l.clone()),
                    // Ours (bootvid.dll) may be versioned lower than the CD's file.
                    None => a.ignore_versions = true,
                }
            }
            Change::Replace { section, old, new } => {
                let find = as_nlite_loads(old)
                    .ok_or_else(|| Error(format!("TXTSETUP.SIF [{section}]: {old:?}")))?;
                let n = loaded.iter().filter(|l| l.contains(&find)).count();
                if n != 1 || find.contains('|') || new.contains('|') {
                    return Err(Error(format!(
                        "TXTSETUP.SIF [{section}] {find:?}: {n} lines have it; nLite would replace it in each"
                    )));
                }
                a.extra_edits.push(format!("TXTSETUP.SIF|{find}|{new}"));
            }
            Change::Add { section, lines } => {
                if sif.section(section).is_none() {
                    a.add_directive.push(format!("TXTSETUP.SIF,{section}"));
                }
                a.edit(&format!("TXTSETUP.SIF,{section}"), lines);
            }
            Change::HiveSys(lines) => a.edit("HIVESYS.INF,AddReg", lines),
        }
        if *part == Part::MpHal && !a.needed.contains(&MULTI_PROCESSOR_SUPPORT) {
            a.needed.push(MULTI_PROCESSOR_SUPPORT);
        }
    }
    Ok(out
        .into_iter()
        .filter(|o| !o.4.is_empty())
        .map(|(_, file, title, description, a)| (file, title, description, a))
        .collect())
}

/// The partition addon: WINNT.SIF as nLite's Unattended page writes it (`Autopartition=0` in [Data],
/// `FileSystem=*` in [Unattended]) gets `c.partition`'s keys (see `nt5::Partition`).
fn partition_addon(p: Partition) -> Option<Addon> {
    let edits: &[&str] = match p {
        Partition::Fat => &[
            "WINNT.SIF|Autopartition=0|Autopartition=1",
            "WINNT.SIF|FileSystem=*|FileSystem=LeaveAlone",
        ],
        Partition::Ntfs => &["WINNT.SIF|FileSystem=*|FileSystem=*<NEXT>Repartition=Yes"],
        Partition::None => return None,
    };
    Some(Addon {
        extra_edits: edits.iter().map(|e| e.to_string()).collect(),
        ..Addon::default()
    })
}

/// How long a [SetupHotfixesToRun] line may be: svcpack.dll (XP, Server 2003) puts the CD's
/// `\I386\svcpack\` and the line into a buffer of 130 characters (260 bytes taken for characters).
const SVCPACK_LINE_MAX: usize = 100;

/// The predev addon (see `nt5::predev`): predev.exe for \I386\SVCPACK and a line per device for
/// SVCPACK.INF [SetupHotfixesToRun], which nLite merges as an update pack's. svcpack.dll runs the
/// lines as SYSTEM at T-13 of GUI-mode setup (setup-cd's CDs run predev from cmdlines.txt, at
/// T-12). The INFs go by name, predev finds them in DevicePath: nLite numbers its driver folders
/// (%SystemRoot%\NLDRV\NNN) as they are added. GUIDs without braces, to keep the lines short.
fn predev_addon(exe: Vec<u8>, devices: &[(&str, &str, &str)]) -> Result<Addon> {
    let guid = |g: &str| g.trim_matches(|c| c == '{' || c == '}').to_string();
    let lines = devices
        .iter()
        .map(|(inst, ty, inf)| {
            let name = inf.rsplit('\\').next().unwrap_or(inf);
            let l = format!("predev.exe {} {} {name}", guid(inst), guid(ty));
            if l.len() > SVCPACK_LINE_MAX || l.contains([',', ';', '"', '%']) {
                return Err(Error(format!("svcpack.inf cannot take this line: {l}")));
            }
            Ok(l)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Addon {
        svcpack_files: vec![("predev.exe".into(), exe)],
        svcpack_lines: lines,
        ..Addon::default()
    })
}

pub fn build(n: &Nlite, log: &mut dyn FnMut(String)) -> Result<()> {
    extract_cached(&n.source, &n.cache, log)?;
    let options = nt5::Options {
        files: &n.files,
        kd: n.kd,
        load_options: n.load_options.as_deref(),
        patch_ntldr: n.patch_ntldr,
    };
    let plan = Plan::new(&n.cache, &options, log)?;
    let sys = n.cache.join(if plan.amd64 { "AMD64" } else { "I386" });
    let sif = read_text(&sys.join("TXTSETUP.SIF"))?;
    let comps = Components::select(plan.version, n.leave_out, n.opt_in);
    log(format!(
        "{}{}: components {}",
        plan.version,
        if plan.amd64 { " x64" } else { "" },
        comps.describe()
    ));

    let (addons_dir, drivers) = (n.out.join("addons"), n.out.join("drivers"));
    for d in [&addons_dir, &drivers] {
        if d.exists() {
            std::fs::remove_dir_all(d).map_err(io(d))?;
        }
    }
    std::fs::create_dir_all(&addons_dir).map_err(io(&addons_dir))?;
    let mut written = Vec::new();
    let mut all = addons(&plan, &sif)?;
    if let Some(a) = partition_addon(n.partition) {
        all.push((
            "hvgen2-partition",
            "Hyper-V Gen2: partition",
            "Hyper-V Generation 2: text mode installs without asking for the partition (needs the Unattended page)",
            a,
        ));
    }
    if let Some((exe, devices)) = nt5::predev(&n.files, comps, plan.amd64)? {
        all.push((
            "hvgen2-predev",
            "Hyper-V Gen2: predev",
            "Hyper-V Generation 2: pre-installs the devices that turn up after setup (no Found New Hardware wizard)",
            predev_addon(exe, &devices)?,
        ));
        log(format!(
            "svcpack: predev.exe pre-installs {}",
            devices
                .iter()
                .map(|d| d.2.split('\\').next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for (file, title, description, a) in &all {
        let p = addons_dir.join(format!("{file}.cab"));
        std::fs::write(&p, a.cab(file, title, description)?).map_err(io(&p))?;
        log(format!("addons\\{file}.cab"));
        written.push(*file);
    }

    nt5::packages(&drivers, &n.ic, &n.files, comps, plan.amd64, log)?;
    let mut pkgs: Vec<String> = std::fs::read_dir(&drivers)
        .map_err(io(&drivers))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    pkgs.sort();
    log(format!("drivers\\: {}", pkgs.join(", ")));

    match windows_path(&n.out) {
        Some(w) => {
            let infs: Vec<String> = pkgs
                .iter()
                .flat_map(|p| {
                    let dir = drivers.join(p);
                    let mut v: Vec<String> = std::fs::read_dir(&dir)
                        .into_iter()
                        .flatten()
                        .filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|f| f.to_lowercase().ends_with(".inf"))
                        .map(|f| format!("{w}\\drivers\\{p}\\{f}"))
                        .collect();
                    v.sort();
                    v
                })
                .collect();
            let addons: Vec<String> = written
                .iter()
                .map(|a| format!("{w}\\addons\\{a}.cab"))
                .collect();
            let addons: Vec<&str> = addons.iter().map(String::as_str).collect();
            let server = sif
                .get("SetupData", "ProductType")
                .is_some_and(|t| t.trim() != "0");
            let hivesft = read_text(&sys.join("HIVESFT.INF"))?;
            let tz = time_zone_name(&hivesft, n.time_zone)?;
            write_preset(
                &n.out,
                &addons,
                &infs,
                server,
                &tz,
                n.product_key.as_deref(),
            )?;
            log(format!(
                "hvgen2.ini, hvgen2_u.ini: nLite preset (time zone {tz})"
            ));
        }
        None => log(format!(
            "no nLite preset: {} is not a path Windows sees",
            n.out.display()
        )),
    }

    let readme = n.out.join("README.txt");
    std::fs::write(
        &readme,
        readme_text(&written, &pkgs, n.partition).replace('\n', "\r\n"),
    )
    .map_err(io(&readme))?;
    Ok(())
}

/// `p` as Windows sees it: on Windows the absolute path, elsewhere a WSL path under /mnt/<drive>.
fn windows_path(p: &Path) -> Option<String> {
    if cfg!(windows) {
        return std::path::absolute(p)
            .ok()
            .map(|a| a.to_string_lossy().into_owned());
    }
    let a = std::path::absolute(p).ok()?;
    let s = a.to_str()?;
    let rest = s.strip_prefix("/mnt/")?;
    let (drive, tail) = rest.split_at(1);
    if !drive.chars().all(|c| c.is_ascii_alphabetic())
        || !(tail.is_empty() || tail.starts_with('/'))
    {
        return None;
    }
    Some(format!(
        "{}:{}",
        drive.to_uppercase(),
        tail.replace('/', "\\")
    ))
}

/// The display name of the time zone with this index, as nLite lists it from the CD's HIVESFT.INF
/// (`...\Time Zones\<zone>","Display"` through [Strings], `&` trimmed): what a preset's TimeZone
/// names.
fn time_zone_name(hivesft: &Text, index: u32) -> Result<String> {
    const TZ: &str = r#"HKLM,"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Time Zones\"#;
    let value = |l: &str| l.rsplit_once(',').map(|(_, v)| v.trim().to_string());
    let zone = hivesft
        .lines
        .iter()
        .filter_map(|l| l.strip_prefix(TZ))
        .find(|l| {
            l.split_once("\",\"")
                .is_some_and(|(_, r)| r.starts_with("Index\""))
                && value(l).and_then(|v| v.parse::<u32>().ok()) == Some(index)
        })
        .and_then(|l| l.split_once("\",\"").map(|(z, _)| z.to_string()))
        .ok_or_else(|| Error(format!("HIVESFT.INF: no time zone with index {index}")))?;
    let display = hivesft
        .lines
        .iter()
        .filter_map(|l| l.strip_prefix(TZ))
        .find(|l| l.starts_with(&format!("{zone}\",\"Display\"")))
        .and_then(value)
        .ok_or_else(|| Error(format!("HIVESFT.INF: time zone {zone} has no Display")))?;
    let display = display.trim_matches('"');
    let text = match display.strip_prefix('%').and_then(|d| d.strip_suffix('%')) {
        Some(name) => hivesft
            .get("Strings", name)
            .ok_or_else(|| Error(format!("HIVESFT.INF: [Strings] {name} not found")))?,
        None => display.to_string(),
    };
    Ok(text.trim().trim_matches('"').trim_matches('&').to_string())
}

/// An nLite preset (`hvgen2.ini`, and `hvgen2_u.ini` with the user's part, UTF-16 as nLite reads
/// it with a byte order mark): the addons, the driver packages, a fully unattended setup without the
/// Windows Welcome, the time zone and the product key, and for a server the licensing mode. With
/// `nLite.exe /path:<CD> /preset:<this>` nLite processes the CD and exits (no ISO).
fn write_preset(
    out: &Path,
    addons: &[&str],
    infs: &[String],
    server: bool,
    time_zone: &str,
    key: Option<&str>,
) -> Result<()> {
    let mut l = vec![
        "[Main]",
        "",
        "[Tasks]",
        "Unattended Setup",
        "Integrate Drivers",
        "Hotfixes and Update Packs",
        "",
        "[Components]",
        "",
        "[Options]",
        // Else nLite copies the preset, product key included, into the CD's root.
        "NoISOPreset",
        "",
        "[Unattended]",
        // FullUnattended.
        "UnattendMode = 1",
        "OOBEOff",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    if server {
        l.push("PerServer,5".into());
    }
    l.extend(["".into(), "[Drivers]".into()]);
    l.extend(infs.iter().map(|i| format!("{i},0")));
    l.extend(["".into(), "[Hotfixes]".into()]);
    l.extend(addons.iter().map(|a| a.to_string()));
    let p = out.join("hvgen2.ini");
    std::fs::write(&p, l.join("\r\n") + "\r\n").map_err(io(&p))?;
    let mut u = vec!["[Personal]".to_string(), format!("TimeZone = {time_zone}")];
    if let Some(k) = key {
        let k: String = k.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        u.push(format!("CDKey = \"{k}\""));
    }
    u.extend(["".into(), "[Users]".into(), "".into()]);
    let text = u.join("\r\n");
    let mut b = vec![0xff, 0xfe];
    b.extend(text.encode_utf16().flat_map(|c| c.to_le_bytes()));
    let p = out.join("hvgen2_u.ini");
    std::fs::write(&p, b).map_err(io(&p))
}

fn readme_text(addons: &[&str], pkgs: &[String], partition: Partition) -> String {
    let mut s = String::from(
        "Hyper-V Generation 2 support for this Windows CD, for nLite 1.4.9.3 (made by hvkit nlite).\n\
         They contain Microsoft files of yours: do not pass them on.\n\n\
         In nLite:\n\n\
         1. Hotfixes, Add-ons and Update Packs: insert every addons\\*.cab:\n",
    );
    for a in addons {
        s.push_str(&format!("     {a}.cab\n"));
    }
    s.push_str("2. Drivers: insert the .inf of every folder in drivers\\ (mode: PnP):\n");
    for p in pkgs {
        s.push_str(&format!("     drivers\\{p}\n"));
    }
    s.push_str(
        "3. Unattended: turn it on. Its DriverSigningPolicy=Ignore lets setup install the changed\n   \
         drivers without asking",
    );
    if partition != Partition::None {
        s.push_str(", and hvgen2-partition.cab changes the WINNT.SIF it writes");
    }
    s.push_str(
        ".\n4. Remove Components: keep Multi-Processor Support (hvgen2-mphal keeps it anyway).\n\n\
         hvgen2.ini is an nLite preset with all of that (fully unattended, no Windows Welcome, the time\n\
         zone, the product key if given). Load it on nLite's Presets page, or run\n\
         nLite.exe /path:<the CD's folder> /preset:<this folder>\\hvgen2.ini, which processes the CD\n\
         and exits without making the ISO.\n\n\
         The VM boots CSMWrap from a CD of its own (hvkit csmwrap-cd), as for hvkit setup-cd's CDs.\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nlite_line_view() {
        assert_eq!(
            as_nlite_loads("  vga = vgapnp.sys  ").as_deref(),
            Some("vga = vgapnp.sys")
        );
        assert_eq!(as_nlite_loads("a = b ; note").as_deref(), Some("a = b"));
        assert_eq!(
            as_nlite_loads("a = \"x;y\"").as_deref(),
            Some("a = \"x;y\"")
        );
        assert_eq!(as_nlite_loads("; only a comment"), None);
        assert!(plain("a = b ; note").is_err());
        assert!(plain("[Section]").is_err());
    }

    #[test]
    fn predev_lines() {
        let a = predev_addon(b"MZ".to_vec(), &[nt5::PREDEV_VMBAUD, nt5::PREDEV_GSI]).unwrap();
        assert_eq!(a.svcpack_files[0].0, "predev.exe");
        assert_eq!(
            a.svcpack_lines,
            [
                "predev.exe 2a7f3e10-9c4d-4b8a-a6e5-7d1c0f3b8e62 8b57f4e3-2a3c-4f6e-9c8d-1e5a70b9c4d2 vmbaud.inf",
                "predev.exe eb765408-105f-49b6-b4aa-c123b64d17d4 34d14be3-dee4-41c8-9ae7-6b174977c192 vmic.inf",
            ]
        );
        assert!(a.cab("hvgen2-predev", "t", "d").is_ok());
        assert!(predev_addon(vec![], &[("{a}", "{b}", r"x\%y%.inf")]).is_err());
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(1_791_417_600), (2026, 10, 8, 0, 0, 0));
    }

    /// A small CD's worth of TXTSETUP.SIF: the addons' sections and lines.
    #[test]
    fn entries_from_a_plan() {
        let sif = Text::parse(
            b"[SetupData]\r\nOsLoadOptions = \"/fastdetect /noguiboot /nodebug\"\r\n\
              [Display.Load]\r\nvga = vgapnp.sys\r\n[Hal.Load]\r\nacpiapic_mp = halaacpi.dll\r\n",
        )
        .unwrap();
        let plan = Plan {
            version: crate::components::NtVersion::from_numbers(5, 1).unwrap(),
            amd64: false,
            changes: vec![
                (
                    Part::Core,
                    Change::File {
                        name: "vmbus.sys".into(),
                        bytes: vec![1],
                        loader: false,
                        sdf: Some("vmbus.sys    = 1,,,,,,4_,4,0,0,,1,4".into()),
                    },
                ),
                (
                    Part::Core,
                    Change::Add {
                        section: "files.vmbus".into(),
                        lines: vec!["vmbus.sys,4".into()],
                    },
                ),
                (
                    Part::Core,
                    Change::HiveSys(vec![r#"HKLM,"SYSTEM\X","Y",0x00010001,1"#.into()]),
                ),
                (
                    Part::MpHal,
                    Change::Replace {
                        section: "Hal.Load".into(),
                        old: "acpiapic_mp = halaacpi.dll".into(),
                        new: "acpiapic_mp    = halmacpi.dll".into(),
                    },
                ),
            ],
        };
        let a = addons(&plan, &sif).unwrap();
        assert_eq!(a.len(), 2);
        let core = a[0].3.entries("t", "d", "2026/10/08").unwrap();
        for l in [
            "[txtsetup_files]\r\nvmbus.sys    = 1,,,,,,4_,4,0,0,,1,4\r\n",
            "[AddDirective]\r\nTXTSETUP.SIF,files.vmbus\r\n",
            "[EditFile]\r\nTXTSETUP.SIF,files.vmbus,hvgen2_1\r\nHIVESYS.INF,AddReg,hvgen2_2\r\n",
            "[hvgen2_1]\r\nvmbus.sys,4\r\n",
        ] {
            assert!(core.contains(l), "{core}");
        }
        let mphal = a[1].3.entries("t", "d", "2026/10/08").unwrap();
        assert!(mphal.contains(
            "[ExtraFileEdits]\r\nTXTSETUP.SIF|acpiapic_mp = halaacpi.dll|acpiapic_mp    = halmacpi.dll\r\n"
        ));
        assert!(mphal.contains("[NeededComponents]\r\n606\r\n"));
    }
}
