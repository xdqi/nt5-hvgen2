# Moving an installed XP to Gen2

`migrate/Convert-XPToGen2.ps1` converts the disk of a Windows XP
Professional SP3 x86 that runs on a Gen1 VM with the Integration Services
of Windows Server 2012 R2 (6.3.9600) into a new disk for a Gen2 VM, and
with `-VMName` also creates the VM. It runs in Windows PowerShell on the
Hyper-V host, elevated; `migrate/Convert-XPToGen2.cmd` starts it from
Explorer. The source (`.vhd`, `.vhdx` or a checkpoint's `.avhdx`) is only
read. Usage, options and limits are in [migrate/README.txt](README.txt);
the comments at the top of the script list where each file comes from.

What the new disk gets (through `migrate/inject.ps1`):

- CSMWrap as `\EFI\BOOT\BOOTX64.EFI` on the XP partition, which therefore
  has to be FAT32, with `madt_pcat_compat = true` and this repository's
  DSDT (`acpi_dsdt`);
- storvsc.sys from the Integration Services and storport.sys/diskdump.sys
  from KB943295 (SP2 QFE branch; the SP2 RTM storport rejects this storvsc)
  as the boot storage stack, with CriticalDeviceDatabase entries for the
  VMBus devices XP needs before its first Gen2 logon;
- hvfb, bootvid.dll and bootwait (with `RepairStorvsc`, the device table for
  the Activation component and the Remote Desktop channels, and the value
  table for the Guest Service Interface's `ServiceDll`);
- `system32\icsvcgsi.dll`, a copy of the Integration Services' `icsvc.dll`
  patched by `migrate/IcSvcGuestInterface.ps1`, run by the
  `vmicguestinterface` service only, so that `Copy-VMFile` works. The
  service logs on `NT AUTHORITY\SYSTEM` for every file it receives, with an
  empty password (`LOGON32_LOGON_SERVICE`), which only Windows Vista and later
  allow; the patch hands it the service's own token instead, so the files
  are written as SYSTEM. The copy is separate because Plug and Play puts the
  original `icsvc.dll` back from the driver store on the first boot of a new
  VM, and the driver store's file cannot be patched (its catalog signature is
  checked); the VM gets that integration service turned on;
- with `-DynamicMemory`: `dmvsc.sys` patched by `migrate/Patch-Dmvsc.ps1`,
  `mdlex.sys` and the `dmvsc` service (see [drivers/mdlex](../drivers/mdlex/README.md)), and
  a VM with dynamic memory (minimum 512 MB, startup and maximum equal).

The Hyper-V Video driver (SynthVid) needs care on the first boot. The
Gen2 VMBus is a new parent device, so the video channel is a new device
node. Left alone, user-mode Plug and Play installed and started SynthVid in
the middle of the first session while hvfb drew the desktop; hvfb's drawing
then slowed to a crawl and the display watchdog stopped the system (0xEA in
`framebuf`). Bound through a CriticalDeviceDatabase entry alone, SynthVid
became `\Device\Video0` before its installation was finished and win32k
enabled no display driver at all (the screen kept showing autochk's
output). With the entry and the SynthVid service disabled, the first boot
runs on hvfb at 1024x768x32, Plug and Play installs SynthVid (its INF sets
the service back to demand start) and asks for a restart, and from the
second boot on SynthVid is the primary display with hvfb detached.

The Microsoft files are not part of this repository. The script takes
storvsc.sys from the disk itself and KB943295 from the package
(`-Kb943295`) or from vmguest.iso (`-VmGuestIso`), and checks their
versions. `tools/mkdist.sh` builds a package with the scripts and the
built files in `resources\`:

```
tools/mkdist.sh CSMWRAP_EFI=<release csmwrap.efi>            # out/dist/nt5-hvgen2-migrate.zip
tools/mkdist.sh CSMWRAP_EFI=<...> MS_DIR=<dir>               # ...-private.zip, needs nothing else
```

`MS_DIR` holds storvsc.sys, storport.sys and diskdump.sys; a package made
with it contains Microsoft files and is for private use only. The scripts
and text files in a package have CRLF line endings.
