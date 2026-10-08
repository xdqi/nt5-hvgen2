//! Installation media built from the user's own CDs and files, and installations changed offline.

pub mod components;
#[cfg(feature = "iso")]
pub mod csmwrap_cd;
#[cfg(feature = "iso")]
pub mod hvfb_cd;
pub mod inject;
pub mod migrate;
pub mod nlite;
pub mod nt5;
mod offline;
#[cfg(feature = "iso")]
pub mod setup_cd;
pub mod w98_disk;

use formats::inf::Text;
use formats::iso9660::Iso;
use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Copies a directory tree, keeping the files' and directories' modification times (as `cp -a`;
/// an ISO made from the copy then has the original's dates).
pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let err = |p: &Path, e: std::io::Error| Error(format!("{}: {e}", p.display()));
    std::fs::create_dir_all(to).map_err(|e| err(to, e))?;
    for entry in std::fs::read_dir(from).map_err(|e| err(from, e))? {
        let entry = entry.map_err(|e| err(from, e))?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().map_err(|e| err(&src, e))?.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| err(&src, e))?;
            let t = entry
                .metadata()
                .and_then(|m| m.modified())
                .map_err(|e| err(&src, e))?;
            std::fs::File::options()
                .write(true)
                .open(&dst)
                .and_then(|f| f.set_modified(t))
                .map_err(|e| err(&dst, e))?;
        }
    }
    let t = std::fs::metadata(from)
        .and_then(|m| m.modified())
        .map_err(|e| err(from, e))?;
    std::fs::File::open(to)
        .and_then(|f| f.set_modified(t))
        .map_err(|e| err(to, e))?;
    Ok(())
}

pub(crate) fn io(p: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error(format!("{}: {e}", p.display()))
}

pub(crate) fn fmt_err(e: formats::Error) -> Error {
    Error(e.0)
}

/// Extracts `iso` into `dir` unless a complete copy is there, in the layout of `7z x`: the files,
/// and the boot image as [BOOT]/Boot-NoEmul.img.
pub fn extract_cached(iso: &Path, dir: &Path, log: &mut dyn FnMut(String)) -> Result<()> {
    if dir.join(".done").exists() {
        return Ok(());
    }
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(io(dir))?;
    }
    let mut i = Iso::open(iso).map_err(fmt_err)?;
    let n = i.extract_all(dir).map_err(fmt_err)?;
    let boot = i
        .boot_image()
        .map_err(fmt_err)?
        .ok_or_else(|| Error(format!("{}: not bootable", iso.display())))?;
    let b = dir.join("[BOOT]");
    std::fs::create_dir_all(&b).map_err(io(&b))?;
    std::fs::write(b.join("Boot-NoEmul.img"), boot).map_err(io(&b))?;
    std::fs::write(dir.join(".done"), b"").map_err(io(dir))?;
    log(format!(
        "extracted {} ({n} files) to {}",
        iso.display(),
        dir.display()
    ));
    Ok(())
}

pub(crate) fn read_text(p: &Path) -> Result<Text> {
    Text::parse(&std::fs::read(p).map_err(io(p))?)
        .map_err(|e| Error(format!("{}: {e}", p.display())))
}

pub(crate) fn write_text(p: &Path, t: &Text) -> Result<()> {
    std::fs::write(
        p,
        t.to_bytes()
            .map_err(|e| Error(format!("{}: {e}", p.display())))?,
    )
    .map_err(io(p))
}

pub(crate) fn copy(from: &Path, to: &Path) -> Result<()> {
    std::fs::copy(from, to)
        .map_err(|e| Error(format!("{} -> {}: {e}", from.display(), to.display())))?;
    Ok(())
}

pub(crate) fn remove_if_exists(p: &Path) -> Result<()> {
    match std::fs::remove_file(p) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(p)(e)),
        _ => Ok(()),
    }
}
