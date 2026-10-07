# Patch for icsvc.dll of the Hyper-V Integration Services 6.3.9600.16384 (the last release for Windows XP):
# makes the Guest Service Interface, i.e. `Copy-VMFile -FileSource Host`, work on Windows XP.
#
#   . .\IcSvcGuestInterface.ps1
#   Install-IcSvcGuestInterfacePatch C:\path\to\icsvc.dll
#
# Why it is needed: for every file the host copies in, the service logs on NT AUTHORITY\SYSTEM
# (LogonUserExW with an empty password and LOGON32_LOGON_SERVICE) to get a token that it
# impersonates while it checks the destination path and creates the file.  That kind of logon
# exists from Windows Vista on; on XP it fails with ERROR_LOGON_FAILURE, which the host reports as
# "The user name or password is incorrect (0x8007052E)".  The service runs as LocalSystem itself, so
# the patch makes the logon call return a token of its own identity instead (ImpersonateSelf,
# OpenThreadToken, RevertToSelf), which is what logging on as SYSTEM gives on newer systems.
# The files that are copied in are therefore written as SYSTEM, as on a newer Windows guest.
#
# How: the call site of LogonUserExW (RVA 0x32959, `call [IAT]`) becomes a relative call to a stub
# in the unused tail of the .text section (it extends .text's VirtualSize by the stub's length, which
# stays within the section's last page).  The stub addresses its three imports relative to its own
# position, so it needs no base relocations; the base relocation of the old call operand is
# neutralised.  Nothing else in the file changes.
#
# The patch is recognised by the bytes it changes, not by a hash of the whole file, so it can be
# combined with patches to other parts of icsvc.dll.  The file's version is checked, an already
# patched file is left alone.
function Install-IcSvcGuestInterfacePatch([Parameter(Mandatory)] [string]$Path) {
  $Site = 0x32959                                   # call [__imp_LogonUserExW]
  $Slots = @{ ImpersonateSelf = 0x64070; OpenThreadToken = 0x64078; RevertToSelf = 0x64068; LogonUserExW = 0x6407c }

  # [IO.File] resolves a relative path against the process's directory, not against the PowerShell location.
  $Path = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Path)
  $fv = (Get-Item -LiteralPath $Path).VersionInfo.FileVersion
  if ($fv -notmatch '^6\.3\.9600\.16384\b') { throw "${Path}: file version '$fv', the patch is for 6.3.9600.16384 (Integration Services of Windows Server 2012 R2)" }
  $b = [IO.File]::ReadAllBytes($Path)
  $pe = [BitConverter]::ToInt32($b, 0x3c)
  if ([BitConverter]::ToUInt32($b, $pe) -ne 0x4550 -or [BitConverter]::ToUInt16($b, $pe + 4) -ne 0x14c) { throw "${Path}: not an x86 PE file" }
  $nsec = [BitConverter]::ToUInt16($b, $pe + 6); $sec0 = $pe + 24 + [BitConverter]::ToUInt16($b, $pe + 20)
  $secs = for ($i = 0; $i -lt $nsec; $i++) {
    $o = $sec0 + $i * 40
    [pscustomobject]@{ Hdr = $o; Name = [Text.Encoding]::ASCII.GetString($b, $o, 8).TrimEnd([char]0); VSize = [BitConverter]::ToUInt32($b, $o + 8)
                       Rva = [BitConverter]::ToUInt32($b, $o + 12); RawSize = [BitConverter]::ToUInt32($b, $o + 16); Raw = [BitConverter]::ToUInt32($b, $o + 20) }
  }
  function Off([uint32]$rva) {
    foreach ($s in $secs) { if ($rva -ge $s.Rva -and $rva -lt $s.Rva + [Math]::Max($s.VSize, $s.RawSize)) { return [int]($rva - $s.Rva + $s.Raw) } }
    throw ('RVA 0x{0:x} is in no section' -f $rva)
  }
  $text = $secs | ? { $_.Name -eq '.text' }
  if (-not $text) { throw "${Path}: no .text section" }

  # The import slots must be the functions the stub assumes.
  foreach ($n in $Slots.Keys) {
    $thunk = [BitConverter]::ToUInt32($b, (Off $Slots[$n]))
    if ($thunk -band 0x80000000) { throw "${Path}: import slot of $n is by ordinal" }
    $o = Off $thunk; $e = $o + 2; while ($b[$e] -ne 0) { $e++ }
    $name = [Text.Encoding]::ASCII.GetString($b, $o + 2, $e - $o - 2)
    if ($name -ne $n) { throw "${Path}: import slot 0x$('{0:x}' -f $Slots[$n]) is '$name', expected '$n'" }
  }

  $so = Off $Site
  $orig = [byte[]](0xff, 0x15) + [BitConverter]::GetBytes([uint32](0x10000000 + $Slots.LogonUserExW))
  $cur = $b[$so..($so + 5)]
  if ($cur[0] -eq 0xe8) { return "$Path : Guest Service Interface patch is already applied" }
  if (($cur | % { '{0:x2}' -f $_ }) -join ' ' -ne (($orig | % { '{0:x2}' -f $_ }) -join ' ')) { throw "${Path}: unexpected code at the LogonUserExW call site ($(($cur | % { '{0:x2}' -f $_ }) -join ' '))" }

  # stub: stdcall, the 10 arguments of LogonUserExW (40 bytes); the phToken output is the 6th
  $stubRva = [uint32](($text.Rva + $text.VSize + 15) -band (-bnot 15))
  $code = New-Object Collections.Generic.List[byte]
  $emit = { param([byte[]]$x) $code.AddRange($x) }
  $disp = { param($slot) $l1 = $stubRva + 6; [BitConverter]::GetBytes([int32]($slot - $l1)) }
  & $emit (0x53)                                                # push ebx
  & $emit (0xe8, 0, 0, 0, 0)                                    # call L1
  & $emit (0x5b)                                                # L1: pop ebx
  & $emit (0x6a, 0x02)                                          # push SecurityImpersonation
  & $emit ([byte[]](0xff, 0x93) + (& $disp $Slots.ImpersonateSelf))   # call [ebx+ImpersonateSelf]
  & $emit (0x85, 0xc0)                                          # test eax,eax
  & $emit (0x74, 0x00); $jz = $code.Count - 1                   # jz out
  & $emit (0x8b, 0x4c, 0x24, 0x1c)                              # mov ecx,[esp+1Ch]   ; phToken
  & $emit (0x51)                                                # push ecx
  & $emit (0x6a, 0x01)                                          # push TRUE (OpenAsSelf)
  & $emit ([byte[]](0x68) + [BitConverter]::GetBytes([uint32]0xF01FF))  # push TOKEN_ALL_ACCESS
  & $emit (0x6a, 0xfe)                                          # push -2 (current thread)
  & $emit ([byte[]](0xff, 0x93) + (& $disp $Slots.OpenThreadToken))    # call [ebx+OpenThreadToken]
  & $emit (0x50)                                                # push eax
  & $emit ([byte[]](0xff, 0x93) + (& $disp $Slots.RevertToSelf))       # call [ebx+RevertToSelf]
  & $emit (0x58)                                                # pop eax
  $code[$jz] = [byte]($code.Count - $jz - 1)
  & $emit (0x5b)                                                # out: pop ebx
  & $emit (0xc2, 0x28, 0x00)                                    # ret 28h

  $stubOff = Off $stubRva
  if ($stubRva + $code.Count - $text.Rva -gt $text.RawSize) { throw "${Path}: no room for the stub after .text" }
  for ($i = 0; $i -lt $code.Count; $i++) { if ($b[$stubOff + $i] -ne 0) { throw "${Path}: the tail of .text is not empty" } }
  $code.CopyTo($b, $stubOff)
  [BitConverter]::GetBytes([int32]($stubRva - ($Site + 5))).CopyTo($b, $so + 1); $b[$so] = 0xe8; $b[$so + 5] = 0x90
  [BitConverter]::GetBytes([uint32]($stubRva + $code.Count - $text.Rva)).CopyTo($b, $text.Hdr + 8)

  # the base relocation of the old call operand must not change the new relative call
  $dir = $pe + 24 + 0x60 + 5 * 8
  $rrva = [BitConverter]::ToUInt32($b, $dir); $rsz = [BitConverter]::ToUInt32($b, $dir + 4)
  $p = Off $rrva; $end = $p + $rsz; $done = $false
  while ($p -lt $end) {
    $page = [BitConverter]::ToUInt32($b, $p); $sz = [BitConverter]::ToUInt32($b, $p + 4)
    if ($page -eq (($Site + 2) -band (-bnot 0xfff))) {
      for ($q = $p + 8; $q -lt $p + $sz; $q += 2) {
        if ([BitConverter]::ToUInt16($b, $q) -eq (0x3000 -bor (($Site + 2) -band 0xfff))) { $b[$q] = 0; $b[$q + 1] = 0; $done = $true }
      }
    }
    $p += $sz
  }
  if (-not $done) { throw "${Path}: no base relocation for the LogonUserExW call operand" }

  [IO.File]::WriteAllBytes($Path, $b)
  "$Path : Guest Service Interface patch applied (stub at RVA 0x$('{0:x}' -f $stubRva), $($code.Count) bytes)"
}
