<#
.SYNOPSIS
    Patch Hyper-V Integration Services 6.3.9600.16384 dmvsc.sys so that it loads
    on Windows XP SP3 x86, by rebinding its one import that XP's kernel lacks
    (MmAllocatePagesForMdlEx) to the companion driver mdlex.sys.

.DESCRIPTION
    dmvsc.sys (the Dynamic Memory VSC, built for Windows Server 2003 SP1) imports
    exactly one routine missing from XP SP3's ntoskrnl: MmAllocatePagesForMdlEx.
    Everything else it needs already exists on XP.  mdlex.sys (this repository)
    exports MmAllocatePagesForMdlEx, implemented over XP's MmAllocatePagesForMdl.

    This script edits dmvsc.sys's import table, touching no code:

      * It appends a new section (.dmx) holding a fresh import-descriptor array:
        the four original descriptors verbatim (same INT/IAT addresses) plus a
        fifth for mdlex.sys whose single import is MmAllocatePagesForMdlEx and
        whose FirstThunk reuses the existing IAT slot the code already calls.
      * It repoints ntoskrnl's INT entry for MmAllocatePagesForMdlEx to an
        already-imported ntoskrnl export (MmFreePagesFromMdl), so the loader can
        resolve ntoskrnl's thunk; the mdlex descriptor, processed afterwards,
        overwrites that IAT slot with mdlex!MmAllocatePagesForMdlEx.
      * It repoints the import data directory at the new array and fixes the PE
        checksum and SizeOfImage.

    The kernel loads mdlex.sys as a dependency of the patched dmvsc.sys (place
    both in system32\drivers).  No code bytes, relocations or other imports
    change; the four original IAT slots keep their addresses.

    The input is verified by SHA-256 so the hard-coded import-table geometry
    always matches.  No Microsoft binary is redistributed: the caller supplies
    its own dmvsc.sys from the Integration Services and keeps the output locally.

.PARAMETER InputPath
    Path to the original IC 6.3.9600.16384 dmvsc.sys.

.PARAMETER OutputPath
    Where to write the patched driver.  Defaults to <InputPath dir>\dmvsc.patched.sys.

.EXAMPLE
    .\Patch-Dmvsc.ps1 -InputPath C:\IS\dmvsc\dmvsc.sys -OutputPath C:\out\dmvsc.sys
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$InputPath,
    [string]$OutputPath
)
$ErrorActionPreference = 'Stop'

# SHA-256 of IC 6.3.9600.16384 dmvsc.sys (x86), the only supported input.
$ExpectSha = 'E23B6657E1126603D195145BED77AA239625057A28378AF535E5A3A7A4D1F36D'
# ntoskrnl imports rebound to mdlex.sys:
#   MmAllocatePagesForMdlEx  XP's kernel does not export it at all (load fails
#                            without this); mdlex implements it over the base
#                            MmAllocatePagesForMdl.
#   MmAddPhysicalMemory      XP exports it but cannot usefully hot-add memory;
#                            dmvsc's start-time probe wrongly succeeds and makes
#                            it advertise hot-add with a bogus maximum, which the
#                            host rejects (STATUS 0xC000A013) so the device fails
#                            to start.  mdlex's stub returns STATUS_NOT_SUPPORTED
#                            so dmvsc degrades to balloon-only.
$Redirects         = @('MmAllocatePagesForMdlEx', 'MmAddPhysicalMemory')
# An ntoskrnl export that is NOT redirected, used as the placeholder that keeps
# each repointed ntoskrnl thunk resolvable (its value is immediately overwritten
# by mdlex's descriptor).
$PlaceholderImport = 'MmFreePagesFromMdl'
$HelperModule      = 'mdlex.sys'

if (-not $OutputPath) {
    $OutputPath = Join-Path (Split-Path -Parent $InputPath) 'dmvsc.patched.sys'
}

$d = [System.IO.File]::ReadAllBytes($InputPath)

$sha = ([System.Security.Cryptography.SHA256]::Create().ComputeHash($d) |
        ForEach-Object { $_.ToString('X2') }) -join ''
if ($sha -ne $ExpectSha) {
    throw "Input SHA-256 $sha does not match the expected IC 6.3.9600.16384 dmvsc.sys ($ExpectSha). Refusing to patch."
}

# --- little-endian helpers over the byte array ---
function U16([int]$o) { [BitConverter]::ToUInt16($d, $o) }
function U32([int]$o) { [BitConverter]::ToUInt32($d, $o) }
function SetU16([int]$o, [int]$v) { [Array]::Copy([BitConverter]::GetBytes([uint16]$v), 0, $d, $o, 2) }
function SetU32([int]$o, [long]$v) { [Array]::Copy([BitConverter]::GetBytes([uint32]$v), 0, $d, $o, 4) }

$pe   = U32 0x3c
if ((U16 $pe) -ne 0x4550) { throw 'not a PE file' }
$coff = $pe + 4
$nsec    = U16 ($coff + 2)
$optsz   = U16 ($coff + 16)
$opt     = $coff + 20
if ((U16 $opt) -ne 0x10b) { throw 'not a PE32 image' }
$secAlign  = U32 ($opt + 32)
$fileAlign = U32 ($opt + 36)
$sizeHdrs  = U32 ($opt + 60)
$dd        = $opt + 96
$sec       = $opt + $optsz

# section table -> RVA/offset mapping
$secs = @()
for ($i = 0; $i -lt $nsec; $i++) {
    $o = $sec + $i * 40
    $secs += [pscustomobject]@{
        HdrOff = $o
        VA   = U32 ($o + 12); VSize = U32 ($o + 8)
        Raw  = U32 ($o + 20); RSize = U32 ($o + 16)
    }
}
function Rva2Off([long]$rva) {
    foreach ($s in $secs) {
        $span = [Math]::Max($s.VSize, $s.RSize)
        if ($rva -ge $s.VA -and $rva -lt $s.VA + $span) { return $s.Raw + ($rva - $s.VA) }
    }
    throw "RVA $rva not in any section"
}
function StrAt([long]$rva) {
    $o = Rva2Off $rva; $e = $o
    while ($d[$e] -ne 0) { $e++ }
    [Text.Encoding]::ASCII.GetString($d, $o, $e - $o)
}

$importRva = U32 ($dd + 1 * 8)

# read the existing import descriptors (20 bytes each) up to the null terminator
$descOff = Rva2Off $importRva
$descs = New-Object System.Collections.ArrayList
$i = 0
while ($true) {
    $base = $descOff + $i * 20
    $oft = U32 $base; $name = U32 ($base + 12); $ft = U32 ($base + 16)
    if ($oft -eq 0 -and $name -eq 0 -and $ft -eq 0) { break }
    [void]$descs.Add($d[$base..($base + 19)])
    $i++
}
if ($descs.Count -ne 4) { throw "expected 4 import descriptors, found $($descs.Count)" }

# descriptor 0 is ntoskrnl.exe; find, for every redirected import, its INT-entry
# offset and the IAT slot that goes with it, plus the placeholder import's name.
$ntOft = U32 $descOff
$ntFt  = U32 ($descOff + 16)
if ((StrAt (U32 ($descOff + 12))) -notlike 'ntoskrnl*') { throw 'descriptor 0 is not ntoskrnl' }
$redir = @{}                 # name -> @{ IntOff = ...; SlotRva = ... }
$placeholderNameRva = 0
$j = 0
while ($true) {
    $t = U32 ((Rva2Off $ntOft) + $j * 4)
    if ($t -eq 0) { break }
    if (($t -band 0x80000000L) -eq 0) {
        $nm = StrAt ($t + 2)   # skip the 2-byte hint
        if ($Redirects -contains $nm) {
            $redir[$nm] = @{ IntOff = (Rva2Off $ntOft) + $j * 4; SlotRva = $ntFt + $j * 4 }
        }
        if ($nm -eq $PlaceholderImport) { $placeholderNameRva = $t }
    }
    $j++
}
foreach ($r in $Redirects) { if (-not $redir.ContainsKey($r)) { throw "ntoskrnl import $r not found" } }
if (-not $placeholderNameRva) { throw "placeholder import $PlaceholderImport not found" }

# --- build the new .dmx section ---
$newVa = 0
foreach ($s in $secs) { $newVa = [Math]::Max($newVa, $s.VA + $s.VSize) }
$newVa = [int](([Math]::Floor(($newVa + $secAlign - 1) / $secAlign)) * $secAlign)
$newRaw = [int](([Math]::Floor(($d.Length + $fileAlign - 1) / $fileAlign)) * $fileAlign)

$n       = $Redirects.Count
$nDesc   = $descs.Count + $n + 1       # 4 original + one per redirect + null terminator
$descSz  = $nDesc * 20
# Per redirect: an INT (two dwords) then an IMAGE_IMPORT_BY_NAME (hint + name +
# NUL, padded to even).  One shared module-name string follows them all.
$intRva    = @{}; $bynameRva = @{}
$cursor = $newVa + $descSz
foreach ($r in $Redirects) { $intRva[$r] = $cursor; $cursor += 8 }
foreach ($r in $Redirects) {
    $bynameRva[$r] = $cursor
    $len = 2 + [Text.Encoding]::ASCII.GetByteCount($r) + 1
    if ($len % 2) { $len++ }
    $cursor += $len
}
$dllNameRva = $cursor

$blob = New-Object System.Collections.Generic.List[byte]
foreach ($rec in $descs) { $blob.AddRange([byte[]]$rec) }
# one mdlex descriptor per redirect, each reusing that import's ntoskrnl IAT slot
foreach ($r in $Redirects) {
    $blob.AddRange([BitConverter]::GetBytes([uint32]$intRva[$r]))
    $blob.AddRange([BitConverter]::GetBytes([uint32]0))
    $blob.AddRange([BitConverter]::GetBytes([uint32]0))
    $blob.AddRange([BitConverter]::GetBytes([uint32]$dllNameRva))
    $blob.AddRange([BitConverter]::GetBytes([uint32]$redir[$r].SlotRva))
}
$blob.AddRange((New-Object byte[] 20)) # null terminator
# the INTs, in the same order
foreach ($r in $Redirects) {
    $blob.AddRange([BitConverter]::GetBytes([uint32]$bynameRva[$r]))
    $blob.AddRange([BitConverter]::GetBytes([uint32]0))
}
# the IMAGE_IMPORT_BY_NAME entries
foreach ($r in $Redirects) {
    while ($blob.Count -lt ($bynameRva[$r] - $newVa)) { $blob.Add(0) }
    $blob.AddRange([BitConverter]::GetBytes([uint16]0))      # hint
    $blob.AddRange([Text.Encoding]::ASCII.GetBytes($r)); $blob.Add(0)
}
# the shared module name
while ($blob.Count -lt ($dllNameRva - $newVa)) { $blob.Add(0) }
$blob.AddRange([Text.Encoding]::ASCII.GetBytes($HelperModule)); $blob.Add(0)

$vsNew  = $blob.Count
$rawNew = [int](([Math]::Floor(($vsNew + $fileAlign - 1) / $fileAlign)) * $fileAlign)

# grow the file: pad to newRaw, then append the (file-aligned) section data
$out = New-Object System.Collections.Generic.List[byte]
$out.AddRange($d)
while ($out.Count -lt $newRaw) { $out.Add(0) }
$out.AddRange($blob)
while ($out.Count -lt $newRaw + $rawNew) { $out.Add(0) }
$d = $out.ToArray()

# repoint each redirected ntoskrnl INT entry to the placeholder export, so the
# loader can resolve ntoskrnl's thunk; the mdlex descriptors (processed later)
# then overwrite those IAT slots with mdlex's own exports.
foreach ($r in $Redirects) { SetU32 $redir[$r].IntOff $placeholderNameRva }

# write the new section header
$newHdr = $sec + $nsec * 40
if ($newHdr + 40 -gt $sizeHdrs) { throw 'no room in the header for a new section' }
$nameField = New-Object byte[] 8
[Array]::Copy([Text.Encoding]::ASCII.GetBytes('.dmx'), $nameField, 4)
[Array]::Copy($nameField, 0, $d, $newHdr, 8)
SetU32 ($newHdr + 8)  $vsNew       # VirtualSize
SetU32 ($newHdr + 12) $newVa       # VirtualAddress
SetU32 ($newHdr + 16) $rawNew      # SizeOfRawData
SetU32 ($newHdr + 20) $newRaw      # PointerToRawData
SetU32 ($newHdr + 24) 0
SetU32 ($newHdr + 28) 0
SetU16 ($newHdr + 32) 0
SetU16 ($newHdr + 34) 0
SetU32 ($newHdr + 36) 0x40000040   # CNT_INITIALIZED_DATA | MEM_READ

SetU16 ($coff + 2) ($nsec + 1)     # NumberOfSections
$sizeImage = [int](([Math]::Floor(($newVa + $vsNew + $secAlign - 1) / $secAlign)) * $secAlign)
SetU32 ($opt + 56) $sizeImage      # SizeOfImage
SetU32 ($dd + 1 * 8)       $newVa          # import dir RVA
SetU32 ($dd + 1 * 8 + 4)   ($nDesc * 20)   # import dir size (incl. null)

# --- recompute the PE checksum ---
SetU32 ($opt + 64) 0
[long]$sum = 0
for ($o = 0; $o -lt ($d.Length -band -2); $o += 2) {
    $sum += U16 $o
    $sum = ($sum -band 0xffff) + ($sum -shr 16)
}
if ($d.Length -band 1) { $sum += $d[$d.Length - 1]; $sum = ($sum -band 0xffff) + ($sum -shr 16) }
$sum = ($sum -band 0xffff) + ($sum -shr 16)
$checksum = ([long]$sum + $d.Length) -band 0xffffffff
SetU32 ($opt + 64) $checksum

[System.IO.File]::WriteAllBytes($OutputPath, $d)
Write-Host ("Patched {0} -> {1}" -f $InputPath, $OutputPath)
Write-Host ("  added section .dmx at RVA 0x{0:X}, {1} import descriptors, checksum 0x{2:X}" -f $newVa, ($nDesc - 1), $checksum)
Write-Host ("  rebound from {0}: {1}; place both in system32\drivers" -f $HelperModule, ($Redirects -join ', '))
