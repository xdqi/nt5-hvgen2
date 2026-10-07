# Changing an installed system offline

```
hvkit migrate xp.vhdx --files resources --files kb943295 [--efi csmwrap.efi] [--debug]
    [--keep-paging-executive] [--no-dynamic-memory] [--no-vss] [--no-gsi] [--vmbaud] [--force] [--check]
hvkit migrate --check --files resources ...   # the files alone
hvkit inject xp.vhdx --files out          # XP: Dynamic Memory, VSS, Guest Service Interface, SynthVid
hvkit inject xp.vhdx:1 --files out --no-synthvid --vmbaud
```

Both write into a FAT volume of a raw or VHDX image (partition N with `IMAGE:N`, by default the one
with `\WINDOWS\system32\config\system`); no Mount-VHD, no `reg load`, no administrator. The VM must
be off. Everything is checked and prepared first; a failed check writes nothing. `--files` may be
given several times; the first directory that has a file wins. Hives whose log was not written back
(an unclean shutdown) need `--force`.

## migrate

Makes the XP SP3 x86 of a Generation 1 VM with the Integration Services 6.3 (on FAT32) boot on
Generation 2 through CSMWrap; `migrate/Convert-XPToGen2.ps1` runs it on its copy of the VM's disk.
Then the components of inject, with the same defaults and switches, except SynthVid's patch (Plug
and Play reinstalls SynthVid's files on the first Generation 2 boot). The reasons are in
`crates/media/src/migrate.rs`.

- Checks: FAT32, ntoskrnl.exe 5.1.2600.5512 or later, vmbus.sys 6.3.9600; the files below by
  version (storvsc 6.3.9600.16384, storport and diskdump 5.2.3790.4163 of the SP2 QFE branch;
  others with `--force`), csmwrap.efi (x64 EFI application) and dsdt.aml (checksum).
- Files: `\EFI\BOOT\BOOTX64.EFI` (`--efi` or csmwrap.efi), `\EFI\BOOT\csmwrap.ini`,
  `\EFI\CSMWrap\dsdt.aml`; storvsc.sys, storport.sys, diskdump.sys, hvfb.sys, bootwait.sys in
  system32\drivers; bootvid.dll in system32 and dllcache (XP's kept once as bootvid.xp). storvsc.sys
  (and dmvsc.sys, dmvscres.dll) come from `--files`, `--ic` or the volume's Integration Services
  folder.
- SYSTEM, current control set: storvsc (boot start) and Critical Device Database entries for the
  SCSI controller, VMBus, keyboard and mouse; SynthVid bound but disabled until Plug and Play
  installs it on the first boot; storflt off; hvfb with a fixed VideoID and 1024x768x32 in the
  hardware profile; bootwait with `RepairStorvsc` and names for the Activation and Remote Desktop
  devices; `Memory Management\DisablePagingExecutive` = 1 (keeps XP's Msfs.sys from a bug check
  on the first boot), unless `--keep-paging-executive`.
- `--debug`: CSMWrap's log on COM1, a default boot.ini entry with the kernel debugger on COM2,
  `CrashControl\AutoReboot` = 0.

## inject

Adds the components of [setup-cd](setup-cd.md#setup-cd), with the same defaults and switches, to an
XP or Server 2003 that already boots on Generation 2 and has the Integration Services 6.3.

The Integration Services' INFs are installed already and their NULL drivers stay. Instead, the
services, Dynamic Memory's Critical Device Database entry and the bootwait entries that keep them
(`Parameters\Devices`, `Parameters\Values`) are written into the registry. A bootwait.sys too old
for those tables is replaced by the one in `--files`. Running inject again changes nothing.

dmvsc.sys and dmvscres.dll come from `--files`, `--ic` or the system's Program Files copy of the
Integration Services, icsvc.dll and the SynthVid files from the system. A file a recipe does not
know stops inject before anything is written; the message names the switch that leaves it out.
vmbaud goes to `\Drivers\HV\vmbaud`, which is added to DevicePath; predev cannot run offline, so the
sound card's first appearance brings the Found New Hardware wizard.

Not solved yet: the SynthVid patch is undone on the disk's first Gen2 boot, when Plug and Play
reinstalls SynthVid from its INF's source files (setupapi.log).
