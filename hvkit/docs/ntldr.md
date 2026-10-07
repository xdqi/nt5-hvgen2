# NTLDR's and SETUPLDR's mode 12h screens

```
hvkit patch ntldr I386/NTLDR            # in place
hvkit patch ntldr SETUPLDR.BIN -o OUT   # setup-cd applies it to the CD's two loaders
hvkit patch ntldr NTLDR --check         # stock, patched or not patchable
```

The recipe makes reverse video, and with it the loaders' menus, visible on Hyper-V Gen2 through
CSMWrap.

With `BOOTFONT.BIN` (Chinese, Japanese and Korean XP), NTLDR and SETUPLDR draw their screens (the
boot.ini menu, the F8 Advanced Options and Windows Error Recovery menus, text-mode setup's status
bar) in VGA mode 12h with their own font and leave the colours to the VGA:
TextGrSetCurrentAttribute loads the latches with the background colour, selects the XOR function
and enables set/reset for the planes on which foreground and background agree; the glyph writer
stores only the glyph bits (1 = foreground) and the clear routines store zeros. CSMWrap's SeaVGABIOS
keeps mode 12h as one bit plane in RAM (`vga_planar_approx`), so the attribute is lost and reverse
video (`ESC[7m`, attribute 0x70, black on light grey: the menu highlight, the status bar) looks like
normal text. The menus show but cannot be used.

For an attribute with a light background (bit 6 set; in practice only 0x70, the loaders use no
other), the patch hands TextGrSetCurrentAttribute the attribute with foreground and background
swapped and inverts every byte stored while it is current: the glyph bits (in the font, put back
when the glyph writer returns), the top and bottom fill bytes, and the zeros of the clear routines.
On VGA hardware that draws the same pixels (where the colours differ, ~bits XOR fg = bits XOR bg;
elsewhere set/reset gives fg = bg); in the single plane the highlight becomes a white bar with blue
text. Scrolling is left alone; the menus and setup's screens do not scroll.

The loaders are a 16-bit startup module followed by a PE image (osloader). The five hooked routines
(TextGrSetCurrentAttribute, the glyph writer, the two clear routines and the attribute dispatcher
that tells where the attribute is kept) are found by their code and data, not by address: NTLDR and
SETUPLDR.BIN of XP SP3 (5.1.2600.5512) and Server 2003 SP2 (5.2.3790.3959), the four tested files,
have the same instructions at other addresses in every language. Another file gets a warning and is
patched only if all five are found. 111 bytes of new code go into the zero padding at the end of the
first executable section with room (`.text`; in XP's SETUPLDR.BIN, where `.text` has only 51 free
bytes, `INIT`, which holds code used throughout), reached by 5-byte jumps or calls over whole
instructions. That section's VirtualSize is raised to its raw size, as the loader copies only
min(VirtualSize, SizeOfRawData) bytes. The PE checksum is updated, though the loader ignores it. A
patched file is recognised by the new code and left alone.

Tested with the zh-hans XP SP3 and Server 2003 R2 SP2 files on Hyper-V Gen2 through CSMWrap: the
highlight follows the arrow keys in NTLDR's boot and F8 menus, and text-mode setup's status bar (CD
boot) is a white bar. On Gen1 (emulated VGA), both NTLDR menus are pixel-identical before and after
the patch, as are almost all frames of a CD boot through SETUPLDR (the others differ in which
message was on the screen).
