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

#[test]
fn edits_survive_commit() {
    let Some(dir) = testdata() else { return };
    // Keys and values created, replaced and deleted, written out and read back (on Windows through
    // offreg, which writes a new file, elsewhere hivex).
    let tmp = std::env::temp_dir().join(format!("hvkit-hive-edit-{}.hiv", std::process::id()));
    let out = tmp.with_extension("out");
    std::fs::copy(dir.join("in/setupreg-in.hiv"), &tmp).unwrap();
    let mut h = Hive::open(&tmp, true).unwrap();
    let root = h.root();
    let k = h
        .create(root, r"ControlSet001\Services\zz\Parameters\Deep")
        .unwrap();
    h.set(k, &hive::Value::dword("A", 1)).unwrap();
    h.set(k, &hive::Value::string("B", hive::REG_SZ, "b"))
        .unwrap();
    h.set(k, &hive::Value::dword("C", 3)).unwrap();
    h.set(k, &hive::Value::dword("a", 2)).unwrap(); // replaces A in its place
    assert!(h.delete_value(k, "c").unwrap());
    assert!(!h.delete_value(k, "c").unwrap());
    let gone = h
        .create(root, r"ControlSet001\Services\zz\Gone\Below")
        .unwrap();
    h.set(gone, &hive::Value::dword("X", 1)).unwrap();
    let services = h.find(root, r"controlset001\SERVICES").unwrap().unwrap();
    assert_eq!(
        h.child(services, "ZZ").unwrap(),
        h.find(root, r"ControlSet001\Services\zz").unwrap()
    );
    let g = h.find(services, r"ZZ\gone").unwrap().unwrap();
    assert_eq!(h.name(g).unwrap(), "Gone");
    h.delete(g).unwrap();
    assert!(h.find(services, r"zz\Gone").unwrap().is_none());
    h.commit(Some(&out)).unwrap();
    h.commit(Some(&out)).unwrap(); // over an existing file
    drop(h);
    let h = Hive::open(&out, false).unwrap();
    let k = h
        .find(h.root(), r"ControlSet001\Services\zz\Parameters\Deep")
        .unwrap()
        .unwrap();
    let v: Vec<(String, u32)> = h
        .values(k)
        .unwrap()
        .into_iter()
        .map(|v| (v.name, v.ty))
        .collect();
    assert_eq!(
        v,
        [
            ("A".to_string(), hive::REG_DWORD),
            ("B".to_string(), hive::REG_SZ)
        ]
    );
    assert_eq!(h.value(k, "a").unwrap().unwrap().as_dword(), Some(2));
    let zz = h
        .find(h.root(), r"ControlSet001\Services\zz")
        .unwrap()
        .unwrap();
    let names: Vec<String> = h
        .children(zz)
        .unwrap()
        .into_iter()
        .map(|c| h.name(c).unwrap())
        .collect();
    assert_eq!(names, ["Parameters"]);
    assert!(
        !hive::Header::read(&std::fs::read(&out).unwrap())
            .unwrap()
            .dirty()
    );
    drop(h);
    std::fs::remove_file(&tmp).unwrap();
    std::fs::remove_file(&out).unwrap();
}
