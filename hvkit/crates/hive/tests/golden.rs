//! Comparison with reg.exe. The hives are from real XP installations and CDs and are not in the
//! repository: HVKIT_TESTDATA names a directory with them (see CASES); without it these tests do
//! nothing. The expected .reg files are `reg.exe export` of the hive loaded as HKLM\SPK.

use hive::{Hive, reg};
use std::path::{Path, PathBuf};

const ROOT: &str = "HKEY_LOCAL_MACHINE\\SPK";

/// (hive, its reg.exe export)
const EXPORTS: &[(&str, &str)] = &[
    ("in/system-xpv1.hiv", "expected/system-xpv1.reg"),
    ("in/system-xpvss.hiv", "expected/system-xpvss.reg"),
    ("in/setupreg-in.hiv", "expected/setupreg-in.reg"),
    ("expected/setupreg-out.hiv", "expected/setupreg-out.reg"),
];

fn testdata() -> Option<PathBuf> {
    std::env::var_os("HVKIT_TESTDATA").map(PathBuf::from)
}

fn export_file(path: &Path) -> Vec<u8> {
    let h = Hive::open(path, false).unwrap();
    reg::to_utf16(&reg::export(&h, h.root(), ROOT).unwrap())
}

/// The first line that differs, for a readable failure.
fn first_difference(a: &[u8], b: &[u8]) -> String {
    let (a, b) = (reg::decode(a).unwrap(), reg::decode(b).unwrap());
    for (i, (x, y)) in a.split("\r\n").zip(b.split("\r\n")).enumerate() {
        if x != y {
            return format!("line {}:\n  ours:    {x:?}\n  reg.exe: {y:?}", i + 1);
        }
    }
    format!(
        "lengths differ ({} vs {} lines)",
        a.split("\r\n").count(),
        b.split("\r\n").count()
    )
}

#[test]
fn export_is_reg_exe_export() {
    let Some(dir) = testdata() else {
        eprintln!("HVKIT_TESTDATA not set; skipped");
        return;
    };
    for (hive, expected) in EXPORTS {
        let ours = export_file(&dir.join(hive));
        let want = std::fs::read(dir.join(expected)).unwrap();
        assert!(ours == want, "{hive}: {}", first_difference(&ours, &want));
    }
}

#[test]
fn import_is_reg_exe_import() {
    let Some(dir) = testdata() else { return };
    // zhcd/build.sh's edits of SETUPREG.HIV, which reg.exe imported into expected/setupreg-out.hiv.
    let tmp = std::env::temp_dir().join(format!("hvkit-hive-test-{}.hiv", std::process::id()));
    std::fs::copy(dir.join("in/setupreg-in.hiv"), &tmp).unwrap();
    let text = reg::decode(&std::fs::read(dir.join("in/setupreg.reg")).unwrap()).unwrap();
    let mut h = Hive::open(&tmp, true).unwrap();
    let stats = reg::import(&mut h, &text, None).unwrap();
    assert_eq!((stats.keys, stats.values_set), (4, 12));
    h.commit(None).unwrap();
    drop(h);
    let ours = export_file(&tmp);
    let header = hive::Header::read(&std::fs::read(&tmp).unwrap()).unwrap();
    std::fs::remove_file(&tmp).unwrap();
    let want = std::fs::read(dir.join("expected/setupreg-out.reg")).unwrap();
    assert!(ours == want, "{}", first_difference(&ours, &want));
    assert!(!header.dirty());
}
