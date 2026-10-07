//! hvkit: patching and media tools for running Windows NT 5.x on Hyper-V Generation 2.

mod cab_cmd;
mod disk_cmd;
#[cfg(feature = "hive")]
mod hive_cmd;
mod iso_cmd;
#[cfg(feature = "setup-cd")]
mod setup_cd_cmd;
mod vxd_cmd;

use clap::{Parser, Subcommand};
use recipes::{RECIPES, State};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Apply a binary patch recipe to the user's own copy of a Microsoft file
    Patch {
        /// The recipe (see --list)
        #[arg(required_unless_present = "list")]
        recipe: Option<String>,
        /// The file to patch
        #[arg(required_unless_present = "list")]
        input: Option<PathBuf>,
        /// Where to write the result (default: patch INPUT in place)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Only tell what INPUT is (stock, already patched, or not patchable); write nothing
        #[arg(long)]
        check: bool,
        /// List the recipes
        #[arg(long)]
        list: bool,
    },
    /// Read and write Microsoft cabinets and cabinet sets
    Cab {
        #[command(subcommand)]
        command: cab_cmd::CabCommand,
    },
    /// Create and inspect disk images (raw, VHDX) with MBR partitions
    Disk {
        #[command(subcommand)]
        command: disk_cmd::DiskCommand,
    },
    /// Read and change FAT file systems in disk images
    Fat {
        #[command(subcommand)]
        command: disk_cmd::FatCommand,
    },
    /// Read ISO 9660 images
    Iso {
        #[command(subcommand)]
        command: iso_cmd::IsoCommand,
    },
    /// Build a Windows XP / Server 2003 setup CD that installs on Hyper-V Generation 2
    #[cfg(feature = "setup-cd")]
    SetupCd(setup_cd_cmd::SetupCdArgs),
    /// Build a Windows XP setup CD that uses hvfb.sys for its display (no other drivers; e.g. QEMU)
    #[cfg(feature = "setup-cd")]
    HvfbCd(setup_cd_cmd::HvfbCdArgs),
    /// Inspect and patch Windows 9x VxDs (LE files)
    Vxd {
        #[command(subcommand)]
        command: vxd_cmd::VxdCommand,
    },
    /// Read and change offline registry hives (needs hivex)
    #[cfg(feature = "hive")]
    Hive {
        #[command(subcommand)]
        command: hive_cmd::HiveCommand,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hvkit: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Patch { list: true, .. } => {
            for r in RECIPES {
                println!("{:<10} {}", r.name, r.summary);
            }
            Ok(())
        }
        Command::Patch {
            recipe: Some(name),
            input: Some(input),
            output,
            check,
            ..
        } => {
            let Some(recipe) = RECIPES.iter().find(|r| r.name == name) else {
                return Err(format!("no recipe '{name}' (see hvkit patch --list)"));
            };
            let data = std::fs::read(&input).map_err(|e| format!("{}: {e}", input.display()))?;
            let out = (recipe.apply)(&data).map_err(|e| format!("{}: {e}", input.display()))?;
            if check {
                println!(
                    "{}: {}",
                    input.display(),
                    match out.state {
                        State::Known(name) => format!("patchable ({name})"),
                        State::Untested => "patchable (not one of the tested files)".into(),
                        State::Patched => "already patched".into(),
                    }
                );
                return Ok(());
            }
            for line in &out.log {
                println!("{}: {line}", input.display());
            }
            let output = output.unwrap_or(input);
            if out.bytes != data || !same_file(&output, &data) {
                std::fs::write(&output, &out.bytes)
                    .map_err(|e| format!("{}: {e}", output.display()))?;
                println!("wrote {}", output.display());
            }
            Ok(())
        }
        Command::Patch { .. } => unreachable!("clap requires RECIPE and INPUT without --list"),
        Command::Cab { command } => cab_cmd::run(command),
        Command::Disk { command } => disk_cmd::run_disk(command),
        Command::Fat { command } => disk_cmd::run_fat(command),
        Command::Iso { command } => iso_cmd::run(command),
        #[cfg(feature = "setup-cd")]
        Command::SetupCd(args) => setup_cd_cmd::run(args),
        #[cfg(feature = "setup-cd")]
        Command::HvfbCd(args) => setup_cd_cmd::run_hvfb(args),
        Command::Vxd { command } => vxd_cmd::run(command),
        #[cfg(feature = "hive")]
        Command::Hive { command } => hive_cmd::run(command),
    }
}

/// Whether `path` already holds exactly `data`.
fn same_file(path: &std::path::Path, data: &[u8]) -> bool {
    std::fs::read(path).is_ok_and(|d| d == data)
}
