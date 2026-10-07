//! `hvkit hive`: offline registry hives (export, import, show, info).

use clap::Subcommand;
use disk::ImageFile;
use hive::{Header, Hive, reg};
use std::path::{Path, PathBuf};

const DEFAULT_ROOT: &str = "HKEY_LOCAL_MACHINE\\HIVE";

#[derive(Subcommand)]
pub enum HiveCommand {
    /// Export a hive (or one key of it) as a .reg file, in the format of `reg.exe export`
    Export {
        /// The hive: a file, or IMAGE:N:PATH / IMAGE::PATH in a FAT volume of a disk image
        hive: PathBuf,
        /// Where to write it (UTF-16 with CR LF, as reg.exe; default: standard output, UTF-8)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// The path the hive's root key gets in the file
        #[arg(long, default_value = DEFAULT_ROOT)]
        root: String,
        /// Export only this key (path below the root)
        #[arg(long)]
        key: Option<String>,
        /// Write UTF-8 without carriage returns to the file, for diff and grep (what hive-dump.sh
        /// made of reg.exe's output with iconv and tr -d '\r')
        #[arg(long)]
        utf8: bool,
    },
    /// Apply a .reg file to a hive, as `reg.exe import` would with the hive loaded at ROOT
    Import {
        /// The hive: a file, or IMAGE:N:PATH / IMAGE::PATH in a FAT volume of a disk image (changed
        /// in place)
        hive: PathBuf,
        reg: PathBuf,
        /// The path in the .reg file that stands for the hive's root key (default: the first two
        /// components of its first key, e.g. HKEY_LOCAL_MACHINE\XPMIG)
        #[arg(long)]
        root: Option<String>,
        /// Write the result here instead of changing HIVE
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Change a hive whose log has not been written back (sequence numbers differ)
        #[arg(long)]
        force: bool,
    },
    /// Print the keys whose path (below the root) matches one of the regular expressions, with
    /// their values decoded
    Show {
        /// The hive: a file, or IMAGE:N:PATH / IMAGE::PATH
        hive: PathBuf,
        #[arg(required = true)]
        patterns: Vec<String>,
    },
    /// Print the hive's header: sequence numbers, version, whether it is clean
    Info {
        /// The hive: a file, or IMAGE:N:PATH / IMAGE::PATH
        hive: PathBuf,
    },
}

/// A hive given as a file, or inside a FAT volume of a disk image. The latter is copied to a
/// temporary file, which `write_back` copies back and which is removed when this is dropped.
struct HiveFile {
    local: PathBuf,
    inside: Option<ImageFile>,
}

impl HiveFile {
    fn open(spec: &Path) -> Result<HiveFile, String> {
        let Some(f) = spec.to_str().and_then(ImageFile::parse) else {
            return Ok(HiveFile {
                local: spec.to_path_buf(),
                inside: None,
            });
        };
        let data = f.read().map_err(|err| format!("{f}: {err}"))?;
        let name = f.path.rsplit(['\\', '/']).next().unwrap_or("hive");
        let local = std::env::temp_dir().join(format!("hvkit-{}-{name}", std::process::id()));
        std::fs::write(&local, data).map_err(|err| format!("{}: {err}", local.display()))?;
        Ok(HiveFile {
            local,
            inside: Some(f),
        })
    }

    fn write_back(&self) -> Result<(), String> {
        let Some(f) = &self.inside else { return Ok(()) };
        let data =
            std::fs::read(&self.local).map_err(|err| format!("{}: {err}", self.local.display()))?;
        f.write(&data).map_err(|err| format!("{f}: {err}"))
    }
}

impl Drop for HiveFile {
    fn drop(&mut self) {
        if self.inside.is_some() {
            let _ = std::fs::remove_file(&self.local);
        }
    }
}

/// Prefixes an error with the file it is about.
fn e(p: &Path) -> impl Fn(hive::Error) -> String + '_ {
    move |err| format!("{}: {err}", p.display())
}

pub fn run(cmd: HiveCommand) -> Result<(), String> {
    match cmd {
        HiveCommand::Export {
            hive,
            output,
            root,
            key,
            utf8,
        } => {
            let file = HiveFile::open(&hive)?;
            let h = Hive::open(&file.local, false).map_err(e(&hive))?;
            let (node, name) = match key.as_deref().map(|k| k.trim_matches('\\')) {
                None | Some("") => (h.root(), root.clone()),
                Some(k) => {
                    let n = h
                        .find(h.root(), k)
                        .map_err(e(&hive))?
                        .ok_or_else(|| format!("{}: no key {k}", hive.display()))?;
                    (n, format!("{root}\\{k}"))
                }
            };
            let text = reg::export(&h, node, &name).map_err(e(&hive))?;
            match output {
                Some(out) => {
                    let bytes = if utf8 {
                        text.replace('\r', "").into_bytes()
                    } else {
                        reg::to_utf16(&text)
                    };
                    std::fs::write(&out, bytes)
                        .map_err(|err| format!("{}: {err}", out.display()))?;
                }
                None => print!("{}", text.replace('\r', "")),
            }
            Ok(())
        }
        HiveCommand::Import {
            hive,
            reg: reg_file,
            root,
            output,
            force,
        } => {
            let file = HiveFile::open(&hive)?;
            let header = Header::read(
                &std::fs::read(&file.local).map_err(|err| format!("{}: {err}", hive.display()))?,
            )
            .map_err(e(&hive))?;
            if header.dirty() && !force {
                return Err(format!(
                    "{}: the hive is dirty (sequence {} / {}): its log was not written back; refusing without --force",
                    hive.display(),
                    header.sequence.0,
                    header.sequence.1
                ));
            }
            let bytes =
                std::fs::read(&reg_file).map_err(|err| format!("{}: {err}", reg_file.display()))?;
            let text = reg::decode(&bytes).map_err(e(&reg_file))?;
            let mut h = Hive::open(&file.local, true).map_err(e(&hive))?;
            let s = reg::import(&mut h, &text, root.as_deref()).map_err(e(&reg_file))?;
            h.commit(output.as_deref()).map_err(e(&hive))?;
            drop(h);
            if output.is_none() {
                file.write_back()?;
            }
            println!(
                "{}: {} key(s) touched, {} deleted; {} value(s) set, {} deleted",
                output.as_ref().unwrap_or(&hive).display(),
                s.keys,
                s.keys_deleted,
                s.values_set,
                s.values_deleted
            );
            Ok(())
        }
        HiveCommand::Show { hive, patterns } => {
            let res: Vec<regex::Regex> = patterns
                .iter()
                .map(|p| {
                    regex::RegexBuilder::new(p)
                        .case_insensitive(true)
                        .build()
                        .map_err(|err| format!("{p}: {err}"))
                })
                .collect::<Result<_, _>>()?;
            let file = HiveFile::open(&hive)?;
            let h = Hive::open(&file.local, false).map_err(e(&hive))?;
            show(&h, h.root(), "", &res).map_err(e(&hive))
        }
        HiveCommand::Info { hive } => {
            let file = HiveFile::open(&hive)?;
            let bytes =
                std::fs::read(&file.local).map_err(|err| format!("{}: {err}", hive.display()))?;
            let hd = Header::read(&bytes).map_err(e(&hive))?;
            println!(
                "{}: regf version {}.{}, sequence {} / {}{}, hbins {} bytes, file {} bytes",
                hive.display(),
                hd.version.0,
                hd.version.1,
                hd.sequence.0,
                hd.sequence.1,
                if hd.dirty() {
                    " (DIRTY: log not written back)"
                } else {
                    ""
                },
                hd.hbins,
                bytes.len()
            );
            Ok(())
        }
    }
}

fn show(h: &Hive, node: hive::Node, path: &str, res: &[regex::Regex]) -> hive::Result<()> {
    if res.iter().any(|r| r.is_match(path)) {
        println!("[{path}]");
        for v in h.values(node)? {
            println!("    {}", reg::describe(&v));
        }
    }
    for c in h.children(node)? {
        let name = h.name(c)?;
        let sub = if path.is_empty() {
            name
        } else {
            format!("{path}\\{name}")
        };
        show(h, c, &sub, res)?;
    }
    Ok(())
}
