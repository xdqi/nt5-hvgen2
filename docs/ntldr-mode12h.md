# NTLDR's and SETUPLDR's mode 12h screens

With `BOOTFONT.BIN` (Chinese, Japanese and Korean XP), NTLDR and SETUPLDR
draw their screens - the boot.ini menu, the F8 Advanced Options menu, the
Windows Error Recovery menu, the status bar of text-mode setup - in VGA mode
12h with their own font, and leave the colours to the VGA hardware: they
load the VGA latches with the background colour of the attribute, set the
XOR function and set/reset, and then store only the glyph bits. On Gen2,
SeaVGABIOS keeps mode 12h as a single bit plane in RAM (CSMWrap's
`vga_planar_approx`), so the attribute is lost and reverse video (`ESC[7m`,
black on light grey: the highlighted menu entry, the status bar) looks like
normal text. The menus show but cannot be used.

`migrate/Patch-Ntldr.ps1` patches the NTLDR and the SETUPLDR.BIN. For an
attribute with a light background (bit 6 set; in practice only 0x70) it
swaps foreground and background for the VGA latches and inverts the bytes
that are stored. On VGA hardware that draws the same pixels; in the single
plane reverse video becomes a white bar with blue text. The loaders use no
other attributes. 111 bytes of new code go into the padding at the end of
`.text`, or, in XP's SETUPLDR.BIN, of its `INIT` section (which holds code
that is used throughout; `.text` has only 51 free bytes there), five 5-byte
jumps reach it, and the section's VirtualSize is raised to its raw size,
because the loader copies only that many bytes. The script's header has the
details.

The script finds the five routines it hooks by their code, not by version:
they are the same instructions in the NTLDR and SETUPLDR.BIN of XP SP3
(5.1.2600.5512) and of Server 2003 SP2 (5.2.3790.3959), in any language (only
the message resources differ), at other addresses. Those four files are the
ones it was tested with. Any other file gets a warning and is patched anyway
if the routines are found, and left alone if they are not; whether such a
file works is up to you. A file that was patched before is recognised and
left as it is. It patches in place unless given `-OutFile`:

```
powershell -ExecutionPolicy Bypass -File migrate\Patch-Ntldr.ps1 -InFile E:\NTLDR
```

Tested with the zh-hans files of XP SP3 and Server 2003 R2 SP2 on Hyper-V Gen2
through CSMWrap: the highlight follows the arrow keys in NTLDR's boot menu
and F8 menu, and the status bar of text-mode setup (CD boot) is a white bar.
On Gen1 (emulated VGA), screenshots of both NTLDR menus are pixel-identical
before and after the patch, and so are almost all frames of a CD boot through
SETUPLDR (the others differ in which message was on the screen).
