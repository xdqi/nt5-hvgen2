//! A disk that installs Windows 98 SE on Hyper-V Generation 2 by itself: it boots the CD's DOS through
//! CSMWrap and runs setup from C:\WIN98 with an MSBATCH.INF, and setup installs the Gen2 pieces along
//! with Windows. Replaces the CSMWrap testbed's w98 disk scripts (mkdos.sh, mkmini.sh and the shell
//! prototype of this).
//!
//! The disk: an MBR ([`MBR_CODE`]), a 64 MiB FAT16 EFI system partition (type EF) with CSMWrap as
//! \EFI\BOOT\BOOTX64.EFI, and an active FAT16 LBA partition (type 0E) over the rest with the boot
//! floppy's boot code and DOS (IO.SYS, MSDOS.SYS, COMMAND.COM, HIMEM.SYS), CONFIG.SYS, AUTOEXEC.BAT
//! (w9x/setup/autoexec.bat: run setup until Windows is installed) and C:\WIN98: the CD's WIN98
//! directory with
//!
//! - MINI.CAB/MINI1.CAB rebuilt with the patched KEYBOARD.DRV (setup's mini-Windows takes the
//!   scancode from the BDA, see recipes::win98_keyboard), laid out like the originals: setup only
//!   moves on to MINI1.CAB where a folder continues there;
//! - loose files, which setup prefers to the copies in the cabinets: SYSDETMG.DLL
//!   (recipes::win98_sysdetmg) and VPICD.VXD, VTD.VXD, VKD.VXD (recipes::vxd_portio), from which setup
//!   builds the installed VMM32.VXD, and the files patcher9x makes when it is given: it runs on the
//!   staged directory in its install-media mode and leaves patched VMM32.VXD, NDIS.VXD, ... there;
//! - GEN2LEG.VXD, VESAMINI.DRV and VESAMINI.VXD from `files`, GEN2DISP.INF and GEN2MON.INF
//!   (w9x/setup), which MSBATCH.INF's [Install] copies;
//! - MSBATCH.INF from w9x/setup/msbatch.inf, with the name, organisation and product key.

use crate::setup_cd::{extract_cached, fmt_err, io};
use crate::{Error, Result};
use disk::fat::{self, FormatOptions};
use disk::image::Image;
use fatfs::FileAttributes;
use formats::cab::{self, CabSet, NewFile};
use formats::mbr::{Mbr, Partition};
use recipes::vxd_portio::{self, VECTORS};
use recipes::{win98_keyboard, win98_sysdetmg};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MSBATCH: &str = include_str!("../../../../../w9x/setup/msbatch.inf");
const GEN2DISP: &str = include_str!("../../../../../w9x/setup/gen2disp.inf");
const GEN2MON: &str = include_str!("../../../../../w9x/setup/gen2mon.inf");
const AUTOEXEC: &str = include_str!("../../../../../w9x/setup/autoexec.bat");
const CONFIG: &str = "DEVICE=C:\\HIMEM.SYS /TESTMEM:OFF\nDOS=HIGH\nFILES=30\n";

/// w9x/setup/mbr.asm: boot the active partition, reading its first sector with INT 13h AH=42h.
pub const MBR_CODE: [u8; 174] = [
    0xfa, 0x31, 0xc0, 0x8e, 0xd0, 0xbc, 0x00, 0x7c, 0x8e, 0xd8, 0x8e, 0xc0, 0xfb, 0xfc, 0xbe, 0x00,
    0x7c, 0xbf, 0x00, 0x06, 0xb9, 0x00, 0x01, 0xf3, 0xa5, 0xea, 0x1e, 0x06, 0x00, 0x00, 0xbe, 0xbe,
    0x07, 0xb9, 0x04, 0x00, 0xf6, 0x04, 0x80, 0x75, 0x0a, 0x83, 0xc6, 0x10, 0xe2, 0xf6, 0xbe, 0x79,
    0x06, 0xeb, 0x23, 0x66, 0x8b, 0x44, 0x08, 0x66, 0xa3, 0x71, 0x06, 0x56, 0xbe, 0x69, 0x06, 0xb4,
    0x42, 0xcd, 0x13, 0x5e, 0x72, 0x0d, 0x81, 0x3e, 0xfe, 0x7d, 0x55, 0xaa, 0x75, 0x05, 0xea, 0x00,
    0x7c, 0x00, 0x00, 0xbe, 0x8f, 0x06, 0xac, 0x84, 0xc0, 0x74, 0x09, 0xb4, 0x0e, 0xbb, 0x07, 0x00,
    0xcd, 0x10, 0xeb, 0xf2, 0xcd, 0x18, 0xf4, 0xeb, 0xfb, 0x10, 0x00, 0x01, 0x00, 0x00, 0x7c, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4e, 0x6f, 0x20, 0x61, 0x63, 0x74, 0x69,
    0x76, 0x65, 0x20, 0x70, 0x61, 0x72, 0x74, 0x69, 0x74, 0x69, 0x6f, 0x6e, 0x0d, 0x0a, 0x00, 0x45,
    0x72, 0x72, 0x6f, 0x72, 0x20, 0x72, 0x65, 0x61, 0x64, 0x69, 0x6e, 0x67, 0x20, 0x74, 0x68, 0x65,
    0x20, 0x62, 0x6f, 0x6f, 0x74, 0x20, 0x73, 0x65, 0x63, 0x74, 0x6f, 0x72, 0x0d, 0x0a,
];

/// The files taken from `files`, for MSBATCH.INF's [Install].
pub const FILES: [&str; 3] = ["GEN2LEG.VXD", "VESAMINI.DRV", "VESAMINI.VXD"];

const SECTOR: u64 = 512;
const ESP_START: u64 = 2048;
const ESP_SECTORS: u64 = 131072;

pub struct W98Disk {
    /// The Windows 98 SE CD (ISO image).
    pub cd: PathBuf,
    /// The extracted CD, kept between runs.
    pub cache: PathBuf,
    /// The directory with [`FILES`] (`make w9x`: out/w9x).
    pub files: PathBuf,
    /// patcher9x (github.com/JHRobotics/patcher9x), for its fixes for current CPUs.
    pub patcher9x: Option<PathBuf>,
    /// The GEN2LEG shim's vectors (w9x/gen2leg/vectors.txt).
    pub vectors: [u8; VECTORS],
    pub product_key: String,
    pub owner: String,
    pub org: String,
    /// CSMWrap, and its csmwrap.ini.
    pub efi: PathBuf,
    pub ini: Option<PathBuf>,
    /// Size of the disk in bytes, and the VHDX block size.
    pub size: u64,
    pub block_size: u32,
    pub out: PathBuf,
}

fn dt() -> (u16, u16) {
    cab::dos_date_time(1999, 5, 5, 22, 22, 0)
}

fn crlf(s: &str) -> Vec<u8> {
    s.replace("\r\n", "\n").replace('\n', "\r\n").into_bytes()
}

/// The directory entry `name` in `dir`, compared without case.
fn find(dir: &Path, name: &str) -> Result<Option<PathBuf>> {
    for e in std::fs::read_dir(dir).map_err(io(dir))? {
        let e = e.map_err(io(dir))?;
        if e.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
            return Ok(Some(e.path()));
        }
    }
    Ok(None)
}

/// Puts a file into the staged directory, replacing one with the same name in any case.
fn place(dir: &Path, name: &str, data: &[u8]) -> Result<()> {
    if let Some(p) = find(dir, name)? {
        std::fs::remove_file(&p).map_err(io(&p))?;
    }
    let p = dir.join(name);
    std::fs::write(&p, data).map_err(io(&p))
}

/// Reads files from the cabinet sets of `dir` (each set once), by name without case.
fn from_cabinets(dir: &Path, names: &[&str]) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut cabs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(io(dir))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("cab")))
        .collect();
    cabs.sort();
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut out = BTreeMap::new();
    for c in cabs {
        if out.len() == names.len() {
            break;
        }
        if seen.contains(&c) {
            continue;
        }
        let set = CabSet::open(&c).map_err(fmt_err)?;
        seen.extend(set.cabinets().into_iter().map(Path::to_path_buf));
        for n in names {
            if !out.contains_key(*n) && set.files.iter().any(|f| f.name.eq_ignore_ascii_case(n)) {
                out.insert(n.to_string(), set.read(n).map_err(fmt_err)?);
            }
        }
    }
    if let Some(n) = names.iter().find(|n| !out.contains_key(**n)) {
        return Err(Error(format!(
            "{n}: in none of the cabinets of {}",
            dir.display()
        )));
    }
    Ok(out)
}

/// MINI.CAB/MINI1.CAB with the patched KEYBOARD.DRV: the files of folder 0 (whose last one continues
/// into MINI1.CAB) first, then the other folder's, as in the original set.
fn mini_set(win98: &Path, log: &mut dyn FnMut(String)) -> Result<(Vec<u8>, Vec<u8>)> {
    let mini = find(win98, "MINI.CAB")?.ok_or_else(|| Error("no WIN98\\MINI.CAB".into()))?;
    let set = CabSet::open(&mini).map_err(fmt_err)?;
    let first_folder = set.files.iter().map(|f| f.folder).min().unwrap_or(0);
    let (mut a, mut b) = (Vec::new(), Vec::new());
    let mut names: Vec<String> = Vec::new();
    let (date, time) = dt();
    for f in &set.files {
        if names.iter().any(|n| n.eq_ignore_ascii_case(&f.name)) {
            continue;
        }
        names.push(f.name.clone());
        let mut data = set.read(&f.name).map_err(fmt_err)?;
        if f.name.eq_ignore_ascii_case("KEYBOARD.DRV") {
            let o = win98_keyboard::apply(&data)
                .map_err(|e| Error(format!("MINI.CAB\\KEYBOARD.DRV: {e}")))?;
            log(format!("KEYBOARD.DRV: {}", o.log.join("; ")));
            data = o.bytes;
        }
        let nf = NewFile {
            name: f.name.clone(),
            data,
            date,
            time,
        };
        if f.folder == first_folder {
            a.push(nf)
        } else {
            b.push(nf)
        }
    }
    cab::create_span(
        &a,
        &b,
        0x6101,
        [("MINI.CAB", "Disk 1"), ("MINI1.CAB", "Disk 2")],
    )
    .map_err(fmt_err)
}

/// Fills in MSBATCH.INF's @NAME@, @ORG@ and @PRODUCTKEY@.
pub fn msbatch(owner: &str, org: &str, key: &str) -> Result<Vec<u8>> {
    for (what, v) in [("name", owner), ("organisation", org), ("product key", key)] {
        if v.contains(['"', '\r', '\n']) {
            return Err(Error(format!(
                "the {what} may not contain quotes or line breaks"
            )));
        }
    }
    Ok(crlf(
        &MSBATCH
            .replace("@NAME@", owner)
            .replace("@ORG@", org)
            .replace("@PRODUCTKEY@", key),
    ))
}

/// Runs patcher9x on the staged WIN98 directory: menu choice 4 patches the files it extracts from the
/// cabinets (VMM32.VXD directly) and leaves them there; then confirm. Logs the files it added.
fn patcher9x(exe: &Path, dir: &Path, log: &mut dyn FnMut(String)) -> Result<()> {
    use std::io::Write;
    let names = |d: &Path| -> Result<Vec<String>> {
        let mut v = Vec::new();
        for e in std::fs::read_dir(d).map_err(io(d))? {
            v.push(e.map_err(io(d))?.file_name().to_string_lossy().into_owned());
        }
        Ok(v)
    };
    let before = names(dir)?;
    let mut child = std::process::Command::new(exe)
        .arg("-no-backup")
        .arg(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(io(exe))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"4\ny\n")
        .map_err(io(exe))?;
    let out = child.wait_with_output().map_err(io(exe))?;
    let mut added: Vec<String> = names(dir)?
        .into_iter()
        .filter(|n| !before.contains(n))
        .collect();
    added.sort();
    if !out.status.success() || !added.iter().any(|n| n.eq_ignore_ascii_case("VMM32.VXD")) {
        return Err(Error(format!(
            "{} made no VMM32.VXD ({}):\n{}",
            exe.display(),
            out.status,
            String::from_utf8_lossy(&out.stdout)
        )));
    }
    log(format!("patcher9x: {}", added.join(" ")));
    Ok(())
}

/// Stages C:\WIN98 in `stage` and returns the DOS files from the boot floppy and its boot sector.
fn stage(o: &W98Disk, stage: &Path, log: &mut dyn FnMut(String)) -> Result<Vec<(String, Vec<u8>)>> {
    extract_cached(&o.cd, &o.cache, log)?;
    let cd_win98 =
        find(&o.cache, "WIN98")?.ok_or_else(|| Error("the CD has no WIN98 directory".into()))?;
    crate::copy_tree(&cd_win98, stage)?;
    if let Some(p) = &o.patcher9x {
        patcher9x(p, stage, log)?;
    }

    let (c1, c2) = mini_set(stage, log)?;
    for n in ["MINI.CAB", "MINI1.CAB"] {
        place(stage, n, if n == "MINI.CAB" { &c1 } else { &c2 })?;
    }

    let mut got = from_cabinets(stage, &["sysdetmg.dll", "vpicd.vxd", "vtd.vxd", "vkd.vxd"])?;
    let s = win98_sysdetmg::apply(&got["sysdetmg.dll"], &o.vectors)
        .map_err(|e| Error(format!("SYSDETMG.DLL: {e}")))?;
    log(format!("SYSDETMG.DLL: {}", s.log.join("; ")));
    place(stage, "SYSDETMG.DLL", &s.bytes)?;
    for n in ["vpicd.vxd", "vtd.vxd", "vkd.vxd"] {
        let b = got.remove(n).unwrap();
        let p = vxd_portio::patch(&b, None, &o.vectors).map_err(|e| Error(format!("{n}: {e}")))?;
        log(format!("{}: {}", n.to_uppercase(), p.log.join("; ")));
        place(stage, &n.to_uppercase(), &p.bytes)?;
    }

    for n in FILES {
        let p =
            find(&o.files, n)?.ok_or_else(|| Error(format!("{}: no {n}", o.files.display())))?;
        place(stage, n, &std::fs::read(&p).map_err(io(&p))?)?;
    }
    place(stage, "GEN2DISP.INF", &crlf(GEN2DISP))?;
    place(stage, "GEN2MON.INF", &crlf(GEN2MON))?;
    place(
        stage,
        "MSBATCH.INF",
        &msbatch(&o.owner, &o.org, &o.product_key)?,
    )?;

    // DOS from the El Torito boot floppy.
    let floppy = o.cache.join("[BOOT]").join("Boot-NoEmul.img");
    let mut img = Image::open(&floppy, false).map_err(|e| Error(e.to_string()))?;
    let len = img.size();
    let mut out = Vec::new();
    let mut bs = vec![0u8; 512];
    img.read_at(0, &mut bs).map_err(|e| Error(e.to_string()))?;
    out.push(("".into(), bs));
    let fs = fat::open(img.window(0, len)).map_err(|e| Error(format!("boot floppy: {e}")))?;
    for n in ["IO.SYS", "MSDOS.SYS", "COMMAND.COM", "HIMEM.SYS"] {
        out.push((
            n.into(),
            fat::read(&fs, n).map_err(|e| Error(format!("boot floppy: {e}")))?,
        ));
    }
    Ok(out)
}

pub fn build(o: &W98Disk, work: &Path, log: &mut dyn FnMut(String)) -> Result<()> {
    let s = work.join("WIN98");
    if s.exists() {
        std::fs::remove_dir_all(&s).map_err(io(&s))?;
    }
    let dos = stage(o, &s, log)?;

    let total = o.size / SECTOR;
    let c_start = ESP_START + ESP_SECTORS;
    if total < c_start || (total - c_start) * SECTOR < 512 << 20 {
        return Err(Error(
            "the disk is too small (C: needs at least 512 MiB)".into(),
        ));
    }
    if (total - c_start) * SECTOR > 2047 << 20 {
        return Err(Error(
            "the disk is too big for a FAT16 C: (at most 2 GiB)".into(),
        ));
    }
    let mut mbr = Mbr::new(0x5767_3938); // "89gW"
    mbr.boot_code[..MBR_CODE.len()].copy_from_slice(&MBR_CODE);
    mbr.partitions[0] = Some(Partition {
        active: false,
        kind: 0xef,
        start: ESP_START as u32,
        sectors: ESP_SECTORS as u32,
    });
    mbr.partitions[1] = Some(Partition {
        active: true,
        kind: 0x0e,
        start: c_start as u32,
        sectors: (total - c_start) as u32,
    });
    let err = |e: disk::Error| Error(e.to_string());
    let mut img = Image::create(&o.out, o.size, o.block_size).map_err(err)?;
    img.write_at(0, &mbr.to_bytes().map_err(fmt_err)?)
        .map_err(err)?;

    let (es, el) = (ESP_START * SECTOR, ESP_SECTORS * SECTOR);
    fat::format(
        img.window(es, el),
        &FormatOptions {
            fat: Some(16),
            label: Some("CSMWRAP".into()),
            hidden_sectors: ESP_START as u32,
            ..Default::default()
        },
    )
    .map_err(err)?;
    {
        let fs = fat::open(img.window(es, el)).map_err(err)?;
        fat::mkdir_p(&fs, "EFI/BOOT").map_err(err)?;
        fat::write(
            &fs,
            "EFI/BOOT/BOOTX64.EFI",
            &std::fs::read(&o.efi).map_err(io(&o.efi))?,
            None,
        )
        .map_err(err)?;
        if let Some(ini) = &o.ini {
            fat::write(
                &fs,
                "EFI/BOOT/csmwrap.ini",
                &std::fs::read(ini).map_err(io(ini))?,
                None,
            )
            .map_err(err)?;
        }
        fs.unmount().map_err(|e| Error(format!("ESP: {e}")))?;
    }

    let (cs, cl) = (c_start * SECTOR, (total - c_start) * SECTOR);
    fat::format(
        img.window(cs, cl),
        &FormatOptions {
            fat: Some(16),
            hidden_sectors: c_start as u32,
            drive_num: Some(0x80),
            ..Default::default()
        },
    )
    .map_err(err)?;
    fat::set_boot_code(&mut img.window(cs, cl), &dos[0].1).map_err(err)?;
    {
        let fs = fat::open(img.window(cs, cl)).map_err(err)?;
        // IO.SYS first: the boot sector loads its start from the first clusters it finds.
        for (n, b) in &dos[1..] {
            fat::write(&fs, n, b, None).map_err(err)?;
        }
        fat::write(&fs, "CONFIG.SYS", &crlf(CONFIG), None).map_err(err)?;
        fat::write(&fs, "AUTOEXEC.BAT", &crlf(AUTOEXEC), None).map_err(err)?;
        let n = fat::put(&fs, &s, "/").map_err(err)?;
        let hrs = FileAttributes::HIDDEN | FileAttributes::SYSTEM | FileAttributes::READ_ONLY;
        for f in ["IO.SYS", "MSDOS.SYS"] {
            fat::set_attributes(&fs, f, hrs, FileAttributes::empty()).map_err(err)?;
        }
        let st = fs.stats().map_err(|e| Error(format!("C: {e}")))?;
        log(format!(
            "C: {n} files in WIN98, {} MiB free",
            (st.free_clusters() as u64 * u64::from(st.cluster_size())) >> 20
        ));
        fs.unmount().map_err(|e| Error(format!("C: {e}")))?;
    }
    img.flush().map_err(err)?;
    std::fs::remove_dir_all(&s).map_err(io(&s))?;
    Ok(())
}
