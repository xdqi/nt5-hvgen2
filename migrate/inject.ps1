# Prepares a copy of an installed Windows XP SP3 x86 disk (Hyper-V Gen1 VM with the 6.3
# Integration Services installed) to boot on a Hyper-V Generation 2 VM through CSMWrap.
# Run elevated on the Hyper-V host (Mount-VHD and `reg load` need a full admin token). The VHDX
# must not be attached to a running VM.
#
#   inject.ps1 -Vhd work.vhdx -Storvsc storvsc.sys -Storport storport.sys -Hvfb hvfb.sys `
#              -BootIni boot.ini -Efi csmwrap.efi -Ini csmwrap.ini [-Dsdt dsdt.aml] [-Export after.reg]
#   inject.ps1 -Vhd work.vhdx -EfiOnly -Efi csmwrap.efi -Ini csmwrap.ini [-Dsdt dsdt.aml]
#
# What it does (the reasoning is in the comments at each step below):
#   files     system32\drivers: storvsc.sys (2012 R2 IC, Server 2003 storport miniport),
#             storport.sys + diskdump.sys (Server 2003 SP2 storport update KB943295, SP2QFE branch;
#             the SP2 RTM storport rejects the IC 6.3 storvsc with STATUS_REVISION_MISMATCH),
#             hvfb.sys (linear frame buffer display miniport), bootwait.sys (-Bootwait: holds the boot
#             until the VMBus SCSI boot disk has appeared)
#   boot.ini  replaced by -BootIni
#   EFI       \EFI\BOOT\BOOTX64.EFI = -Efi, \EFI\BOOT\csmwrap.ini = -Ini, \EFI\CSMWrap\dsdt.aml = -Dsdt
#   SYSTEM    ControlSet001 only (ControlSet002 = LastKnownGood stays as the Gen1 configuration):
#             storvsc service (boot start, SCSI miniport) + CriticalDeviceDatabase entry,
#             VMBusHID CriticalDeviceDatabase entry, storflt removed from the disk class filters,
#             hvfb service (boot start, Video) with Device0, CrashControl\AutoReboot = 0,
#             bootwait service (boot start, -Bootwait only).
param(
  [Parameter(Mandatory)] [string]$Vhd,
  [string]$Storvsc, [string]$Storport, [string]$Diskdump, [string]$Hvfb, [string]$Bootwait, [string]$BootIni,
  [string]$Efi, [string]$Ini, [string]$Dsdt,
  [string]$Export,
  [switch]$EfiOnly
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$HiveRoot = 'XPMIG'                      # HKLM\XPMIG while loaded
$CS = "$HiveRoot\ControlSet001"

function Need([string]$p) { if (-not $p -or -not (Test-Path -LiteralPath $p)) { throw "missing file: '$p'" } }

function Set-Reg([string]$Key, [string]$Name, $Value, [string]$Kind = 'String') {
  $k = [Microsoft.Win32.Registry]::LocalMachine.CreateSubKey($Key)
  try {
    $old = $k.GetValue($Name, $null, 'DoNotExpandEnvironmentNames')
    $k.SetValue($Name, $Value, [Microsoft.Win32.RegistryValueKind]$Kind)
    $fmt = { param($v) if ($v -is [array]) { '[' + ($v -join ', ') + ']' } else { "$v" } }
    $o = if ($null -eq $old) { '(none)' } else { & $fmt $old }
    "  HKLM\$Key : $Name = $(& $fmt $Value) ($Kind)  was $o"
  } finally { $k.Close() }
}

function Remove-RegValue([string]$Key, [string]$Name) {
  $k = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($Key, $true)
  if (-not $k) { return }
  try {
    $old = $k.GetValue($Name, $null)
    if ($null -ne $old) { $k.DeleteValue($Name); "  HKLM\$Key : deleted $Name (was [$($old -join ', ')])" }
  } finally { $k.Close() }
}

function Copy-Into([string]$Src, [string]$Dst) {
  Need $Src
  $d = Split-Path $Dst
  if (-not (Test-Path $d)) { New-Item -ItemType Directory -Force $d | Out-Null }
  if (Test-Path -LiteralPath $Dst) { Set-ItemProperty -LiteralPath $Dst -Name Attributes -Value 'Normal' }
  Copy-Item -Force -LiteralPath $Src -Destination $Dst
  "  $Dst <- $Src ($((Get-Item -LiteralPath $Dst).Length) bytes)"
}

function Hive-Header([string]$f) {
  $b = [System.IO.File]::ReadAllBytes($f)
  $seq1 = [BitConverter]::ToUInt32($b, 4); $seq2 = [BitConverter]::ToUInt32($b, 8)
  $maj = [BitConverter]::ToUInt32($b, 0x14); $min = [BitConverter]::ToUInt32($b, 0x18)
  "  hive $f : regf seq $seq1/$seq2 version $maj.$min" + $(if ($seq1 -ne $seq2) { '  !! sequence numbers differ (dirty hive)' } else { '' })
}

# --- mount ------------------------------------------------------------------------------------
$vm = Get-VM | ? { (Get-VMHardDiskDrive -VM $_ | ? { $_.Path -eq $Vhd }) -and $_.State -ne 'Off' }
if ($vm) { throw "$Vhd is attached to running VM $($vm.Name)" }
$disk = Mount-VHD -Path $Vhd -Passthru | Get-Disk
try {
  Start-Sleep -Seconds 2
  $part = Get-Partition -DiskNumber $disk.Number | ? { $_.Type -match 'FAT32|IFS|FAT' } | Select -First 1
  if (-not $part) { throw 'no FAT32 partition on the disk' }
  if (-not $part.DriveLetter -or $part.DriveLetter -eq "`0") {
    $part | Add-PartitionAccessPath -AssignDriveLetter; Start-Sleep -Seconds 1
    $part = Get-Partition -DiskNumber $disk.Number -PartitionNumber $part.PartitionNumber
  }
  $L = "$($part.DriveLetter):"
  "mounted $Vhd as disk $($disk.Number), partition $($part.PartitionNumber) = $L"
  if (-not (Test-Path "$L\WINDOWS\system32\config\system")) { throw "$L does not look like an XP system volume" }

  # --- CSMWrap on the XP partition ------------------------------------------------------------
  "EFI files:"
  if ($Efi)  { Copy-Into $Efi  "$L\EFI\BOOT\BOOTX64.EFI" }
  if ($Ini)  { Copy-Into $Ini  "$L\EFI\BOOT\csmwrap.ini" }
  if ($Dsdt) { Copy-Into $Dsdt "$L\EFI\CSMWrap\dsdt.aml" }
  if ($EfiOnly) { return }

  # --- driver files and boot.ini ----------------------------------------------------------------
  "driver files:"
  $drv = "$L\WINDOWS\system32\drivers"
  if ($Storvsc)  { Copy-Into $Storvsc  "$drv\storvsc.sys" }
  if ($Storport) { Copy-Into $Storport "$drv\storport.sys" }
  if ($Diskdump) { Copy-Into $Diskdump "$drv\diskdump.sys" }
  if ($Hvfb)     { Copy-Into $Hvfb     "$drv\hvfb.sys" }
  if ($Bootwait) { Copy-Into $Bootwait "$drv\bootwait.sys" }
  if ($BootIni) {
    "boot.ini:"
    $bi = "$L\boot.ini"
    if (Test-Path -LiteralPath $bi) { Set-ItemProperty -LiteralPath $bi -Name Attributes -Value 'Normal' }
    $text = (Get-Content -Raw -LiteralPath $BootIni) -replace "`r?`n", "`r`n"
    [System.IO.File]::WriteAllText($bi, $text, [System.Text.Encoding]::ASCII)
    Set-ItemProperty -LiteralPath $bi -Name Attributes -Value 'Hidden, System'
    Get-Content -LiteralPath $bi | % { "  $_" }
  }

  # --- SYSTEM hive ------------------------------------------------------------------------------
  $hive = "$L\WINDOWS\system32\config\system"
  "SYSTEM hive before:"; Hive-Header $hive
  $before = @(Get-ChildItem "$L\WINDOWS\system32\config" -Force | % Name)
  & reg.exe load "HKLM\$HiveRoot" $hive | Out-Null
  if ($LASTEXITCODE) { throw "reg load failed ($LASTEXITCODE)" }
  try {
    $cur = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey("$HiveRoot\Select").GetValue('Current')
    if ($cur -ne 1) { throw "Select\Current is $cur, expected 1" }
    "registry (HKLM\$HiveRoot = the XP SYSTEM hive):"
    $cddb = "$CS\Control\CriticalDeviceDatabase"
    $svc = "$CS\Services"

    # Boot storage: VMBus SCSI controller -> storvsc (storport miniport). vmbus, Wdf01000 are
    # already boot start on the Gen1 install; winhv/vmbkmcl/WdfLdr/storport are export drivers
    # that NTLDR loads as imports of the boot drivers.
    Set-Reg "$svc\storvsc" 'Type' 1 DWord
    Set-Reg "$svc\storvsc" 'Start' 0 DWord
    Set-Reg "$svc\storvsc" 'ErrorControl' 1 DWord
    Set-Reg "$svc\storvsc" 'Group' 'SCSI miniport'
    Set-Reg "$svc\storvsc" 'ImagePath' 'system32\DRIVERS\storvsc.sys' ExpandString
    Set-Reg "$svc\storvsc" 'DisplayName' 'Microsoft Hyper-V SCSI Controller'
    Set-Reg "$svc\storvsc\Parameters" 'BusType' 10 DWord                     # storvsc.inf bus_type_sas
    Set-Reg "$svc\storvsc\Parameters\Device" 'EnableQueryAccessAlignment' 1 DWord   # pnpsafe_pci_addreg
    Set-Reg "$cddb\vmbus#{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f}" 'Service' 'storvsc'
    Set-Reg "$cddb\vmbus#{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f}" 'ClassGUID' '{4D36E97B-E325-11CE-BFC1-08002BE10318}'
    # ACPI\VMBus -> vmbus already exists (Gen1 install); written again so the script also works
    # on an install that never saw the device.
    Set-Reg "$cddb\acpi#vmbus" 'Service' 'vmbus'
    Set-Reg "$cddb\acpi#vmbus" 'ClassGUID' '{4D36E97D-E325-11CE-BFC1-08002BE10318}'
    # Keyboard (exists on the Gen1 install) and the synthetic mouse (HID over VMBus).
    Set-Reg "$cddb\vmbus#{f912ad6d-2b17-48ea-bd65-f927a61c7684}" 'Service' 'hyperkbd'
    Set-Reg "$cddb\vmbus#{f912ad6d-2b17-48ea-bd65-f927a61c7684}" 'ClassGUID' '{4D36E96B-E325-11CE-BFC1-08002BE10318}'
    Set-Reg "$cddb\vmbus#{cfa8b69e-5b4a-4cc0-b98b-8ba1a1f3f95a}" 'Service' 'VMBusHID'
    Set-Reg "$cddb\vmbus#{cfa8b69e-5b4a-4cc0-b98b-8ba1a1f3f95a}" 'ClassGUID' '{745A17A0-74D3-11D0-B6FE-00A0C90F57DA}'

    # storflt (Hyper-V IDE "storage accelerator") is a class lower filter below every disk on the
    # Gen1 install; Gen2 has no emulated IDE, so keep it out of the boot disk's stack.
    Remove-RegValue "$CS\Control\Class\{4D36E967-E325-11CE-BFC1-08002BE10318}" 'LowerFilters'
    Set-Reg "$svc\storflt" 'Start' 4 DWord

    # hvfb: legacy VideoPort miniport, as hvfb.inf installs it, starting at 1024x768x32.
    Set-Reg "$svc\hvfb" 'Type' 1 DWord
    Set-Reg "$svc\hvfb" 'Start' 1 DWord
    Set-Reg "$svc\hvfb" 'ErrorControl' 0 DWord
    Set-Reg "$svc\hvfb" 'Group' 'Video'
    Set-Reg "$svc\hvfb" 'ImagePath' 'system32\DRIVERS\hvfb.sys' ExpandString
    Set-Reg "$svc\hvfb" 'DisplayName' 'hvfb frame buffer display miniport'
    Set-Reg "$svc\hvfb\Device0" 'InstalledDisplayDrivers' ([string[]]@('framebuf')) MultiString
    Set-Reg "$svc\hvfb\Device0" 'VgaCompatible' 0 DWord
    Set-Reg "$svc\hvfb\Device0" 'Device Description' 'Linear frame buffer display (VBE / Hyper-V Gen2)'
    Set-Reg "$svc\hvfb\Device0" 'DefaultSettings.XResolution' 1024 DWord
    Set-Reg "$svc\hvfb\Device0" 'DefaultSettings.YResolution' 768 DWord
    Set-Reg "$svc\hvfb\Device0" 'DefaultSettings.BitsPerPel' 32 DWord
    Set-Reg "$svc\hvfb\Device0" 'DefaultSettings.VRefresh' 60 DWord

    # bootwait: boot-start helper whose boot driver reinitialization routine waits (up to
    # TimeoutSeconds) for the boot partition. vmbus.sys reports the SCSI controller from a work
    # item bound to CPU 0, which the boot thread (priority 31) holds until IopMarkBootPartition.
    if ($Bootwait) {
      Set-Reg "$svc\bootwait" 'Type' 1 DWord
      Set-Reg "$svc\bootwait" 'Start' 0 DWord
      Set-Reg "$svc\bootwait" 'ErrorControl' 0 DWord
      Set-Reg "$svc\bootwait" 'ImagePath' 'system32\DRIVERS\bootwait.sys' ExpandString
      Set-Reg "$svc\bootwait" 'DisplayName' 'Wait for the boot disk'
      Set-Reg "$svc\bootwait\Parameters" 'TimeoutSeconds' 30 DWord
    }

    # Keep a bugcheck on screen/in KD instead of rebooting into a loop.
    Set-Reg "$CS\Control\CrashControl" 'AutoReboot' 0 DWord

    if ($Export) { & reg.exe export "HKLM\$HiveRoot" $Export /y | Out-Null; "exported hive to $Export" }
  } finally {
    [gc]::Collect(); [gc]::WaitForPendingFinalizers()
    & reg.exe unload "HKLM\$HiveRoot" | Out-Null
    if ($LASTEXITCODE) { throw "reg unload failed ($LASTEXITCODE)" }
  }
  "SYSTEM hive after:"; Hive-Header $hive
  # Transaction/log files the host's registry created next to the hive; XP has no use for them.
  Get-ChildItem "$L\WINDOWS\system32\config" -Force | ? { $before -notcontains $_.Name } |
    % { Remove-Item -Force -LiteralPath $_.FullName; "  removed host artefact $($_.Name)" }
} finally {
  Dismount-VHD -Path $Vhd
  "dismounted $Vhd"
}
