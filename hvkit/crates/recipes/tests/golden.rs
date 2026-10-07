//! Byte-for-byte comparison with the PowerShell scripts the recipes were ported from.
//!
//! The inputs are Microsoft files and are not in the repository. HVKIT_TESTDATA names a directory
//! with `in/<name>` (the stock files) and `expected/<name>` (what the scripts in migrate/ made of
//! them); without it these tests do nothing. The names are listed in CASES.

use formats::pe::Pe;
use recipes::{RECIPES, State};
use std::collections::{BTreeMap, BTreeSet};
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
    ("icsvc-vss", "icsvc.dll", "icsvc-vss.dll"),
    ("synthvid", "VMBusVideoM.sys", "VMBusVideoM.sys"),
    ("synthvid", "VMBusVideoD.dll", "VMBusVideoD.dll"),
    ("win98-keyboard", "keyboard.drv", "keyboard.drv"),
    // No PowerShell script for these: the expected file is what the recipe was tested with.
    ("hal-clock", "hal-x64-clock.dll", "hal-x64-clock.dll"),
    ("netvsc", "netvsc50.sys", "netvsc50.sys"),
    ("dmvsc", "dmvsc-x64.sys", "dmvsc-x64.sys"),
    ("icsvc-gsi", "icsvc-x64.dll", "icsvc-x64-gsi.dll"),
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
    assert!(recipe("icsvc-gsi")(&read("hal-x64-clock.dll")).is_err());
    assert!(recipe("icsvc-vss")(&read("dmvsc.sys")).is_err());
    assert!(recipe("ntldr")(&read("dmvsc.sys")).is_err());
    assert!(recipe("ntldr")(&read("icsvc.dll")).is_err());
    assert!(recipe("synthvid")(&read("dmvsc.sys")).is_err());
    assert!(recipe("dmvsc")(&read("VMBusVideoM.sys")).is_err());
    assert!(recipe("dmvsc")(&read("hal-x64-clock.dll")).is_err());
}

/// (module, function, IAT slot RVA) of every import by name, in descriptor order. PE32+ thunks are 8
/// bytes, with the ordinal flag in the top bit.
fn imports(b: &[u8]) -> Vec<(String, String, u32)> {
    let pe = Pe::parse(b, 0).unwrap();
    let w = if pe.pe32_plus { 8 } else { 4 };
    let mut v = Vec::new();
    for d in pe.imports(b).unwrap() {
        let o = pe.rva_to_offset(d.original_first_thunk).unwrap();
        for j in 0.. {
            let t = &b[o + w * j..o + w * (j + 1)];
            if t.iter().all(|&x| x == 0) {
                break;
            }
            if t[w - 1] & 0x80 != 0 {
                continue;
            }
            let rva = u32::from_le_bytes(t[..4].try_into().unwrap());
            let name = pe.thunk_name(b, rva).unwrap().unwrap();
            v.push((d.dll.to_lowercase(), name, d.first_thunk + (w * j) as u32));
        }
    }
    v
}

/// Checks the dmvsc recipe's outputs from their import tables rather than against the expected
/// files (which it made itself): after the loader has gone through the descriptors in order, each
/// rebound routine's IAT slot holds mdlex.sys's routine and every other slot what it held before;
/// and every import the output names is one the input had, or a mdlex.sys one, so it all resolves.
#[test]
fn dmvsc_rebinds_to_mdlex() {
    let Some(dir) = testdata() else { return };
    let cases: [(&str, &[&str]); 2] = [
        (
            "dmvsc.sys",
            &["MmAllocatePagesForMdlEx", "MmAddPhysicalMemory"],
        ),
        ("dmvsc-x64.sys", &["MmAddPhysicalMemory"]),
    ];
    let slots = |v: &[(String, String, u32)]| -> BTreeMap<u32, (String, String)> {
        v.iter()
            .map(|(m, f, s)| (*s, (m.clone(), f.clone())))
            .collect()
    };
    let named = |v: &[(String, String, u32)]| -> BTreeSet<(String, String)> {
        v.iter().map(|(m, f, _)| (m.clone(), f.clone())).collect()
    };
    for (name, rebound) in cases {
        let input = std::fs::read(dir.join("in").join(name)).unwrap();
        let out = recipe("dmvsc")(&input).unwrap().bytes;
        let (before, after) = (imports(&input), imports(&out));

        let mut want = slots(&before);
        for (m, f) in want.values_mut() {
            if rebound.contains(&f.as_str()) {
                *m = "mdlex.sys".into();
            }
        }
        assert_eq!(slots(&after), want, "{name}: IAT slots after loading");

        let mut allowed = named(&before);
        allowed.extend(rebound.iter().map(|f| ("mdlex.sys".into(), f.to_string())));
        assert!(named(&after).is_subset(&allowed), "{name}: new imports");
        let mdlex: Vec<_> = after.iter().filter(|i| i.0 == "mdlex.sys").collect();
        assert_eq!(mdlex.len(), rebound.len(), "{name}: mdlex.sys imports");
    }
}
