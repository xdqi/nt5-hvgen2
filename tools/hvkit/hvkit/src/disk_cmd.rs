//! `hvkit disk` and `hvkit fat`: disk images (raw or VHDX), MBR partitions and FAT file systems.

use clap::Subcommand;
use disk::fat::{self, FormatOptions};
use disk::{Image, SECTOR};
use fatfs::FileAttributes;
use formats::mbr::{Mbr, Partition};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum DiskCommand {
    /// Create a disk image (VHDX for a .vhdx name, else sparse raw) with an MBR and partitions,
    /// formatting those with fat=
    Create {
        out: PathBuf,
        /// Size, e.g. 64M, 8G
        #[arg(long)]
        size: String,
        /// A partition: start=SECTOR|end-SIZE (default: 2048, or after the previous one),
        /// size=SIZE|rest (rest: up to the next partition with a start, or the end), type=HEX (e.g.
        /// ef, e, c, 7), active, fat=12|16|32, label=NAME, cluster=BYTES, bootcode=FILE (a FAT boot
        /// sector whose code to use, e.g. a DOS floppy's)
        #[arg(long = "part")]
        parts: Vec<String>,
        /// Copy into partition N's new FAT file system: N:SRC=/DEST, a file to the path DEST or a
        /// directory's contents into the directory DEST (missing directories are created)
        #[arg(long = "put")]
        puts: Vec<String>,
        /// MBR boot code (up to 440 bytes)
        #[arg(long)]
        boot_code: Option<PathBuf>,
        /// Disk signature (hex); default: random
        #[arg(long)]
        signature: Option<String>,
        /// VHDX block size
        #[arg(long, default_value = "8M")]
        block_size: String,
    },
    /// Print the partitions and their file systems
    Info { image: PathBuf },
    /// Copy an image to another format (by extension: .vhdx or raw); blocks of zeros are left out
    Convert {
        input: PathBuf,
        out: PathBuf,
        #[arg(long, default_value = "8M")]
        block_size: String,
    },
}

#[derive(Subcommand)]
pub enum FatCommand {
    /// List a directory (IMAGE:N is partition N; without :N the first partition, or the whole image
    /// if it has no MBR)
    Ls {
        image: String,
        #[arg(default_value = "/")]
        path: String,
        /// Also list subdirectories
        #[arg(short, long)]
        recursive: bool,
    },
    /// Copy a file out
    Get {
        image: String,
        path: String,
        dest: PathBuf,
    },
    /// Copy a host file to the path given (replacing a file there, whose attributes are kept;
    /// missing directories are created)
    Cp {
        image: String,
        src: PathBuf,
        path: String,
    },
    /// Copy host files and directory trees into a directory (created), keeping their names
    Put {
        image: String,
        #[arg(required = true)]
        srcs: Vec<PathBuf>,
        #[arg(last = true, default_value = "/")]
        dir: String,
    },
    /// Remove files and directory trees
    Rm {
        image: String,
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// Create directories with their parents
    Mkdir {
        image: String,
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// Change attributes: +r -r +h -h +s -s +a -a
    Attrib {
        image: String,
        path: String,
        #[arg(required = true, allow_hyphen_values = true)]
        changes: Vec<String>,
    },
    /// Use the boot code of another FAT boot sector (the first sector of FILE[:N]), keeping the BPB
    Bootcode { image: String, from: String },
}

/// Parses 64M, 8G, 512K, 1024 (bytes).
pub fn parse_size(s: &str) -> Result<u64, String> {
    let (num, mul) = match s.chars().last() {
        Some('K' | 'k') => (&s[..s.len() - 1], 1u64 << 10),
        Some('M' | 'm') => (&s[..s.len() - 1], 1 << 20),
        Some('G' | 'g') => (&s[..s.len() - 1], 1 << 30),
        _ => (s, 1),
    };
    num.parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(mul))
        .ok_or_else(|| format!("bad size {s:?}"))
}

/// Splits IMAGE:N.
fn image_part(s: &str) -> (PathBuf, Option<usize>) {
    match s.rsplit_once(':') {
        Some((p, n)) if !p.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty() => {
            (PathBuf::from(p), n.parse().ok())
        }
        _ => (PathBuf::from(s), None),
    }
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Opens IMAGE[:N] and runs `f` on its FAT file system.
fn with_fs<T>(
    spec: &str,
    writable: bool,
    f: impl FnOnce(&fat::Fs<'_>) -> Result<T, String>,
) -> Result<T, String> {
    let (path, n) = image_part(spec);
    let mut img = Image::open(&path, writable).map_err(err)?;
    let n = match n {
        Some(n) => n,
        None => disk::default_partition(&mut img).map_err(err)?,
    };
    let (start, len) = disk::partition(&mut img, n).map_err(err)?;
    let fs = fat::open(img.window(start, len)).map_err(|e| format!("{spec}: {e}"))?;
    let r = f(&fs)?;
    fs.unmount().map_err(|e| format!("{spec}: {e}"))?;
    img.flush().map_err(err)?;
    Ok(r)
}

/// The first 512 bytes of FILE, or of partition N of FILE[:N].
fn boot_sector(spec: &str) -> Result<Vec<u8>, String> {
    let (path, n) = image_part(spec);
    let mut img = Image::open(&path, false).map_err(err)?;
    let (start, _) = disk::partition(&mut img, n.unwrap_or(0)).map_err(err)?;
    let mut s = vec![0u8; 512];
    img.read_at(start, &mut s).map_err(err)?;
    Ok(s)
}

#[derive(Clone, Copy)]
enum Start {
    Sector(u64),
    /// Bytes before the end of the disk
    FromEnd(u64),
}

struct PartSpec {
    start: Option<Start>,
    size: Option<u64>,
    kind: u8,
    active: bool,
    format: Option<FormatOptions>,
    bootcode: Option<String>,
}

fn parse_part(s: &str) -> Result<PartSpec, String> {
    let mut p = PartSpec {
        start: None,
        size: None,
        kind: 0,
        active: false,
        format: None,
        bootcode: None,
    };
    let mut fo = FormatOptions::default();
    let mut fat = false;
    for item in s.split(',').filter(|i| !i.is_empty()) {
        let (k, v) = item.split_once('=').unwrap_or((item, ""));
        let bad = || format!("--part {s}: bad {item:?}");
        match k {
            "start" => {
                p.start = Some(match v.strip_prefix("end-") {
                    Some(b) => Start::FromEnd(parse_size(b)?),
                    None => Start::Sector(v.parse().map_err(|_| bad())?),
                })
            }
            "size" if v == "rest" => p.size = None,
            "size" => p.size = Some(parse_size(v)?),
            "type" => p.kind = u8::from_str_radix(v, 16).map_err(|_| bad())?,
            "active" => p.active = true,
            "fat" => {
                fat = true;
                fo.fat = Some(v.parse().map_err(|_| bad())?);
            }
            "label" => fo.label = Some(v.to_string()),
            "cluster" => fo.cluster_size = Some(parse_size(v)? as u32),
            "bootcode" => p.bootcode = Some(v.to_string()),
            _ => return Err(format!("--part {s}: unknown {k:?}")),
        }
    }
    if p.kind == 0 {
        return Err(format!("--part {s}: needs type=HEX"));
    }
    if fat {
        p.format = Some(fo);
    } else if fo.label.is_some() || fo.cluster_size.is_some() || p.bootcode.is_some() {
        return Err(format!("--part {s}: label, cluster and bootcode need fat="));
    }
    Ok(p)
}

fn signature(s: &Option<String>) -> Result<u32, String> {
    match s {
        Some(h) => u32::from_str_radix(h.trim_start_matches("0x"), 16)
            .map_err(|_| format!("bad signature {h}")),
        None => Ok(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.as_nanos() as u32).rotate_left(13) ^ std::process::id())
            .unwrap_or(0x5eed)),
    }
}

/// Parses N:SRC=/DEST for a disk with `parts` partitions.
fn parse_put(s: &str, parts: usize) -> Result<(usize, PathBuf, String), String> {
    let bad = || format!("--put {s}: expected N:SRC=/DEST");
    let (n, rest) = s.split_once(':').ok_or_else(bad)?;
    let (src, dest) = rest.rsplit_once('=').ok_or_else(bad)?;
    let n: usize = n.parse().map_err(|_| bad())?;
    if n == 0 || n > parts || src.is_empty() || !dest.starts_with(['/', '\\']) {
        return Err(bad());
    }
    Ok((n, PathBuf::from(src), dest.to_string()))
}

/// Copies a host file to the path `dest`, or a host directory's contents into the directory `dest`.
fn put_into(fs: &fat::Fs<'_>, src: &std::path::Path, dest: &str) -> Result<(), String> {
    if src.is_dir() {
        fat::mkdir_p(fs, dest).map_err(err)?;
        let mut entries: Vec<_> = std::fs::read_dir(src)
            .map_err(|e| format!("{}: {e}", src.display()))?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("{}: {e}", src.display()))?;
        entries.sort();
        for e in entries {
            fat::put(fs, &e, dest).map_err(err)?;
        }
        return Ok(());
    }
    let data = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let mtime = std::fs::metadata(src).and_then(|m| m.modified()).ok();
    if let Some((d, _)) = dest.replace('\\', "/").trim_matches('/').rsplit_once('/') {
        fat::mkdir_p(fs, d).map_err(err)?;
    }
    fat::write(fs, dest, &data, mtime).map_err(err)
}

pub fn run_disk(cmd: DiskCommand) -> Result<(), String> {
    match cmd {
        DiskCommand::Create {
            out,
            size,
            parts,
            puts,
            boot_code,
            signature: sig,
            block_size,
        } => {
            let size = parse_size(&size)?;
            let total = size / SECTOR;
            let mut mbr = Mbr::new(signature(&sig)?);
            if let Some(b) = boot_code {
                let code = std::fs::read(&b).map_err(|e| format!("{}: {e}", b.display()))?;
                mbr.boot_code = code[..code.len().min(440)].to_vec();
            }
            let specs: Vec<PartSpec> = parts
                .iter()
                .map(|p| parse_part(p))
                .collect::<Result<_, _>>()?;
            if specs.len() > 4 {
                return Err("at most 4 partitions".into());
            }
            let start_of = |p: &PartSpec| match p.start {
                Some(Start::Sector(s)) => Some(s),
                Some(Start::FromEnd(b)) => Some(total.saturating_sub(b / SECTOR)),
                None => None,
            };
            let puts = puts
                .iter()
                .map(|p| parse_put(p, specs.len()))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some((n, ..)) = puts.iter().find(|p| specs[p.0 - 1].format.is_none()) {
                return Err(format!("--put {n}:...: partition {n} has no fat="));
            }
            let mut next = 2048u64;
            for (i, p) in specs.iter().enumerate() {
                let start = start_of(p).unwrap_or(next);
                let end = specs[i + 1..].iter().find_map(start_of).unwrap_or(total);
                let sectors = p.size.map_or(end.saturating_sub(start), |s| s / SECTOR);
                if sectors == 0 || start + sectors > total {
                    return Err(format!("partition {} does not fit the disk", i + 1));
                }
                mbr.partitions[i] = Some(Partition {
                    active: p.active,
                    kind: p.kind,
                    start: start as u32,
                    sectors: sectors as u32,
                });
                next = start + sectors;
            }
            let mut img =
                Image::create(&out, size, parse_size(&block_size)? as u32).map_err(err)?;
            img.write_at(0, &mbr.to_bytes().map_err(err)?)
                .map_err(err)?;
            for (i, p) in specs.iter().enumerate() {
                let Some(fo) = &p.format else { continue };
                let part = mbr.partitions[i].unwrap();
                let (start, len) = (
                    u64::from(part.start) * SECTOR,
                    u64::from(part.sectors) * SECTOR,
                );
                let o = FormatOptions {
                    hidden_sectors: part.start,
                    ..fo.clone()
                };
                fat::format(img.window(start, len), &o)
                    .map_err(|e| format!("partition {}: {e}", i + 1))?;
                if let Some(b) = &p.bootcode {
                    let code = boot_sector(b)?;
                    fat::set_boot_code(&mut img.window(start, len), &code)
                        .map_err(|e| format!("partition {}: {e}", i + 1))?;
                }
                let mine: Vec<_> = puts.iter().filter(|p| p.0 == i + 1).collect();
                if !mine.is_empty() {
                    let fs = fat::open(img.window(start, len)).map_err(err)?;
                    for (_, src, dest) in mine {
                        put_into(&fs, src, dest)
                            .map_err(|e| format!("partition {}: {e}", i + 1))?;
                    }
                    fs.unmount()
                        .map_err(|e| format!("partition {}: {e}", i + 1))?;
                }
            }

            img.flush().map_err(err)?;
            println!("wrote {} ({} bytes virtual)", out.display(), size);
            Ok(())
        }
        DiskCommand::Info { image } => {
            let mut img = Image::open(&image, false).map_err(err)?;
            println!("{}: {} bytes", image.display(), img.size());
            let mut first = [0u8; 512];
            img.read_at(0, &mut first).map_err(err)?;
            if disk::is_fat_boot_sector(&first) {
                let len = img.size();
                let fs = fat::open(img.window(0, len)).map_err(err)?;
                println!(
                    "  no MBR: {:?} \"{}\" on the whole image",
                    fs.fat_type(),
                    fs.volume_label().trim()
                );
                return Ok(());
            }
            let m = match disk::mbr(&mut img) {
                Ok(m) => m,
                Err(e) => {
                    println!("  {e}");
                    return Ok(());
                }
            };
            println!(
                "  MBR signature {:08x}, boot code {}",
                m.signature,
                if m.boot_code.iter().all(|&b| b == 0) {
                    "none"
                } else {
                    "present"
                }
            );
            for (i, p) in m.partitions.iter().enumerate() {
                let Some(p) = p else { continue };
                let (start, len) = (u64::from(p.start) * SECTOR, u64::from(p.sectors) * SECTOR);
                let fsinfo = match fat::open(img.window(start, len)) {
                    Ok(fs) => {
                        let s = fs.stats().map_err(|e| e.to_string())?;
                        let r = format!(
                            "{:?} \"{}\", {} of {} clusters of {} bytes free",
                            fs.fat_type(),
                            fs.volume_label().trim(),
                            s.free_clusters(),
                            s.total_clusters(),
                            s.cluster_size()
                        );
                        drop(fs);
                        r
                    }
                    Err(_) => "no FAT file system".into(),
                };
                println!(
                    "  {} type {:02x}{} start {} sectors {} ({} MiB): {fsinfo}",
                    i + 1,
                    p.kind,
                    if p.active { " active" } else { "" },
                    p.start,
                    p.sectors,
                    u64::from(p.sectors) / 2048
                );
            }
            Ok(())
        }
        DiskCommand::Convert {
            input,
            out,
            block_size,
        } => {
            let mut src = Image::open(&input, false).map_err(err)?;
            let mut dst =
                Image::create(&out, src.size(), parse_size(&block_size)? as u32).map_err(err)?;
            let chunk = 4u64 << 20;
            let mut buf = vec![0u8; chunk as usize];
            let mut off = 0;
            while off < src.size() {
                let n = chunk.min(src.size() - off) as usize;
                src.read_at(off, &mut buf[..n]).map_err(err)?;
                if buf[..n].iter().any(|&b| b != 0) {
                    dst.write_at(off, &buf[..n]).map_err(err)?;
                }
                off += n as u64;
            }
            dst.flush().map_err(err)?;
            println!("wrote {}", out.display());
            Ok(())
        }
    }
}

fn attr_name(a: FileAttributes) -> String {
    [
        (FileAttributes::READ_ONLY, 'R'),
        (FileAttributes::HIDDEN, 'H'),
        (FileAttributes::SYSTEM, 'S'),
        (FileAttributes::ARCHIVE, 'A'),
        (FileAttributes::DIRECTORY, 'D'),
    ]
    .iter()
    .map(|&(f, c)| if a.contains(f) { c } else { '-' })
    .collect()
}

fn ls(fs: &fat::Fs<'_>, path: &str, recursive: bool) -> Result<(), String> {
    let mut entries = fat::list(fs, path).map_err(err)?;
    entries.sort_by_key(|e| e.name.to_lowercase());
    let base = path.trim_matches('/');
    for e in &entries {
        let full = if base.is_empty() {
            e.name.clone()
        } else {
            format!("{base}/{}", e.name)
        };
        let m = e.modified;
        println!(
            "{} {:>10} {:04}-{:02}-{:02} {:02}:{:02}:{:02}  {}{}",
            attr_name(e.attributes),
            if e.dir {
                String::new()
            } else {
                e.size.to_string()
            },
            m.date.year,
            m.date.month,
            m.date.day,
            m.time.hour,
            m.time.min,
            m.time.sec,
            full,
            if e.short_name != e.name {
                format!("  ({})", e.short_name)
            } else {
                String::new()
            }
        );
    }
    if recursive {
        for e in entries.iter().filter(|e| e.dir) {
            ls(fs, &format!("{base}/{}", e.name), true)?;
        }
    }
    Ok(())
}

pub fn run_fat(cmd: FatCommand) -> Result<(), String> {
    match cmd {
        FatCommand::Ls {
            image,
            path,
            recursive,
        } => with_fs(&image, false, |fs| ls(fs, &path, recursive)),
        FatCommand::Get { image, path, dest } => {
            let data = with_fs(&image, false, |fs| fat::read(fs, &path).map_err(err))?;
            std::fs::write(&dest, data).map_err(|e| format!("{}: {e}", dest.display()))
        }
        FatCommand::Cp { image, src, path } => {
            let data = std::fs::read(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            let mtime = std::fs::metadata(&src).and_then(|m| m.modified()).ok();
            with_fs(&image, true, |fs| {
                if let Some((d, _)) = path.replace('\\', "/").trim_matches('/').rsplit_once('/') {
                    fat::mkdir_p(fs, d).map_err(err)?;
                }
                fat::write(fs, &path, &data, mtime).map_err(err)
            })
        }
        FatCommand::Put { image, srcs, dir } => with_fs(&image, true, |fs| {
            fat::mkdir_p(fs, &dir).map_err(err)?;
            let mut n = 0;
            for s in &srcs {
                n += fat::put(fs, s, &dir).map_err(err)?;
            }
            println!("{n} file(s) copied");
            Ok(())
        }),
        FatCommand::Rm { image, paths } => with_fs(&image, true, |fs| {
            paths
                .iter()
                .try_for_each(|p| fat::remove(fs, p).map_err(err))
        }),
        FatCommand::Mkdir { image, paths } => with_fs(&image, true, |fs| {
            paths
                .iter()
                .try_for_each(|p| fat::mkdir_p(fs, p).map_err(err))
        }),
        FatCommand::Attrib {
            image,
            path,
            changes,
        } => {
            let (mut set, mut clear) = (FileAttributes::empty(), FileAttributes::empty());
            for c in &changes {
                for (i, ch) in c.char_indices().skip(1) {
                    let f = match ch.to_ascii_lowercase() {
                        'r' => FileAttributes::READ_ONLY,
                        'h' => FileAttributes::HIDDEN,
                        's' => FileAttributes::SYSTEM,
                        'a' => FileAttributes::ARCHIVE,
                        _ => return Err(format!("bad attribute change {c:?} at {i}")),
                    };
                    match c.as_bytes()[0] {
                        b'+' => set |= f,
                        b'-' => clear |= f,
                        _ => return Err(format!("attribute changes start with + or -: {c:?}")),
                    }
                }
            }
            let a = with_fs(&image, true, |fs| {
                fat::set_attributes(fs, &path, set, clear).map_err(err)
            })?;
            println!("{} {path}", attr_name(a));
            Ok(())
        }
        FatCommand::Bootcode { image, from } => {
            let code = boot_sector(&from)?;
            let (path, n) = image_part(&image);
            let mut img = Image::open(&path, true).map_err(err)?;
            let n = match n {
                Some(n) => n,
                None => disk::default_partition(&mut img).map_err(err)?,
            };
            let (start, len) = disk::partition(&mut img, n).map_err(err)?;
            fat::set_boot_code(&mut img.window(start, len), &code).map_err(err)?;
            img.flush().map_err(err)
        }
    }
}
