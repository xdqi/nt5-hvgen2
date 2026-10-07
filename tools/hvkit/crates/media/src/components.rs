//! The Integration Services extras that need patched Microsoft files, for the setup CD and for an
//! installation changed offline:
//!
//! - Dynamic Memory: dmvsc.sys with the `dmvsc` recipe, which binds two kernel imports XP lacks to
//!   mdlex.sys (built in this repository), and dmvscres.dll;
//! - VSS (production checkpoints): icsvcvss.dll, the stock icsvc.dll with the `icsvc-vss` recipe;
//! - the Guest Service Interface (Copy-VMFile): icsvcgsi.dll, the stock icsvc.dll with `icsvc-gsi`;
//! - SynthVid at 32 bpp with 56 modes: VMBusVideoM.sys and VMBusVideoD.dll with `synthvid`.
//!
//! Server 2003 runs the stock dmvsc.sys and the VSS service as they are (the Integration Services'
//! INFs install them there), so the first three are for XP only; SynthVid's 16 bpp limit is the
//! same on both.

use crate::{Error, Result};
use std::path::{Path, PathBuf};

/// The Windows version, as far as the components care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NtVersion {
    /// Windows XP (5.1)
    Xp,
    /// Windows Server 2003 (5.2)
    Server2003,
}

impl NtVersion {
    pub fn from_numbers(major: u32, minor: u32) -> Result<NtVersion> {
        match (major, minor) {
            (5, 1) => Ok(NtVersion::Xp),
            (5, 2) => Ok(NtVersion::Server2003),
            _ => Err(Error(format!(
                "Windows {major}.{minor}: only XP (5.1) and Server 2003 (5.2) are supported"
            ))),
        }
    }
}

impl std::fmt::Display for NtVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NtVersion::Xp => "Windows XP (5.1)",
            NtVersion::Server2003 => "Windows Server 2003 (5.2)",
        })
    }
}

/// Which components to install (or, given to `select`, which to leave out).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Components {
    pub dynamic_memory: bool,
    pub vss: bool,
    pub gsi: bool,
    pub synthvid: bool,
}

impl Components {
    /// The defaults for `v` without the ones in `leave_out`: Dynamic Memory, VSS and the Guest
    /// Service Interface on XP, SynthVid on both.
    pub fn select(v: NtVersion, leave_out: Components) -> Components {
        let xp = v == NtVersion::Xp;
        Components {
            dynamic_memory: xp && !leave_out.dynamic_memory,
            vss: xp && !leave_out.vss,
            gsi: xp && !leave_out.gsi,
            synthvid: !leave_out.synthvid,
        }
    }

    /// For the log: "Dynamic Memory, VSS, ..." or "none".
    pub fn describe(&self) -> String {
        let names: Vec<&str> = [
            (self.dynamic_memory, "Dynamic Memory"),
            (self.vss, "VSS"),
            (self.gsi, "Guest Service Interface"),
            (self.synthvid, "SynthVid 32 bpp"),
        ]
        .iter()
        .filter(|c| c.0)
        .map(|c| c.1)
        .collect();
        if names.is_empty() {
            "none".into()
        } else {
            names.join(", ")
        }
    }
}

/// The file `name` in `dir`, whatever the case of its name (the Integration Services' packages
/// have vmbusvideom.sys, an installation VMBusVideoM.sys).
pub fn find_file(dir: &Path, name: &str) -> Result<PathBuf> {
    let entries = std::fs::read_dir(dir).map_err(|e| Error(format!("{}: {e}", dir.display())))?;
    entries
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
        .ok_or_else(|| Error(format!("{}: no {name}", dir.display())))
}

/// Applies a recipe to a stock file; a file the recipe patched before is taken as it is. `what`
/// names the file and `switch` the option that leaves the component out, for the error message.
fn patch(
    recipe: fn(&[u8]) -> recipes::Result<recipes::Outcome>,
    input: &[u8],
    what: &str,
    switch: &str,
    log: &mut dyn FnMut(String),
) -> Result<Vec<u8>> {
    let o =
        recipe(input).map_err(|e| Error(format!("{what}: {e} (leave it out with {switch})")))?;
    log(format!(
        "{what}: {}",
        match o.state {
            recipes::State::Known(name) => format!("patched ({name})"),
            recipes::State::Untested => "patched (not one of the tested files)".into(),
            recipes::State::Patched => "already patched".into(),
        }
    ));
    Ok(o.bytes)
}

pub fn dmvsc(stock: &[u8], log: &mut dyn FnMut(String)) -> Result<Vec<u8>> {
    patch(
        recipes::dmvsc::apply,
        stock,
        "dmvsc.sys",
        "--no-dynamic-memory",
        log,
    )
}

pub fn icsvc_vss(stock_icsvc: &[u8], log: &mut dyn FnMut(String)) -> Result<Vec<u8>> {
    patch(
        recipes::icsvc_vss::apply,
        stock_icsvc,
        "icsvc.dll -> icsvcvss.dll",
        "--no-vss",
        log,
    )
}

pub fn icsvc_gsi(stock_icsvc: &[u8], log: &mut dyn FnMut(String)) -> Result<Vec<u8>> {
    patch(
        recipes::icsvc_gsi::apply,
        stock_icsvc,
        "icsvc.dll -> icsvcgsi.dll",
        "--no-gsi",
        log,
    )
}

/// VMBusVideoM.sys or VMBusVideoD.dll (the recipe tells them apart).
pub fn synthvid(stock: &[u8], name: &str, log: &mut dyn FnMut(String)) -> Result<Vec<u8>> {
    patch(recipes::synthvid::apply, stock, name, "--no-synthvid", log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_per_version() {
        let none = Components::default();
        let xp = Components::select(NtVersion::Xp, none);
        assert!(xp.dynamic_memory && xp.vss && xp.gsi && xp.synthvid);
        let s = Components::select(NtVersion::Server2003, none);
        assert_eq!(
            s,
            Components {
                synthvid: true,
                ..none
            }
        );
        let no_vss = Components { vss: true, ..none };
        assert!(!Components::select(NtVersion::Xp, no_vss).vss);
        assert_eq!(
            Components::select(
                NtVersion::Server2003,
                Components {
                    synthvid: true,
                    ..none
                }
            )
            .describe(),
            "none"
        );
        assert!(NtVersion::from_numbers(6, 0).is_err());
    }
}
