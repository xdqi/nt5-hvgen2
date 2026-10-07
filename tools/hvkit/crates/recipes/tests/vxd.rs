//! VxD port I/O redirection, the SYSDETMG.DLL patch and the wlink entry fix, compared byte for byte
//! with the CSMWrap testbed's w98/patch-io.py, w98/patch-sysdetmg.py and gen2leg-ow/fixentry.py. Needs Microsoft files: reads them from the directory
//! HVKIT_TESTDATA names (in/ and expected/), and does nothing without it.

use recipes::State;
use recipes::{vxd_portio, win98_sysdetmg};
use std::path::PathBuf;

fn testdata() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HVKIT_TESTDATA")?))
}

#[test]
fn portio_same_bytes_as_the_script() {
    let Some(dir) = testdata() else {
        eprintln!("HVKIT_TESTDATA not set; skipped");
        return;
    };
    let vectors = vxd_portio::parse_vectors(
        &std::fs::read_to_string(dir.join("in/gen2leg-vectors.txt")).unwrap(),
    )
    .unwrap();
    for (name, sites) in [
        ("vpicd", "VPICD: 108 sites"),
        ("vtd", "VTD: 36 sites"),
        ("vkd", "VKD: 23 sites"),
    ] {
        let input = std::fs::read(dir.join(format!("in/{name}.vxd"))).unwrap();
        let expected = std::fs::read(dir.join(format!("expected/{name}.vxd"))).unwrap();
        let out = vxd_portio::patch(&input, None, &vectors).unwrap();
        assert_eq!(out.log, [sites]);
        assert!(
            out.bytes == expected,
            "{name}: output differs from patch-io.py's"
        );
        let again = vxd_portio::patch(&out.bytes, None, &vectors).unwrap();
        assert_eq!(
            again.state,
            State::Patched,
            "{name}: patched output not recognised"
        );
        assert!(
            again.bytes == out.bytes,
            "{name}: second application changed the file"
        );
    }
}

#[test]
fn sysdetmg_same_bytes_as_the_script() {
    let Some(dir) = testdata() else { return };
    let vectors = vxd_portio::parse_vectors(
        &std::fs::read_to_string(dir.join("in/gen2leg-vectors.txt")).unwrap(),
    )
    .unwrap();
    let input = std::fs::read(dir.join("in/sysdetmg.dll")).unwrap();
    let out = win98_sysdetmg::apply(&input, &vectors).unwrap();
    assert!(
        matches!(out.state, State::Known(_)),
        "state {:?}",
        out.state
    );
    assert_eq!(out.log.len(), 4);
    assert!(out.bytes == std::fs::read(dir.join("expected/sysdetmg.dll")).unwrap());
    let again = win98_sysdetmg::apply(&out.bytes, &vectors).unwrap();
    assert_eq!(again.state, State::Patched);
    assert!(again.bytes == out.bytes);
    // Other vectors: refused, not patched twice.
    let mut other = vectors;
    other[vxd_portio::IDX_IN_DX] = 0x41;
    assert!(win98_sysdetmg::apply(&out.bytes, &other).is_err());
}

#[test]
fn fix_entry_same_bytes_as_the_script() {
    let Some(dir) = testdata() else { return };
    let mut b = std::fs::read(dir.join("in/wlink-type2.vxd")).unwrap();
    assert!(formats::le::fix_ddb_entry(&mut b).unwrap());
    assert!(b == std::fs::read(dir.join("expected/wlink-type2.vxd")).unwrap());
    assert!(!formats::le::fix_ddb_entry(&mut b).unwrap());
}
