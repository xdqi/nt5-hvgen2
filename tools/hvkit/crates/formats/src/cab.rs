//! Microsoft cabinet files (MSZIP, LZX), read only, through the `cab` crate.

use crate::{Error, Result};
use std::io::Read;
use std::path::Path;

/// The names of the files in a cabinet, as stored.
pub fn list(path: &Path) -> Result<Vec<String>> {
    let f = std::fs::File::open(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    let cab = cab::Cabinet::new(f).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    Ok(cab
        .folder_entries()
        .flat_map(|f| {
            f.file_entries()
                .map(|e| e.name().to_string())
                .collect::<Vec<_>>()
        })
        .collect())
}

/// The contents of `name` (compared without case) in the cabinet.
pub fn extract(path: &Path, name: &str) -> Result<Vec<u8>> {
    let err = |e: std::io::Error| Error(format!("{}: {e}", path.display()));
    let stored = list(path)?
        .into_iter()
        .find(|n| n.eq_ignore_ascii_case(name))
        .ok_or_else(|| Error(format!("{}: no {name} in the cabinet", path.display())))?;
    let f = std::fs::File::open(path).map_err(err)?;
    let mut cab = cab::Cabinet::new(f).map_err(err)?;
    let mut out = Vec::new();
    cab.read_file(&stored)
        .map_err(err)?
        .read_to_end(&mut out)
        .map_err(err)?;
    Ok(out)
}
