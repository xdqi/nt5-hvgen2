# Convert-XPToGen2.ps1: turns the disk of an installed Windows XP SP3 x86 that runs on a Hyper-V
# Generation 1 VM with the 2012 R2 Integration Services (6.3.9600) into a new disk that boots on a
# Generation 2 VM through CSMWrap, and optionally creates that VM.
#
#   Convert-XPToGen2.ps1 -Source <xp.vhd|.vhdx|.avhdx> [-Destination <new.vhdx>]
#                        [-VMName <name> [-SwitchName <switch>] [-ProcessorCount 4] [-MemoryStartupBytes 2GB]]
#                        [-Resources <dir>] [-Kb943295 <exe> | -VmGuestIso <vmguest.iso>] [-Efi <csmwrap.efi>]
#                        [-Debug] [-Force]
#
# Run it elevated on the Hyper-V host; Convert-XPToGen2.cmd starts it and asks for elevation. The
# source is only read: Convert-VHD copies it (merging a checkpoint's .avhdx chain) into
# -Destination, by default <source name>-gen2.vhdx next to the source, and inject.ps1 changes
# only that copy. The XP system volume must be FAT32, because CSMWrap goes onto it and the Gen2
# firmware reads only FAT.
#
# Files it installs, each from the first place that has it:
#   csmwrap.efi                         -Efi, <Resources>
#   dsdt.aml                            <Resources>, <repository>\acpi\out
#   hvfb.sys bootwait.sys bootvid.dll   <Resources>, <repository>\out
#   storvsc.sys                         <Resources>, the disk's Hyper-V Integration Services folder
#   storport.sys diskdump.sys           <Resources>, the KB943295 package (-Kb943295), or the one
#                                       on vmguest.iso (-VmGuestIso, support\x86)
# <Resources> is -Resources, by default the resources folder next to this script; <repository> is
# the nt5-hvgen2 checkout this script is in, after `make` and `acpi/build.sh`.
# The Microsoft files are checked by version: storvsc 6.3.9600.16384, storport and diskdump
# 5.2.3790.4163 of the Server 2003 SP2 QFE branch (the SP2 RTM storport rejects this storvsc).
#
# -VMName: also create a Generation 2 VM with the new disk (Secure Boot off, static memory,
#          -ProcessorCount processors of which CSMWrap keeps one, no network adapter unless
#          -SwitchName). The VM is not started.
# -Debug:  CSMWrap logs to COM1 and to the screen, boot.ini gets a default entry with the kernel
#          debugger on COM2, a bug check stays on the screen, and the VM's COM1/COM2 go to the
#          pipes \\.\pipe\<VMName> and \\.\pipe\<VMName>-kd.
# -Force:  overwrite -Destination, and accept Microsoft files of other versions.
#
# The log is appended to <Destination>.log.
param(
  [string]$Source, [string]$Destination,
  [string]$VMName, [string]$SwitchName, [int]$ProcessorCount = 4, [long]$MemoryStartupBytes = 2GB,
  [string]$Resources, [string]$Kb943295, [string]$VmGuestIso, [string]$Efi,
  [switch]$Debug, [switch]$Force,
  [switch]$Pause    # from Convert-XPToGen2.cmd: ask for -Source, elevate, wait for a key at the end
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
if ($args) { throw "unknown arguments: $args" }

# The builds this was tested with (ENU KB943295); other languages of the same build differ in hash.
$KnownHashes = @{
  'storvsc.sys'  = 'ECD0071B7229BEB1CEC80A1F302A9864E35958AB7EF659780695E80A14B9E647'
  'storport.sys' = 'F4349AA615559618D6AB5F1A98505BD37C14F9C955503D4581A6FA3D46F5D20C'
  'diskdump.sys' = '2784AE321240287915A36F25FB032839DAB203433E963086CCCA12A7F905BDE1'
}

function Full([string]$p) { if ($p) { $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($p) } }

function Test-Admin {
  ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
}

# --- arguments and elevation ------------------------------------------------------------------
if ($Pause -and -not $Source) {
  $Source = (Read-Host 'XP disk to convert (.vhd, .vhdx or .avhdx; you can drag it into this window)').Trim().Trim('"')
}
if (-not $Source) {
  throw 'usage: Convert-XPToGen2.ps1 -Source <xp.vhd|.vhdx|.avhdx> [-Destination <new.vhdx>] [-VMName <name>] ... (see the comments at the top)'
}
$Source = Full $Source; $Destination = Full $Destination; $Resources = Full $Resources
$Kb943295 = Full $Kb943295; $VmGuestIso = Full $VmGuestIso; $Efi = Full $Efi

if (-not (Test-Admin)) {
  if (-not $Pause) { throw 'run this script elevated (as Administrator): Mount-VHD and loading the XP registry need it' }
  $argv = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-Pause',
            '-ProcessorCount', $ProcessorCount, '-MemoryStartupBytes', $MemoryStartupBytes)
  $named = [ordered]@{ Source = $Source; Destination = $Destination; VMName = $VMName; SwitchName = $SwitchName
                       Resources = $Resources; Kb943295 = $Kb943295; VmGuestIso = $VmGuestIso; Efi = $Efi }
  foreach ($k in $named.Keys) { if ($named[$k]) { $argv += "-$k"; $argv += "`"$($named[$k])`"" } }
  if ($Debug) { $argv += '-Debug' }
  if ($Force) { $argv += '-Force' }
  Start-Process -FilePath powershell.exe -Verb RunAs -ArgumentList ($argv -join ' ')
  return
}

$Here = Split-Path -Parent $PSCommandPath
$Repo = Split-Path -Parent $Here
if (-not $Resources) { $Resources = Join-Path $Here 'resources' }
if (-not $Destination) {
  # A checkpoint's disk is <name>_<GUID>.avhdx; the copy is named after <name>.
  $base = [IO.Path]::GetFileNameWithoutExtension($Source) -replace '_[0-9A-Fa-f]{8}(-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}$', ''
  $Destination = Join-Path (Split-Path -Parent $Source) "$base-gen2.vhdx"
}

# --- helpers -----------------------------------------------------------------------------------
function Step([string]$m) { ''; "== $m" }

function Find-First([string]$Name, [string[]]$Dirs) {
  foreach ($d in $Dirs) {
    if (-not $d) { continue }
    $p = Join-Path $d $Name
    if (Test-Path -LiteralPath $p -PathType Leaf) { return $p }
  }
}

function Assert-Version([string]$Path, [string]$Pattern) {
  $f = Get-Item -LiteralPath $Path
  $v = $f.VersionInfo.FileVersion
  $h = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
  $tested = if ($KnownHashes[$f.Name] -eq $h) { 'the tested build' } else { 'not the tested ENU build' }
  "  $($f.Name): $Path"
  "    version $v, SHA256 $h ($tested)"
  if ($v -notlike $Pattern) {
    $m = "$Path has version '$v', expected '$Pattern'"
    if (-not $Force) { throw "$m (-Force accepts it anyway)" }
    Write-Warning "$m; accepted because of -Force"
  }
}

function Assert-EfiApp([string]$Path) {
  $b = [IO.File]::ReadAllBytes($Path)
  $pe = if ($b.Length -gt 0x40) { [BitConverter]::ToInt32($b, 0x3c) } else { 0 }
  $ok = $b.Length -gt $pe + 0x60 -and $b[0] -eq 0x4d -and $b[1] -eq 0x5a -and
        [BitConverter]::ToUInt32($b, $pe) -eq 0x4550 -and [BitConverter]::ToUInt16($b, $pe + 4) -eq 0x8664 -and
        [BitConverter]::ToUInt16($b, $pe + 24) -eq 0x20b -and [BitConverter]::ToUInt16($b, $pe + 24 + 68) -eq 10
  if (-not $ok) { throw "$Path is not an x64 EFI application" }
  "  csmwrap.efi: $Path ($($b.Length) bytes)"
}

function Assert-Dsdt([string]$Path) {
  $b = [IO.File]::ReadAllBytes($Path)
  $sum = 0; foreach ($x in $b) { $sum = ($sum + $x) -band 0xff }
  if ($b.Length -lt 36 -or [Text.Encoding]::ASCII.GetString($b, 0, 4) -ne 'DSDT' -or
      [BitConverter]::ToUInt32($b, 4) -ne $b.Length -or $sum) { throw "$Path is not a valid DSDT" }
  "  dsdt.aml: $Path (revision $($b[8]), $($b.Length) bytes)"
}

# Extracts KB943295 (an update.exe package) and returns the folder with its SP2 QFE files.
function Expand-Kb943295([string]$Exe, [string]$Tmp) {
  $copy = Join-Path $Tmp (Split-Path -Leaf $Exe)
  Copy-Item -LiteralPath $Exe -Destination $copy -Force
  Unblock-File -LiteralPath $copy
  $out = Join-Path $Tmp 'kb943295'
  Write-Host "  extracting $Exe"
  $p = Start-Process -FilePath $copy -ArgumentList "/x:`"$out`"", '/quiet' -Wait -PassThru
  $qfe = Join-Path $out 'SP2QFE'
  if (-not (Test-Path -LiteralPath (Join-Path $qfe 'storport.sys'))) {
    throw "extracting $Exe (exit code $($p.ExitCode)) gave no SP2QFE\storport.sys"
  }
  $qfe
}

function Get-Kb943295FromIso([string]$Iso, [string]$Tmp) {
  $was = (Get-DiskImage -ImagePath $Iso).Attached
  $img = if ($was) { Get-DiskImage -ImagePath $Iso } else { Mount-DiskImage -ImagePath $Iso -PassThru }
  try {
    $dl = $null
    for ($i = 0; $i -lt 20 -and -not $dl; $i++) { $dl = ($img | Get-Volume).DriveLetter; if (-not $dl) { Start-Sleep -Milliseconds 500 } }
    if (-not $dl) { throw "$Iso has no drive letter after mounting" }
    $all = @(Get-ChildItem -LiteralPath "${dl}:\support\x86" -Filter 'WindowsServer2003-KB943295-x86-*.exe' -ErrorAction SilentlyContinue)
    $exe = @($all | ? Name -like '*-ENU.exe') + $all | Select -First 1
    if (-not $exe) { throw "$Iso has no support\x86\WindowsServer2003-KB943295-x86-*.exe" }
    Expand-Kb943295 $exe.FullName $Tmp
  } finally {
    if (-not $was) { Dismount-DiskImage -ImagePath $Iso | Out-Null }
  }
}

# Mounts the VHD and returns the drive letter (with colon) of the XP system volume.
function Mount-XPVolume([string]$Vhd) {
  $disk = Mount-VHD -Path $Vhd -Passthru | Get-Disk
  Start-Sleep -Seconds 2
  foreach ($part in Get-Partition -DiskNumber $disk.Number) {
    if (-not $part.DriveLetter -or $part.DriveLetter -eq "`0") {
      if ($part.Type -notmatch 'FAT|IFS') { continue }
      $part | Add-PartitionAccessPath -AssignDriveLetter; Start-Sleep -Seconds 1
      $part = Get-Partition -DiskNumber $disk.Number -PartitionNumber $part.PartitionNumber
    }
    $L = "$($part.DriveLetter):"
    if (Test-Path -LiteralPath "$L\WINDOWS\system32\config\system") { return $L }
  }
  throw "no partition of $Vhd has WINDOWS\system32\config\system"
}

# --- the conversion ----------------------------------------------------------------------------
function Convert-Disk([string]$Tmp) {
  Step 'checks'
  if (-not (Get-Command Mount-VHD -ErrorAction SilentlyContinue)) { throw 'the Hyper-V PowerShell module is missing' }
  if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) { throw "no such disk: $Source" }
  if ([IO.Path]::GetExtension($Destination) -ne '.vhdx') { throw "-Destination must end in .vhdx: $Destination" }
  if ($Source -eq $Destination) { throw '-Destination is the source' }
  $busy = @(Get-VM | ? State -ne 'Off' | % { $vm = $_; Get-VMHardDiskDrive -VM $vm | ? Path -eq $Source | % { $vm.Name } })
  if ($busy) { throw "$Source is in use by the running VM $($busy -join ', ')" }
  if ((Test-Path -LiteralPath $Destination) -and -not $Force) { throw "$Destination exists (-Force overwrites it)" }
  if ($VMName) {
    if (Get-VM -Name $VMName -ErrorAction SilentlyContinue) { throw "a VM named $VMName exists already" }
    if ($ProcessorCount -lt 2) { throw '-ProcessorCount must be at least 2: CSMWrap keeps one processor for itself' }
    if ($SwitchName -and -not (Get-VMSwitch -Name $SwitchName -ErrorAction SilentlyContinue)) { throw "no virtual switch named $SwitchName" }
  }
  "  source      $Source"
  "  destination $Destination"
  "  resources   $Resources"

  Step 'files to install'
  $f = @{}
  $missing = @()
  $f.efi = if ($Efi) { $Efi } else { Find-First 'csmwrap.efi' @($Resources) }
  if ($f.efi) { Assert-EfiApp $f.efi } else { $missing += 'csmwrap.efi: put it into the resources folder or pass -Efi (a release build of CSMWrap with Hyper-V Generation 2 support)' }
  $f.dsdt = Find-First 'dsdt.aml' @($Resources, (Join-Path $Repo 'acpi\out'))
  if ($f.dsdt) { Assert-Dsdt $f.dsdt } else { $missing += 'dsdt.aml: put it into the resources folder (nt5-hvgen2: acpi/build.sh)' }
  foreach ($n in 'hvfb.sys', 'bootwait.sys', 'bootvid.dll') {
    $f[$n] = Find-First $n @($Resources, (Join-Path $Repo 'out'))
    if ($f[$n]) { "  ${n}: $($f[$n])" } else { $missing += "${n}: put it into the resources folder (nt5-hvgen2: make)" }
  }
  $kb = Find-First 'storport.sys' @($Resources)
  if ($kb) { $kb = $Resources }
  elseif ($Kb943295) { $kb = Expand-Kb943295 $Kb943295 $Tmp }
  elseif ($VmGuestIso) { $kb = Get-Kb943295FromIso $VmGuestIso $Tmp }
  if ($kb) {
    foreach ($n in 'storport.sys', 'diskdump.sys') {
      $f[$n] = Find-First $n @($kb)
      if ($f[$n]) { Assert-Version $f[$n] '5.2.3790.4163 (srv03_sp2_qfe.*' } else { $missing += "${n}: not next to storport.sys in $kb" }
    }
  } else {
    $missing += 'storport.sys, diskdump.sys: pass -VmGuestIso <vmguest.iso of a 2012 R2 Hyper-V host> or -Kb943295 <WindowsServer2003-KB943295-x86-*.exe>, or put both files into the resources folder'
  }
  $f['storvsc.sys'] = Find-First 'storvsc.sys' @($Resources)
  if ($f['storvsc.sys']) { Assert-Version $f['storvsc.sys'] '6.3.9600.16384 *' } else { '  storvsc.sys: from the disk' }
  if ($missing) { throw "missing files:`n  " + ($missing -join "`n  ") }

  Step 'copying the disk'
  if (Test-Path -LiteralPath $Destination) { Remove-Item -LiteralPath $Destination -Force }
  $null = New-Item -ItemType Directory -Force (Split-Path -Parent $Destination)
  Convert-VHD -Path $Source -DestinationPath $Destination -VHDType Dynamic
  $script:Created = $true
  Get-VHD -Path $Destination | % { "  $($_.Path): $($_.VhdFormat) $($_.VhdType), $([math]::Round($_.FileSize / 1MB)) MB used of $([math]::Round($_.Size / 1GB, 1)) GB" }

  Step 'checking the XP installation'
  $L = Mount-XPVolume $Destination
  try {
    $fs = (Get-Volume -DriveLetter $L[0]).FileSystem
    "  system volume $L ($fs)"
    if ($fs -ne 'FAT32') {
      throw "the XP system volume is $fs; this version supports FAT32 only (the Gen2 firmware cannot read $fs, and there is no separate FAT partition for CSMWrap)"
    }
    $k = (Get-Item -LiteralPath "$L\WINDOWS\system32\ntoskrnl.exe").VersionInfo
    "  ntoskrnl.exe $($k.FileVersion)"
    if ($k.FileMajorPart -ne 5 -or $k.FileMinorPart -ne 1 -or $k.FileBuildPart -ne 2600 -or $k.FilePrivatePart -lt 5512) {
      throw 'this is not Windows XP SP3 (ntoskrnl.exe 5.1.2600.5512 or later)'
    }
    $vmbus = "$L\WINDOWS\system32\drivers\vmbus.sys"
    if (-not (Test-Path -LiteralPath $vmbus)) { throw 'no vmbus.sys: install the Hyper-V Integration Services (6.3.9600) on the Gen1 VM first' }
    $v = (Get-Item -LiteralPath $vmbus).VersionInfo.FileVersion
    "  vmbus.sys $v"
    if ($v -notlike '6.3.9600.*') { throw "vmbus.sys is $v; the Integration Services of Windows Server 2012 R2 (6.3.9600) are required" }
    if (-not $f['storvsc.sys']) {
      $ic = Get-ChildItem -LiteralPath "$L\" -Directory -Force | % { Join-Path $_.FullName 'Hyper-V Integration Services\storvsc\storvsc.sys' } |
            ? { Test-Path -LiteralPath $_ } | Select -First 1
      if (-not $ic) { throw "the disk has no Hyper-V Integration Services\storvsc\storvsc.sys; put storvsc.sys into the resources folder" }
      $f['storvsc.sys'] = Join-Path $Tmp 'storvsc.sys'
      Copy-Item -LiteralPath $ic -Destination $f['storvsc.sys']
      Assert-Version $f['storvsc.sys'] '6.3.9600.16384 *'
    }
  } finally {
    Dismount-VHD -Path $Destination
  }

  Step 'installing'
  $ini = Join-Path $Tmp 'csmwrap.ini'
  $lines = @('; CSMWrap configuration for Windows XP on Hyper-V Generation 2 (written by Convert-XPToGen2.ps1)',
             '; XP''s ACPI HALs need the PC-AT compatibility flag, and XP''s ACPI driver needs a DSDT it can parse.',
             'madt_pcat_compat = true', 'acpi_dsdt = \EFI\CSMWrap\dsdt.aml')
  if ($Debug) { $lines += 'serial = true', 'serial_port = 0x3f8', 'serial_baud = 115200', 'verbose = true' }
  [IO.File]::WriteAllText($ini, ($lines -join "`r`n") + "`r`n", [Text.Encoding]::ASCII)
  $inject = @{
    Vhd = $Destination; Storvsc = $f['storvsc.sys']; Storport = $f['storport.sys']; Diskdump = $f['diskdump.sys']
    Hvfb = $f['hvfb.sys']; Bootwait = $f['bootwait.sys']; RepairStorvsc = $true; GuestInterfacePatch = $true; HoldSynthVid = $true; Bootvid = $f['bootvid.dll']
    Efi = $f.efi; Ini = $ini; Dsdt = $f.dsdt
  }
  # VMBus devices that XP's Integration Services have no driver for (the INF installs nothing). They get
  # the names and the class of the Windows 8+ INF (wvmic.inf) through bootwait; an INF of our own would need
  # a signature, or XP shows the Found New Hardware wizard for it.
  $sys = '{4D36E97D-E325-11CE-BFC1-08002BE10318}'
  $inject.DeviceFix = @(
    @{ HardwareID = 'VMBUS\{3375baf4-9e15-4b30-b765-67acb10d607b}'; FriendlyName = 'Microsoft Hyper-V Activation Component'; ClassGUID = $sys; Class = 'System' },
    @{ HardwareID = 'VMBUS\{f8e65716-3cb3-4a06-9a60-1889c5cccab5}'; FriendlyName = 'Microsoft Hyper-V Remote Desktop Control Channel'; ClassGUID = $sys; Class = 'System' },
    @{ HardwareID = 'VMBUS\{f9e9c0d3-b511-4a48-8046-d38079a8830c}'; FriendlyName = 'Microsoft Hyper-V Remote Desktop Data Channel'; ClassGUID = $sys; Class = 'System' })
  if ($Debug) { $inject.DebugBootEntry = $true; $inject.NoAutoReboot = $true }
  & (Join-Path $Here 'inject.ps1') @inject

  if ($VMName) {
    Step "creating the VM $VMName"
    $vm = New-VM -Name $VMName -Generation 2 -MemoryStartupBytes $MemoryStartupBytes -VHDPath $Destination
    $script:CreatedVM = $true
    Get-VMNetworkAdapter -VM $vm | Remove-VMNetworkAdapter
    if ($SwitchName) { Add-VMNetworkAdapter -VM $vm -SwitchName $SwitchName }
    Enable-VMIntegrationService -VM $vm -Name 'Guest Service Interface'    # Copy-VMFile; works with the icsvc patch
    Set-VMMemory -VM $vm -DynamicMemoryEnabled $false
    Set-VMProcessor -VM $vm -Count $ProcessorCount
    Set-VMFirmware -VM $vm -EnableSecureBoot Off -FirstBootDevice (Get-VMHardDiskDrive -VM $vm)
    # Production checkpoints need VSS in the guest, which XP's Integration Services do not offer.
    Set-VM -VM $vm -CheckpointType Standard
    if ((Get-Command Set-VM).Parameters.ContainsKey('AutomaticCheckpointsEnabled')) { Set-VM -VM $vm -AutomaticCheckpointsEnabled $false }
    if ($Debug) {
      Set-VMComPort -VM $vm -Number 1 -Path "\\.\pipe\$VMName"
      Set-VMComPort -VM $vm -Number 2 -Path "\\.\pipe\$VMName-kd"
    }
    $vm = Get-VM -Name $VMName
    "  Generation $($vm.Generation), $($vm.ProcessorCount) processors, $($vm.MemoryStartup / 1MB) MB static memory, Secure Boot $((Get-VMFirmware -VM $vm).SecureBoot)"
    "  disk: $((Get-VMHardDiskDrive -VM $vm).Path)"
    "  network: $(if ($SwitchName) { $SwitchName } else { 'none' })"
    if ($Debug) { Get-VMComPort -VM $vm | % { "  $($_.Name): $($_.Path)" } }
  }

  Step 'done'
  "  $Destination"
  if ($VMName) { "  start the VM $VMName" } else { '  attach it to a Generation 2 VM (Secure Boot off, at least 2 processors, static memory) as its first boot device' }
  '  On the first boot XP finds new hardware and asks for a restart; after that it boots normally.'
}

$Tmp = Join-Path ([IO.Path]::GetTempPath()) ('Convert-XPToGen2-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$null = New-Item -ItemType Directory $Tmp
$Created = $false
$CreatedVM = $false
$failed = $false
$log = "$Destination.log"
$null = New-Item -ItemType Directory -Force (Split-Path -Parent $log)
Start-Transcript -LiteralPath $log -Append | Out-Null
try {
  Convert-Disk $Tmp
} catch {
  $failed = $true
  Write-Host "ERROR: $($_.Exception.Message)" -ForegroundColor Red
  if ($CreatedVM) {
    Remove-VM -Name $VMName -Force -ErrorAction SilentlyContinue
    Write-Host "removed the incomplete VM $VMName"
  }
  if ($Created) {
    if ((Get-VHD -Path $Destination -ErrorAction SilentlyContinue).Attached) { Dismount-VHD -Path $Destination }
    Remove-Item -LiteralPath $Destination -Force -ErrorAction SilentlyContinue
    Write-Host "removed the incomplete $Destination"
  }
  if (-not $Pause) { throw }
} finally {
  Remove-Item -LiteralPath $Tmp -Recurse -Force -ErrorAction SilentlyContinue
  Stop-Transcript | Out-Null
  Write-Host "log: $log"
  if ($Pause) { $null = Read-Host 'Press Enter to close' }
}
if ($failed) { exit 1 }
