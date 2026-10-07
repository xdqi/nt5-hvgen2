# Setup CDs and ISO images

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
so enable them before installing; the sound card's only while vmbaud-host.ps1 or vmbaudtray runs.
A device that first appears after setup would get XP's Found New Hardware wizard, as Plug and Play's
non-interactive install refuses unsigned files whatever the signing policy; so for the sound card and
the Guest Service Interface (both have fixed instance GUIDs) the CD runs `predev.exe` (from
`--files`, nt5-hvgen2's guest/predev) from `$OEM$\cmdlines.txt` near the end of GUI-mode setup: it
creates their device nodes and installs their drivers on them, so they start without a wizard when
they appear (a node already installed during setup is left alone). The edited INFs no longer match
their catalogs; the CD sets `DriverSigningPolicy=Ignore`.

Both read the source CD only, keep its
extracted copy in `~/.cache/hvkit/cd`, and write CDs like Microsoft's: ISO 9660 names without `;1`,
Joliet, no Rock Ridge (it can push SETUPLDR.BIN past the 128 sectors of `\I386` the CD boot sector
reads, which they check), the boot image hidden from the ISO 9660 tree. An existing output file is
rewritten in place, so it keeps the ACL Hyper-V gives the VM that has it in its DVD drive.

Mastering uses [libisofs](https://dev.lovelyhq.com/libburnia/libisofs) (GPL-2.0-or-later), linked dynamically
(`pacman -S libisofs`, `apt install libisofs-dev`). A binary built with it falls under the GPL, so it is
an optional feature (`iso`, and `setup-cd`, which also needs `hive`); reading ISO images does not need it.
