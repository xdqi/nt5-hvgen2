Windows XP on Hyper-V Generation 2: converting an installed XP
==============================================================

This package converts the disk of a Windows XP that runs on a Hyper-V
Generation 1 VM into a new disk that boots on a Generation 2 VM. The boot
goes through CSMWrap (a legacy BIOS for UEFI machines) and the nt5-hvgen2
drivers. The original disk is not changed.

Requirements
------------
- A Windows host with Hyper-V and its PowerShell module, and an
  administrator account.
- The XP disk (.vhd, .vhdx, or a checkpoint's .avhdx):
  - Windows XP Professional SP3, 32-bit;
  - the Hyper-V Integration Services of Windows Server 2012 R2 (6.3.9600)
    installed while the VM was Generation 1;
  - the Windows partition formatted as FAT32 (NTFS is not supported yet);
  - the VM shut down, not saved or paused.
- Unless the resources folder already holds them: the Hyper-V guest
  additions ISO (vmguest.iso of a Windows Server 2012 R2 Hyper-V host) or
  the KB943295 package for Windows Server 2003 x86
  (WindowsServer2003-KB943295-x86-*.exe). The converter takes the updated
  storport.sys and diskdump.sys from it.

Usage
-----
Drag the XP disk onto Convert-XPToGen2.cmd, or double-click it and enter
the path. Confirm the elevation prompt. The new disk is written next to the
original as <name>-gen2.vhdx, with a log in <name>-gen2.vhdx.log.

From PowerShell (elevated):

  .\Convert-XPToGen2.ps1 -Source D:\VMs\XP.vhdx -VMName "XP Gen2"
  .\Convert-XPToGen2.ps1 -Source D:\VMs\XP.vhdx -VmGuestIso C:\Windows\System32\vmguest.iso

Options:
  -Destination <file.vhdx>  where to write the new disk
  -VMName <name>            also create the Generation 2 VM (not started)
  -SwitchName <switch>      give that VM a network adapter on this switch
  -ProcessorCount <n>       processors of the VM, default 4 (XP sees n-1:
                            CSMWrap keeps one for itself)
  -MemoryStartupBytes <n>   memory of the VM, default 2GB (static)
  -VmGuestIso <iso>         where to find KB943295 (see Requirements)
  -Kb943295 <exe>           the KB943295 package itself
  -Resources <dir>          folder with the files to install, default:
                            the resources folder next to the script
  -Efi <csmwrap.efi>        use this CSMWrap build
  -Debug                    CSMWrap log on COM1, kernel debugger on COM2,
                            no automatic restart after a blue screen
  -Force                    overwrite the destination; accept other
                            versions of the Microsoft files

To find the disk of a checkpoint, for example "hvintegration" of the VM
"XPv1":

  (Get-VMSnapshot -VMName XPv1 -Name hvintegration | Get-VMHardDiskDrive).Path

A VM of your own needs: Generation 2, Secure Boot off, at least 2
processors, static memory, the new disk as the first boot device.

First boot
----------
The first boot shows the desktop at 1024x768 through the frame buffer
driver (hvfb). XP installs the new Hyper-V devices and asks for a restart;
answer Yes. From the second boot on, the Hyper-V Video driver of the
Integration Services is the display: it starts at 640x480, and Display
Properties offers modes up to 1600x1200 at 16 bits per pixel.

If the original VM was checkpointed or turned off while XP was running,
the first boot also shows "Windows did not start successfully" once; it
continues normally after 30 seconds.

Known limits
------------
- FAT32 system partitions only.
- XP installs the Integration Services' placeholder driver on the SCSI
  controller, which bootwait.sys replaces with the working one on every
  boot. Device Manager shows the controller's real name from the second
  boot on; the first one still says "(not supported)".
- Networking works (the Hyper-V network adapter of the Integration Services;
  with -SwitchName the VM gets one).
- Remote Desktop Virtualization is "(not supported)" in Device Manager: the
  Integration Services have no XP driver for it. The Activation component
  and the Remote Desktop channels of an Enhanced Session, which XP has no
  driver for either, get the names and the class "System" of Windows 8's
  INF, from the second boot on.
- Backup (volume shadow copy) and Dynamic Memory do not work.
- Windows may ask for activation again after the hardware change.
- "Last Known Good Configuration" is the Generation 1 configuration and
  does not boot on Generation 2.

What it changes on the copy
---------------------------
- \EFI\BOOT\BOOTX64.EFI (CSMWrap), \EFI\BOOT\csmwrap.ini and
  \EFI\CSMWrap\dsdt.aml on the XP partition.
- system32\drivers: storvsc.sys (Hyper-V SCSI), storport.sys and
  diskdump.sys (KB943295), hvfb.sys (frame buffer display), bootwait.sys
  (waits for the boot disk).
- system32\bootvid.dll (boot screen on the frame buffer); XP's own copy is
  kept as bootvid.xp.
- system32\icsvc.dll of the Integration Services is patched so that the
  Guest Service Interface works: Copy-VMFile -FileSource Host copies files
  into the VM (they are written as SYSTEM). The original is kept as
  icsvc.dll.orig. The VM gets that integration service turned on.
- The registry entries for these drivers in the current control set, and
  the Hyper-V storage filter (storflt) is turned off.
