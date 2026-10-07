# Disk images and FAT

```
hvkit disk create boot.vhdx --size 64M --boot-code mbr.bin --part type=e,active,fat=16,label=CSMWRAP
hvkit disk create xp.vhdx --size 8G --part size=64M,type=ef,fat=16 --part type=7,active
hvkit disk create xp.vhdx --size 8G --part type=7,active --part start=end-65M,size=64M,type=ef,fat=16 \
    --put 2:csmwrap.efi=/EFI/BOOT/BOOTX64.EFI --put 2:extra=/    # ESP last; files put in while creating
hvkit disk info XP.vhdx            # partitions and their file systems
hvkit disk convert disk.raw disk.vhdx
hvkit fat cp boot.vhdx csmwrap.efi /EFI/BOOT/BOOTX64.EFI
hvkit fat put boot.vhdx extra/* -- /           # trees, keeping names and times
hvkit fat ls XP.vhdx:1 /WINDOWS -r
hvkit fat get XP.vhdx /WINDOWS/system32/config/system system.hiv
hvkit fat attrib dos.vhdx:2 /IO.SYS +h +s +r
hvkit fat bootcode dos.vhdx:2 floppy.img       # a DOS boot sector's code, keeping the BPB
```

- Images are raw, or VHDX by their extension (`.vhdx`, `.avhdx`). A differencing VHDX is read
  through its parents (found by the locator's relative path, next to it) and only it is written, so
  a checkpoint's disk can be changed offline while the parents stay as they are. Writes are stored
  in runs and pages of zeros stay unallocated, so new images are sparse.
- `IMAGE:N` is partition N of the MBR; without it, the first partition, or the whole image when
  there is no MBR (a floppy or a partition image).
- `--part ...,size=rest` reaches up to the next partition with a `start=`, or to the end of the disk;
  `start=end-SIZE` counts from the end. Partitions are formatted with the BPB's hidden sectors set
  to their start, as BIOS boot code needs. The MBR has no code unless `--boot-code` gives it.

FAT, long names included, comes from [fatfs](https://github.com/rafalh/rust-fatfs) and VHDX from
[vhdx-rs](https://github.com/inschrift-spruch-raum/vhdx-rs), both as forks with fixes not yet
upstream (file attributes and hidden sectors; Hyper-V's differencing disks and faster parent reads).
