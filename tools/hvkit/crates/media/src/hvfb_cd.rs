//! A Windows XP setup CD whose text-mode setup uses hvfb.sys as its display miniport instead of
//! vga.sys, and optionally installs hvfb as the new system's boot display driver. It needs no
//! Microsoft file besides the CD (unlike setup_cd), for machines with a GOP frame buffer and CSMWrap
//! where the disk is reachable through the BIOS, such as QEMU. Port of nt5-hvgen2's tools/xp-iso.sh,
//! with two differences: a UTF-16 HIVESYS.INF (stock CDs) is edited instead of rejected, and the CD
//! has no Rock Ridge, which can push SETUPLDR.BIN past the part of \I386 the CD boot sector reads.

use crate::setup_cd::{copy, fmt_err, io, master, read_text, remove_if_exists, stage, write_text};
use crate::{Error, Result};
use formats::inf::{Text, key};
use formats::iso9660::Iso;
use std::path::{Path, PathBuf};

pub struct HvfbCd {
    pub source: PathBuf,
    /// The extracted CD, kept between runs (see setup_cd::extract_cached).
    pub cache: PathBuf,
    pub hvfb: PathBuf,
    /// The frame buffer bootvid.dll to use instead of the CD's.
    pub bootvid: Option<PathBuf>,
    /// Also install hvfb into the new system (system32\drivers, its service in HIVESYS.INF).
    pub install: bool,
    /// The mode the installed system starts in (width, height, bits per pixel). None: setup records
    /// the mode it ran in (hvfb's mode 0, normally 640x480x32).
    pub default_mode: Option<(u32, u32, u32)>,
    /// Default: the source's.
    pub volume_id: Option<String>,
    pub product_key: Option<String>,
    pub work: PathBuf,
    pub out: PathBuf,
}

/// The body (start, end) of the first section `name`, up to the next header.
fn section(t: &Text, name: &str) -> Result<(usize, usize)> {
    let lower = name.to_lowercase();
    let start = t
        .lines
        .iter()
        .position(|l| formats::inf::header(l).is_some_and(|h| h.to_lowercase() == lower))
        .ok_or_else(|| Error(format!("section [{name}] not found")))?;
    let end = (start + 1..t.lines.len())
        .find(|&i| formats::inf::header(&t.lines[i]).is_some())
        .unwrap_or(t.lines.len());
    Ok((start + 1, end))
}

pub fn build(c: &HvfbCd, log: &mut dyn FnMut(String)) -> Result<()> {
    if c.product_key
        .as_deref()
        .is_some_and(|k| !is_product_key(k.trim()))
    {
        return Err(Error(
            "not a product key (expected five groups of five letters and digits)".into(),
        ));
    }
    let root = &c.work;
    stage(&c.source, &c.cache, root, log)?;
    let i386 = root.join("I386");
    copy(&c.hvfb, &i386.join("HVFB.SYS"))?;
    if let Some(b) = &c.bootvid {
        // SETUPLDR and setup's file copy read I386\bootvid.dll uncompressed when BOOTVID.DL_ is
        // absent; [SourceDisksFiles] needs no change.
        remove_if_exists(&i386.join("BOOTVID.DL_"))?;
        copy(b, &i386.join("BOOTVID.DLL"))?;
        log(format!("bootvid.dll: {}", b.display()));
    }

    let sif_path = i386.join("TXTSETUP.SIF");
    let mut sif = read_text(&sif_path)?;
    let fail = |e: Error| Error(format!("TXTSETUP.SIF: {e}"));
    // Text-mode setup loads the miniport named under the "vga" display id.
    let (s, e) = section(&sif, "Display.Load").map_err(fail)?;
    let vga: Vec<usize> = (s..e)
        .filter(|&i| key(&sif.lines[i]).as_deref() == Some("vga"))
        .collect();
    if vga.len() != 1 {
        return Err(Error(
            "TXTSETUP.SIF: [Display.Load] has no single vga entry".into(),
        ));
    }
    sif.lines[vga[0]] = "vga      = hvfb.sys".into();
    // Same source and target attributes as vga.sys: on the boot media, copied to system32\drivers
    // (directory 4) on every fresh install and upgrade.
    let (s, e) = section(&sif, "SourceDisksFiles").map_err(fail)?;
    let vga = (s..e)
        .find(|&i| key(&sif.lines[i]).as_deref() == Some("vga.sys"))
        .ok_or_else(|| Error("TXTSETUP.SIF: vga.sys missing from [SourceDisksFiles]".into()))?;
    if !(s..e).any(|i| key(&sif.lines[i]).as_deref() == Some("hvfb.sys")) {
        sif.lines
            .insert(vga + 1, "hvfb.sys = 100,,,,,,4_,4,0,0,,1,4".into());
    }
    if c.install && c.default_mode.is_none() {
        // Third field of a [Display] entry: the service whose Device0 key setupdd writes the
        // text-mode display settings to (DefaultSettings.*), so the installed system starts in the
        // mode text-mode setup used.
        let (s, e) = section(&sif, "Display").map_err(fail)?;
        for i in s..e {
            if key(&sif.lines[i]).as_deref() == Some("vga") {
                sif.lines[i] = r#"vga      = "Auto Detect",files.none,hvfb"#.into();
            }
        }
    }
    write_text(&sif_path, &sif)?;

    if c.install {
        let hive_path = i386.join("HIVESYS.INF");
        let mut hive = read_text(&hive_path)?;
        let (s, e) = section(&hive, "AddReg").map_err(|e| Error(format!("HIVESYS.INF: {e}")))?;
        let svc = r#"HKLM,"SYSTEM\CurrentControlSet\Services\hvfb"#;
        let mut add = vec![
            format!(r#"{svc}","ErrorControl",0x00010003,0"#),
            format!(r#"{svc}","Group",0x00000000,"Video""#),
            format!(r#"{svc}","ImagePath",0x00020000,"\SystemRoot\System32\drivers\hvfb.sys""#),
            format!(r#"{svc}","Start",0x00010001,1"#),
            format!(r#"{svc}","Type",0x00010001,1"#),
            format!(r#"{svc}\Device0","InstalledDisplayDrivers",0x00010000,"framebuf""#),
            format!(r#"{svc}\Device0","VgaCompatible",0x00010001,0"#),
        ];
        if let Some((w, h, bpp)) = c.default_mode {
            for (name, v) in [
                ("XResolution", w),
                ("YResolution", h),
                ("BitsPerPel", bpp),
                ("VRefresh", 60),
                ("Flags", 0),
                ("XPanning", 0),
                ("YPanning", 0),
            ] {
                add.push(format!(
                    r#"{svc}\Device0","DefaultSettings.{name}",0x00010001,{v}"#
                ));
            }
        }
        if !hive.lines.iter().any(|l| l.contains(r"Services\hvfb")) {
            // Next to the VgaSave service, the other legacy display miniport.
            let last = (s..e)
                .filter(|&i| hive.lines[i].contains(r"Services\VgaSave"))
                .max()
                .ok_or_else(|| Error("HIVESYS.INF: no VgaSave lines in [AddReg]".into()))?;
            hive.lines.splice(last + 1..last + 1, add);
        }
        write_text(&hive_path, &hive)?;
    }
    for (n, l) in sif
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("hvfb"))
    {
        log(format!("TXTSETUP.SIF:{}: {l}", n + 1));
    }

    if let Some(k) = &c.product_key {
        product_key(&i386.join("WINNT.SIF"), k.trim())?;
        log("WINNT.SIF: ProductKey set".into());
    }

    let volume_id = match &c.volume_id {
        Some(v) => v.clone(),
        None => Iso::open(&c.source).map_err(fmt_err)?.volume_id,
    };
    master(root, &volume_id, &c.out, log)
}

fn is_product_key(k: &str) -> bool {
    let groups: Vec<&str> = k.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .all(|g| g.len() == 5 && g.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Puts `ProductKey="key"` into WINNT.SIF's [UserData] (created if missing), dropping other
/// ProductKey lines; keeps the file's line ends.
fn product_key(path: &Path, k: &str) -> Result<()> {
    let bytes = if path.exists() {
        std::fs::read(path).map_err(io(path))?
    } else {
        Vec::new()
    };
    let text: String = bytes.iter().map(|&b| char::from(b)).collect();
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text
        .split(nl)
        .filter(|l| {
            !l.trim_start()
                .to_lowercase()
                .strip_prefix("productkey")
                .is_some_and(|r| r.trim_start().starts_with('='))
        })
        .map(String::from)
        .collect();
    let at = match lines
        .iter()
        .position(|l| l.trim().to_lowercase() == "[userdata]")
    {
        Some(i) => i,
        None => {
            lines.push("[UserData]".into());
            lines.len() - 1
        }
    };
    lines.insert(at + 1, format!("ProductKey=\"{k}\""));
    let out: Vec<u8> = lines
        .join(nl)
        .chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect();
    std::fs::write(path, out).map_err(io(path))
}
