# Convert-XPToGen2.ps1: turns the disk of an installed Windows XP SP3 x86 that runs on a Hyper-V
# Generation 1 VM with the 2012 R2 Integration Services (6.3.9600) into a new disk that boots on a
# Generation 2 VM through CSMWrap, and optionally creates that VM.
#
#   Convert-XPToGen2.ps1 -Source <xp.vhd|.vhdx|.avhdx> [-Destination <new.vhdx>]
#                        [-VMName <name> [-SwitchName <switch>] [-ProcessorCount 4] [-MemoryStartupBytes 2GB]]
#                        [-DynamicMemory] [-KeepPagingExecutive]
#                        [-Resources <dir>] [-Kb943295 <exe> | -VmGuestIso <vmguest.iso>] [-Efi <csmwrap.efi>]
#                        [-Debug] [-Force]
#
# Run it on the Hyper-V host as an administrator or a member of Hyper-V Administrators (the Hyper-V
# cmdlets need it); Convert-XPToGen2.cmd starts it and asks for elevation if need be. The source is
# only read: Convert-VHD copies it (merging a checkpoint's .avhdx chain) into -Destination, by
# default <source name>-gen2.vhdx next to the source, and `hvkit.exe migrate` changes only that copy,
# writing into the VHDX itself. The XP system volume must be FAT32, because CSMWrap goes onto it and
# the Gen2 firmware reads only FAT.
#
# hvkit.exe (nt5-hvgen2's hvkit, built for Windows; tools/mkdist.sh puts it into the package) comes
# from <Resources> or PATH. It finds the files to install in, in this order, <Resources>, KB943295
# (-Kb943295, or the package on vmguest.iso with -VmGuestIso, support\x86), and <repository>\out and
# <repository>\acpi\out; storvsc.sys, and with -DynamicMemory dmvsc.sys and dmvscres.dll, else come
# from the disk's Hyper-V Integration Services folder:
#   csmwrap.efi (or -Efi), dsdt.aml, hvfb.sys, bootwait.sys, bootvid.dll, storport.sys, diskdump.sys,
#   mdlex.sys (-DynamicMemory)
# <Resources> is -Resources, by default the resources folder next to this script; <repository> is
# the nt5-hvgen2 checkout this script is in, after `make` and `acpi/build.sh`.
# hvkit checks the Microsoft files by version: storvsc 6.3.9600.16384, storport and diskdump
# 5.2.3790.4163 of the Server 2003 SP2 QFE branch (the SP2 RTM storport rejects this storvsc).
#
# -VMName: also create a Generation 2 VM with the new disk (Secure Boot off, static memory unless
#          -DynamicMemory, -ProcessorCount processors of which CSMWrap keeps one, no network adapter
#          unless -SwitchName, the Guest Service Interface integration service on). The VM is not started.
# -DynamicMemory: make Hyper-V Dynamic Memory work in the guest (Microsoft's dmvsc.sys, patched, plus
#          mdlex.sys; see README.md) and, with -VMName, turn it on for the VM: minimum 512 MB, startup and
#          maximum -MemoryStartupBytes. XP can only give memory back to the host (balloon), not add RAM, so
#          the VM never grows above its startup memory. Unused memory is taken back a minute or two after
#          the driver has started, not at once.
# -KeepPagingExecutive: leave Memory Management's DisablePagingExecutive as it is. By default it is set
#          to 1, which keeps XP's Msfs.sys from a bug check (0xD3) on the first Gen2 boot; a VM that
#          tests drivers may want paged kernel code, which shows "paged code at raised IRQL" bugs.
# -Debug:  CSMWrap logs to COM1 and to the screen, boot.ini gets a default entry with the kernel
#          debugger on COM2, a bug check stays on the screen, and the VM's COM1/COM2 go to the
#          pipes \\.\pipe\<VMName> and \\.\pipe\<VMName>-kd.
# -Force:  overwrite -Destination, and accept Microsoft files of other versions where only the version is
#          checked (storvsc, storport, diskdump, dmvscres). The patches for icsvc.dll and dmvsc.sys are only
#          for 6.3.9600.16384 and refuse any other file, with or without -Force.
#
# The log is appended to <Destination>.log.
param(
  [string]$Source, [string]$Destination,
  [string]$VMName, [string]$SwitchName, [int]$ProcessorCount = 4, [long]$MemoryStartupBytes = 2GB,
  [string]$Resources, [string]$Kb943295, [string]$VmGuestIso, [string]$Efi,
  [switch]$DynamicMemory, [switch]$KeepPagingExecutive, [switch]$Debug, [switch]$Force,
  [switch]$Pause    # from Convert-XPToGen2.cmd: ask for -Source, elevate, wait for a key at the end
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
if ($args) { throw "unknown arguments: $args" }

function Full([string]$p) { if ($p) { $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($p) } }

# The Hyper-V cmdlets need an administrator or a member of Hyper-V Administrators.
function Test-HyperVAccess {
  $p = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
  $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -or
    $p.IsInRole([Security.Principal.SecurityIdentifier]'S-1-5-32-578')
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

if (-not (Test-HyperVAccess)) {
  if (-not $Pause) { throw 'run this script as an administrator (elevated) or as a member of Hyper-V Administrators: the Hyper-V cmdlets need it' }
  $argv = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-Pause',
            '-ProcessorCount', $ProcessorCount, '-MemoryStartupBytes', $MemoryStartupBytes)
  $named = [ordered]@{ Source = $Source; Destination = $Destination; VMName = $VMName; SwitchName = $SwitchName
                       Resources = $Resources; Kb943295 = $Kb943295; VmGuestIso = $VmGuestIso; Efi = $Efi }
  foreach ($k in $named.Keys) { if ($named[$k]) { $argv += "-$k"; $argv += "`"$($named[$k])`"" } }
  if ($DynamicMemory) { $argv += '-DynamicMemory' }
  if ($KeepPagingExecutive) { $argv += '-KeepPagingExecutive' }
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

# Runs hvkit.exe with the arguments $A, its output (errors included) indented into the log.
function Invoke-Hvkit([string[]]$A) {
  $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
  try { & $script:Hvkit @A 2>&1 | % { "  $_" } } finally { $ErrorActionPreference = $eap }
  if ($LASTEXITCODE) { throw "hvkit $($A[0]) failed (exit code $LASTEXITCODE)" }
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

# --- the conversion ----------------------------------------------------------------------------
function Convert-Disk([string]$Tmp) {
  Step 'checks'
  if (-not (Get-Command Convert-VHD -ErrorAction SilentlyContinue)) { throw 'the Hyper-V PowerShell module is missing' }
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
  $script:Hvkit = Find-First 'hvkit.exe' @($Resources)
  if (-not $script:Hvkit) { $script:Hvkit = (Get-Command hvkit.exe -ErrorAction SilentlyContinue).Source }
  if (-not $script:Hvkit) { throw 'hvkit.exe: put it into the resources folder (nt5-hvgen2: tools/mkdist.sh builds it)' }
  "  hvkit.exe: $script:Hvkit"
  $kb = $null
  if (-not (Find-First 'storport.sys' @($Resources))) {
    if ($Kb943295) { $kb = Expand-Kb943295 $Kb943295 $Tmp }
    elseif ($VmGuestIso) { $kb = Get-Kb943295FromIso $VmGuestIso $Tmp }
    else { throw 'storport.sys, diskdump.sys: pass -VmGuestIso <vmguest.iso of a 2012 R2 Hyper-V host> or -Kb943295 <WindowsServer2003-KB943295-x86-*.exe>, or put both files into the resources folder' }
  }
  $hv = @()
  foreach ($d in $Resources, $kb, (Join-Path $Repo 'out'), (Join-Path $Repo 'acpi\out')) {
    if ($d -and (Test-Path -LiteralPath $d -PathType Container)) { $hv += '--files', $d }
  }
  if ($Efi) { $hv += '--efi', $Efi }
  # The Guest Service Interface always; Dynamic Memory with -DynamicMemory; no VSS (XP's Integration
  # Services have no VSS service; production checkpoints are not offered).
  $hv += '--no-vss'
  if (-not $DynamicMemory) { $hv += '--no-dynamic-memory' }
  if ($KeepPagingExecutive) { $hv += '--keep-paging-executive' }
  if ($Debug) { $hv += '--debug' }
  if ($Force) { $hv += '--force' }
  Invoke-Hvkit (@('migrate', '--check') + $hv)

  Step 'copying the disk'
  if (Test-Path -LiteralPath $Destination) { Remove-Item -LiteralPath $Destination -Force }
  $null = New-Item -ItemType Directory -Force (Split-Path -Parent $Destination)
  Convert-VHD -Path $Source -DestinationPath $Destination -VHDType Dynamic
  $script:Created = $true
  Get-VHD -Path $Destination | % { "  $($_.Path): $($_.VhdFormat) $($_.VhdType), $([math]::Round($_.FileSize / 1MB)) MB used of $([math]::Round($_.Size / 1GB, 1)) GB" }

  Step 'installing'
  Invoke-Hvkit (@('migrate', $Destination) + $hv)

  if ($VMName) {
    Step "creating the VM $VMName"
    $vm = New-VM -Name $VMName -Generation 2 -MemoryStartupBytes $MemoryStartupBytes -VHDPath $Destination
    $script:CreatedVM = $true
    Get-VMNetworkAdapter -VM $vm | Remove-VMNetworkAdapter
    if ($SwitchName) { Add-VMNetworkAdapter -VM $vm -SwitchName $SwitchName }
    # Guest Service Interface (Copy-VMFile), which works with the icsvc patch.  Found by its ID, because
    # Hyper-V localizes the names of the integration services.  A VM without it is still a good VM.
    $gsi = Get-VMIntegrationService -VM $vm | ? { $_.Id -like '*\6C09BB55-D683-4DA0-8931-C9BF705F6480' }
    if ($gsi) { Enable-VMIntegrationService -VMIntegrationService $gsi } else { Write-Warning 'this host has no Guest Service Interface integration service; Copy-VMFile will not work' }
    if ($DynamicMemory) {
      Set-VMMemory -VM $vm -DynamicMemoryEnabled $true -StartupBytes $MemoryStartupBytes -MinimumBytes ([Math]::Min([long]512MB, $MemoryStartupBytes)) -MaximumBytes $MemoryStartupBytes
    } else {
      Set-VMMemory -VM $vm -DynamicMemoryEnabled $false
    }
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
    "  Generation $($vm.Generation), $($vm.ProcessorCount) processors, $($vm.MemoryStartup / 1MB) MB $(if ($DynamicMemory) { 'dynamic' } else { 'static' }) memory, Secure Boot $((Get-VMFirmware -VM $vm).SecureBoot)"
    "  disk: $((Get-VMHardDiskDrive -VM $vm).Path)"
    "  network: $(if ($SwitchName) { $SwitchName } else { 'none' })"
    if ($Debug) { Get-VMComPort -VM $vm | % { "  $($_.Name): $($_.Path)" } }
  }

  Step 'done'
  "  $Destination"
  if ($VMName) { "  start the VM $VMName" } else { "  attach it to a Generation 2 VM (Secure Boot off, at least 2 processors, $(if ($DynamicMemory) { 'dynamic memory with the minimum, startup and maximum you want, the maximum not above the startup memory' } else { 'static memory' })) as its first boot device" }
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
