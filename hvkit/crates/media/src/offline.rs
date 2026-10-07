//! An installed XP or Server 2003 on a FAT volume of a disk image, changed offline: its SYSTEM and
//! SOFTWARE hives (copied to temporary files for the hive backend) and the files to write are kept
//! in memory until [`Session::store`], so that a failed check leaves the image as it was.

use crate::components::NtVersion;
use crate::{Error, Result};
use disk::Image;
use disk::fat::{self, Fs};
use fatfs::{DateTime, FileAttributes};
use hive::{Header, Hive, Node, REG_EXPAND_SZ, REG_SZ, Value};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) const SYSTEM32: &str = r"\WINDOWS\system32";
pub(crate) const SYSTEM_HIVE: &str = r"\WINDOWS\system32\config\system";
pub(crate) const SOFTWARE_HIVE: &str = r"\WINDOWS\system32\config\software";
/// The folder the Integration Services' setup leaves below Program Files (whatever its name).
pub(crate) const IC_FOLDER: &str = "Hyper-V Integration Services";

pub(crate) fn hive_err(what: &str) -> impl Fn(hive::Error) -> Error + '_ {
    move |e| Error(format!("{what}: {e}"))
}

pub(crate) fn disk_err(e: disk::Error) -> Error {
    Error(e.0)
}

/// Reads a host file.
pub(crate) fn host(p: &Path) -> Result<Vec<u8>> {
    std::fs::read(p).map_err(|e| Error(format!("{}: {e}", p.display())))
}

/// Opens an image and finds the installed system: partition `n`, else the FAT partition with the
/// SYSTEM hive. Returns the image, the partition and its byte range.
pub(crate) fn open_image(
    path: &Path,
    n: Option<usize>,
    writable: bool,
) -> Result<(Image, usize, u64, u64)> {
    let mut img = Image::open(path, writable).map_err(disk_err)?;
    let part = match n {
        Some(n) => n,
        None => disk::find_partition(&mut img, SYSTEM_HIVE).map_err(|e| {
            match ntfs_partition(&mut img) {
                Some(n) => Error(format!(
                    "{}: partition {n} is NTFS; only FAT volumes can be changed",
                    path.display()
                )),
                None => disk_err(e),
            }
        })?,
    };
    let (start, len) = disk::partition(&mut img, part).map_err(disk_err)?;
    Ok((img, part, start, len))
}

/// The first MBR partition with an NTFS boot sector.
fn ntfs_partition(img: &mut Image) -> Option<usize> {
    (1..=4).find(|&n| {
        let mut s = [0u8; 512];
        disk::partition(img, n)
            .is_ok_and(|(start, len)| len >= 512 && img.read_at(start, &mut s).is_ok())
            && &s[3..11] == b"NTFS    "
    })
}

/// A temporary directory removed when dropped.
pub(crate) struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(what: &str) -> Result<TempDir> {
        let d = std::env::temp_dir().join(format!("hvkit-{what}-{}", std::process::id()));
        std::fs::create_dir_all(&d).map_err(|e| Error(format!("{}: {e}", d.display())))?;
        Ok(TempDir(d))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A hive of the volume, copied to a temporary file for the hive backend.
pub(crate) struct HiveCopy {
    path: &'static str,
    tmp: PathBuf,
    pub hive: Hive,
    /// Written back by `Session::store` only if set.
    pub changed: bool,
}

impl HiveCopy {
    fn load(fs: &Fs<'_>, path: &'static str, tmp_dir: &Path, force: bool) -> Result<HiveCopy> {
        let data = fat::read(fs, path).map_err(disk_err)?;
        let h = Header::read(&data).map_err(hive_err(path))?;
        if h.dirty() && !force {
            return Err(Error(format!(
                "{path}: the hive is dirty (sequence {} / {}): its log was not written back (shut the system down cleanly); refusing without --force",
                h.sequence.0, h.sequence.1
            )));
        }
        let tmp = tmp_dir.join(path.rsplit('\\').next().unwrap_or("hive"));
        std::fs::write(&tmp, data).map_err(|e| Error(format!("{}: {e}", tmp.display())))?;
        let hive = Hive::open(&tmp, true).map_err(hive_err(path))?;
        Ok(HiveCopy {
            path,
            tmp,
            hive,
            changed: false,
        })
    }

    fn store(mut self, fs: &Fs<'_>) -> Result<()> {
        self.hive.commit(None).map_err(hive_err(self.path))?;
        drop(self.hive);
        let data = host(&self.tmp)?;
        fat::write(fs, self.path, &data, Some(SystemTime::now())).map_err(disk_err)
    }
}

/// A file to write.
struct FileWrite {
    path: String,
    data: Vec<u8>,
    mtime: DateTime,
    /// Exactly these, or (None) those of the file replaced.
    attributes: Option<FileAttributes>,
}

/// The installed system's hives and the files to write.
pub(crate) struct Session {
    pub system: HiveCopy,
    pub software: HiveCopy,
    pub version: NtVersion,
    /// The current control set, e.g. "ControlSet001".
    pub cs: String,
    writes: Vec<FileWrite>,
    _tmp: TempDir,
}

impl Session {
    /// Loads SYSTEM and SOFTWARE; the version comes from SOFTWARE's CurrentVersion.
    pub fn load(fs: &Fs<'_>, force: bool) -> Result<Session> {
        let tmp = TempDir::new("hives")?;
        let mut system = HiveCopy::load(fs, SYSTEM_HIVE, &tmp.0, force)?;
        let mut software = HiveCopy::load(fs, SOFTWARE_HIVE, &tmp.0, force)?;
        let h = &software.hive;
        let cv = h
            .find(h.root(), r"Microsoft\Windows NT\CurrentVersion")
            .map_err(hive_err("SOFTWARE"))?
            .ok_or_else(|| Error("SOFTWARE: no Microsoft\\Windows NT\\CurrentVersion".into()))?;
        let ver = string_value(h, cv, "CurrentVersion");
        let (major, minor) = ver
            .split_once('.')
            .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
            .ok_or_else(|| Error(format!("SOFTWARE: CurrentVersion {ver:?}")))?;
        let version = NtVersion::from_numbers(major, minor)?;
        // offreg writes the hives for this version (XP 5.1, Server 2003 5.2).
        system.hive.set_os_version(major, minor);
        software.hive.set_os_version(major, minor);
        let cs = system
            .hive
            .current_control_set()
            .map_err(hive_err("SYSTEM"))?;
        Ok(Session {
            system,
            software,
            version,
            cs,
            writes: Vec::new(),
            _tmp: tmp,
        })
    }

    /// The current control set of SYSTEM, to change.
    pub fn keys(&mut self) -> Result<Keys<'_>> {
        let h = &mut self.system.hive;
        let root = h
            .find(h.root(), &self.cs)
            .map_err(hive_err("SYSTEM"))?
            .ok_or_else(|| Error(format!("SYSTEM: no {}", self.cs)))?;
        self.system.changed = true;
        Ok(Keys {
            h,
            root,
            what: "SYSTEM",
        })
    }

    /// Writes `path` with `data` (now, archive bit only, as Windows writes a new file).
    pub fn put(&mut self, path: &str, data: Vec<u8>) {
        self.write(
            path,
            data,
            fat::fat_time(SystemTime::now()),
            Some(FileAttributes::ARCHIVE),
        );
    }

    /// Writes `path` with a host file, keeping its modification time (as Copy-Item does).
    pub fn copy_host(&mut self, path: &str, from: &Path) -> Result<()> {
        let data = host(from)?;
        let t = std::fs::metadata(from)
            .and_then(|m| m.modified())
            .map_err(|e| Error(format!("{}: {e}", from.display())))?;
        self.write(path, data, fat::fat_time(t), Some(FileAttributes::ARCHIVE));
        Ok(())
    }

    /// Writes `to` with a file of the volume, keeping its time and attributes (as Copy-Item).
    pub fn copy_within(&mut self, fs: &Fs<'_>, from: &str, to: &str) -> Result<()> {
        let e = fat::entry(fs, from).map_err(disk_err)?;
        let data = fat::read(fs, from).map_err(disk_err)?;
        self.write(to, data, e.modified, Some(e.attributes));
        Ok(())
    }

    /// Writes `to` with `data` from `src`, keeping the source's time (and on the volume, its
    /// attributes).
    pub fn copy_from(&mut self, fs: &Fs<'_>, src: &Source, to: &str) -> Result<()> {
        match src {
            Source::Host(p) => self.copy_host(to, p),
            Source::Volume(p) => self.copy_within(fs, p, to),
        }
    }

    pub fn write(
        &mut self,
        path: &str,
        data: Vec<u8>,
        mtime: DateTime,
        attributes: Option<FileAttributes>,
    ) {
        self.writes.retain(|w| !w.path.eq_ignore_ascii_case(path));
        self.writes.push(FileWrite {
            path: path.to_string(),
            data,
            mtime,
            attributes,
        });
    }

    /// A file as it will be: the pending write, else the volume's.
    pub fn read(&self, fs: &Fs<'_>, path: &str) -> Result<Vec<u8>> {
        match self
            .writes
            .iter()
            .find(|w| w.path.eq_ignore_ascii_case(path))
        {
            Some(w) => Ok(w.data.clone()),
            None => fat::read(fs, path).map_err(disk_err),
        }
    }

    /// Writes the files, then the hives that were changed.
    pub fn store(self, fs: &Fs<'_>, log: &mut dyn FnMut(String)) -> Result<()> {
        for w in &self.writes {
            if let Some((d, _)) = w.path.rsplit_once('\\') {
                fat::mkdir_p(fs, d).map_err(disk_err)?;
            }
            fat::write_as(fs, &w.path, &w.data, Some(w.mtime), w.attributes).map_err(disk_err)?;
            log(format!("wrote {} ({} bytes)", w.path, w.data.len()));
        }
        for h in [self.system, self.software] {
            if h.changed {
                let p = h.path;
                h.store(fs)?;
                log(format!("wrote {p}"));
            }
        }
        Ok(())
    }
}

/// Values written into keys below a root (the current control set), creating the keys.
pub(crate) struct Keys<'a> {
    pub h: &'a mut Hive,
    pub root: Node,
    pub what: &'static str,
}

impl Keys<'_> {
    pub fn key(&mut self, path: &str) -> Result<Node> {
        self.h.create(self.root, path).map_err(hive_err(self.what))
    }
    pub fn find(&self, path: &str) -> Result<Option<Node>> {
        self.h.find(self.root, path).map_err(hive_err(self.what))
    }
    pub fn exists(&self, path: &str) -> Result<bool> {
        Ok(self.find(path)?.is_some())
    }
    pub fn set(&mut self, path: &str, v: Value) -> Result<()> {
        let k = self.key(path)?;
        self.h.set(k, &v).map_err(hive_err(self.what))
    }
    pub fn sz(&mut self, path: &str, name: &str, s: &str) -> Result<()> {
        self.set(path, Value::string(name, REG_SZ, s))
    }
    pub fn expand(&mut self, path: &str, name: &str, s: &str) -> Result<()> {
        self.set(path, Value::string(name, REG_EXPAND_SZ, s))
    }
    pub fn dword(&mut self, path: &str, name: &str, v: u32) -> Result<()> {
        self.set(path, Value::dword(name, v))
    }
    /// The value `name` of the key `path`, if both exist.
    pub fn get(&self, path: &str, name: &str) -> Result<Option<Value>> {
        match self.find(path)? {
            Some(k) => self.h.value(k, name).map_err(hive_err(self.what)),
            None => Ok(None),
        }
    }
    /// Deletes the value `name` of the key `path`; whether it was there.
    pub fn delete_value(&mut self, path: &str, name: &str) -> Result<bool> {
        match self.find(path)? {
            Some(k) => self.h.delete_value(k, name).map_err(hive_err(self.what)),
            None => Ok(false),
        }
    }
}

/// The REG_SZ value `name` of `node`, or "".
pub(crate) fn string_value(h: &Hive, node: Node, name: &str) -> String {
    h.value(node, name)
        .ok()
        .flatten()
        .and_then(|v| v.as_strings().into_iter().next())
        .unwrap_or_default()
}

/// Where a file comes from.
pub(crate) enum Source {
    Host(PathBuf),
    /// A path on the volume.
    Volume(String),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Host(p) => write!(f, "{}", p.display()),
            Source::Volume(p) => write!(f, "{p} (on the volume)"),
        }
    }
}

/// The Integration Services file `pkg\name`, from the first of: `files` (flat), `ic` (one folder
/// per package), the Integration Services folder below a top-level folder of the volume (Program
/// Files, whatever its name).
pub(crate) fn ic_file(
    fs: &Fs<'_>,
    files: &[PathBuf],
    ic: Option<&Path>,
    pkg: &str,
    name: &str,
) -> Result<(Vec<u8>, Source)> {
    if let Some(p) = find_in(files, name) {
        return Ok((host(&p)?, Source::Host(p)));
    }
    if let Some(ic) = ic {
        let p = crate::components::find_file(&ic.join(pkg), name)?;
        return Ok((host(&p)?, Source::Host(p)));
    }
    for e in fat::list(fs, "").map_err(disk_err)? {
        let p = format!(r"\{}\{IC_FOLDER}\{pkg}\{name}", e.name);
        if e.dir && fat::exists(fs, &p) {
            return Ok((fat::read(fs, &p).map_err(disk_err)?, Source::Volume(p)));
        }
    }
    Err(Error(format!(
        "{name}: not in {} and the volume has no {IC_FOLDER}\\{pkg}\\{name}",
        dir_list(files)
    )))
}

fn dir_list(dirs: &[PathBuf]) -> String {
    if dirs.is_empty() {
        return "--files (none given)".into();
    }
    dirs.iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The file `name` in the first of the directories that has it (any case).
pub(crate) fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter()
        .find_map(|d| crate::components::find_file(d, name).ok())
}

/// The file `name` from the first of `dirs` that has it, or an error naming `what` it is.
pub(crate) fn need(dirs: &[PathBuf], name: &str, what: &str) -> Result<PathBuf> {
    find_in(dirs, name).ok_or_else(|| Error(format!("{name} ({what}): not in {}", dir_list(dirs))))
}
