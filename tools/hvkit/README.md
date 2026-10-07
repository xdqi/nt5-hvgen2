# hvkit

One Rust tool for the binary patches and media this repository needs, replacing the scattered bash,
Python and PowerShell scripts step by step. So far it has the patch recipes for Microsoft files,
offline registry hives, ISO images, Windows setup CDs, and disk images with FAT file systems.

## Patch recipes

```
hvkit patch --list
hvkit patch ntldr     I386/NTLDR          # NTLDR / SETUPLDR.BIN: menu highlight in single-plane mode 12h
hvkit patch dmvsc     dmvsc.sys -o out/dmvsc.sys   # rebind XP-missing imports to mdlex.sys
hvkit patch icsvc-gsi icsvc.dll -o icsvcgsi.dll    # Guest Service Interface on XP
hvkit patch ntldr     NTLDR --check       # only tell whether it is stock, patched or not patchable
```

Without `-o` the file is patched in place. Every recipe recognises a file it patched before and leaves
it alone, and refuses files it does not know how to patch. The recipes are ports of
`migrate/Patch-Ntldr.ps1`, `migrate/Patch-Dmvsc.ps1` and `migrate/IcSvcGuestInterface.ps1` and give
the same bytes; the scripts stay until the converter uses hvkit. No Microsoft file is in this
repository: the recipes patch the user's own copies.

## Registry hives

```
hvkit hive info   SYSTEM                       # sequence numbers, version, dirty or not
hvkit hive export SYSTEM -o system.reg --root 'HKEY_LOCAL_MACHINE\XPMIG'   # = reg.exe export
hvkit hive export SYSTEM --key 'ControlSet001\Services\hvfb'              # to stdout, UTF-8
hvkit hive import SYSTEM edits.reg             # = reg.exe import with the hive loaded at the .reg's root
hvkit hive show   SYSTEM 'Services\\vmbus$' 'Control\\Video'           # regdump.py-style view
```

No `reg load`, no Windows, no administrator. `export` writes the file `reg.exe export` writes (UTF-16
with CR LF), byte for byte, including keys whose ACL keeps administrators out; `--utf8` gives the
text hive-dump.sh made of it. `import` takes REGEDIT4 and version 5.00 files, `[-key]` and `"v"=-`
deletions included; the root of the .reg file (by default the first two components of its first
key) stands for the hive's root key. It refuses a dirty hive (log not written back) without
`--force`. Hives are read and written with [hivex](https://libguestfs.org/hivex.3.html) (LGPL-2.1),
linked dynamically: install it (`pacman -S hivex`, `apt install libhivex-dev`) or build without the
`hive` feature.

## Setup CDs and ISO images

```
hvkit hvfb-cd XP.iso OUT.iso --hvfb out/hvfb.sys [--bootvid out/bootvid.dll] [--default-mode 1024x768x32]
hvkit setup-cd XP.iso OUT.iso --files DIR --ic DIR [--mp-source XP-SAME-BUILD.iso] [--kd] [--unattend ...]
hvkit iso info CD.iso       # volume id, El Torito entry, where SETUPLDR.BIN's record is in \I386
hvkit iso extract CD.iso DIR [--boot-image boot.img]
hvkit iso ls CD.iso         # files with their first block (to map a disk trace's LBAs to files)
hvkit iso build DIR OUT.iso --volume-id ID [--nt5-setup /boot.img] [--efi /esp.img]
```

`hvfb-cd` makes text-mode setup draw through hvfb.sys and installs it as the new system's display
driver; it needs nothing but the CD and our own drivers (`tools/xp-iso.sh` calls it). `setup-cd` makes
a CD that installs on Hyper-V Generation 2: KMDF, the VMBus, storvsc with the KB943295 storport,
bootwait, the synthetic keyboard, hvfb, the NTLDR recipe, the multiprocessor HAL and the Integration
Services' INFs for GUI-mode setup; `--files` holds those drivers (Microsoft files among them, so they
come from the user), `--ic` the Integration Services packages. Both read the source CD only, keep its
extracted copy in `~/.cache/hvkit/cd`, and write CDs like Microsoft's: ISO 9660 names without `;1`,
Joliet, no Rock Ridge (it can push SETUPLDR.BIN past the 128 sectors of `\I386` the CD boot sector
reads, which they check), the boot image hidden from the ISO 9660 tree. An existing output file is
rewritten in place, so it keeps the ACL Hyper-V gives the VM that has it in its DVD drive.

Mastering uses [libisofs](https://dev.lovelyhq.com/libburnia/libisofs) (GPL-2.0-or-later), linked dynamically
(`pacman -S libisofs`, `apt install libisofs-dev`). A binary built with it falls under the GPL, so it is
an optional feature (`iso`, and `setup-cd`, which also needs `hive`); reading ISO images does not need it.

## Disk images and FAT

```
hvkit disk create boot.vhdx --size 64M --boot-code mbr.bin --part type=e,active,fat=16,label=CSMWRAP
hvkit disk create xp.vhdx --size 8G --part size=64M,type=ef,fat=16 --part type=7,active
hvkit disk info XP.vhdx            # partitions and their file systems
hvkit disk convert disk.raw disk.vhdx
hvkit fat cp boot.vhdx csmwrap.efi /EFI/BOOT/BOOTX64.EFI
hvkit fat put boot.vhdx extra/* -- /           # trees, keeping names and times
hvkit fat ls XP.vhdx:1 /WINDOWS -r
hvkit fat get XP.vhdx /WINDOWS/system32/config/system system.hiv
hvkit fat attrib dos.vhdx:2 /IO.SYS +h +s +r
hvkit fat bootcode dos.vhdx:2 floppy.img       # a DOS boot sector's code, keeping the BPB
```

Images are raw, or VHDX by their extension (`.vhdx`, `.avhdx`). A differencing VHDX is read through
its parents (found by the locator's relative path, next to it) and written only itself, so a
checkpoint's disk can be changed offline while the parents stay as they are. Writes are collected and
stored in runs, and pages of zeros stay unallocated, so new images are sparse. `IMAGE:N` is partition N
of the MBR; without it, the first partition, or the whole image when there is no MBR (a floppy or a
partition image). Partitions are formatted with the BPB's hidden sectors set to their start, as BIOS
boot code needs. FAT, long names included, comes from [fatfs](https://github.com/rafalh/rust-fatfs)
and VHDX from [vhdx-rs](https://github.com/inschrift-spruch-raum/vhdx-rs), both as forks with fixes
not yet upstream (file attributes and hidden sectors; Hyper-V's differencing disks and faster parent
reads).

## Building

Rust 1.85 or later (edition 2024). From this directory:

```
cargo build --release      # target/release/hvkit (needs hivex and libisofs, see above)
cargo install --path hvkit # the same into ~/.cargo/bin, where tools/xp-iso.sh finds it
cargo build --release --no-default-features   # without hives and ISO mastering: no C libraries, MIT only
cargo build --release --target x86_64-pc-windows-gnu --no-default-features   # hvkit.exe
```

## Tests

```
cargo test
```

The recipe and hive tests compare with the scripts' and reg.exe's output byte for byte. They need
Microsoft files, so they read them from the directory named by `HVKIT_TESTDATA` and do nothing
without it:

| `in/` | `expected/` (made by the script in migrate/) |
|---|---|
| `ntldr-zh`, `ntldr-en`, `ntldr-2k3`: I386\NTLDR of the zh-hans and en XP SP3 CDs and of Server 2003 SP2 | the same names, `Patch-Ntldr.ps1 -InFile in\X -OutFile expected\X` |
| `setupldr-zh`, `setupldr-en`, `setupldr-2k3`: I386\SETUPLDR.BIN of the same CDs | as above |
| `dmvsc.sys`: Integration Services 6.3.9600.16384 | `dmvsc.sys`, `Patch-Dmvsc.ps1` |
| `icsvc.dll`: Integration Services 6.3.9600.16384 | `icsvc-gsi.dll`, `Install-IcSvcGuestInterfacePatch` on a copy |
| `system-xpv1.hiv`, `system-xpvss.hiv`: SYSTEM hives of XP installations | `system-xpv1.reg`, `system-xpvss.reg`: `reg.exe export` of the hive loaded as HKLM\SPK, run as SYSTEM |
| `setupreg-in.hiv`: SETUPREG.HIV of the zh-hans XP SP3 CD; `setupreg.reg`: zhcd/build.sh's edits | `setupreg-in.reg` (export as above); `setupreg-out.hiv`: reg.exe's import of `setupreg.reg` into it, and its export `setupreg-out.reg` |

The setup CD builders were compared with the scripts they replace (the CSMWrap testbed's
zhcd/build.sh on the zh-hans XP SP3, WinLite and zh-hans Server 2003 R2 SP2 CDs, and tools/xp-iso.sh)
by building the same CDs both ways: the trees are identical except for SETUPREG.HIV, whose keys and
values are (hivex writes other bytes than reg.exe), and the first comment of WINNT.SIF; booted on
Hyper-V Generation 2, both CDs show the same screens up to the partition list. That needs gigabytes of
CDs and Microsoft files, so it is not part of `cargo test`.

Layout: `crates/formats` (PE images, byte patterns, setup text files, ISO 9660 reading, cabinets,
MBRs), `crates/recipes` (the patches), `crates/hive` (hivex and .reg files), `crates/iso` (libisofs),
`crates/disk` (raw and VHDX images, FAT), `crates/media` (the setup CDs), `hvkit` (the command line).
The design, including the steps still to come, is in the CSMWrap testbed's `docs/rust-toolkit-design.md`.
