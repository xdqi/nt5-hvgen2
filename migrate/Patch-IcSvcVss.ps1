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

    3. VssClientBase::WaitAndCheckForAsyncOperation passes the VssClientBase*
       'this' in EDI in/out of the call (a custom register convention, implemented
       by pushing EDI as an extra argument of IVssAsync::Wait and popping it back
       in the success-path epilogue).  The code was built for the 2003-SP1+/Vista
       signature HRESULT IVssAsync::Wait(DWORD dwTimeout) whose callee cleans 8 of
       the 12 pushed bytes.  XP SP3's Wait has NO dwTimeout parameter (ret 4), so
       the pushed INFINITE stays on the stack and the epilogue's "pop edi" loads
       that 0xFFFFFFFF instead of the saved register: GatherWriterMetadata keeps
       its 'this' in EDI across the call, so InitializeWriterMetadata runs with
       this = -1 and faults on [this+0x10] (guest AV wrapped into the host error
       0x80020009).  Patch: drop the "push 0xffffffff" ("6a ff" -> "90 90") at the
       Wait call site inside WaitAndCheckForAsyncOperation.  The call becomes
       "push edi; push esi; call [vt+0x10]": XP's Wait cleans 'this' and the pushed
       EDI is popped back by the epilogue -- the stack balances and EDI is
       restored correctly.  XP's parameterless Wait already waits indefinitely, so
       dropping the timeout argument does not change the behaviour.  (Verified on
       the VM: with this patch the freeze path gets past writer metadata.)

    4. CHyperVICProvider::_QueryInterface answers only 2003-SP1+/Vista-era IIDs
       ({609E123E} IVssSoftwareSnapshotProvider, {5F894E5B} IVssProviderCreate-
       SnapshotSet).  XP SP3's vssvc asks for the XP-era IIDs ({FBE2D3E8} SSP,
       {C4226E73} CSS), so every QI fails with E_NOINTERFACE (0x80004002) and the
       XP coordinator logs event 12292 "provider ... had an unexpected error"
       and aborts the snapshot with VSS_E_UNEXPECTED_PROVIDER_ERROR (0x8004230F).
       Patch: replace the two GUID constants so QI answers the XP IIDs.

    5. The IVssSoftwareSnapshotProvider vtable layout differs between XP and
       2003-SP1+.  XP has MakeSnapshotReadWrite at +0x28 and SetSnapshotProperty
       at +0x2c; 2003-SP1+ dropped MakeSnapshotReadWrite and shifted
       SetSnapshotProperty to +0x28 (followed by RevertToSnapshot and
       QueryRevertStatus).  Patch: point +0x28 at a stub returning S_OK (the
       dead KVP cave at RVA 0x2A312 is safe because KVP runs from ICSvc.dll,
       not icsvcvss.dll) and move the SetSnapshotProperty pointer to +0x2c.

    6. The IVssProviderCreateSnapshotSet vtable: XP has AbortSnapshots at +0x1c
       while 2003-SP1+ inserted PreFinalCommitSnapshots/PostFinalCommitSnapshots
       before it (shifting AbortSnapshots to +0x24).  Patch: put AbortSnapshots
       at +0x1c.

    7. CHyperVICProvider::BeginPrepareSnapshot is compiled for the 2003-SP1+
       signature (this, GUID, GUID, PWSTR, LONG) = ret 0x2C.  XP's wrapper calls
       it with only 3 args (this, GUID, GUID, PWSTR) = ret 0x28; the 5th (LONG)
       parameter is never read (confirmed by disassembly: the function body
       accesses only [ebp+0x2c]).  Patch: change "ret 0x2C" to "ret 0x28" so the
       callee matches XP's calling convention.

    8. MakeSnapshotReadWrite stub (the dead KVP cave): "xor eax,eax; ret 0x14"
       returns S_OK and cleans 5 dwords to match XP's wrapper, which calls
       (this, GUID) and expects ret 0x14.

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

# Patch 3: VssClientBase::WaitAndCheckForAsyncOperation, the IVssAsync::Wait call site.
# The stock code pushes (EDI=this-save, INFINITE, ESI=async) for the 2003-SP1+/Vista
# "Wait(this, dwTimeout)" (callee ret 8); XP SP3's parameterless Wait is ret 4, which
# leaves the INFINITE on the stack so the epilogue's "pop edi" restores 0xFFFFFFFF
# instead of the caller's this (-> InitializeWriterMetadata(this=-1) -> AV).
# Replace "push 0xffffffff" with NOPs: the call becomes (EDI, ESI), XP cleans ESI and
# the pushed EDI is what the epilogue pops back.
$WaitArityOffset = 0x3ACEC
$WaitArityFrom   = @(0x6A,0xFF)
$WaitArityTo     = @(0x90,0x90)

# Patch 4: CHyperVICProvider::_QueryInterface GUIDs -> XP-era IIDs.
# SSP: {609E123E-2C5A-44D3-8F01-0B1D9A47D1FF} -> {FBE2D3E8-4C8C-4464-9FE9-C3B6AE023357}
$QI_SSP_Offset = 0x658
$QI_SSP_From   = @(0x3E,0x12,0x9E,0x60,0x5A,0x2C,0xD3,0x44,0x8F,0x01,0x0B,0x1D,0x9A,0x47,0xD1,0xFF)
$QI_SSP_To     = @(0xE8,0xD3,0xE2,0xFB,0x8C,0x4C,0x64,0x44,0x9F,0xE9,0xC3,0xB6,0xAE,0x02,0x33,0x57)
# CSS: {5F894E5B-1E39-4778-8E23-9ABAD9F0E08C} -> {C4226E73-2F8B-490D-84E0-F9486B4ED627}
$QI_CSS_Offset = 0x648
$QI_CSS_From   = @(0x5B,0x4E,0x89,0x5F,0x39,0x1E,0x78,0x47,0x8E,0x23,0x9A,0xBA,0xD9,0xF0,0xE0,0x8C)
$QI_CSS_To     = @(0x73,0x6E,0x22,0xC4,0x8B,0x2F,0x0D,0x49,0x84,0xE0,0xF9,0x48,0x6B,0x4E,0xD6,0x27)

# Patch 5: SSP vtable +0x28 = MakeSnapshotReadWrite stub (RVA 0x2A312), +0x2c = SetSnapshotProperty.
$SSP_VT_28_Offset = 0xB7CC
$SSP_VT_28_From   = @(0xEB,0x36,0x04,0x10)          # 0x100436EB SetSnapshotProperty
$SSP_VT_28_To     = @(0x12,0xA3,0x02,0x10)          # 0x1002A312 MakeSnapshotReadWrite stub
$SSP_VT_2C_Offset = 0xB7D0
$SSP_VT_2C_From   = @(0x45,0x30,0x04,0x10)          # 0x10043045 RevertToSnapshot
$SSP_VT_2C_To     = @(0xEB,0x36,0x04,0x10)          # 0x100436EB SetSnapshotProperty

# Patch 6: CSS vtable +0x1c = AbortSnapshots (was PreFinalCommitSnapshots).
$CSS_VT_1C_Offset = 0xB728
$CSS_VT_1C_From   = @(0x35,0x32,0x04,0x10)          # 0x10043235 PreFinalCommitSnapshots
$CSS_VT_1C_To     = @(0x28,0x35,0x04,0x10)          # 0x10043528 AbortSnapshots

# Patch 7: BeginPrepareSnapshot ret 0x2C -> 0x28 (5th arg is never read).
$BPS_RetOffset = 0x423CF
$BPS_RetFrom   = @(0xC2,0x2C,0x00)
$BPS_RetTo     = @(0xC2,0x28,0x00)

# Patch 8: MakeSnapshotReadWrite stub in the dead KVP cave (RVA 0x2A312, file 0x29712).
# xor eax,eax; ret 0x14  (S_OK, 5 dwords -- matches XP wrapper's 3-arg call).
$StubOffset = 0x29712
$StubFrom   = @(0x68,0x6C,0x09,0x00,0x00)           # push 0x96c (KVP prologue)
$StubTo     = @(0x33,0xC0,0xC2,0x14,0x00)           # xor eax,eax; ret 0x14

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
Apply-Patch 'WaitAndCheckForAsyncOperation Wait arity' $WaitArityOffset $WaitArityFrom $WaitArityTo
Apply-Patch 'QueryInterface SSP IID (XP)' $QI_SSP_Offset $QI_SSP_From $QI_SSP_To
Apply-Patch 'QueryInterface CSS IID (XP)' $QI_CSS_Offset $QI_CSS_From $QI_CSS_To
Apply-Patch 'SSP vtable MakeSnapshotReadWrite' $SSP_VT_28_Offset $SSP_VT_28_From $SSP_VT_28_To
Apply-Patch 'SSP vtable SetSnapshotProperty' $SSP_VT_2C_Offset $SSP_VT_2C_From $SSP_VT_2C_To
Apply-Patch 'CSS vtable AbortSnapshots' $CSS_VT_1C_Offset $CSS_VT_1C_From $CSS_VT_1C_To
Apply-Patch 'BeginPrepareSnapshot ret 0x28' $BPS_RetOffset $BPS_RetFrom $BPS_RetTo
Apply-Patch 'MakeSnapshotReadWrite stub' $StubOffset $StubFrom $StubTo

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
