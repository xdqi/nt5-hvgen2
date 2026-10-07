//! An installed XP or Server 2003 changed offline: the components of `media::components` put into
//! its FAT volume in a disk image (raw or VHDX), without Windows. For an installation that already
//! boots on Generation 2 (bootwait installed) and has the Integration Services 6.3; this is what
//! `migrate/inject.ps1 -Dmvsc -Mdlex -DmvscRes -GuestInterfacePatch -VssPatch` does there.
//!
//! Unlike on the setup CD, the Integration Services' INFs are installed already, with NULL drivers
//! for Dynamic Memory and VSS on XP (Server 2003 has the stock dmvsc.sys, which is replaced), so
//! their services and bindings are written into the registry here. Plug and Play installs the devices again from those INFs now and then (on a new VM, for
//! one), which takes dmvsc's binding away and points the services' ServiceDll back to icsvc.dll;
//! bootwait's tables (Parameters\Devices, Parameters\Values) put them back on every boot, so the
//! entries go there too. SynthVid's two files are patched where they are installed (a new install
//! of the device by Plug and Play copies the stock ones back; then run this again). The vmbaud
//! package goes next to where a setup CD puts it, into the system's DevicePath.

use crate::components::{self, Components, NtVersion, find_file};
use crate::{Error, Result};
use disk::fat::{self, Fs};
use disk::{Image, SECTOR};
use hive::{Header, Hive, Node, REG_EXPAND_SZ, REG_SZ, Value};
use std::path::{Path, PathBuf};

pub struct Inject {
    pub image: PathBuf,
    /// The partition (default: the FAT partition that has \WINDOWS\system32\config\system).
    pub partition: Option<usize>,
    /// mdlex.sys for Dynamic Memory; vmbaud.inf and vmbaud.sys for the sound card.
    pub files: Option<PathBuf>,
    /// The Integration Services packages, for dmvsc.sys and dmvscres.dll (default: the copy the
    /// Integration Services left in Program Files on the volume).
    pub ic: Option<PathBuf>,
    pub leave_out: Components,
    pub opt_in: Components,
    /// Change hives whose logs were not written back (an unclean shutdown).
    pub force: bool,
}

const SYSTEM32: &str = r"\WINDOWS\system32";
const SYSTEM_HIVE: &str = r"\WINDOWS\system32\config\system";
const SOFTWARE_HIVE: &str = r"\WINDOWS\system32\config\software";
const IC_ON_DISK: &str = r"\Program Files\Hyper-V Integration Services";
const VMBAUD_DIR: &str = r"\Drivers\HV\vmbaud";
const DMVSC_HWID: &str = r"VMBUS\{525074dc-8985-46e2-8057-a307dc18a502}";
/// bootwait.c's table sizes (BW_MAX_VALUES, BW_MAX_FIXES).
const BOOTWAIT_MAX_VALUES: usize = 8;
const BOOTWAIT_MAX_DEVICES: usize = 16;

fn hive_err(what: &str) -> impl Fn(hive::Error) -> Error + '_ {
    move |e| Error(format!("{what}: {e}"))
}

fn disk_err(e: disk::Error) -> Error {
    Error(e.0)
}

/// A hive of the volume, copied to a temporary file for hivex and copied back by `store`.
struct HiveCopy {
    path: &'static str,
    tmp: PathBuf,
    hive: Hive,
}

impl HiveCopy {
    fn load(fs: &Fs<'_>, path: &'static str, tmp_dir: &Path, force: bool) -> Result<HiveCopy> {
        let data = fat::read(fs, path).map_err(disk_err)?;
        let h = Header::read(&data).map_err(hive_err(path))?;
        if h.dirty() && !force {
            return Err(Error(format!(
                "{path}: the hive is dirty (sequence {} / {}): its log was not written back (shut XP down cleanly); refusing without --force",
                h.sequence.0, h.sequence.1
            )));
        }
        let tmp = tmp_dir.join(path.rsplit('\\').next().unwrap_or("hive"));
        std::fs::write(&tmp, data).map_err(|e| Error(format!("{}: {e}", tmp.display())))?;
        let hive = Hive::open(&tmp, true).map_err(hive_err(path))?;
        Ok(HiveCopy { path, tmp, hive })
    }

    fn store(mut self, fs: &Fs<'_>) -> Result<()> {
        self.hive.commit(None).map_err(hive_err(self.path))?;
        drop(self.hive);
        let data =
            std::fs::read(&self.tmp).map_err(|e| Error(format!("{}: {e}", self.tmp.display())))?;
        fat::write(fs, self.path, &data, Some(std::time::SystemTime::now())).map_err(disk_err)
    }
}

/// A temporary directory removed when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Values written into a key below some root, creating the keys.
struct Keys<'a> {
    h: &'a mut Hive,
    root: Node,
    what: &'static str,
}

impl Keys<'_> {
    fn key(&mut self, path: &str) -> Result<Node> {
        self.h.create(self.root, path).map_err(hive_err(self.what))
    }
    fn set(&mut self, path: &str, v: Value) -> Result<()> {
        let k = self.key(path)?;
        self.h.set(k, &v).map_err(hive_err(self.what))
    }
    fn sz(&mut self, path: &str, name: &str, s: &str) -> Result<()> {
        self.set(path, Value::string(name, REG_SZ, s))
    }
    fn expand(&mut self, path: &str, name: &str, s: &str) -> Result<()> {
        self.set(path, Value::string(name, REG_EXPAND_SZ, s))
    }
    fn dword(&mut self, path: &str, name: &str, v: u32) -> Result<()> {
        self.set(path, Value::dword(name, v))
    }
    fn exists(&self, path: &str) -> Result<bool> {
        Ok(self
            .h
            .find(self.root, path)
            .map_err(hive_err(self.what))?
            .is_some())
    }
}

/// The REG_SZ value `name` of `node`, or "".
fn string_value(h: &Hive, node: Node, name: &str) -> String {
    h.value(node, name)
        .ok()
        .flatten()
        .and_then(|v| v.as_strings().into_iter().next())
        .unwrap_or_default()
}

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
fn bootwait_value(k: &mut Keys<'_>, key: &str, name: &str, data: &str) -> Result<()> {
    let table = r"Services\bootwait\Parameters\Values";
    let e = table_entry(k, table, BOOTWAIT_MAX_VALUES, |h, n| {
        string_value(h, n, "Key").eq_ignore_ascii_case(key)
            && string_value(h, n, "Name").eq_ignore_ascii_case(name)
    })?;
    k.sz(&e, "Key", key)?;
    k.sz(&e, "Name", name)?;
    k.sz(&e, "Data", data)
}

/// A bootwait Devices entry: device nodes of `hwid` without a Service get `service`, and the name.
fn bootwait_device(k: &mut Keys<'_>, hwid: &str, service: &str, name: &str) -> Result<()> {
    let table = r"Services\bootwait\Parameters\Devices";
    let e = table_entry(k, table, BOOTWAIT_MAX_DEVICES, |h, n| {
        string_value(h, n, "HardwareID").eq_ignore_ascii_case(hwid)
    })?;
    k.sz(&e, "HardwareID", hwid)?;
    k.sz(&e, "Service", service)?;
    k.sz(&e, "FriendlyName", name)
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

/// Whether a bootwait.sys reads Parameters\Devices and Parameters\Values (its UTF-16 key names).
fn has_bootwait_tables(sys: &[u8]) -> bool {
    let has = |s: &str| {
        let w: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
        sys.windows(w.len()).any(|x| x == w.as_slice())
    };
    has(r"\Parameters\Devices") && has(r"\Parameters\Values")
}

/// Reads a host file.
fn host(p: &Path) -> Result<Vec<u8>> {
    std::fs::read(p).map_err(|e| Error(format!("{}: {e}", p.display())))
}

pub fn inject(c: &Inject, log: &mut dyn FnMut(String)) -> Result<()> {
    let mut img = Image::open(&c.image, true).map_err(disk_err)?;
    let part = match c.partition {
        Some(n) => n,
        None => disk::find_partition(&mut img, SYSTEM_HIVE).map_err(disk_err)?,
    };
    let (start, len) = disk::partition(&mut img, part).map_err(disk_err)?;
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
    let tmp = TempDir(std::env::temp_dir().join(format!("hvkit-inject-{}", std::process::id())));
    std::fs::create_dir_all(&tmp.0).map_err(|e| Error(format!("{}: {e}", tmp.0.display())))?;
    let mut system = HiveCopy::load(&fs, SYSTEM_HIVE, &tmp.0, c.force)?;
    let mut software = HiveCopy::load(&fs, SOFTWARE_HIVE, &tmp.0, c.force)?;

    let sw_root = software.hive.root();
    let cv = software
        .hive
        .find(sw_root, r"Microsoft\Windows NT\CurrentVersion")
        .map_err(hive_err("SOFTWARE"))?
        .ok_or_else(|| Error("SOFTWARE: no Microsoft\\Windows NT\\CurrentVersion".into()))?;
    let ver = string_value(&software.hive, cv, "CurrentVersion");
    let (major, minor) = ver
        .split_once('.')
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .ok_or_else(|| Error(format!("SOFTWARE: CurrentVersion {ver:?}")))?;
    let version = NtVersion::from_numbers(major, minor)?;
    let comps = Components::select(version, c.leave_out, c.opt_in);
    log(format!("{version}: components {}", comps.describe()));
    if comps == Components::default() {
        return Ok(());
    }

    let cs = system
        .hive
        .current_control_set()
        .map_err(hive_err("SYSTEM"))?;
    let cs_node = system
        .hive
        .find(system.hive.root(), &cs)
        .map_err(hive_err("SYSTEM"))?
        .expect("checked by current_control_set");
    let mut k = Keys {
        h: &mut system.hive,
        root: cs_node,
        what: "SYSTEM",
    };
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
    let files = || {
        c.files.as_deref().ok_or_else(|| {
            Error("--files is needed (mdlex.sys, bootwait.sys, vmbaud.inf, vmbaud.sys)".into())
        })
    };

    // The new files, all made before anything is written.
    let mut writes: Vec<(String, Vec<u8>)> = Vec::new();
    if comps.dynamic_memory || comps.vss || comps.gsi {
        // A bootwait.sys from before its Devices and Values tables ignores the entries below, and
        // Dynamic Memory loses its binding on the first boot; the current one reads the same
        // Parameters otherwise.
        let p = format!(r"{SYSTEM32}\drivers\bootwait.sys");
        let installed = fat::read(&fs, &p).map_err(disk_err)?;
        if !has_bootwait_tables(&installed) {
            let new = host(&files()?.join("bootwait.sys"))?;
            if !has_bootwait_tables(&new) {
                return Err(Error(format!(
                    "{}: this bootwait.sys has no Devices and Values tables either",
                    files()?.join("bootwait.sys").display()
                )));
            }
            log("bootwait.sys: the installed one has no Devices and Values tables; updated".into());
            writes.push((p, new));
        }
    }
    if comps.dynamic_memory {
        let (sys, res) = match &c.ic {
            Some(ic) => {
                let d = ic.join("dmvsc");
                (
                    host(&find_file(&d, "dmvsc.sys")?)?,
                    host(&find_file(&d, "dmvscres.dll")?)?,
                )
            }
            None => (
                fat::read(&fs, &format!(r"{IC_ON_DISK}\dmvsc\dmvsc.sys")).map_err(disk_err)?,
                fat::read(&fs, &format!(r"{IC_ON_DISK}\dmvsc\dmvscres.dll")).map_err(disk_err)?,
            ),
        };
        writes.push((
            format!(r"{SYSTEM32}\drivers\dmvsc.sys"),
            components::dmvsc(&sys, log)?,
        ));
        writes.push((
            format!(r"{SYSTEM32}\drivers\mdlex.sys"),
            host(&files()?.join("mdlex.sys"))?,
        ));
        writes.push((format!(r"{SYSTEM32}\dmvscres.dll"), res));
    }
    if comps.vss || comps.gsi {
        let icsvc = fat::read(&fs, &format!(r"{SYSTEM32}\icsvc.dll")).map_err(disk_err)?;
        if comps.vss {
            writes.push((
                format!(r"{SYSTEM32}\icsvcvss.dll"),
                components::icsvc_vss(&icsvc, log)?,
            ));
        }
        if comps.gsi {
            writes.push((
                format!(r"{SYSTEM32}\icsvcgsi.dll"),
                components::icsvc_gsi(&icsvc, log)?,
            ));
        }
    }
    if comps.synthvid {
        for (dir, name) in [(r"\drivers", "VMBusVideoM.sys"), ("", "VMBusVideoD.dll")] {
            let p = format!(r"{SYSTEM32}{dir}\{name}");
            let stock = fat::read(&fs, &p).map_err(|e| {
                Error(format!(
                    "{e} (the Integration Services' video driver is not installed? leave it out with --no-synthvid)"
                ))
            })?;
            let b = components::synthvid(&stock, name, log)?;
            if b != stock {
                writes.push((p, b));
            }
        }
    }
    if comps.vmbaud {
        for f in ["vmbaud.inf", "vmbaud.sys"] {
            writes.push((format!(r"{VMBAUD_DIR}\{f}"), host(&files()?.join(f))?));
        }
    }

    // SYSTEM, current control set.
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
            DMVSC_HWID,
            "dmvsc",
            "Microsoft Hyper-V Dynamic Memory",
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
    if comps.vss {
        let n = software
            .hive
            .create(sw_root, r"Microsoft\Windows NT\CurrentVersion\Svchost")
            .map_err(hive_err("SOFTWARE"))?;
        if software
            .hive
            .append_multi_sz(n, "ICService", "vmicvss")
            .map_err(hive_err("SOFTWARE"))?
        {
            log("SOFTWARE: vmicvss added to svchost's ICService group".into());
        }
    }
    if comps.vmbaud {
        let n = software
            .hive
            .create(sw_root, r"Microsoft\Windows\CurrentVersion")
            .map_err(hive_err("SOFTWARE"))?;
        let dir = format!("%SystemDrive%{VMBAUD_DIR}");
        if add_device_path(&mut software.hive, n, &dir)? {
            log(format!("SOFTWARE: DevicePath += {dir}"));
        }
    }

    for (p, data) in &writes {
        if let Some((d, _)) = p.rsplit_once('\\') {
            fat::mkdir_p(&fs, d).map_err(disk_err)?;
        }
        fat::write(&fs, p, data, Some(std::time::SystemTime::now())).map_err(disk_err)?;
        log(format!("wrote {p}"));
    }
    system.store(&fs)?;
    software.store(&fs)?;
    log(format!("wrote {SYSTEM_HIVE}, {SOFTWARE_HIVE}"));
    fs.unmount()
        .map_err(|e| Error(format!("partition {part}: {e}")))?;
    img.flush().map_err(disk_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A SYSTEM hive of an XP installation (HVKIT_TESTDATA/in/system-xpv1.hiv, a Microsoft file
    /// not in the repository); without it this test does nothing.
    #[test]
    fn bootwait_tables_are_updated_not_duplicated() {
        let Some(dir) = std::env::var_os("HVKIT_TESTDATA").map(PathBuf::from) else {
            eprintln!("HVKIT_TESTDATA not set; skipped");
            return;
        };
        let tmp =
            TempDir(std::env::temp_dir().join(format!("hvkit-inject-test-{}", std::process::id())));
        std::fs::create_dir_all(&tmp.0).unwrap();
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
        for _ in 0..2 {
            bootwait_value(&mut k, r"Services\x\Parameters", "ServiceDll", "a.dll").unwrap();
            bootwait_value(&mut k, r"services\X\parameters", "servicedll", "b.dll").unwrap();
            bootwait_value(&mut k, r"Services\y\Parameters", "ServiceDll", "c.dll").unwrap();
            bootwait_device(&mut k, DMVSC_HWID, "dmvsc", "DM").unwrap();
        }
        let values = k.key(r"Services\bootwait\Parameters\Values").unwrap();
        let entries = k.h.children(values).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(string_value(k.h, entries[0], "Data"), "b.dll");
        let devices = k.key(r"Services\bootwait\Parameters\Devices").unwrap();
        assert_eq!(k.h.children(devices).unwrap().len(), 1);
        let sw = k.key("DevicePathTest").unwrap();
        assert!(add_device_path(k.h, sw, r"%SystemDrive%\D").unwrap());
        assert!(!add_device_path(k.h, sw, r"%systemdrive%\d").unwrap());
        assert_eq!(
            string_value(k.h, sw, "DevicePath"),
            r"%SystemRoot%\inf;%SystemDrive%\D"
        );
    }
}
