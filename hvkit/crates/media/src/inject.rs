//! An installed XP or Server 2003 changed offline: the components of `media::components` put into
//! its FAT volume in a disk image (raw or VHDX), without Windows. For an installation that already
//! boots on Generation 2 (bootwait installed) and has the Integration Services 6.3; `migrate` uses
//! the same code for a Generation 1 installation it moves.
//!
//! Unlike on the setup CD, the Integration Services' INFs are installed already, with NULL drivers
//! for Dynamic Memory and VSS on XP (Server 2003 has the stock dmvsc.sys, which is replaced), so
//! their services and bindings are written into the registry here. Plug and Play installs the
//! devices again from those INFs now and then (on a new VM, for one), which takes dmvsc's binding
//! away and points the services' ServiceDll back to icsvc.dll; bootwait's tables
//! (Parameters\Devices, Parameters\Values) put them back on every boot, so the entries go there too.
//! SynthVid's two files are patched where they are installed (a new install of the device by Plug
//! and Play copies the stock ones back; then run this again). The vmbaud package goes next to where
//! a setup CD puts it, into the system's DevicePath.

use crate::components::{self, Components};
use crate::offline::{
    Keys, SYSTEM32, Session, disk_err, find_in, hive_err, ic_file, need, open_image, string_value,
};
use crate::{Error, Result};
use disk::SECTOR;
use disk::fat::{self, Fs};
use hive::{Hive, Node, REG_BINARY, REG_EXPAND_SZ, Value};
use recipes::netvsc;
use std::path::{Path, PathBuf};

pub struct Inject {
    pub image: PathBuf,
    /// The partition (default: the FAT partition that has \WINDOWS\system32\config\system).
    pub partition: Option<usize>,
    /// Where mdlex.sys (Dynamic Memory), bootwait.sys (for an old one installed), vmbaud.inf and
    /// vmbaud.sys are, searched in this order; also dmvsc.sys and dmvscres.dll, if there.
    pub files: Vec<PathBuf>,
    /// The Integration Services packages, for dmvsc.sys and dmvscres.dll (default: the copy the
    /// Integration Services left below Program Files on the volume).
    pub ic: Option<PathBuf>,
    pub leave_out: Components,
    pub opt_in: Components,
    /// Change hives whose logs were not written back (an unclean shutdown).
    pub force: bool,
}

const VMBAUD_DIR: &str = r"\Drivers\HV\vmbaud";
const DMVSC_HWID: &str = r"VMBUS\{525074dc-8985-46e2-8057-a307dc18a502}";
/// bootwait.c's table sizes (BW_MAX_VALUES, BW_MAX_FIXES, BW_MAX_PATCHES).
const BOOTWAIT_MAX_VALUES: usize = 8;
const BOOTWAIT_MAX_DEVICES: usize = 16;
const BOOTWAIT_MAX_PATCHES: usize = 8;

/// The subkey of bootwait's table `table` (Values, Devices) that `is_it` recognises, else a new one
/// with the lowest free two-digit name.
fn table_entry(
    k: &mut Keys<'_>,
    table: &str,
    max: usize,
    is_it: impl Fn(&Hive, Node) -> bool,
) -> Result<String> {
    let t = k.key(table)?;
    let mut names = Vec::new();
    for c in k.h.children(t).map_err(hive_err(k.what))? {
        let name = k.h.name(c).map_err(hive_err(k.what))?;
        if is_it(k.h, c) {
            return Ok(format!("{table}\\{name}"));
        }
        names.push(name);
    }
    if names.len() >= max {
        return Err(Error(format!(
            "bootwait's {table} has {} entries; it reads at most {max}",
            names.len()
        )));
    }
    let free = (0..)
        .map(|i| format!("{i:02}"))
        .find(|n| !names.iter().any(|m| m.eq_ignore_ascii_case(n)))
        .unwrap();
    Ok(format!("{table}\\{free}"))
}

/// A bootwait Values entry: `name` of the key `key` (below CurrentControlSet) is set to `data` on
/// every boot.
pub(crate) fn bootwait_value(k: &mut Keys<'_>, key: &str, name: &str, data: &str) -> Result<()> {
    let table = r"Services\bootwait\Parameters\Values";
    let e = table_entry(k, table, BOOTWAIT_MAX_VALUES, |h, n| {
        string_value(h, n, "Key").eq_ignore_ascii_case(key)
            && string_value(h, n, "Name").eq_ignore_ascii_case(name)
    })?;
    k.sz(&e, "Key", key)?;
    k.sz(&e, "Name", name)?;
    k.sz(&e, "Data", data)
}

/// A bootwait Devices entry: device nodes whose first hardware ID is `hwid` get the service (if
/// they have none), the friendly name and the class given.
pub(crate) struct DeviceFix<'a> {
    pub hwid: &'a str,
    pub service: Option<&'a str>,
    pub name: Option<&'a str>,
    /// (ClassGUID, Class)
    pub class: Option<(&'a str, &'a str)>,
}

pub(crate) fn bootwait_device(k: &mut Keys<'_>, d: &DeviceFix<'_>) -> Result<()> {
    let table = r"Services\bootwait\Parameters\Devices";
    let e = table_entry(k, table, BOOTWAIT_MAX_DEVICES, |h, n| {
        string_value(h, n, "HardwareID").eq_ignore_ascii_case(d.hwid)
    })?;
    k.sz(&e, "HardwareID", d.hwid)?;
    if let Some(s) = d.service {
        k.sz(&e, "Service", s)?;
    }
    if let Some(n) = d.name {
        k.sz(&e, "FriendlyName", n)?;
    }
    if let Some((guid, class)) = d.class {
        k.sz(&e, "ClassGUID", guid)?;
        k.sz(&e, "Class", class)?;
    }
    Ok(())
}

/// A bootwait Patches entry: a recipe that bootwait applies to a driver image in memory as it loads
/// (`PsSetLoadImageNotifyRoutine`), before its entry point runs.  The point is that Plug and Play
/// copying a stock driver over `system32\drivers` no longer undoes the change - the file on disk is
/// left alone, so its catalog still matches and there is no Found New Hardware wizard either.
pub(crate) fn bootwait_patch(k: &mut Keys<'_>) -> Result<()> {
    let table = r"Services\bootwait\Parameters\Patches";
    let e = table_entry(k, table, BOOTWAIT_MAX_PATCHES, |h, n| {
        string_value(h, n, "Image").eq_ignore_ascii_case(netvsc::IMAGE)
    })?;
    k.sz(&e, "Image", netvsc::IMAGE)?;
    k.dword(&e, "TimeStamp", netvsc::TIMESTAMP)?;
    k.dword(&e, "Size", netvsc::SIZE)?;
    for (i, &(at, expect, write, _)) in netvsc::SITES.iter().enumerate() {
        let s = format!("{e}\\Sites\\{i:02}");
        k.dword(&s, "At", at as u32)?;
        k.set(&s, Value::new("Expect", REG_BINARY, expect.to_vec()))?;
        k.set(&s, Value::new("Write", REG_BINARY, write.to_vec()))?;
    }
    Ok(())
}

/// Adds `dir` to the semicolon-separated REG_EXPAND_SZ `DevicePath`; returns whether it was added.
fn add_device_path(h: &mut Hive, node: Node, dir: &str) -> Result<bool> {
    let cur = string_value(h, node, "DevicePath");
    let cur = if cur.is_empty() {
        r"%SystemRoot%\inf".to_string()
    } else {
        cur
    };
    if cur.split(';').any(|d| d.trim().eq_ignore_ascii_case(dir)) {
        return Ok(false);
    }
    let v = Value::string("DevicePath", REG_EXPAND_SZ, &format!("{cur};{dir}"));
    h.set(node, &v).map_err(hive_err("SOFTWARE"))?;
    Ok(true)
}

/// Whether a bootwait.sys knows the registry tables we write: Devices, Values and Patches (the
/// load-time in-memory patches). The key names sit in its data as UTF-16.
fn has_bootwait_tables(sys: &[u8]) -> bool {
    let has = |s: &str| {
        let w: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
        sys.windows(w.len()).any(|x| x == w.as_slice())
    };
    has(r"\Parameters\Devices") && has(r"\Parameters\Values") && has(r"\Parameters\Patches")
}

/// Installs `comps` into the session's system: the files (patched from the system's or from
/// `files`/`ic`) and the registry entries. Nothing is written before `Session::store`.
pub(crate) fn components(
    s: &mut Session,
    fs: &Fs<'_>,
    comps: Components,
    files: &[PathBuf],
    ic: Option<&Path>,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    log(format!("{}: components {}", s.version, comps.describe()));
    if comps == Components::default() {
        return Ok(());
    }
    {
        let k = s.keys()?;
        if !k.exists(r"Services\vmbus")? {
            return Err(Error(
                "no vmbus service: the Hyper-V Integration Services are not installed on this system"
                    .into(),
            ));
        }
        if (comps.dynamic_memory || comps.vss || comps.gsi) && !k.exists(r"Services\bootwait")? {
            return Err(Error(
                "no bootwait service: Dynamic Memory, VSS and the Guest Service Interface need bootwait's tables (convert the disk to Generation 2 first, or leave them out with --no-dynamic-memory --no-vss --no-gsi)".into(),
            ));
        }
    }

    // The new files, all made before anything is written.
    //
    // bootwait's tables: Devices and Values keep Dynamic Memory, VSS and the Guest Service Interface
    // bound, Patches holds the load-time image patches (netvsc50). An older bootwait.sys ignores
    // them, so it is replaced; without a new one in `files` the components that need the tables
    // fail, and the patches are left out.
    let has_bootwait = s.keys()?.exists(r"Services\bootwait")?;
    let mut patches = has_bootwait;
    if has_bootwait {
        let p = format!(r"{SYSTEM32}\drivers\bootwait.sys");
        if !has_bootwait_tables(&s.read(fs, &p)?) {
            let needed = comps.dynamic_memory || comps.vss || comps.gsi;
            match find_in(files, "bootwait.sys") {
                Some(new) => {
                    if !has_bootwait_tables(&crate::offline::host(&new)?) {
                        return Err(Error(format!(
                            "{}: this bootwait.sys has no Devices, Values and Patches tables either",
                            new.display()
                        )));
                    }
                    log("bootwait.sys: the installed one predates bootwait's registry tables; updated"
                        .into());
                    s.copy_host(&p, &new)?;
                }
                None if needed => {
                    need(
                        files,
                        "bootwait.sys",
                        "one with Devices, Values and Patches tables",
                    )?;
                }
                None => {
                    log("bootwait.sys: the installed one predates the Patches table and --files has no newer one; load-time patches left out".into());
                    patches = false;
                }
            }
        }
    }
    if comps.dynamic_memory {
        let (sys, from) = ic_file(fs, files, ic, "dmvsc", "dmvsc.sys")?;
        log(format!("dmvsc.sys: {from}"));
        let (_, res) = ic_file(fs, files, ic, "dmvsc", "dmvscres.dll")?;
        log(format!("dmvscres.dll: {res}"));
        s.put(
            &format!(r"{SYSTEM32}\drivers\dmvsc.sys"),
            components::dmvsc(&sys, log)?,
        );
        s.copy_host(
            &format!(r"{SYSTEM32}\drivers\mdlex.sys"),
            &need(files, "mdlex.sys", "Dynamic Memory")?,
        )?;
        s.copy_from(fs, &res, &format!(r"{SYSTEM32}\dmvscres.dll"))?;
    }
    if comps.vss || comps.gsi {
        let icsvc = fat::read(fs, &format!(r"{SYSTEM32}\icsvc.dll")).map_err(disk_err)?;
        if comps.vss {
            s.put(
                &format!(r"{SYSTEM32}\icsvcvss.dll"),
                components::icsvc_vss(&icsvc, log)?,
            );
        }
        if comps.gsi {
            s.put(
                &format!(r"{SYSTEM32}\icsvcgsi.dll"),
                components::icsvc_gsi(&icsvc, log)?,
            );
        }
    }
    if comps.synthvid {
        for (dir, name) in [(r"\drivers", "VMBusVideoM.sys"), ("", "VMBusVideoD.dll")] {
            let p = format!(r"{SYSTEM32}{dir}\{name}");
            let stock = fat::read(fs, &p).map_err(|e| {
                Error(format!(
                    "{e} (the Integration Services' video driver is not installed? leave it out with --no-synthvid)"
                ))
            })?;
            let b = components::synthvid(&stock, name, log)?;
            if b != stock {
                let t = fat::fat_time(std::time::SystemTime::now());
                s.write(&p, b, t, None);
            }
        }
    }
    if comps.vmbaud {
        for f in ["vmbaud.inf", "vmbaud.sys"] {
            s.copy_host(
                &format!(r"{VMBAUD_DIR}\{f}"),
                &need(files, f, "the vmbaud sound card")?,
            )?;
        }
    }

    // SYSTEM, current control set.
    let mut k = s.keys()?;
    if patches {
        bootwait_patch(&mut k)?;
        log("SYSTEM: bootwait's load-time patch table (netvsc50)".into());
    }
    if comps.dynamic_memory {
        // dmvsc.sys is a KMDF driver; mdlex.sys has no service: the kernel loads it as dmvsc's
        // import. The INF's NULL driver leaves the device without a service, so the Critical Device
        // Database binds it, and bootwait's Devices entry keeps it bound.
        let s = r"Services\dmvsc";
        k.dword(s, "Type", 1)?;
        k.dword(s, "Start", 3)?;
        k.dword(s, "ErrorControl", 1)?;
        k.expand(s, "ImagePath", r"system32\DRIVERS\dmvsc.sys")?;
        k.sz(s, "DisplayName", "Microsoft Hyper-V Dynamic Memory")?;
        let e = r"Services\Eventlog\System\dmvsc";
        k.expand(
            e,
            "EventMessageFile",
            r"%SystemRoot%\System32\IoLogMsg.dll;%SystemRoot%\System32\dmvscres.dll",
        )?;
        k.dword(e, "TypesSupported", 7)?;
        let d = r"Control\CriticalDeviceDatabase\vmbus#{525074dc-8985-46e2-8057-a307dc18a502}";
        k.sz(d, "Service", "dmvsc")?;
        k.sz(d, "ClassGUID", "{4D36E97D-E325-11CE-BFC1-08002BE10318}")?;
        bootwait_device(
            &mut k,
            &DeviceFix {
                hwid: DMVSC_HWID,
                service: Some("dmvsc"),
                name: Some("Microsoft Hyper-V Dynamic Memory"),
                class: None,
            },
        )?;
        log(
            "SYSTEM: dmvsc service, its Critical Device Database entry and bootwait Devices entry"
                .into(),
        );
    }
    if comps.gsi {
        let p = r"Services\vmicguestinterface\Parameters";
        let dll = r"%SystemRoot%\System32\icsvcgsi.dll";
        // Only where the INF installed the service; else bootwait sets it once it is there.
        if k.exists(r"Services\vmicguestinterface")? {
            k.expand(p, "ServiceDll", dll)?;
        } else {
            log("SYSTEM: no vmicguestinterface service yet (Guest Service Interface off on the VM?); bootwait sets its ServiceDll once Plug and Play has installed it".into());
        }
        bootwait_value(&mut k, p, "ServiceDll", dll)?;
        log("SYSTEM: vmicguestinterface ServiceDll icsvcgsi.dll, kept by bootwait".into());
    }
    if comps.vss {
        // The INF installs no vmicvss service on XP (the device gets a NULL driver), so the whole
        // service comes from here, as the INF's VmIcVss_NT5 makes it on Server 2003.
        let s = r"Services\vmicvss";
        k.dword(s, "Type", 32)?;
        k.dword(s, "Start", 2)?;
        k.dword(s, "ErrorControl", 1)?;
        k.expand(
            s,
            "ImagePath",
            r"%SystemRoot%\system32\svchost.exe -k ICService",
        )?;
        k.sz(s, "DisplayName", "Hyper-V Volume Shadow Copy Requestor")?;
        k.sz(
            s,
            "Description",
            "Coordinates the components that are needed to back up this virtual machine while it is running, using Volume Shadow Copy.",
        )?;
        k.sz(s, "Group", "Extended Base")?;
        k.sz(s, "ObjectName", "LocalSystem")?;
        let p = r"Services\vmicvss\Parameters";
        let dll = r"%SystemRoot%\System32\icsvcvss.dll";
        k.expand(p, "ServiceDll", dll)?;
        k.sz(p, "ServiceMain", "VssServiceMain")?;
        k.dword(p, "ServiceDllUnloadOnStop", 1)?;
        let e = r"Services\Eventlog\Application\vmicvss";
        k.expand(e, "EventMessageFile", r"%SystemRoot%\System32\vmicres.dll")?;
        k.dword(e, "TypesSupported", 7)?;
        bootwait_value(&mut k, p, "ServiceDll", dll)?;
        log("SYSTEM: vmicvss service from icsvcvss.dll, kept by bootwait".into());
    }

    // SOFTWARE.
    let sw = &mut s.software;
    let sw_root = sw.hive.root();
    if comps.vss {
        let n = sw
            .hive
            .create(sw_root, r"Microsoft\Windows NT\CurrentVersion\Svchost")
            .map_err(hive_err("SOFTWARE"))?;
        if sw
            .hive
            .append_multi_sz(n, "ICService", "vmicvss")
            .map_err(hive_err("SOFTWARE"))?
        {
            sw.changed = true;
            log("SOFTWARE: vmicvss added to svchost's ICService group".into());
        }
    }
    if comps.vmbaud {
        let n = sw
            .hive
            .create(sw_root, r"Microsoft\Windows\CurrentVersion")
            .map_err(hive_err("SOFTWARE"))?;
        let dir = format!("%SystemDrive%{VMBAUD_DIR}");
        if add_device_path(&mut sw.hive, n, &dir)? {
            sw.changed = true;
            log(format!("SOFTWARE: DevicePath += {dir}"));
        }
    }
    Ok(())
}

pub fn inject(c: &Inject, log: &mut dyn FnMut(String)) -> Result<()> {
    let (mut img, part, start, len) = open_image(&c.image, c.partition, true)?;
    log(format!(
        "{}: partition {part} (at sector {})",
        c.image.display(),
        start / SECTOR
    ));
    let fs = fat::open(img.window(start, len)).map_err(|e| {
        Error(format!(
            "partition {part}: {e} (only FAT volumes can be changed)"
        ))
    })?;
    let mut s = Session::load(&fs, c.force)?;
    let comps = Components::select(s.version, c.leave_out, c.opt_in);
    components(&mut s, &fs, comps, &c.files, c.ic.as_deref(), log)?;
    s.store(&fs, log)?;
    fs.unmount()
        .map_err(|e| Error(format!("partition {part}: {e}")))?;
    img.flush().map_err(disk_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offline::TempDir;

    /// A SYSTEM hive of an XP installation (HVKIT_TESTDATA/in/system-xpv1.hiv, a Microsoft file
    /// not in the repository); without it this test does nothing.
    #[test]
    fn bootwait_tables_are_updated_not_duplicated() {
        let Some(dir) = std::env::var_os("HVKIT_TESTDATA").map(PathBuf::from) else {
            eprintln!("HVKIT_TESTDATA not set; skipped");
            return;
        };
        let tmp = TempDir::new("inject-test").unwrap();
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
        let dm = DeviceFix {
            hwid: DMVSC_HWID,
            service: Some("dmvsc"),
            name: Some("DM"),
            class: None,
        };
        for _ in 0..2 {
            bootwait_value(&mut k, r"Services\x\Parameters", "ServiceDll", "a.dll").unwrap();
            bootwait_value(&mut k, r"services\X\parameters", "servicedll", "b.dll").unwrap();
            bootwait_value(&mut k, r"Services\y\Parameters", "ServiceDll", "c.dll").unwrap();
            bootwait_device(&mut k, &dm).unwrap();
        }
        let values = k.key(r"Services\bootwait\Parameters\Values").unwrap();
        let entries = k.h.children(values).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(string_value(k.h, entries[0], "Data"), "b.dll");
        let devices = k.key(r"Services\bootwait\Parameters\Devices").unwrap();
        assert_eq!(k.h.children(devices).unwrap().len(), 1);
        for _ in 0..2 {
            bootwait_patch(&mut k).unwrap();
        }
        let patches = k.key(r"Services\bootwait\Parameters\Patches").unwrap();
        let entries = k.h.children(patches).unwrap();
        assert_eq!(entries.len(), 1, "the same patch is written once");
        let e = entries[0];
        assert_eq!(string_value(k.h, e, "Image"), netvsc::IMAGE);
        assert_eq!(
            k.h.value(e, "TimeStamp").unwrap().unwrap().as_dword(),
            Some(netvsc::TIMESTAMP)
        );
        assert_eq!(
            k.h.value(e, "Size").unwrap().unwrap().as_dword(),
            Some(netvsc::SIZE)
        );
        let sites = k
            .key(r"Services\bootwait\Parameters\Patches\00\Sites")
            .unwrap();
        let sites = k.h.children(sites).unwrap();
        assert_eq!(sites.len(), netvsc::SITES.len());
        for (i, node) in sites.iter().enumerate() {
            let (at, expect, write, _) = netvsc::SITES[i];
            assert_eq!(
                k.h.value(*node, "At").unwrap().unwrap().as_dword(),
                Some(at as u32)
            );
            assert_eq!(k.h.value(*node, "Expect").unwrap().unwrap().data, expect);
            assert_eq!(k.h.value(*node, "Write").unwrap().unwrap().data, write);
        }
        let sw = k.key("DevicePathTest").unwrap();
        assert!(add_device_path(k.h, sw, r"%SystemDrive%\D").unwrap());
        assert!(!add_device_path(k.h, sw, r"%systemdrive%\d").unwrap());
        assert_eq!(
            string_value(k.h, sw, "DevicePath"),
            r"%SystemRoot%\inf;%SystemDrive%\D"
        );
    }
}
