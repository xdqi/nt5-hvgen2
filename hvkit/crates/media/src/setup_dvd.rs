//! `hvkit setup-dvd`: a Windows NT 6.x (Vista and later) setup DVD for Hyper-V Generation 2.
//!
//! Where `setup-cd` builds an NT 5.x CD out of TXTSETUP.SIF and a hive, NT 6 keeps its drivers and
//! registry in two Windows images: `sources/boot.wim` (its image 2 is Windows Setup, a Windows PE)
//! and `sources/install.wim` (the system it installs). This changes both through the `wim` crate,
//! so it needs neither Windows nor DISM: files are added or replaced (a replaced file keeps its
//! security descriptor, attributes and DOS name), hives are edited with [`hive::reg::import`], and
//! the images are written anew with every unchanged resource copied as it is.
//!
//! Driver packages go into `$WinPEDriver$` at the root of the DVD, which Windows Setup reads by
//! itself (Windows 7 and later): it loads them into Windows PE when it starts and adds them to the
//! installed system. Setup stops on a boot-critical package it cannot install, and it installs
//! none unsigned, so those (VMBus, storage) reach the images only through `--replace-inbox`, and a
//! package that took over one of the images' own is not staged a second time.
//!
//! Two things the NT 5 CDs never needed, both from Hyper-V Generation 2 having neither VGA nor the
//! old boot devices:
//!
//! - **A display.** Gen2 has no VGA, so `vga.sys` never makes a device and the screen stays black
//!   (Setup does not even show its text pages). Pass `--hvfb`: a legacy VideoPort miniport over the
//!   firmware frame buffer, installed with the registry Win7 needs around it (VideoID and the mode).
//!   The alternative is SynthVid, `--driver` its package instead and let PnP take it; with hvfb
//!   either of them owning the frame buffer, use `--deny-synthvid` so the other one cannot start
//!   mid-setup and move the VRAM away.
//! - **A wait for the boot disk.** It is a VMBus child that storvsc reports late. Windows waits for
//!   it only when `PollBootPartitionTimeout` is set (`nt!PnpBootDeviceWait` reads it first; without
//!   it there is no wait at all) and then gives a 0x7B. The default here is 30000 ms.
//!
//! Drivers have to be one consistent set. Setup stages the packages unsigned, and at device install
//! the image's own signed package of the same name outranks them (rank 0x00ff0000 against
//! 0x80ff0000): the system then runs its own `vmbus.sys` beside the package's `vmbkmcl.sys` and
//! `hyperkbd.sys`, which is why the keyboard starts with code 10. Windows PE has the same problem
//! before Setup even starts, with its own storage and VMBus drivers. `--replace-inbox` turns the
//! image's own package of each name into the given one (see [`replace_inbox`]).

use crate::{Error, Result, extract_cached, io, remove_if_exists};
use hive::{Hive, reg};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use wim::{AddOptions, ImageIndex, OpenOptions, Wim};
use wim_format::{file_archive::FileArchive, metadata::Metadata, ntfs_upcase::uppercase};

/// The root the .reg texts built here address a hive with (any second component does;
/// hive::reg::import takes the first two components as the hive's root).
const REG_ROOT: &str = "HKEY_LOCAL_MACHINE\\W7";
/// What hive::reg::import wants on the first line of the texts built here.
const REG_HEADER: &str = "Windows Registry Editor Version 5.00\r\n\r\n";

/// Windows Setup's image in the DVD's sources directory.
const BOOT_WIM: &str = "sources/boot.wim";
/// The image the installed system comes from.
const INSTALL_WIM: &str = "sources/install.wim";

/// Where the driver store keeps each package's files, in a folder named after its INF.
const STORE: &str = "/Windows/System32/DriverStore/FileRepository";

pub struct SetupDvd {
    /// The original DVD (only read).
    pub source: PathBuf,
    /// The ISO to write (an existing one is rewritten in place, keeping its ACL).
    pub out: PathBuf,
    /// Driver package folders, for Setup to install from `$WinPEDriver$`.
    pub drivers: Vec<PathBuf>,
    /// Turn the images' own packages of the same names into these (see [`replace_inbox`]).
    pub replace_inbox: bool,
    /// Files placed in both images, `(host file, path below the Windows volume root)`.
    pub copies: Vec<(PathBuf, String)>,
    /// .reg files applied to a hive of both images, `(hive name, host file)`.
    pub regs: Vec<(String, PathBuf)>,
    /// hvfb.sys: the display for a machine with no VGA (it gets its registry here too).
    pub hvfb: Option<PathBuf>,
    /// The mode hvfb starts the installed system in, WIDTHxHEIGHTxBPP (default 1024x768x32).
    pub hvfb_mode: String,
    /// Keep SynthVid from being installed, so hvfb keeps the frame buffer.
    pub deny_synthvid: bool,
    /// `PollBootPartitionTimeout` in milliseconds; 0 leaves the value alone.
    pub poll_boot_partition_ms: u32,
    /// Which image of sources/boot.wim is Windows Setup (default 2).
    pub boot_index: u32,
    /// Which image of sources/install.wim to change (default 1).
    pub install_index: u32,
    /// Leave sources/boot.wim alone.
    pub no_boot_wim: bool,
    /// Leave sources/install.wim alone.
    pub no_install_wim: bool,
    /// Answer the pages of the install that the DVD would otherwise ask (autounattend.xml).
    pub unattend: bool,
    /// A file with the product key (one line; never printed).
    pub product_key_file: Option<PathBuf>,
    /// The name autounattend.xml gives the machine.
    pub computer_name: Option<String>,
    /// autounattend.xml's processorArchitecture (default x86).
    pub arch: Option<String>,
    /// Where extracted DVDs are kept between runs (default ~/.cache/hvkit/dvd/<DVD>).
    pub cache: PathBuf,
    /// Where the DVD is assembled (default: OUT with .d appended; deleted first).
    pub work: PathBuf,
    /// A bash script run in the tree just before mastering.
    pub hook: Option<PathBuf>,
}

fn remove_dir_if_exists(p: &Path) -> Result<()> {
    match std::fs::remove_dir_all(p) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(p)(e)),
        _ => Ok(()),
    }
}

/// Beside the tree, kept out of the DVD: what the images are given, until they are written.
fn scratch(c: &SetupDvd) -> PathBuf {
    let mut s = c.work.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

/// A copy of the extracted DVD, hard links where the two are on one filesystem.
fn link_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(io(to))?;
    for e in std::fs::read_dir(from).map_err(io(from))? {
        let e = e.map_err(io(from))?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type().map_err(io(&src))?.is_dir() {
            link_tree(&src, &dst)?;
        } else if std::fs::hard_link(&src, &dst).is_err() {
            std::fs::copy(&src, &dst).map_err(io(&src))?;
            let t = e.metadata().and_then(|m| m.modified()).map_err(io(&src))?;
            std::fs::File::options()
                .write(true)
                .open(&dst)
                .and_then(|f| f.set_modified(t))
                .map_err(io(&dst))?;
        }
    }
    Ok(())
}

fn wim_err(what: &Path) -> impl Fn(wim::Error) -> Error + '_ {
    move |e| Error(format!("{}: {e}", what.display()))
}

/// Windows compares names without case.
fn same(a: &str, b: &str) -> bool {
    a.encode_utf16()
        .map(uppercase)
        .eq(b.encode_utf16().map(uppercase))
}

/// The names of one image as the DVD has it, to find what is there; the images are only ever
/// added to.
struct Tree(Vec<(String, bool, Vec<usize>)>);

impl Tree {
    fn read(wim: &Path, index: u32) -> Result<Tree> {
        let bad = |e: wim_format::ParseError| Error(format!("{}: {e:?}", wim.display()));
        let a = FileArchive::open(std::fs::File::open(wim).map_err(io(wim))?).map_err(bad)?;
        let bytes = a.read_metadata(index).map_err(bad)?;
        let m = Metadata::parse(&bytes).map_err(bad)?;
        let nodes = m
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let units: Vec<u16> = n
                    .entry
                    .name
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect();
                let dir = m.inode_entry(i).is_some_and(|e| e.attributes & 0x10 != 0);
                (String::from_utf16_lossy(&units), dir, n.children.clone())
            })
            .collect();
        Ok(Tree(nodes))
    }

    fn find(&self, path: &str) -> Option<usize> {
        let mut node = 0;
        for c in path.split('/').filter(|c| !c.is_empty()) {
            node = *self.0[node].2.iter().find(|&&i| same(&self.0[i].0, c))?;
        }
        Some(node)
    }

    fn is_file(&self, path: &str) -> bool {
        self.find(path).is_some_and(|n| !self.0[n].1)
    }

    /// The names of a directory's subdirectories.
    fn subdirs(&self, path: &str) -> Vec<String> {
        self.find(path)
            .map(|n| {
                self.0[n]
                    .2
                    .iter()
                    .filter(|&&i| self.0[i].1)
                    .map(|&i| self.0[i].0.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The path as the image spells it, as far as the image has it.
    fn spelled(&self, path: &str) -> String {
        let mut node = Some(0);
        let mut out = String::new();
        for c in path.split('/').filter(|c| !c.is_empty()) {
            node = node.and_then(|n| self.0[n].2.iter().copied().find(|&i| same(&self.0[i].0, c)));
            out.push('/');
            out.push_str(node.map_or(c, |n| &self.0[n].0));
        }
        out
    }
}

/// One image being changed. What it is given stays on disk in `scratch` until it is written.
struct Image {
    wim: Wim,
    index: ImageIndex,
    tree: Tree,
    scratch: PathBuf,
    files: usize,
    source: PathBuf,
}

impl Image {
    fn open(wim: &Path, index: u32, scratch: PathBuf) -> Result<Image> {
        let tree = Tree::read(wim, index)?;
        let index = ImageIndex::try_from(index).map_err(wim_err(wim))?;
        std::fs::create_dir_all(&scratch).map_err(io(&scratch))?;
        Ok(Image {
            wim: Wim::open(wim, OpenOptions::default()).map_err(wim_err(wim))?,
            index,
            tree,
            scratch,
            files: 0,
            source: wim.to_path_buf(),
        })
    }

    /// Adds a file, or replaces one keeping its security descriptor, attributes and DOS name.
    fn put(&mut self, from: &Path, to: &str) -> Result<()> {
        let to = self.tree.spelled(to);
        self.wim
            .add_file_with_options(
                self.index,
                from,
                &to,
                AddOptions {
                    keep_replaced_metadata: true,
                },
            )
            .map_err(|e| Error(format!("{}: {to}: {e}", self.source.display())))
    }

    /// Deletes a file of the image.
    fn delete(&mut self, path: &str) -> Result<()> {
        let path = self.tree.spelled(path);
        self.wim
            .delete_path(self.index, &path, false)
            .map_err(|e| Error(format!("{}: {path}: {e}", self.source.display())))
    }

    /// A file of the image, copied out to the scratch directory.
    fn get(&mut self, from: &str) -> Result<PathBuf> {
        self.files += 1;
        let dir = self.scratch.join(self.files.to_string());
        std::fs::create_dir_all(&dir).map_err(io(&dir))?;
        self.wim
            .extract_path_case_insensitive(self.index, from, &dir)
            .map_err(|e| Error(format!("{}: {from}: {e}", self.source.display())))
    }

    /// Applies .reg texts to one of the image's hives.
    fn edit_hive(
        &mut self,
        hive: &str,
        texts: &[String],
        log: &mut dyn FnMut(String),
    ) -> Result<()> {
        let at = format!("/Windows/System32/config/{hive}");
        let local = self.get(&at)?;
        import_regs_file(&local, texts, log)?;
        self.put(&local, &at)
    }
}

/// One driver package: an INF and the files beside it.
struct Package {
    dir: PathBuf,
    inf: String,
    files: Vec<String>,
}

/// Every INF below the folders, as a package.
fn packages(roots: &[PathBuf], out: &mut Vec<Package>) -> Result<()> {
    for root in roots {
        let mut infs = Vec::new();
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for e in std::fs::read_dir(root).map_err(io(root))? {
            let e = e.map_err(io(root))?;
            let name = e.file_name().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            if e.file_type().map_err(io(root))?.is_dir() {
                dirs.push(e.path());
            } else if lower.ends_with(".inf") {
                infs.push(name);
            } else if !lower.ends_with(".pnf") {
                files.push(name);
            }
        }
        for inf in infs {
            out.push(Package {
                dir: root.clone(),
                inf,
                files: files.clone(),
            });
        }
        packages(&dirs, out)?;
    }
    Ok(())
}

/// Whether Setup counts the package as boot-critical: a service that starts at boot, or a storage
/// controller class. Setup stops on such a package from $WinPEDriver$ when it cannot install it,
/// which unsigned it cannot.
fn boot_critical(p: &Package) -> Result<bool> {
    let path = p.dir.join(&p.inf);
    let b = std::fs::read(&path).map_err(io(&path))?;
    let text = match b.strip_prefix(&[0xff, 0xfe]) {
        Some(w) => String::from_utf16_lossy(
            &w.chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        ),
        None => String::from_utf8_lossy(&b).into_owned(),
    };
    Ok(text.lines().any(|l| {
        let l = l.split(';').next().unwrap_or("");
        let Some((k, v)) = l.split_once('=') else {
            return false;
        };
        let (k, v) = (k.trim(), v.trim());
        (k.eq_ignore_ascii_case("StartType")
            && (v == "0" || v.eq_ignore_ascii_case("%SERVICE_BOOT_START%")))
            || (k.eq_ignore_ascii_case("Class")
                && (v.eq_ignore_ascii_case("SCSIAdapter") || v.eq_ignore_ascii_case("HDC")))
    }))
}

/// `--replace-inbox`: where the image has a package of a driver package's name (its INF in
/// Windows\inf), that package becomes the given one: both copies of its INF (Windows\inf and the
/// driver store's folder), every file of the driver store's folder, and its files installed in
/// System32\drivers (the .sys files) and System32 (the others), as the Integration Services
/// packages lay them out, and the INF cache goes. Whichever INF then wins at device install, the
/// files are the package's. Not a standard interface: the image's own package is no longer what
/// its catalog signed, so it ranks as unsigned like the staged one.
fn replace_inbox(
    img: &mut Image,
    pkgs: &[Package],
    taken: &mut BTreeSet<String>,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    let mut any = false;
    for p in pkgs {
        let inf_at = format!("/Windows/inf/{}", p.inf);
        if !img.tree.is_file(&inf_at) {
            continue;
        }
        any = true;
        taken.insert(p.inf.to_lowercase());
        let inf = p.dir.join(&p.inf);
        img.put(&inf, &inf_at)?;
        let prefix = format!("{}_", p.inf.to_lowercase());
        let stores: Vec<String> = img
            .tree
            .subdirs(STORE)
            .into_iter()
            .filter(|d| d.to_lowercase().starts_with(&prefix))
            .collect();
        for s in &stores {
            img.put(&inf, &format!("{STORE}/{s}/{}", p.inf))?;
            for f in &p.files {
                img.put(&p.dir.join(f), &format!("{STORE}/{s}/{f}"))?;
            }
        }
        // Installed as the package would install them, whether or not the image had them: a boot
        // driver's imports have to be there before PnP installs anything (the IC 6.3 vmbus.sys and
        // storvsc.sys import vmbkmcl.sys, which Windows 7's own package does not have).
        let mut installed = 0;
        for f in &p.files {
            let d = if f.to_lowercase().ends_with(".sys") {
                "/Windows/System32/drivers"
            } else {
                "/Windows/System32"
            };
            img.put(&p.dir.join(f), &format!("{d}/{f}"))?;
            installed += 1;
        }
        log(format!(
            "{}: the image's own package replaced ({} driver store folder(s), {installed} installed file(s))",
            p.inf,
            stores.len()
        ));
    }
    // SetupAPI finds the INFs for a device through this cache of their hardware IDs, which still
    // has the replaced INFs' (a device only the new INF knows, such as the Guest Service Interface
    // by its instance ID, would get the null driver). Without it, SetupAPI builds it anew.
    const CACHE: &str = "/Windows/System32/DriverStore/INFCACHE.1";
    if any && img.tree.is_file(CACHE) {
        img.delete(CACHE)?;
        log("deleted Windows/System32/DriverStore/INFCACHE.1 (rebuilt on first use)".into());
    }
    Ok(())
}

/// Changes one image of `from` and writes the whole WIM to `to`.
fn patch_wim(
    c: &SetupDvd,
    from: &Path,
    to: &Path,
    index: u32,
    pkgs: &[Package],
    taken: &mut BTreeSet<String>,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    let tmp = scratch(c).join(format!(
        "{}-{index}",
        from.file_name().unwrap_or_default().to_string_lossy()
    ));
    let mut img = Image::open(from, index, tmp)?;
    if c.replace_inbox {
        replace_inbox(&mut img, pkgs, taken, log)?;
    }
    for (src, dst) in &c.copies {
        img.put(src, &format!("/{dst}"))?;
        log(format!("added {dst}"));
    }
    // .reg texts by hive, each hive edited once.
    let mut hives: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (hive_name, file) in &c.regs {
        let b = std::fs::read(file).map_err(io(file))?;
        let t = reg::decode(&b).map_err(|e| Error(format!("{}: {e}", file.display())))?;
        hives.entry(hive_name.to_uppercase()).or_default().push(t);
    }
    if c.poll_boot_partition_ms != 0 {
        let t = boot_wait_reg(c.poll_boot_partition_ms);
        hives.entry("SYSTEM".into()).or_default().push(t);
    }
    if c.deny_synthvid {
        hives
            .entry("SOFTWARE".into())
            .or_default()
            .push(deny_synthvid_reg());
    }
    if let Some(hvfb) = &c.hvfb {
        img.put(hvfb, "/Windows/System32/drivers/hvfb.sys")?;
        log("added Windows/System32/drivers/hvfb.sys".into());
        hives
            .entry("SYSTEM".into())
            .or_default()
            .push(hvfb_reg(&c.hvfb_mode));
    }
    for (hive_name, texts) in &hives {
        img.edit_hive(hive_name, texts, log)?;
    }
    // A hard link to the cached original until now: a new file, not a rewrite of the source.
    remove_if_exists(to)?;
    img.wim.write(to).map_err(wim_err(to))?;
    log(format!("wrote {} (image {index} changed)", to.display()));
    Ok(())
}

/// The boot wait, for the SYSTEM hive.
fn boot_wait_reg(poll_ms: u32) -> String {
    format!(
        "{REG_HEADER}[{REG_ROOT}\\ControlSet001\\Control\\PnP]\r\n\"PollBootPartitionTimeout\"=dword:{poll_ms:08x}\r\n"
    )
}

/// The device installation policy "Prevent installation of devices that match any of these device
/// IDs" for SynthVid's VMBus type, for the SOFTWARE hive (HKLM\SOFTWARE\Policies).
/// DenyDeviceIDsRetroactive stays off, so a device already installed keeps working.
fn deny_synthvid_reg() -> String {
    format!(
        "{REG_HEADER}[{REG_ROOT}\\Policies\\Microsoft\\Windows\\DeviceInstall\\Restrictions]\r\n\
         \"DenyDeviceIDs\"=dword:00000001\r\n\"DenyDeviceIDsRetroactive\"=dword:00000000\r\n\r\n\
         [{REG_ROOT}\\Policies\\Microsoft\\Windows\\DeviceInstall\\Restrictions\\DenyDeviceIDs]\r\n\
         \"1\"=\"VMBUS\\\\{{DA0A7802-E377-4aac-8E77-0558EB1073F8}}\"\r\n"
    )
}

/// hvfb's registry: a legacy VideoPort miniport in the Video group with framebuf.dll at
/// 1024x768x32, laid out as hvkit migrate does it for XP.
fn hvfb_reg(mode: &str) -> String {
    let id = "{449ECA2B-4408-4A8C-979B-72B866C035D8}";
    let (w, h, bpp) = parse_mode(mode);
    let dev = |t: &mut String| {
        t.push_str(&format!(
            "\"InstalledDisplayDrivers\"=hex(7):{}\r\n\"VgaCompatible\"=dword:00000000\r\n\
             \"Device Description\"=\"Linear frame buffer display (VBE / Hyper-V Gen2)\"\r\n",
            hex7("framebuf")
        ));
        for (n, v) in [
            ("BitsPerPel", bpp),
            ("XResolution", w),
            ("YResolution", h),
            ("VRefresh", 60u32),
            ("Flags", 0),
            ("XPanning", 0),
            ("YPanning", 0),
        ] {
            t.push_str(&format!("\"DefaultSettings.{n}\"=dword:{v:08x}\r\n"));
        }
    };
    let mut keys: Vec<String> = Vec::new();
    let mut t = format!(
        "{REG_HEADER}[{REG_ROOT}\\ControlSet001\\Services\\hvfb]\r\n\"Type\"=dword:00000001\r\n\
         \"Start\"=dword:00000001\r\n\"ErrorControl\"=dword:00000000\r\n\"Group\"=\"Video\"\r\n\
         \"ImagePath\"=hex(2):{}\r\n\"DisplayName\"=\"hvfb frame buffer display miniport\"\r\n",
        hex2("system32\\DRIVERS\\hvfb.sys")
    );
    dev(&mut t);
    keys.push(t);
    for k in [
        format!("{REG_ROOT}\\ControlSet001\\Services\\hvfb\\Device0"),
        format!("{REG_ROOT}\\ControlSet001\\Services\\hvfb\\Video"),
        format!("{REG_ROOT}\\ControlSet001\\Control\\Video\\{id}\\0000"),
        format!("{REG_ROOT}\\ControlSet001\\Control\\Video\\{id}\\Video"),
        format!(
            "{REG_ROOT}\\ControlSet001\\Hardware Profiles\\0001\\System\\CurrentControlSet\\Control\\VIDEO\\{id}\\0000"
        ),
    ] {
        let mut t = format!("[{k}]\r\n");
        if k.ends_with("\\Video") {
            if k.ends_with("hvfb\\Video") {
                t.push_str(&format!("\"VideoID\"=\"{id}\"\r\n\"Service\"=\"hvfb\"\r\n"));
            } else {
                t.push_str("\"Service\"=\"hvfb\"\r\n");
            }
        } else {
            dev(&mut t);
            if k.contains("Hardware Profiles") {
                t.push_str("\"Attach.ToDesktop\"=dword:00000001\r\n");
            }
        }
        keys.push(t);
    }
    keys.join("\r\n")
}

/// "WIDTHxHEIGHTxBPP" -> (w, h, bpp), falling back to Gen2's frame buffer mode.
fn parse_mode(mode: &str) -> (u32, u32, u32) {
    let n: Vec<u32> = mode
        .split('x')
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    if n.len() == 3 {
        (n[0], n[1], n[2])
    } else {
        (1024, 768, 32)
    }
}

fn hex7(s: &str) -> String {
    let mut b: Vec<u8> = Vec::new();
    for u in s.encode_utf16() {
        b.extend_from_slice(&u.to_le_bytes());
    }
    b.extend_from_slice(&[0, 0, 0, 0]);
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn hex2(s: &str) -> String {
    let mut b: Vec<u8> = Vec::new();
    for u in s.encode_utf16() {
        b.extend_from_slice(&u.to_le_bytes());
    }
    b.extend_from_slice(&[0, 0]);
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn import_regs_file(hive_path: &Path, texts: &[String], log: &mut dyn FnMut(String)) -> Result<()> {
    let mut h = Hive::open(hive_path, true).map_err(|e| Error(e.to_string()))?;
    h.set_os_version(6, 1);
    let mut total = reg::ImportStats::default();
    for t in texts {
        let s = reg::import(&mut h, t, None).map_err(|e| Error(e.to_string()))?;
        total.keys += s.keys;
        total.keys_deleted += s.keys_deleted;
        total.values_set += s.values_set;
        total.values_deleted += s.values_deleted;
    }
    log(format!(
        "{}: {} key(s) touched, {} deleted; {} value(s) set, {} deleted",
        hive_path.file_name().unwrap_or_default().to_string_lossy(),
        total.keys,
        total.keys_deleted,
        total.values_set,
        total.values_deleted
    ));
    h.commit(None).map_err(|e| Error(e.to_string()))?;
    Ok(())
}

/// The unattend file that takes a DVD from the language page to the desktop by itself.
fn autounattend(key: Option<&str>, computer: Option<&str>, arch: &str, index: u32) -> String {
    let key = key.map(|k| k.trim().to_string());
    let pid = match &key {
        Some(k) => format!(
            "      <UserData>\r\n        <AcceptEula>true</AcceptEula>\r\n        <FullName>user</FullName>\r\n        <Organization></Organization>\r\n        <ProductKey>\r\n          <Key>{}</Key>\r\n          <WillShowUI>OnError</WillShowUI>\r\n        </ProductKey>\r\n      </UserData>\r\n",
            xml(k)
        ),
        None => "      <UserData>\r\n        <AcceptEula>true</AcceptEula>\r\n        <FullName>user</FullName>\r\n        <Organization></Organization>\r\n      </UserData>\r\n".to_string(),
    };
    let name = computer
        .map(|n| format!("<ComputerName>{}</ComputerName>\r\n      ", xml(n)))
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<!-- Written by hvkit setup-dvd: Windows NT 6.x on Hyper-V Generation 2 through CSMWrap. Unattended
     install onto the VM's empty disk 0 (the only disk; the DVDs are not disks). One active NTFS
     partition, the built-in Administrator with an empty password and auto-logon, OOBE skipped. -->
<unattend xmlns="urn:schemas-microsoft-com:unattend">
  <settings pass="windowsPE">
    <component name="Microsoft-Windows-International-Core-WinPE" processorArchitecture="{arch}" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
      <SetupUILanguage>
        <UILanguage>en-US</UILanguage>
      </SetupUILanguage>
      <InputLocale>en-US</InputLocale>
      <SystemLocale>en-US</SystemLocale>
      <UILanguage>en-US</UILanguage>
      <UserLocale>en-US</UserLocale>
    </component>
    <component name="Microsoft-Windows-Setup" processorArchitecture="{arch}" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
      <DiskConfiguration>
        <WillShowUI>OnError</WillShowUI>
        <Disk wcm:action="add">
          <DiskID>0</DiskID>
          <WillWipeDisk>true</WillWipeDisk>
          <CreatePartitions>
            <CreatePartition wcm:action="add">
              <Order>1</Order>
              <Type>Primary</Type>
              <Extend>true</Extend>
            </CreatePartition>
          </CreatePartitions>
          <ModifyPartitions>
            <ModifyPartition wcm:action="add">
              <Order>1</Order>
              <PartitionID>1</PartitionID>
              <Active>true</Active>
              <Format>NTFS</Format>
              <Label>System</Label>
              <Letter>C</Letter>
            </ModifyPartition>
          </ModifyPartitions>
        </Disk>
      </DiskConfiguration>
      <ImageInstall>
        <OSImage>
          <InstallFrom>
            <MetaData wcm:action="add">
              <Key>/IMAGE/INDEX</Key>
              <Value>{index}</Value>
            </MetaData>
          </InstallFrom>
          <InstallTo>
            <DiskID>0</DiskID>
            <PartitionID>1</PartitionID>
          </InstallTo>
          <WillShowUI>OnError</WillShowUI>
        </OSImage>
      </ImageInstall>
{pid}    </component>
  </settings>
  <settings pass="specialize">
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="{arch}" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
      {name}<TimeZone>China Standard Time</TimeZone>
    </component>
  </settings>
  <settings pass="oobeSystem">
    <component name="Microsoft-Windows-International-Core" processorArchitecture="{arch}" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
      <InputLocale>en-US</InputLocale>
      <SystemLocale>en-US</SystemLocale>
      <UILanguage>en-US</UILanguage>
      <UserLocale>en-US</UserLocale>
    </component>
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="{arch}" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
      <OOBE>
        <HideEULAPage>true</HideEULAPage>
        <HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>
        <NetworkLocation>Work</NetworkLocation>
        <ProtectYourPC>3</ProtectYourPC>
        <SkipMachineOOBE>true</SkipMachineOOBE>
        <SkipUserOOBE>true</SkipUserOOBE>
      </OOBE>
      <UserAccounts>
        <AdministratorPassword>
          <Value></Value>
          <PlainText>true</PlainText>
        </AdministratorPassword>
      </UserAccounts>
      <AutoLogon>
        <Enabled>true</Enabled>
        <Username>Administrator</Username>
        <Password>
          <Value></Value>
          <PlainText>true</PlainText>
        </Password>
        <LogonCount>9999</LogonCount>
      </AutoLogon>
    </component>
  </settings>
</unattend>
"#,
        arch = arch,
        index = index,
        pid = pid,
        name = name,
    )
}

/// `hvkit patch`-style XML escaping for the few strings this file interpolates.
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn build(c: &SetupDvd, log: &mut dyn FnMut(String)) -> Result<()> {
    if c.unattend && c.product_key_file.is_none() {
        log("note: --unattend without --product-key-file; a DVD that asks for a key will stop there".into());
    }
    // Paths in the images as Windows has them: without case.
    wim::ffi::wimlib_global_init(0x20);
    extract_cached(&c.source, &c.cache, log)?;
    remove_dir_if_exists(&c.work)?;
    remove_dir_if_exists(&scratch(c))?;
    link_tree(&c.cache, &c.work)?;
    remove_if_exists(&c.work.join(".done"))?;
    // extract_cached's copy of the El Torito boot image; NT 6 keeps its own under \boot.
    remove_dir_if_exists(&c.work.join("[BOOT]"))?;
    let boot = if c.work.join("boot/etfsboot.com").is_file() {
        "/boot/etfsboot.com".to_string()
    } else if c.work.join("BOOT/ETFSBOOT.COM").is_file() {
        "/BOOT/ETFSBOOT.COM".to_string()
    } else {
        return Err(Error("no boot/etfsboot.com in the source DVD".into()));
    };
    let mut pkgs = Vec::new();
    packages(&c.drivers, &mut pkgs)?;
    let mut taken = BTreeSet::new();
    for (what, skip, index) in [
        (BOOT_WIM, c.no_boot_wim, c.boot_index),
        (INSTALL_WIM, c.no_install_wim, c.install_index),
    ] {
        if skip {
            continue;
        }
        let from = c.cache.join(what);
        if !from.is_file() {
            if what == BOOT_WIM {
                return Err(Error(format!("{what}: missing")));
            }
            log(format!("{what}: missing, skipped"));
            continue;
        }
        patch_wim(c, &from, &c.work.join(what), index, &pkgs, &mut taken, log)?;
    }
    remove_dir_if_exists(&scratch(c))?;
    let mut staged = BTreeSet::new();
    for p in &pkgs {
        if taken.contains(&p.inf.to_lowercase()) || !staged.insert(p.dir.clone()) {
            continue;
        }
        if boot_critical(p)? {
            log(format!(
                "{}: left out, boot-critical without a package of its name in the images (Setup does not install it unsigned)",
                p.inf
            ));
            continue;
        }
        let to = c.work.join("$WinPEDriver$").join(format!(
            "{}-{}",
            staged.len(),
            p.dir.file_name().unwrap_or_default().to_string_lossy()
        ));
        crate::copy_tree(&p.dir, &to)?;
        log(format!("{}: in $WinPEDriver$", p.inf));
    }
    if c.unattend {
        let xml = autounattend(
            c.product_key_file
                .as_deref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .as_deref(),
            c.computer_name.as_deref(),
            c.arch.as_deref().unwrap_or("x86"),
            c.install_index,
        );
        std::fs::write(c.work.join("autounattend.xml"), xml).map_err(io(&c.work))?;
        log("wrote autounattend.xml".into());
    }
    if let Some(hook) = &c.hook {
        let st = Command::new("bash")
            .arg(hook)
            .current_dir(&c.work)
            .status()
            .map_err(|e| Error(format!("{}: {e}", hook.display())))?;
        if !st.success() {
            return Err(Error(format!(
                "{}: exit {}",
                hook.display(),
                st.code().unwrap_or(-1)
            )));
        }
    }
    master(&c.work, &boot, &volume_id(&c.source), &c.out, log)
}

/// The source DVD's volume id, so the new one keeps it (ISO 9660 files are read only).
fn volume_id(source: &Path) -> String {
    formats::iso9660::Iso::open(source)
        .map(|i| i.volume_id)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "CCCOMA_X86FRE_EN-US_DV5".to_string())
}

/// Masters `root` as an NT 6.x setup DVD into `out` (rewritten in place, keeping its ACL).
fn master(
    root: &Path,
    boot: &str,
    volume_id: &str,
    out: &Path,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    let tmp = root.with_extension("iso");
    iso::build(root, &tmp, &iso::Options::nt6_setup(volume_id, boot))
        .map_err(|e| Error(format!("{}: {e}", tmp.display())))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every element the answer file opens is closed, in order (Setup ignores a malformed one
    /// and asks its questions).
    fn balanced(x: &str) -> bool {
        let mut open: Vec<&str> = Vec::new();
        for tag in x
            .split('<')
            .skip(1)
            .map(|t| t.split('>').next().unwrap_or(""))
        {
            if tag.starts_with('?') || tag.starts_with('!') || tag.ends_with('/') {
                continue;
            }
            match tag.strip_prefix('/') {
                Some(name) => {
                    if open.pop() != Some(name) {
                        return false;
                    }
                }
                None => open.push(tag.split_whitespace().next().unwrap_or("")),
            }
        }
        open.is_empty()
    }

    #[test]
    fn autounattend_is_well_formed() {
        for key in [None, Some("KEY")] {
            let x = autounattend(key, Some("PC"), "x86", 3);
            assert!(balanced(&x), "{x}");
            assert!(x.contains("<Value>3</Value>"));
        }
        assert!(!balanced("<a><b></a></b>"));
    }
}
