<#
.SYNOPSIS
  Produce a Windows-XP-enabled copy of Microsoft's Hyper-V Integration Components
  user-mode host DLL (ICSvc.dll, IC 6.3.9600.16384) for the Backup (VSS) service.

.DESCRIPTION
  ICSvc.dll already contains a complete Windows XP (NT 5.1) code path for the VSS
  integration service: its IVssBackupComponents adapter switches to XP's 2003-era
  vtable layout when GetVersionEx reports 5.1 (gm_UseOldInterface), and its VssApi
  delay-load failure hook falls back from the Vista-era *Internal exports to the
  2003 exports that XP's vssapi.dll has.  Two things still stop it on XP, both in
  VSS-only code that heartbeat / KVP / shutdown / time-sync never reach:

    1. A runtime gate, ICVssCheckOsVersionForHotBackup(), only returns TRUE for OS
       5.2+ (Server 2003) and 6.x; it rejects 5.1 (Windows XP).  Patch: flip its
       minor-version threshold from 2 to 1 (`cmp $2,minor` -> `cmp $1,minor`).
       After it, the gate returns TRUE for 5.1 and keeps its existing behaviour
       for 5.0 (still rejected), 5.2 (unchanged, incl. its amd64 special case)
       and 6.x.  One byte.

    2. XP's vssapi.dll IVssBackupComponents::SetContext is a stub that always
       returns E_NOTIMPL (0x80004001); icsvc's VssClientBase::Initialize treats
       any SetContext failure as fatal, so VSS init throws on XP.  XP has no VSS
       snapshot contexts -- it only supports the default VSS_CTX_BACKUP -- so the
       call is unnecessary there.  Patch: in the 'old interface' (XP) branch of
       ICVssComponentAdapter::SetContext, skip the vtable call and return S_OK
       (pop the context arg, xor eax,eax, ret).  The 'new interface' branch (used
       on 5.2+/6.x) is untouched, and the branch is only taken when
       gm_UseOldInterface is set, i.e. only on 5.1.

  This script makes a SEPARATE patched copy (default name icsvcvss.dll) so the
  shared ICSvc.dll used by the other integration services is never touched.

  No Microsoft binary is redistributed: the script reads the user's own ICSvc.dll,
  verifies its SHA-256, and writes the patched copy next to it (or to -OutFile).

.PARAMETER InFile
  Path to the stock ICSvc.dll (IC 6.3.9600.16384, x86).  On a converted guest it
  is C:\WINDOWS\system32\ICSvc.dll; offline it is <XP volume>\WINDOWS\system32\ICSvc.dll.

.PARAMETER OutFile
  Where to write the patched copy.  Defaults to icsvcvss.dll beside -InFile.

.EXAMPLE
  .\Patch-IcSvcVss.ps1 -InFile D:\WINDOWS\system32\ICSvc.dll
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)] [string]$InFile,
  [string]$OutFile
)
$ErrorActionPreference = 'Stop'

# Stock IC 6.3.9600.16384 ICSvc.dll (x86), timestamp 2013-08-21.
$ExpectedSha = 'CEF218418F65513DDC91215D82ECAE6624A259013F4C84EA0229465266EB07AF'

# Patch 1: ICVssCheckOsVersionForHotBackup minor-version threshold 2 -> 1.
$OsGateOffset = 0x362F8
$OsGateFrom   = @(0x02)
$OsGateTo     = @(0x01)

# Patch 2: ICVssComponentAdapter::SetContext 'old interface' (XP) branch.
# Replace  mov eax,[ecx+4]; push eax; mov ecx,[eax]; call [ecx+0x80]; ret
# with     pop eax; xor eax,eax; ret  (+ NOP padding) -> return S_OK, no vtable call.
$SetCtxOffset = 0x3FF4D
$SetCtxFrom   = @(0x8B,0x41,0x04,0x50,0x8B,0x08,0xFF,0x91,0x80,0x00,0x00,0x00,0xC3)
$SetCtxTo     = @(0x58,0x33,0xC0,0xC3,0x90,0x90,0x90,0x90,0x90,0x90,0x90,0x90,0x90)

if (-not $OutFile) { $OutFile = Join-Path (Split-Path -Parent (Resolve-Path $InFile)) 'icsvcvss.dll' }

$bytes = [System.IO.File]::ReadAllBytes($InFile)
$sha = ([System.Security.Cryptography.SHA256]::Create().ComputeHash($bytes) |
        ForEach-Object { $_.ToString('X2') }) -join ''
if ($sha -ne $ExpectedSha) {
  throw "Input SHA-256 mismatch.`n  expected $ExpectedSha`n  got      $sha`nThis script only patches IC 6.3.9600.16384 ICSvc.dll (x86)."
}

function Apply-Patch([string]$Name, [int]$At, [int[]]$From, [int[]]$To) {
  for ($i = 0; $i -lt $From.Count; $i++) {
    if ($bytes[$At + $i] -ne $From[$i]) {
      throw ("{0}: byte at 0x{1:X} is 0x{2:X2}, expected 0x{3:X2}; refusing to patch." -f $Name, ($At + $i), $bytes[$At + $i], $From[$i])
    }
  }
  for ($i = 0; $i -lt $To.Count; $i++) { $bytes[$At + $i] = $To[$i] }
  Write-Host ("{0}: patched {1} byte(s) at 0x{2:X}" -f $Name, $To.Count, $At)
}

Apply-Patch 'ICVssCheckOsVersionForHotBackup' $OsGateOffset $OsGateFrom $OsGateTo
Apply-Patch 'ICVssComponentAdapter::SetContext' $SetCtxOffset $SetCtxFrom $SetCtxTo

# Fix the PE checksum so the image stays well-formed (OptionalHeader.CheckSum).
function Update-PeChecksum([byte[]]$img) {
  $peOff = [BitConverter]::ToInt32($img, 0x3C)
  $csOff = $peOff + 0x58            # IMAGE_OPTIONAL_HEADER32.CheckSum
  for ($i = 0; $i -lt 4; $i++) { $img[$csOff + $i] = 0 }
  [uint64]$sum = 0
  for ($i = 0; $i -lt $img.Length - 1; $i += 2) {
    $sum += [BitConverter]::ToUInt16($img, $i)
    if ($sum -gt 0xFFFFFFFF) { $sum = ($sum -band 0xFFFFFFFF) + ($sum -shr 32) }
  }
  if ($img.Length -band 1) { $sum += $img[$img.Length - 1] }
  $sum = ($sum -band 0xFFFF) + ($sum -shr 16)
  $sum = $sum + ($sum -shr 16)
  $sum = $sum -band 0xFFFF
  $sum += [uint64]$img.Length
  [BitConverter]::GetBytes([uint32]$sum).CopyTo($img, $csOff)
}
Update-PeChecksum $bytes

[System.IO.File]::WriteAllBytes($OutFile, $bytes)
$outSha = ([System.Security.Cryptography.SHA256]::Create().ComputeHash($bytes) |
           ForEach-Object { $_.ToString('X2') }) -join ''
Write-Host "wrote $OutFile"
Write-Host "  sha256 $outSha"
