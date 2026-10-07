# Setup CDs and ISO images

```
hvkit setup-cd XP.iso OUT.iso --files DIR --ic DIR [--mp-source XP-SAME-BUILD.iso] [--kd] [--unattend ...]
    [--no-dynamic-memory] [--no-vss] [--no-gsi] [--no-synthvid] [--vmbaud]
hvkit csmwrap-cd OUT.iso --efi csmwrap.efi [--ini csmwrap.ini] [--dsdt dsdt.aml] [--put SRC=/DEST]
hvkit hvfb-cd XP.iso OUT.iso --hvfb out/hvfb.sys [--bootvid out/bootvid.dll] [--default-mode 1024x768x32]
hvkit iso info CD.iso       # volume id, El Torito entry, where SETUPLDR.BIN's record is in \I386
hvkit iso extract CD.iso DIR [--boot-image boot.img]
hvkit iso ls CD.iso         # files with their first block (to map a disk trace's LBAs to files)
hvkit iso build DIR OUT.iso --volume-id ID [--nt5-setup /boot.img] [--efi /esp.img]
```

`hvkit help setup-cd` lists all options.

## setup-cd

Makes an XP SP3 or Server 2003 SP2 CD (nLite'd ones too) that installs on Hyper-V Generation 2:
KMDF, the VMBus, storvsc with the KB943295 storport, bootwait, the synthetic keyboard, hvfb and
bootvid, the NTLDR recipe, the multiprocessor HAL (from `--mp-source` if nLite removed it) and the
Integration Services' INFs for GUI-mode setup. `--files` holds hvfb.sys, bootwait.sys,
wdf01000.sys, wdfldr.sys, vmbus.sys, winhv.sys, vmbkmcl.sys, storvsc.sys, storport.sys,
hyperkbd.sys, bootvid.dll, storvsc-xp.inf, and for the components below mdlex.sys, predev.exe,
vmbaud.inf and vmbaud.sys; the Microsoft files among them come from the user. `--ic` holds the
Integration Services 6.3 packages (vmbus, synthkbd, vmbushid, vmbusvideo, vmic, netvsc, dmvsc), e.g.
an XP installation's `Program Files\Hyper-V Integration Services`. The reason for each change is in
`crates/media/src/setup_cd.rs`.

Components with patched files (`crates/media/src/components.rs`), each left out with its `--no-...`:

| Component | Files | Versions |
|---|---|---|
| Dynamic Memory | dmvsc.sys patched, mdlex.sys | XP, 2003, XP x64 (x64 files) |
| Guest Service Interface (Copy-VMFile) | icsvcgsi.dll | XP, 2003 |
| SynthVid at 32 bpp, 56 modes | VMBusVideoM.sys, VMBusVideoD.dll | XP, 2003 |
| VSS service (production checkpoints) | icsvcvss.dll | XP (2003's works as it is) |
| Sound card, only with `--vmbaud` | vmbaud.inf, vmbaud.sys | |

The version comes from TXTSETUP.SIF. Dynamic Memory and the Guest Service Interface need Vista's
kernel and logon, so they fail on NT 5.x on a current host without the patches. The CD's copies of
dmvsc.inf and vmic.inf install them on XP the way they install on Server 2003, with the patched
files on both. The edited INFs no longer match their catalogs; the CD sets
`DriverSigningPolicy=Ignore`.

GUI-mode setup installs drivers only for devices that exist. Dynamic Memory's and the Guest Service
Interface's devices exist only when they are enabled on the VM (`Set-VMMemory -DynamicMemoryEnabled`,
`Enable-VMIntegrationService`), so enable them before installing. A device that first appears after
setup gets XP's Found New Hardware wizard, because Plug and Play's non-interactive install refuses
unsigned files whatever the signing policy. So for the sound card (present only while
vmbaud-host.ps1 or vmbaudtray runs) and the Guest Service Interface, which have fixed instance
GUIDs, the CD runs `predev.exe` (guest/predev) from `$OEM$\cmdlines.txt` near the end of GUI-mode
setup: it creates their device nodes and installs the drivers on them, so they start without a
wizard when they appear. A node installed during setup is left alone.

## csmwrap-cd

Makes a CD that boots CSMWrap: one El Torito entry, for UEFI, with a FAT12 image holding
`\EFI\BOOT\BOOTX64.EFI`, `csmwrap.ini` next to it and the DSDT as `\dsdt.aml`. CSMWrap reads them
from that CD; SeaBIOS passes over it (no BIOS entry) to the install CD and then the disk. The disk
needs no ESP: setup installs onto an empty disk and the system gets C:. Put the CD at a SCSI
location after the install CD's (which keeps D:), make it the VM's first boot device, and leave it
in: it is the VM's BIOS.

## hvfb-cd

Makes text-mode setup draw through hvfb.sys and installs hvfb as the new system's display driver;
it needs only the CD and our own drivers (e.g. for QEMU).

## Writing CDs

`setup-cd` and `hvfb-cd` only read the source CD, keep its extracted copy in `~/.cache/hvkit/cd`, and
write CDs like Microsoft's: ISO 9660 names without `;1`, Joliet, the boot image hidden from the ISO
9660 tree, no Rock Ridge (it can push SETUPLDR.BIN past the 128 sectors of `\I386` the CD boot sector
reads, which they check). An existing output file is rewritten in place, so it keeps the ACL Hyper-V
gave the VM that has it in a DVD drive. Mastering needs the `iso` feature (libisofs).
