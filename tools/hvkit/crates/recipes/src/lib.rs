//! Binary patch recipes for the Microsoft files that need changes to work on Hyper-V Generation 2.
//!
//! Each recipe works on the user's own copy of a file: no Microsoft bytes are shipped beyond what the
//! checks need (hashes and the short instruction sequences that are verified before they are
//! replaced). Every recipe tells a stock file, a file it patched before and anything else apart, and
//! patching an already patched file changes nothing.

pub mod dmvsc;
pub mod icsvc_gsi;
pub mod ntldr;
pub mod win98_keyboard;

use sha2::{Digest, Sha256};

pub use formats::{Error, Result};

/// What a file is, as far as a recipe can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// A file the recipe was tested with (`name` says which).
    Known(&'static str),
    /// A file the recipe can patch but was not tested with.
    Untested,
    /// Already patched by this recipe.
    Patched,
}

/// The result of applying a recipe.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// What the input was.
    pub state: State,
    /// The output file (the input unchanged when it was already patched).
    pub bytes: Vec<u8>,
    /// What was done, one line per step, for the user.
    pub log: Vec<String>,
}

/// A recipe, by name, for the command line.
pub struct Recipe {
    pub name: &'static str,
    pub summary: &'static str,
    pub apply: fn(&[u8]) -> Result<Outcome>,
}

pub const RECIPES: &[Recipe] = &[
    Recipe {
        name: "ntldr",
        summary: ntldr::SUMMARY,
        apply: ntldr::apply,
    },
    Recipe {
        name: "dmvsc",
        summary: dmvsc::SUMMARY,
        apply: dmvsc::apply,
    },
    Recipe {
        name: "icsvc-gsi",
        summary: icsvc_gsi::SUMMARY,
        apply: icsvc_gsi::apply,
    },
    Recipe {
        name: "win98-keyboard",
        summary: win98_keyboard::SUMMARY,
        apply: win98_keyboard::apply,
    },
];

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02X}"))
        .collect()
}

/// Checks that `b[at..]` starts with `expected`, naming the place in the error.
fn expect_bytes(b: &[u8], at: usize, expected: &[u8], what: &str) -> Result<()> {
    for (i, &e) in expected.iter().enumerate() {
        match b.get(at + i) {
            Some(&v) if v == e => {}
            Some(&v) => formats::bail!(
                "{what}: byte at 0x{:X} is 0x{v:02X}, expected 0x{e:02X}; refusing to patch.",
                at + i
            ),
            None => formats::bail!("{what}: offset 0x{:X} is past the end of the file", at + i),
        }
    }
    Ok(())
}
