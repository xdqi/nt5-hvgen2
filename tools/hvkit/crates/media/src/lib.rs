//! Installation media built from the user's own CDs and files.

pub mod components;
pub mod hvfb_cd;
pub mod setup_cd;

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
