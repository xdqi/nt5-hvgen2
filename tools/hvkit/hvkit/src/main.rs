//! hvkit: patching and media tools for running Windows NT 5.x on Hyper-V Generation 2.

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
    }
}

/// Whether `path` already holds exactly `data`.
fn same_file(path: &std::path::Path, data: &[u8]) -> bool {
    std::fs::read(path).is_ok_and(|d| d == data)
}
