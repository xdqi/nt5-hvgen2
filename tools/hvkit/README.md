# hvkit

One Rust tool for the binary patches and media this repository needs, replacing the scattered bash,
Python and PowerShell scripts step by step. So far it has the patch recipes for Microsoft files,
offline registry hives, ISO images, Windows setup CDs, disk images with FAT file systems, cabinets,
and the Windows 98 pieces of the Gen2 work (VxDs, setup's keyboard driver).

Any argument `@FILE` stands for the arguments in FILE, one per line, without quoting; blank lines and
lines starting with `#` are left out. Paths in it are taken as written, so such files use absolute
paths. That keeps the settings of one CD or disk in a file, e.g. `hvkit setup-cd @zh.args --kd`, where
zh.args holds `/path/XP.iso`, `/path/OUT.iso`, `--files=/path/files`, ... (an option and its value
either as `--opt=value` or on two lines).

## Patch recipes

```
hvkit patch --list
hvkit patch ntldr     I386/NTLDR          # NTLDR / SETUPLDR.BIN: menu highlight in single-plane mode 12h
hvkit patch dmvsc     dmvsc.sys -o out/dmvsc.sys   # rebind XP-missing imports to mdlex.sys
hvkit patch icsvc-gsi icsvc.dll -o icsvcgsi.dll    # Guest Service Interface on XP
hvkit patch icsvc-vss icsvc.dll -o icsvcvss.dll    # VSS (Backup) service on XP: production checkpoints
hvkit patch synthvid  VMBusVideoM.sys -o out/VMBusVideoM.sys   # SynthVid at 32 bpp, 56 modes:
hvkit patch synthvid  VMBusVideoD.dll -o out/VMBusVideoD.dll   #   both files, installed together
hvkit patch win98-keyboard KEYBOARD.DRV  # Win98 setup's keyboard driver: scancode from the BDA, not port 60h
hvkit patch ntldr     NTLDR --check       # only tell whether it is stock, patched or not patchable
```

Without `-o` the file is patched in place. Every recipe recognises a file it patched before and leaves
it alone, and refuses files it does not know how to patch. The recipes are ports of
`migrate/Patch-Ntldr.ps1`, `migrate/Patch-Dmvsc.ps1`, `migrate/IcSvcGuestInterface.ps1`,
`migrate/Patch-IcSvcVss.ps1` and the CSMWrap testbed's `vid32/patch.py --table` and
`w98/patch-kbd.py`, and give the same bytes; the scripts stay until the converter uses hvkit. The
patched SynthVid files no longer match the Integration Services catalog's signature. No Microsoft
file is in this repository: the recipes patch the user's own copies.

## Registry hives

```
hvkit hive info   SYSTEM                       # sequence numbers, version, dirty or not
hvkit hive export SYSTEM -o system.reg --root 'HKEY_LOCAL_MACHINE\XPMIG'   # = reg.exe export
hvkit hive export SYSTEM --key 'ControlSet001\Services\hvfb'              # to stdout, UTF-8
hvkit hive import SYSTEM edits.reg             # = reg.exe import with the hive loaded at the .reg's root
hvkit hive show   SYSTEM 'Services\\vmbus$' 'Control\\Video'           # regdump.py-style view
hvkit hive export 'xp.vhdx::\WINDOWS\system32\config\system' --utf8 -o system.txt
hvkit hive import 'xp.vhdx:1:\WINDOWS\system32\config\system' edits.reg   # in place, in the image
```

No `reg load`, no Windows, no administrator. `export` writes the file `reg.exe export` writes (UTF-16
with CR LF), byte for byte, including keys whose ACL keeps administrators out; `--utf8` gives the
text hive-dump.sh made of it. `import` takes REGEDIT4 and version 5.00 files, `[-key]` and `"v"=-`
deletions included; the root of the .reg file (by default the first two components of its first
key) stands for the hive's root key. It refuses a dirty hive (log not written back) without
`--force`. A hive can also be named inside a FAT volume of a disk image: `IMAGE:N:PATH` for
partition N, `IMAGE::PATH` for the one FAT partition that has PATH; `import` writes it back there.
Hives are read and written with [hivex](https://libguestfs.org/hivex.3.html) (LGPL-2.1),
linked dynamically: install it (`pacman -S hivex`, `apt install libhivex-dev`) or build without the
`hive` feature.

## Setup CDs and ISO images

```
hvkit hvfb-cd XP.iso OUT.iso --hvfb out/hvfb.sys [--bootvid out/bootvid.dll] [--default-mode 1024x768x32]
hvkit setup-cd XP.iso OUT.iso --files DIR --ic DIR [--mp-source XP-SAME-BUILD.iso] [--kd] [--unattend ...]
    [--no-dynamic-memory] [--no-vss] [--no-gsi] [--no-synthvid] [--vmbaud]
hvkit csmwrap-cd OUT.iso --efi csmwrap.efi [--ini csmwrap.ini] [--dsdt dsdt.aml] [--put SRC=/DEST]
hvkit iso info CD.iso       # volume id, El Torito entry, where SETUPLDR.BIN's record is in \I386
hvkit iso extract CD.iso DIR [--boot-image boot.img]
hvkit iso ls CD.iso         # files with their first block (to map a disk trace's LBAs to files)
hvkit iso build DIR OUT.iso --volume-id ID [--nt5-setup /boot.img] [--efi /esp.img]
```

`hvfb-cd` makes text-mode setup draw through hvfb.sys and installs it as the new system's display
driver; it needs nothing but the CD and our own drivers. `setup-cd` makes a CD that installs on
Hyper-V Generation 2: KMDF, the VMBus, storvsc with the KB943295 storport, bootwait, the synthetic
keyboard, hvfb, the NTLDR recipe, the multiprocessor HAL and the Integration Services' INFs for
GUI-mode setup; `--files` holds those drivers (Microsoft files among them, so they come from the
user) and mdlex.sys, `--ic` the Integration Services packages.

`csmwrap-cd` makes a CD that boots CSMWrap: one El Torito entry for UEFI with a FAT12 image holding
`\EFI\BOOT\BOOTX64.EFI`, `csmwrap.ini` next to it and the DSDT as `\dsdt.aml`. CSMWrap reads them from
that CD, and SeaBIOS passes over it (no BIOS entry) to the install CD and then the disk, so the VM's
disk needs no ESP: setup installs onto an empty disk and the system gets C:. Make it the VM's first
boot device at a SCSI location after the install CD's (which then keeps D:), and leave it in.

On top of that come the components that need patched files (`crates/media/src/components.rs`):
Dynamic Memory (dmvsc.sys patched, mdlex.sys), the Guest Service Interface for Copy-VMFile
(icsvcgsi.dll) and SynthVid at 32 bpp with 56 modes on both versions, and on XP (TXTSETUP.SIF's
version) the VSS service for production checkpoints (icsvcvss.dll), which works on Server 2003 as
it is. The Integration Services were released for both, but their Dynamic Memory and Guest Service
Interface need Vista's kernel and logon, so neither works on NT 5.x on a current host without the
patches. The copies of dmvsc.inf and vmic.inf on the CD install them on XP the way they install the
stock files on Server 2003, with the patched files on both. `--no-...` leaves one out. `--vmbaud` adds the sound card (vmbaud.inf and
vmbaud.sys from `--files`). The devices of Dynamic Memory and the Guest Service Interface exist only
when they are enabled on the VM (`Set-VMMemory -DynamicMemoryEnabled`, `Enable-VMIntegrationService`),
so enable them before installing; the sound card's only while vmbaud-host.ps1 runs, which is why it
goes into DevicePath for later. The edited INFs no longer match their catalogs; the CD sets
`DriverSigningPolicy=Ignore`.

Both read the source CD only, keep its
extracted copy in `~/.cache/hvkit/cd`, and write CDs like Microsoft's: ISO 9660 names without `;1`,
Joliet, no Rock Ridge (it can push SETUPLDR.BIN past the 128 sectors of `\I386` the CD boot sector
reads, which they check), the boot image hidden from the ISO 9660 tree. An existing output file is
rewritten in place, so it keeps the ACL Hyper-V gives the VM that has it in its DVD drive.

Mastering uses [libisofs](https://dev.lovelyhq.com/libburnia/libisofs) (GPL-2.0-or-later), linked dynamically
(`pacman -S libisofs`, `apt install libisofs-dev`). A binary built with it falls under the GPL, so it is
an optional feature (`iso`, and `setup-cd`, which also needs `hive`); reading ISO images does not need it.

## Changing an installed system offline

```
hvkit inject xp.vhdx --files out          # XP: Dynamic Memory, VSS, Guest Service Interface, SynthVid
hvkit inject xp.vhdx:1 --files out --no-synthvid --vmbaud
```

The same components as on the setup CD, with the same defaults and switches, for an XP or Server 2003
on a FAT volume (default: the one with `\WINDOWS\system32\config\system`) that already boots on
Generation 2 and has the Integration Services 6.3; the VM must be off. The Integration Services'
INFs are installed there already, so their NULL drivers stay and the services, Dynamic Memory's
Critical Device Database entry and bootwait entries that keep them (Parameters\Devices,
Parameters\Values) are written into the registry, as `migrate/inject.ps1` does with `-Dmvsc -Mdlex
-VssPatch -GuestInterfacePatch`. Running it again changes nothing. dmvsc.sys and dmvscres.dll come
from `--ic` or the Integration Services' copy in the system's Program Files, icsvc.dll and the
SynthVid files from the system; a file the recipe does not know stops it before anything is written,
and the message names the switch that leaves it out. vmbaud goes to `\Drivers\HV\vmbaud`, added to
DevicePath.

## Disk images and FAT

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

Images are raw, or VHDX by their extension (`.vhdx`, `.avhdx`). A differencing VHDX is read through
its parents (found by the locator's relative path, next to it) and written only itself, so a
checkpoint's disk can be changed offline while the parents stay as they are. Writes are collected and
stored in runs, and pages of zeros stay unallocated, so new images are sparse. `IMAGE:N` is partition N
of the MBR; without it, the first partition, or the whole image when there is no MBR (a floppy or a
partition image). `--part ...,size=rest` reaches up to the next partition that has a `start=`, or to
the end of the disk; `start=end-SIZE` counts from the end. Partitions are formatted with the BPB's hidden sectors set to their start, as BIOS
boot code needs. FAT, long names included, comes from [fatfs](https://github.com/rafalh/rust-fatfs)
and VHDX from [vhdx-rs](https://github.com/inschrift-spruch-raum/vhdx-rs), both as forks with fixes
not yet upstream (file attributes and hidden sectors; Hyper-V's differencing disks and faster parent
reads).

## Cabinets

```
hvkit cab ls BASE5.CAB                    # with the rest of its set (linked cabinets next to it)
hvkit cab extract BASE5.CAB out vpicd.vxd vtd.vxd
hvkit cab create OUT.CAB files... [--set 0x9898] [--date 1999-05-05 --time 22:22:00]
hvkit cab create-set MINI.CAB MINI1.CAB --first 11 --set 0x6101 files...
```

Reading handles MSZIP, LZX and stored folders, and sets whose folders continue from cabinet to
cabinet (Windows 98's BASE5.CAB sits in the middle of a set of 77); it gives the same files as 7z for
all four sets on the Windows 98 SE CD. Writing makes MSZIP cabinets; `create-set` lays a set of two out
like Windows 98 setup's MINI.CAB/MINI1.CAB, whose extractor only moves on to the next cabinet where a
folder continues: the first `--first` files form a folder that continues into the second cabinet
inside the last of them, the other files a second folder of the second cabinet. Windows' expand.exe
treats such a set like the original one.

## Windows 98 VxDs

```
hvkit vxd info VPICD.VXD                  # header, objects, entries, DDB (FILE@0xOFF: an LE inside a W3)
hvkit vxd scan VPICD.VXD VTD.VXD          # port I/O to the PIC, PIT, port 61h, i8042
hvkit vxd show VPICD.VXD 1 17c0 17f0      # disassembly of part of an object, fixups marked
hvkit vxd patch-io VPICD.VXD out.vxd --vectors gen2leg/vectors.txt
hvkit vxd fix-entry gen2leg.vxd           # wlink's type 2 DDB export -> type 3
```

`patch-io` redirects VPICD's, VTD's and VKD's port I/O into the GEN2LEG shim VxD (an `int vv` per
port and direction, the vectors from the shim's build) and is the CSMWrap testbed's `w98/patch-io.py`,
with the same output; `fix-entry` is its `gen2leg-ow/fixentry.py`. The sweep uses iced-x86 instead
of ndisasm; it finds the same sites in all 266 VxDs of the CD except in data inside code objects.

The testbed's w98 harness can do the rest with the commands above: `prep.sh` = `hvkit iso extract
w98se.iso cd --boot-image bootfd.img`, `hvkit fat get bootfd.img /IO.SYS ...` and `hvkit cab extract
cd/win98/BASE5.CAB ...`; `mkmini.sh` = `hvkit cab extract`, `hvkit patch win98-keyboard`, `hvkit cab
create-set`; `mkdos.sh` = `hvkit disk create ... --part type=e,active,fat=16,bootcode=bootfd.img` and
`hvkit fat cp|put|attrib` (a disk made so boots the CD's DOS on Gen2 like one made by mkdos.sh);
`inst.sh` = `hvkit fat cp|rm|attrib` on the VHDX itself.

## Building

Rust 1.85 or later (edition 2024). From this directory:

```
cargo build --release      # target/release/hvkit (needs hivex and libisofs, see above)
cargo install --path hvkit # the same into ~/.cargo/bin
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
| `icsvc.dll` (the same file) | `icsvc-vss.dll`, `Patch-IcSvcVss.ps1` |
| `VMBusVideoM.sys`, `VMBusVideoD.dll`: Integration Services 6.3.9600.16384 | the same names, `vid32/patch.py in out --table` (CSMWrap testbed) |
| `dmvsc.inf`, `vmic.inf`: Integration Services 6.3.9600.16384 | none: the media tests check the CD's edits of them line by line |
| `system-xpv1.hiv`, `system-xpvss.hiv`: SYSTEM hives of XP installations | `system-xpv1.reg`, `system-xpvss.reg`: `reg.exe export` of the hive loaded as HKLM\SPK, run as SYSTEM |
| `keyboard.drv`: KEYBOARD.DRV of the zh-hans Windows 98 SE CD's MINI.CAB | `keyboard.drv`, `w98/patch-kbd.py` |
| `vpicd.vxd`, `vtd.vxd`, `vkd.vxd` (BASE5.CAB); `gen2leg-vectors.txt` (the vectors.txt used) | the same names, `w98/patch-io.py patch` |
| `wlink-type2.vxd`: a VxD with wlink's type 2 DDB entry | `wlink-type2.vxd`, `gen2leg-ow/fixentry.py` |
| `setupreg-in.hiv`: SETUPREG.HIV of the zh-hans XP SP3 CD; `setupreg.reg`: zhcd/build.sh's edits | `setupreg-in.reg` (export as above); `setupreg-out.hiv`: reg.exe's import of `setupreg.reg` into it, and its export `setupreg-out.reg` |

The setup CD builders were compared with the scripts they replace (the CSMWrap testbed's
zhcd/build.sh on the zh-hans XP SP3, WinLite and zh-hans Server 2003 R2 SP2 CDs, and tools/xp-iso.sh)
by building the same CDs both ways: the trees are identical except for SETUPREG.HIV, whose keys and
values are (hivex writes other bytes than reg.exe), and the first comment of WINNT.SIF; booted on
Hyper-V Generation 2, both CDs show the same screens up to the partition list. That needs gigabytes of
CDs and Microsoft files, so it is not part of `cargo test`.

Layout: `crates/formats` (PE, LE and NE images, byte patterns, setup text files, ISO 9660 reading,
cabinets, MBRs), `crates/recipes` (the patches), `crates/hive` (hivex and .reg files), `crates/iso` (libisofs),
`crates/disk` (raw and VHDX images, FAT), `crates/media` (the setup CDs), `hvkit` (the command line).
The design, including the steps still to come, is in the CSMWrap testbed's `docs/rust-toolkit-design.md`.
