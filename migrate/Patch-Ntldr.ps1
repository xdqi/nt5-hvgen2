<#
.SYNOPSIS
  Make the screens of NTLDR and SETUPLDR.BIN of Windows XP and Windows Server 2003 usable on machines
  whose VGA mode 12h is a single bit plane in RAM, such as Hyper-V Generation 2 VMs booting through
  CSMWrap (SeaVGABIOS, vga_planar_approx): the highlighted entry of the boot.ini and F8 menus, and the
  status bar of text-mode setup, are invisible without it. Any language: only the message resources
  differ.

.DESCRIPTION
  When BOOTFONT.BIN is next to them (Chinese, Japanese and Korean versions), NTLDR and SETUPLDR draw
  their screens in VGA mode 12h (640x480, 16 colours) with the font from BOOTFONT.BIN, letting the VGA
  hardware do the colours:

    - TextGrSetCurrentAttribute loads the VGA latches with the background colour (it writes and reads
      the first byte after the screen), selects the XOR function and enables set/reset for the planes
      on which the foreground and background colours agree;
    - the glyph writer then stores only the glyph bits (1 = foreground) of each scan line of a
      character, and the clear routines store zeros (background).

  Without VGA hardware, SeaVGABIOS keeps mode 12h as one bit plane in the RAM at 0xA0000, shown white
  on blue. Every byte stored lands there as it is, so the attribute is lost: reverse video (ESC[7m,
  attribute 0x70, black on light grey), which marks the highlighted menu entry and the status bar,
  looks exactly like normal text (0x07). The loaders use no other attributes.

  The patch makes the stored bits mark the brighter colour. For an attribute with a light background
  (bit 6 set, in practice only 0x70) it

    - passes the attribute to TextGrSetCurrentAttribute with foreground and background swapped, so
      the latches get the foreground colour, and
    - inverts every byte stored while that attribute is current: the glyph bits and the top and bottom
      fill bytes of each character, and the zeros of the clear routines.

  On VGA hardware this draws the same pixels as before: on a plane where the two colours differ the
  result is ~bits XOR fg = bits XOR bg, and on the other planes set/reset gives fg = bg. (Any test on
  the attribute would do, as long as every hook uses the same one; this one is the cheapest.) In the
  single plane the highlighted entry becomes a white bar with blue text.

  How: NTLDR and SETUPLDR.BIN are a 16-bit startup module followed by a PE image (osloader). The
  script finds that image, then the five routines to hook (TextGrSetCurrentAttribute, the glyph
  writer, the two clear routines and the attribute dispatcher that tells where the attribute is kept)
  by their code and the data they use, so it does not depend on the version: the routines are the same
  instructions in the XP SP3 and Server 2003 SP2 files, which are the ones it was tested with (listed
  in $Known). A file that is not one of them gets a warning and is patched anyway if the routines are
  found; if they are not, nothing is changed. Whether such a file works is up to you.

  111 bytes of new code (listed below) go into the zero padding at the end of the first
  executable section with enough room (.text; in the SETUPLDR.BIN of XP SP3, where .text has only 51
  free bytes, INIT, which holds code that is used throughout), and that section's VirtualSize is
  raised to its raw size: the loader copies only min(VirtualSize, SizeOfRawData) bytes of a section.
  The five routines jump or call there with a 5-byte rel32 over whole instructions that nothing
  branches into. The code is the same in every file; only its few address operands (fixups) differ.
  The glyph writer inverts the glyph in the font in place and puts it back before it returns. The
  loader does not look at the PE checksum; it is updated anyway. Scrolling is left as it is; the menus
  and setup's screens do not scroll.

  No Microsoft binary is redistributed: the script patches the user's own files. A file patched before
  is recognised by the new code and left as it is.

  Tested with the zh-hans files: on Hyper-V Gen2 through CSMWrap the highlight is visible and follows
  the arrow keys in the boot menu and the F8 menu (NTLDR loaded by syslinux's chain.c32), and the
  status bar of text-mode setup is a white bar (CD boot); on Hyper-V Gen1 (emulated VGA) screenshots
  of both NTLDR menus are pixel-identical before and after the patch, and so are almost all frames of
  a CD boot through SETUPLDR (the others differ in which message was on the screen).

.PARAMETER InFile
  NTLDR or SETUPLDR.BIN. On the CD they are I386\NTLDR and I386\SETUPLDR.BIN, on an installed system
  NTLDR is at the root of the active partition.

.PARAMETER OutFile
  Where to write the patched file. Defaults to -InFile (patched in place).

.EXAMPLE
  .\Patch-Ntldr.ps1 -InFile E:\NTLDR
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)] [string]$InFile,
  [string]$OutFile
)
$ErrorActionPreference = 'Stop'
if (-not $OutFile) { $OutFile = $InFile }

# The new code. Its address-dependent operands are zero here and filled in per file (Fixups below);
# CURATTR, GLYPHROWS, SETATTR, GLYPH and CURSOR in the listing stand for the attribute byte set last,
# the number of rows of a BOOTFONT.BIN glyph, the instructions after the prologues of
# TextGrSetCurrentAttribute and of the glyph writer, and the routine that ends the glyph writer. The
# glyph writer is stdcall (bits, width 8 or 16, top fill, bottom fill).
$Cave = [byte[]]@(
  # NTLDR 5.1.2600.5512 (osloader at file offset 0x4cc0, image base 0x400000):
  # code put into the zero padding at the end of .text (VA 0x42ad00).
  # Drawn inverted: attributes with a light background (bit 6 of the background; in
  # practice only 0x70, reverse video).
  # jmp from TextGrSetCurrentAttribute: the VGA latches get the foreground colour
  0xF6,0x44,0x24,0x04,0x40,       # 00  test byte [esp+4], 0x40
  0x74,0x05,                      # 05  jz .1
  0xC0,0x44,0x24,0x04,0x04,       # 07  rol byte [esp+4], 4
  0x55,                           # 0C  .1: push ebp
  0x89,0xE5,                      # 0D  mov ebp, esp
  0xE9,0x00,0x00,0x00,0x00,       # 0F  jmp SETATTR  (fixup: SETATTR)
  # jmp from the glyph writer's entry
  0x55,                           # 14  push ebp
  0x89,0xE5,                      # 15  mov ebp, esp
  0xE8,0x0F,0x00,0x00,0x00,       # 17  call flip
  0xE9,0x00,0x00,0x00,0x00,       # 1C  jmp GLYPH  (fixup: GLYPH)
  # call from the glyph writer's last call: puts the font back
  0xE8,0x05,0x00,0x00,0x00,       # 21  call flip
  0xE9,0x00,0x00,0x00,0x00,       # 26  jmp CURSOR  (fixup: CURSOR)
  # Invert the fill bytes and the glyph's bits (in the font) of the glyph writer
  # whose frame is ebp, if the current attribute is drawn inverted.
  0xF6,0x05,0x00,0x00,0x00,0x00,0x40,# 2B  test byte [CURATTR], 0x40  (fixup: CURATTR)
  0x74,0x22,                      # 32  jz .r
  0xF7,0x55,0x10,                 # 34  not dword [ebp+0x10]
  0xF7,0x55,0x14,                 # 37  not dword [ebp+0x14]
  0x8B,0x55,0x08,                 # 3A  mov edx, [ebp+8]
  0x85,0xD2,                      # 3D  test edx, edx
  0x74,0x15,                      # 3F  jz .r
  0x8B,0x0D,0x00,0x00,0x00,0x00,  # 41  mov ecx, [GLYPHROWS]  (fixup: GLYPHROWS)
  0xE3,0x0D,                      # 47  jecxz .r
  0x83,0x7D,0x0C,0x10,            # 49  cmp dword [ebp+0xc], 16
  0x75,0x02,                      # 4D  jne .a
  0x01,0xC9,                      # 4F  add ecx, ecx
  0xF6,0x12,                      # 51  .a: not byte [edx]
  0x42,                           # 53  inc edx
  0xE2,0xFB,                      # 54  loop .a
  0xC3,                           # 56  .r: ret
  # calls from the clear screen and the clear to end of screen routines, in place
  # of  mov ecx,0x2580; xor eax,eax; mov edi,0xa0000  and  lea edi,[eax+0xa0000]; xor eax,eax
  # Out: edi, eax = what to store (0, or all ones for an inverted attribute)
  0xB9,0x80,0x25,0x00,0x00,       # 57  mov ecx, 0x2580
  0x31,0xC0,                      # 5C  xor eax, eax
  0x8D,0xB8,0x00,0x00,0x0A,0x00,  # 5E  lea edi, [eax+0xa0000]
  0xA0,0x00,0x00,0x00,0x00,       # 64  mov al, [CURATTR]  (fixup: CURATTR)
  0xC0,0xE0,0x02,                 # 69  shl al, 2
  0x19,0xC0,                      # 6C  sbb eax, eax
    0xC3                          # 6E  ret
)

# Address-dependent operands: At = offset in $Cave; abs: the 4 bytes are the address Sym; rel: a rel32
# jump to Sym.
$Fixups = @(
  @{ At = 0x10; Kind = 'rel'; Sym = 'SETATTR' },
  @{ At = 0x1D; Kind = 'rel'; Sym = 'GLYPH' },
  @{ At = 0x27; Kind = 'rel'; Sym = 'CURSOR' },
  @{ At = 0x2D; Kind = 'abs'; Sym = 'CURATTR' },
  @{ At = 0x43; Kind = 'abs'; Sym = 'GLYPHROWS' },
  @{ At = 0x65; Kind = 'abs'; Sym = 'CURATTR' }
)

# The routines that are hooked: a jump (0xE9) or call (0xE8) into $Cave at offset To over the bytes
# From (whole instructions) at Site, and Pad more bytes that are replaced by NOPs.
$Hooks = @(
  @{ Name = 'TextGrSetCurrentAttribute'; Site = 'sa'; Op = 0xE9; To = 0x00; Pad = 0; From = @(0x8B,0xFF,0x55,0x8B,0xEC) },
  @{ Name = 'glyph writer, entry'; Site = 'ge'; Op = 0xE9; To = 0x14; Pad = 0; From = @(0x8B,0xFF,0x55,0x8B,0xEC) },
  @{ Name = 'glyph writer, last call'; Site = 'gx'; Op = 0xE8; To = 0x21; Pad = 0; From = @(0xE8) },
  @{ Name = 'clear screen'; Site = 'cls'; Op = 0xE8; To = 0x57; Pad = 7; From = @(0xB9,0x80,0x25,0x00,0x00,0x33,0xC0,0xBF,0x00,0x00,0x0A,0x00) },
  @{ Name = 'clear to end of screen'; Site = 'eos'; Op = 0xE8; To = 0x5E; Pad = 3; From = @(0x8D,0xB8,0x00,0x00,0x0A,0x00,0x33,0xC0) }
)

# SHA-256 of the .text section of the files this was tested with.
$Known = @(
  @{ Name = 'NTLDR, XP SP3 (5.1.2600.5512)'; TextSha = '4D42E725B44BFAC18A7BD5FC38388838A5D05AD64EE6220197BAD10E7F0C8452' },
  @{ Name = 'SETUPLDR.BIN, XP SP3 (5.1.2600.5512)'; TextSha = '7B8CA5ECE163DF0F27E3925DEDAD300B2522A8D07CD3E81A9EF60AA9D48FEE9C' },
  @{ Name = 'NTLDR, Server 2003 SP2 (5.2.3790.3959)'; TextSha = 'AED4089FA7CCBBC3DE3BB67C610CC030DAF163B59B23BB9E82E6840310B2FF52' },
  @{ Name = 'SETUPLDR.BIN, Server 2003 SP2 (5.2.3790.3959)'; TextSha = 'FC63713FAB8746590D168C582C0A856702EDD3BD65445D5C4D2E48DC72E204A3' }
)

$latin = [System.Text.Encoding]::GetEncoding(28591)     # one character per byte, for regular expressions

function Get-U32([byte[]]$b, [long]$o) { [BitConverter]::ToUInt32($b, [int]$o) }

function Get-Sha256([byte[]]$Data, [int]$Offset, [int]$Count) {
  ([System.Security.Cryptography.SHA256]::Create().ComputeHash($Data, $Offset, $Count) |
   ForEach-Object { $_.ToString('X2') }) -join ''
}

function Find-One([string]$Text, [string]$Pattern, [string]$What) {
  $m = [regex]::Matches($Text, $Pattern, [System.Text.RegularExpressions.RegexOptions]::Singleline)
  if ($m.Count -ne 1) {
    throw "$InFile`: found $($m.Count) places that look like $What (expected 1); this does not look like an NTLDR or SETUPLDR.BIN with the 640x480x16 text routines. Nothing was changed."
  }
  $m[0]
}

$bytes = [System.IO.File]::ReadAllBytes($InFile)

# Patched before?
$marker = $latin.GetString($Cave, 0, 15)
if ($latin.GetString($bytes).IndexOf($marker, [System.StringComparison]::Ordinal) -ge 0) {
  Write-Host "$InFile is already patched"
  if ((Resolve-Path $InFile).Path -ne [System.IO.Path]::GetFullPath($OutFile)) {
    [System.IO.File]::WriteAllBytes($OutFile, $bytes)
    Write-Host "wrote $OutFile"
  }
  return
}

# The PE image: an 'MZ' header at a multiple of 16 in the first 64 KB, with a 32-bit PE header behind it.
$pe0 = -1
for ($o = 0x1000; $o -lt [Math]::Min($bytes.Length - 0x400, 0x10000); $o += 0x10) {
  if ($bytes[$o] -ne 0x4D -or $bytes[$o + 1] -ne 0x5A) { continue }
  $lf = [BitConverter]::ToInt32($bytes, $o + 0x3C)
  if ($lf -lt 0x40 -or $lf -gt 0x400) { continue }
  $p = $o + $lf
  if ($p + 24 + 0xE0 + 40 -gt $bytes.Length -or (Get-U32 $bytes $p) -ne 0x4550 -or
      [BitConverter]::ToUInt16($bytes, $p + 4) -ne 0x14C -or [BitConverter]::ToUInt16($bytes, $p + 20) -ne 0xE0) { continue }
  $pe0 = $o; $pe = $p; break
}
if ($pe0 -lt 0) { throw "$InFile`: no PE image found in it; this is not an NTLDR or SETUPLDR.BIN. Nothing was changed." }
$base = Get-U32 $bytes ($pe + 24 + 28)
$sizeOfImage = Get-U32 $bytes ($pe + 24 + 56)
$n = [BitConverter]::ToUInt16($bytes, $pe + 6)
$sec0 = $pe + 24 + 0xE0
$secs = @(for ($i = 0; $i -lt $n; $i++) {
  $h = $sec0 + 40 * $i
  [pscustomobject]@{
    Index = $i; Hdr = $h; Name = $latin.GetString($bytes, $h, 8).TrimEnd([char]0)
    VSize = Get-U32 $bytes ($h + 8); Rva = Get-U32 $bytes ($h + 12)
    RawSize = Get-U32 $bytes ($h + 16); Raw = Get-U32 $bytes ($h + 20); Chars = Get-U32 $bytes ($h + 36)
  }
})
$text = $secs | Where-Object { $_.Name -eq '.text' } | Select-Object -First 1
if (-not $text -or $pe0 + $text.Raw + $text.RawSize -gt $bytes.Length) {
  throw "$InFile`: the PE image has no usable .text section. Nothing was changed."
}

$textSha = Get-Sha256 $bytes ($pe0 + $text.Raw) $text.RawSize
$k = $Known | Where-Object { $_.TextSha -eq $textSha }
if ($k) { Write-Host "$InFile is the $($k.Name)" }
else { Write-Warning "$InFile is not one of the files this was tested with (see `$Known); patching it anyway, at your own risk." }

# The routines, by their code.
$textStr = $latin.GetString($bytes, $pe0 + $text.Raw, $text.RawSize)
$tv = $base + $text.Rva                                   # VA of .text
$mSa  = Find-One $textStr '\x8b\xff\x55\x8b\xec\x83\xec\x0c\xc6\x45\xfb\x00\xc7\x45\xf4\x00\x96\x0a\x00\x66\xba\xce\x03' 'TextGrSetCurrentAttribute'
$mGw  = Find-One $textStr '\x8b\xff\x55\x8b\xec\x8b\x55\x08\x33\xc9\x3b\xd1\x0f\x84....\x39\x0d....\x56\x57\x76.\x8b\x75\x0c\x8a\x45\x10\xc1\xee\x03\x83\x7d\x0c\x10' 'the glyph writer'
$mCls = Find-One $textStr '\x8b\xff\x57\xb9\x80\x25\x00\x00\x33\xc0\xbf\x00\x00\x0a\x00\xf3\xab\x5f\xc3' 'the clear screen routine'
$mEos = Find-One $textStr '\x8d\xb8\x00\x00\x0a\x00\x33\xc0\xf3\xab\x8b\xca\x83\xe1\x03\xf3\xaa\x5f\xc3' 'the clear to end of screen routine'
$mDis = Find-One $textStr '\x83\x3d....\x00\x8b\x45\x08\xa2(....)\x50\x74\x07\xe8....\xeb\x05\xe8....' 'the attribute dispatcher'
$gwBody = $textStr.Substring($mGw.Index, [Math]::Min(0x110, $textStr.Length - $mGw.Index))
$mRows = Find-One $gwBody '\x40\x3b\x05(....)\x72' 'the glyph row loop'
$mGx   = Find-One $gwBody '\xe8(....)\x5d\xc2\x10\x00' 'the end of the glyph writer'
$textOff = $pe0 + $text.Raw
$gx = $tv + $mGw.Index + $mGx.Index
$Site = @{
  sa = $tv + $mSa.Index; ge = $tv + $mGw.Index; gx = $gx
  cls = $tv + $mCls.Index + 3; eos = $tv + $mEos.Index
}
$Sym = @{
  CURATTR = Get-U32 $bytes ($textOff + $mDis.Groups[1].Index)
  GLYPHROWS = Get-U32 $bytes ($textOff + $mGw.Index + $mRows.Groups[1].Index)
  SETATTR = $tv + $mSa.Index + 5
  GLYPH = $tv + $mGw.Index + 5
  CURSOR = $gx + 5 + [BitConverter]::ToInt32($bytes, $textOff + $mGw.Index + $mGx.Groups[1].Index)
}

function Get-FileOffset([long]$Va) {
  $rva = $Va - $base
  foreach ($s in $secs) {
    if ($s.Raw -ne 0 -and $rva -ge $s.Rva -and $rva -lt $s.Rva + $s.RawSize) { return $pe0 + $s.Raw + $rva - $s.Rva }
  }
  throw ("no section has VA 0x{0:X}" -f $Va)
}

# Where the new code goes: the first executable section with room after its virtual size (16-aligned),
# room to grow its virtual size to its raw size before the next section, and zeros there.
$cs = $null
foreach ($s in $secs) {
  if (($s.Chars -band 0x20000000) -eq 0 -or $s.Raw -eq 0) { continue }
  $start = ([int]$s.VSize + 15) -band -16
  if ($start + $Cave.Length -gt $s.RawSize -or $pe0 + $s.Raw + $s.RawSize -gt $bytes.Length) { continue }
  $next = if ($s.Index + 1 -lt $secs.Count) { $secs[$s.Index + 1].Rva } else { $sizeOfImage }
  if ($s.Rva + $s.RawSize -gt $next) { continue }
  $zero = $true
  for ($i = 0; $i -lt $Cave.Length; $i++) { if ($bytes[$pe0 + $s.Raw + $start + $i] -ne 0) { $zero = $false; break } }
  if ($zero) { $cs = $s; $caveStart = $start; break }
}
if (-not $cs) { throw "$InFile`: no executable section has room for the new code. Nothing was changed." }
$caveVa = $base + $cs.Rva + $caveStart
$caveOff = $pe0 + $cs.Raw + $caveStart
Write-Host ("new code: {0} bytes at 0x{1:X} in {2}" -f $Cave.Length, $caveVa, $cs.Name)
[Array]::Copy($Cave, 0, $bytes, $caveOff, $Cave.Length)
foreach ($f in $Fixups) {
  $v = if ($f.Kind -eq 'abs') { [int64]$Sym[$f.Sym] } else { [int64]$Sym[$f.Sym] - ($caveVa + $f.At + 4) }
  [BitConverter]::GetBytes([int32]$v).CopyTo($bytes, $caveOff + $f.At)
}

foreach ($h in $Hooks) {
  $va = $Site[$h.Site]
  $at = Get-FileOffset $va
  for ($i = 0; $i -lt $h.From.Count; $i++) {
    if ($bytes[$at + $i] -ne $h.From[$i]) {
      throw ("{0}: byte at 0x{1:X} is 0x{2:X2}, expected 0x{3:X2}; refusing to patch." -f $h.Name, ($at + $i), $bytes[$at + $i], $h.From[$i])
    }
  }
  $rel = [BitConverter]::GetBytes([int32]($caveVa + $h.To - ($va + 5)))
  $bytes[$at] = $h.Op
  [Array]::Copy($rel, 0, $bytes, $at + 1, 4)
  for ($i = 0; $i -lt $h.Pad; $i++) { $bytes[$at + 5 + $i] = 0x90 }
  Write-Host ("{0}: patched {1} byte(s) at 0x{2:X}" -f $h.Name, (5 + $h.Pad), $at)
}
[BitConverter]::GetBytes([uint32]$cs.RawSize).CopyTo($bytes, $cs.Hdr + 8)               # VirtualSize

# PE checksum (OptionalHeader.CheckSum) of the image, which runs to the end of the file.
$csOff = $pe + 0x58
for ($i = 0; $i -lt 4; $i++) { $bytes[$csOff + $i] = 0 }
[uint64]$sum = 0
for ($i = $pe0; $i -lt $bytes.Length - 1; $i += 2) {
  $sum += [BitConverter]::ToUInt16($bytes, $i)
  if ($sum -gt 0xFFFFFFFF) { $sum = ($sum -band 0xFFFFFFFF) + ($sum -shr 32) }
}
if (($bytes.Length - $pe0) -band 1) { $sum += $bytes[$bytes.Length - 1] }
$sum = ($sum -band 0xFFFF) + ($sum -shr 16)
$sum = ($sum + ($sum -shr 16)) -band 0xFFFF
$sum += [uint64]($bytes.Length - $pe0)
[BitConverter]::GetBytes([uint32]$sum).CopyTo($bytes, $csOff)

[System.IO.File]::WriteAllBytes($OutFile, $bytes)
Write-Host "wrote $OutFile"
