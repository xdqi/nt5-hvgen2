//! Disk images end to end: partitioning, formatting, files, attributes, reopening; raw and VHDX, and a
//! VHDX differencing child (written alone, read through its parent).

use disk::Image;
use disk::fat::{self, FormatOptions};
use fatfs::FileAttributes;
use formats::mbr::{Mbr, Partition};
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hvkit-disk-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// A 64 MiB disk with one FAT partition at sector 2048 holding a few files.
fn make(path: &Path, fat_bits: u8) {
    let mut img = Image::create(path, 64 << 20, 2 << 20).unwrap();
    let mut mbr = Mbr::new(0xdead_beef);
    mbr.partitions[0] = Some(Partition {
        active: true,
        kind: 0x0e,
        start: 2048,
        sectors: (64 << 11) - 2048,
    });
    img.write_at(0, &mbr.to_bytes().unwrap()).unwrap();
    let (start, len) = disk::partition(&mut img, 1).unwrap();
    let o = FormatOptions {
        fat: Some(fat_bits),
        label: Some("TEST".into()),
        hidden_sectors: 2048,
        ..Default::default()
    };
    fat::format(img.window(start, len), &o).unwrap();
    let fs = fat::open(img.window(start, len)).unwrap();
    fat::mkdir_p(&fs, "/EFI/BOOT").unwrap();
    fat::write(&fs, "/EFI/BOOT/BOOTX64.EFI", &vec![0x5a; 300_000], None).unwrap();
    fat::write(&fs, "/Mixed Case Long Name.txt", b"hello", None).unwrap();
    fat::write(&fs, "/IO.SYS", b"io", None).unwrap();
    fat::set_attributes(
        &fs,
        "/IO.SYS",
        FileAttributes::HIDDEN | FileAttributes::SYSTEM | FileAttributes::READ_ONLY,
        FileAttributes::empty(),
    )
    .unwrap();
    fs.unmount().unwrap();
    img.flush().unwrap();
}

fn check(path: &Path) {
    let mut img = Image::open(path, false).unwrap();
    let mbr = disk::mbr(&mut img).unwrap();
    assert_eq!(mbr.signature, 0xdead_beef);
    let (start, len) = disk::partition(&mut img, 1).unwrap();
    let mut boot = [0u8; 512];
    img.read_at(start, &mut boot).unwrap();
    assert_eq!(
        u32::from_le_bytes(boot[28..32].try_into().unwrap()),
        2048,
        "hidden sectors"
    );
    let fs = fat::open(img.window(start, len)).unwrap();
    assert_eq!(
        fat::read(&fs, "/efi/boot/bootx64.efi").unwrap(),
        vec![0x5a; 300_000]
    );
    assert_eq!(
        fat::read(&fs, "/Mixed Case Long Name.txt").unwrap(),
        b"hello"
    );
    let io = fat::list(&fs, "/")
        .unwrap()
        .into_iter()
        .find(|e| e.name == "IO.SYS")
        .unwrap();
    assert_eq!(
        io.attributes,
        FileAttributes::HIDDEN | FileAttributes::SYSTEM | FileAttributes::READ_ONLY
    );
}

#[test]
fn raw_and_vhdx() {
    for (name, bits) in [("fat16.img", 16), ("fat32.vhdx", 32), ("fat16.vhdx", 16)] {
        let p = tmp(name);
        make(&p, bits);
        check(&p);
        std::fs::remove_file(&p).unwrap();
    }
}

#[test]
fn vhdx_is_sparse() {
    let p = tmp("sparse.vhdx");
    make(&p, 16);
    // 64 MiB virtual, 2 MiB blocks: only the blocks with the MBR, the FAT structures and the files.
    let size = std::fs::metadata(&p).unwrap().len();
    assert!(size < 16 << 20, "{size} bytes");
    std::fs::remove_file(&p).unwrap();
}

#[test]
fn differencing_child() {
    let (parent, child) = (tmp("parent.vhdx"), tmp("child.vhdx"));
    make(&parent, 16);
    let before = std::fs::read(&parent).unwrap();
    {
        let mut pm = vhdx::Medium::open(std::fs::File::open(&parent).unwrap())
            .finish()
            .unwrap();
        let f = std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&child)
            .unwrap();
        vhdx::Medium::create(f)
            .size(64 << 20)
            .block_size(2 << 20)
            .logical_sector_size(512)
            .physical_sector_size(512)
            .parent(&mut pm, "parent.vhdx")
            .unwrap()
            .finish()
            .unwrap();
    }
    {
        let mut img = Image::open(&child, true).unwrap();
        let (start, len) = disk::partition(&mut img, 1).unwrap();
        let fs = fat::open(img.window(start, len)).unwrap();
        fat::write(&fs, "/child.txt", b"only in the child", None).unwrap();
        fs.unmount().unwrap();
        img.flush().unwrap();
    }
    assert_eq!(
        std::fs::read(&parent).unwrap(),
        before,
        "the parent changed"
    );
    check(&child);
    let mut img = Image::open(&child, false).unwrap();
    let (start, len) = disk::partition(&mut img, 1).unwrap();
    let fs = fat::open(img.window(start, len)).unwrap();
    assert_eq!(fat::read(&fs, "/child.txt").unwrap(), b"only in the child");
    let mut img = Image::open(&parent, false).unwrap();
    let (start, len) = disk::partition(&mut img, 1).unwrap();
    assert!(!fat::exists(
        &fat::open(img.window(start, len)).unwrap(),
        "/child.txt"
    ));
    std::fs::remove_file(&parent).unwrap();
    std::fs::remove_file(&child).unwrap();
}
