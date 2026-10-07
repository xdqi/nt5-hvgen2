# Moving an installed XP to Gen2

`migrate/Convert-XPToGen2.ps1` turns the disk of an XP Professional SP3 x86 Gen1 VM with the Server
2012 R2 Integration Services (6.3.9600) into a new disk for a Gen2 VM and, with `-VMName`, creates
the VM. It runs elevated in Windows PowerShell on the Hyper-V host (`Convert-XPToGen2.cmd` starts it
from Explorer) and only reads the source (`.vhd`, `.vhdx` or a checkpoint's `.avhdx`). Usage,
options and limits are in [migrate/README.txt](README.txt); the comments at the top of the script
list where each file comes from.

## What the new disk gets

Through `migrate/inject.ps1`:

- CSMWrap as `\EFI\BOOT\BOOTX64.EFI` on the XP partition (which must therefore be FAT32), with
  `madt_pcat_compat = true` and this repository's DSDT (`acpi_dsdt`);
- boot storage: storvsc.sys from the Integration Services, storport.sys/diskdump.sys from KB943295
  (SP2 QFE branch; the SP2 RTM storport rejects this storvsc), and CriticalDeviceDatabase entries
  for the VMBus devices XP needs before its first Gen2 logon;
- hvfb, bootvid.dll and [bootwait](../drivers/bootwait/README.md) with `RepairStorvsc`, device
  table entries for the Activation component and the Remote Desktop channels, and a value table
  entry for the Guest Service Interface's `ServiceDll`;
- for `Copy-VMFile`, `system32\icsvcgsi.dll`: `icsvc.dll` patched by `hvkit patch icsvc-gsi` and
  run only by the `vmicguestinterface` service; the VM gets the Guest Service Interface turned on.
  The service logs on `NT AUTHORITY\SYSTEM` with an empty password (`LOGON32_LOGON_SERVICE`) for
  each received file, which only Vista and later allow; the patch hands it the service's own token,
  so files are written as SYSTEM. It is a separate copy because Plug and Play restores the original
  from the driver store on a new VM's first boot, and that file cannot be patched (its catalog
  signature is checked);
- with `-DynamicMemory`: `dmvsc.sys` patched by `hvkit patch dmvsc`, `mdlex.sys` and the `dmvsc`
  service ([drivers/mdlex](../drivers/mdlex/README.md)), and a VM with dynamic memory (minimum
  512 MB or the startup size if smaller, startup and maximum equal).

**SynthVid.** Under the new Gen2 VMBus parent, the Hyper-V Video (SynthVid) channel is a new device
node. The converter binds it to SynthVid through a CriticalDeviceDatabase entry and disables the
service: the first boot runs on hvfb at 1024x768x32 while Plug and Play installs SynthVid (its INF
sets demand start again) and asks for a restart; from the second boot on SynthVid is the primary
display and hvfb is detached. Left alone, Plug and Play started SynthVid mid-session and hvfb's
drawing slowed until the display watchdog stopped the system (0xEA in `framebuf`); with the
entry alone, SynthVid became `\Device\Video0` before it was installed and win32k enabled no
display driver.

## Microsoft files and the package

The Microsoft files are not in this repository: the script takes storvsc.sys from the disk and
KB943295 from its package (`-Kb943295`) or vmguest.iso (`-VmGuestIso`), and checks their versions.
`tools/mkdist.sh` builds a package of the scripts with the built files in `resources\`:

```
tools/mkdist.sh CSMWRAP_EFI=<release csmwrap.efi>            # out/dist/nt5-hvgen2-migrate.zip
tools/mkdist.sh CSMWRAP_EFI=<...> MS_DIR=<dir>               # ...-private.zip, needs nothing else
```

`MS_DIR` holds storvsc.sys, storport.sys and diskdump.sys; such a package contains Microsoft files
and is for private use only. The package's scripts and text files have CRLF line endings.
