//! `hvkit cab`: Microsoft cabinets and cabinet sets.

use clap::Subcommand;
use formats::cab::{self, CabSet, NewFile};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum CabCommand {
    /// List the files of a cabinet and of the rest of its set (cabinets linked to it next to it)
    Ls { cab: PathBuf },
    /// Extract all files, or the named ones (compared without case), of a cabinet and its set
    Extract {
        cab: PathBuf,
        dir: PathBuf,
        names: Vec<String>,
    },
    /// Write an MSZIP cabinet with one folder of the files given (names upper-cased, as stored)
    Create {
        out: PathBuf,
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Set id
        #[arg(long, default_value = "0x9898")]
        set: String,
        /// Date (YYYY-MM-DD) for every file; default: each file's modification time
        #[arg(long)]
        date: Option<String>,
        /// Time (HH:MM:SS) with --date
        #[arg(long, default_value = "00:00:00")]
        time: String,
    },
    /// Write a set of two MSZIP cabinets laid out like Windows 98 setup's MINI.CAB and MINI1.CAB:
    /// the first --first files in a folder that continues into the second cabinet inside the last of
    /// them, the other files in a second folder of the second cabinet
    CreateSet {
        out1: PathBuf,
        out2: PathBuf,
        /// How many of the files go into the first cabinet
        #[arg(long)]
        first: usize,
        /// The disk names recorded in the links
        #[arg(long, num_args = 2, default_values = ["Disk 1", "Disk 2"])]
        disks: Vec<String>,
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long, default_value = "0x9898")]
        set: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long, default_value = "00:00:00")]
        time: String,
    },
}

fn parse_int(s: &str) -> Result<u16, String> {
    let r = match s.strip_prefix("0x") {
        Some(h) => u16::from_str_radix(h, 16),
        None => s.parse(),
    };
    r.map_err(|_| format!("bad number {s:?}"))
}

/// DOS date/time from --date/--time, or from the file's modification time (local time).
fn when(path: &Path, date: &Option<String>, time: &str) -> Result<(u16, u16), String> {
    use chrono::{Datelike, Timelike};
    let t = match date {
        Some(d) => {
            chrono::NaiveDateTime::parse_from_str(&format!("{d} {time}"), "%Y-%m-%d %H:%M:%S")
                .map_err(|e| format!("--date/--time: {e}"))?
        }
        None => {
            let m = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .map_err(|e| format!("{}: {e}", path.display()))?;
            chrono::DateTime::<chrono::Local>::from(m).naive_local()
        }
    };
    Ok(cab::dos_date_time(
        t.year(),
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
    ))
}

fn new_files(paths: &[PathBuf], date: &Option<String>, time: &str) -> Result<Vec<NewFile>, String> {
    paths
        .iter()
        .map(|p| {
            let (d, t) = when(p, date, time)?;
            Ok(NewFile {
                name: p
                    .file_name()
                    .ok_or_else(|| format!("{}: no file name", p.display()))?
                    .to_string_lossy()
                    .to_uppercase(),
                data: std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?,
                date: d,
                time: t,
            })
        })
        .collect()
}

fn write(p: &Path, data: &[u8]) -> Result<(), String> {
    std::fs::write(p, data).map_err(|e| format!("{}: {e}", p.display()))
}

pub fn run(cmd: CabCommand) -> Result<(), String> {
    match cmd {
        CabCommand::Ls { cab } => {
            let set = CabSet::open(&cab).map_err(|e| e.to_string())?;
            for c in set.cabinets() {
                println!("# {}", c.display());
            }
            for f in &set.files {
                let (d, t) = (f.date, f.time);
                println!(
                    "{:>10} {:04}-{:02}-{:02} {:02}:{:02}:{:02} folder {:<3} {}",
                    f.size,
                    1980 + (d >> 9),
                    (d >> 5) & 15,
                    d & 31,
                    t >> 11,
                    (t >> 5) & 63,
                    (t & 31) * 2,
                    f.folder,
                    f.name
                );
            }
            Ok(())
        }
        CabCommand::Extract { cab, dir, names } => {
            let set = CabSet::open(&cab).map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let want =
                |n: &str| names.is_empty() || names.iter().any(|w| w.eq_ignore_ascii_case(n));
            let mut missing: Vec<&String> = names
                .iter()
                .filter(|w| !set.files.iter().any(|f| f.name.eq_ignore_ascii_case(w)))
                .collect();
            let mut done = 0;
            let mut folders: Vec<usize> = set
                .files
                .iter()
                .filter(|f| want(&f.name))
                .map(|f| f.folder)
                .collect();
            folders.dedup();
            for folder in folders {
                let data = set
                    .folder_data(folder)
                    .map_err(|e| format!("{}: {e}", cab.display()))?;
                for f in set
                    .files
                    .iter()
                    .filter(|f| f.folder == folder && want(&f.name))
                {
                    let bytes = data
                        .get(f.offset as usize..(f.offset + f.size) as usize)
                        .ok_or_else(|| format!("{}: past the end of its folder", f.name))?;
                    // Names may hold backslashes (directories).
                    let p = dir.join(f.name.replace('\\', "/"));
                    if let Some(parent) = p.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| format!("{}: {e}", parent.display()))?;
                    }
                    write(&p, bytes)?;
                    done += 1;
                }
            }
            println!("{done} file(s) extracted to {}", dir.display());
            missing.dedup();
            if !missing.is_empty() {
                return Err(format!(
                    "not in the cabinet set: {}",
                    missing
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Ok(())
        }
        CabCommand::Create {
            out,
            files,
            set,
            date,
            time,
        } => {
            let files = new_files(&files, &date, &time)?;
            write(
                &out,
                &cab::create(&files, parse_int(&set)?).map_err(|e| e.to_string())?,
            )?;
            println!("{}: {} files", out.display(), files.len());
            Ok(())
        }
        CabCommand::CreateSet {
            out1,
            out2,
            first,
            disks,
            files,
            set,
            date,
            time,
        } => {
            if first == 0 || first > files.len() {
                return Err(format!("--first {first}: between 1 and {}", files.len()));
            }
            let files = new_files(&files, &date, &time)?;
            let name = |p: &Path| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            let (n1, n2) = (name(&out1), name(&out2));
            let (c1, c2) = cab::create_span(
                &files[..first],
                &files[first..],
                parse_int(&set)?,
                [(&n1, &disks[0]), (&n2, &disks[1])],
            )
            .map_err(|e| e.to_string())?;
            write(&out1, &c1)?;
            write(&out2, &c2)?;
            println!(
                "{}: {} files; {}: {} files",
                out1.display(),
                first,
                out2.display(),
                files.len() - first + 1
            );
            Ok(())
        }
    }
}
