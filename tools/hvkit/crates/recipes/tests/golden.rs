//! Byte-for-byte comparison with the PowerShell scripts the recipes were ported from.
//!
//! The inputs are Microsoft files and are not in the repository. HVKIT_TESTDATA names a directory
//! with `in/<name>` (the stock files) and `expected/<name>` (what the scripts in migrate/ made of
//! them); without it these tests do nothing. The names are listed in CASES.

use recipes::{RECIPES, State};
use std::path::PathBuf;

/// (recipe, input, expected output)
const CASES: &[(&str, &str, &str)] = &[
    ("ntldr", "ntldr-zh", "ntldr-zh"),
    ("ntldr", "ntldr-en", "ntldr-en"),
    ("ntldr", "ntldr-2k3", "ntldr-2k3"),
    ("ntldr", "setupldr-zh", "setupldr-zh"),
    ("ntldr", "setupldr-en", "setupldr-en"),
    ("ntldr", "setupldr-2k3", "setupldr-2k3"),
    ("dmvsc", "dmvsc.sys", "dmvsc.sys"),
    ("icsvc-gsi", "icsvc.dll", "icsvc-gsi.dll"),
    ("synthvid", "VMBusVideoM.sys", "VMBusVideoM.sys"),
    ("synthvid", "VMBusVideoD.dll", "VMBusVideoD.dll"),
    ("win98-keyboard", "keyboard.drv", "keyboard.drv"),
];

fn testdata() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HVKIT_TESTDATA")?);
    Some(dir)
}

fn recipe(name: &str) -> fn(&[u8]) -> recipes::Result<recipes::Outcome> {
    RECIPES
        .iter()
        .find(|r| r.name == name)
        .expect("recipe")
        .apply
}

#[test]
fn same_bytes_as_the_scripts() {
    let Some(dir) = testdata() else {
        eprintln!("HVKIT_TESTDATA not set; skipped");
        return;
    };
    for (r, name, expected) in CASES {
        let input = std::fs::read(dir.join("in").join(name)).unwrap();
        let expected = std::fs::read(dir.join("expected").join(expected)).unwrap();
        let out = recipe(r)(&input).unwrap_or_else(|e| panic!("{r} on {name}: {e}"));
        assert!(
            matches!(out.state, State::Known(_)),
            "{r} on {name}: state {:?}",
            out.state
        );
        assert!(
            out.bytes == expected,
            "{r} on {name}: output differs from the script's ({} vs {} bytes)",
            out.bytes.len(),
            expected.len()
        );

        // Patching the result again changes nothing.
        let again = recipe(r)(&out.bytes).unwrap();
        assert_eq!(
            again.state,
            State::Patched,
            "{r} on {name}: patched output not recognised"
        );
        assert!(
            again.bytes == out.bytes,
            "{r} on {name}: second application changed the file"
        );
    }
}

#[test]
fn wrong_files_are_refused() {
    let Some(dir) = testdata() else { return };
    let read = |n: &str| std::fs::read(dir.join("in").join(n)).unwrap();
    assert!(recipe("dmvsc")(&read("icsvc.dll")).is_err());
    assert!(recipe("icsvc-gsi")(&read("dmvsc.sys")).is_err());
    assert!(recipe("ntldr")(&read("dmvsc.sys")).is_err());
    assert!(recipe("ntldr")(&read("icsvc.dll")).is_err());
    assert!(recipe("synthvid")(&read("dmvsc.sys")).is_err());
    assert!(recipe("dmvsc")(&read("VMBusVideoM.sys")).is_err());
}
