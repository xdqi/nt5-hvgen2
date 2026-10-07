//! Masters a small tree the way setup CDs are mastered and reads it back with formats::iso9660.

use formats::iso9660::Iso;
use std::path::Path;

fn write(root: &Path, rel: &str, data: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

#[test]
fn nt5_setup_cd() {
    let dir = std::env::temp_dir().join(format!("hvkit-iso-test-{}", std::process::id()));
    let root = dir.join("root");
    let _ = std::fs::remove_dir_all(&dir);
    let boot: Vec<u8> = (0..2048u32).map(|i| i as u8).collect();
    write(&root, "boot.img", &boot);
    write(&root, "WIN51", b"no extension");
    write(&root, "I386/SETUPLDR.BIN", b"setupldr");
    write(
        &root,
        "$OEM$/$1/Drivers/HV/vmbus/vmbus.inf",
        b"[Version]\r\n",
    );
    // Nine levels: deeper than ISO 9660 allows without relocation.
    write(&root, "a/b/c/d/e/f/g/h/i/deep.txt", b"deep");
    let out = dir.join("out.iso");
    iso::build(&root, &out, &iso::Options::nt5_setup("TESTCD", "/boot.img")).unwrap();

    let mut i = Iso::open(&out).unwrap();
    assert_eq!(i.volume_id, "TESTCD");
    assert!(i.has_joliet());
    let b = i.boot_entry().unwrap().unwrap();
    assert_eq!((b.media, b.sector_count), (0, 4));
    assert_eq!(i.boot_image().unwrap().unwrap(), boot);
    // ISO 9660 names as on Microsoft's CDs: no ";1", no forced ".".
    assert!(
        i.primary_record_offset("I386", "SETUPLDR.BIN")
            .unwrap()
            .is_some()
    );
    let entries = i.entries().unwrap();
    let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    for p in [
        "WIN51",
        "$OEM$/$1/Drivers/HV/vmbus/vmbus.inf",
        "a/b/c/d/e/f/g/h/i/deep.txt",
        "boot.img",
    ] {
        assert!(paths.contains(&p), "{p} missing from {paths:?}");
    }
    let deep = entries
        .iter()
        .find(|e| e.path.ends_with("deep.txt"))
        .unwrap();
    assert_eq!(i.read(deep).unwrap(), b"deep");
    // The boot image and catalog are hidden from the ISO 9660 tree (Joliet keeps the image).
    assert!(i.primary_record_offset("", "boot.img").unwrap().is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}
