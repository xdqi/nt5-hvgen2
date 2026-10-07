# bootvid.dll

A frame buffer replacement for XP's `bootvid.dll`, for Hyper-V Gen2 through CSMWrap. The kernel
imports `bootvid.dll` directly: `InbvDriverInitialize` calls `VidInitialize` early in phase 1, and
the `Inbv*` functions draw the boot screen, the `/sos` driver list and bug check screens through
the other `Vid*` exports, without the display driver (hvfb or `vga.sys`). XP's own DLL programs VGA
planar mode 12h and writes to 0xA0000, so on Gen2 even a bug check leaves the screen unchanged.

## Interface

As in XP SP3's `bootvid.dll` (5.1.2600.0, which SP3 still ships) and the import tables of
`ntoskrnl.exe`, `ntkrnlmp.exe` and `ntkrpamp.exe`: all `__stdcall`, exported by undecorated name;
stack bytes are each function's `ret N`.

| Ordinal | Export | Stack bytes |
|---|---|---|
| 1 | `VOID VidBitBlt(PUCHAR Bitmap, ULONG Left, ULONG Top)` | 12 |
| 2 | `VOID VidBufferToScreenBlt(PUCHAR Buffer, ULONG Left, ULONG Top, ULONG Width, ULONG Height, ULONG Delta)` | 24 |
| 3 | `VOID VidCleanUp(VOID)` | 0 |
| 4 | `VOID VidDisplayString(PUCHAR String)` | 4 |
| 5 | `VOID VidDisplayStringXY(PUCHAR String, ULONG Left, ULONG Top, BOOLEAN Transparent)` | 16 |
| 6 | `BOOLEAN VidInitialize(BOOLEAN SetMode)` | 4 |
| 7 | `VOID VidResetDisplay(BOOLEAN HalReset)` | 4 |
| 8 | `VOID VidScreenToBufferBlt(PUCHAR Buffer, ULONG Left, ULONG Top, ULONG Width, ULONG Height, ULONG Delta)` | 24 |
| 9 | `VOID VidSetScrollRegion(ULONG Left, ULONG Top, ULONG Right, ULONG Bottom)` | 16 |
| 10 | `ULONG VidSetTextColor(ULONG Color)` (returns the previous colour) | 4 |
| 11 | `VOID VidSolidColorFill(ULONG Left, ULONG Top, ULONG Right, ULONG Bottom, UCHAR Color)` | 20 |

The kernel imports all but `VidDisplayStringXY`. mingw-w64's `libbootvid.a` declares
`VidInitialize@8`, but XP's takes one argument. Rectangles are inclusive, coordinates are pixels of
a 640x480 screen, colours are indices into a 16-entry palette.

## Design

- **Frame buffer** from the coreboot table CSMWrap leaves at physical 0x500
  (`drivers/common/cbtable.c`, shared with hvfb), mapped uncached with `MmMapIoSpace`. VBE is not
  used: bootvid runs before videoprt's INT 10h support exists, and bug checks run at `HIGH_LEVEL`.
  Without a usable record (15, 16, 24 or 32 bpp, at least 640x480) `VidInitialize` returns FALSE
  and the kernel runs without a boot video driver.
- **Screen.** Each pixel's colour index is kept in memory, and every export copies the rectangle it
  changed to the frame buffer, which is never read. The 640x480 screen is scaled by the largest
  integer factor that fits (1 at 1024x768) and centred; the border takes the colour of the last
  full-screen fill, so a bug check screen is blue to the edges.
- **Palette.** 6-bit DAC values as on the VGA, by default in Windows' 16-colour order (4 is the bug
  check's #000080, 15 white). `VidBitBlt` loads the bitmap's colour table, and a palette change
  rewrites the pixels whose entry changed: the kernel draws the boot screen with an all-black table
  and fades it in by drawing a 1x1 bitmap at y = 480 once per progress bar tick.
- **Text.** 8x13 code page 437 font on 14-pixel lines (34 lines in the bug check's scroll region
  0-475), drawn transparently. `font.c` is made by `make font BDF=...` (`tools/mkfont.py`) from the
  public domain `8x13.bdf` of the X.Org misc-misc fonts. As in the original, `VidDisplayString`
  saves each new line's background and restores it on a CR without LF (to rewrite the line) and
  into the line freed by scrolling; the saved line sits below the screen, where scrolling reads.
- **Bitmaps.** `VidBitBlt` draws 4 bpp `BI_RGB` (bottom-up or top-down) and `BI_RLE4`, which covers
  the kernel's bitmaps. The 4 bpp buffers of `VidBufferToScreenBlt`/`VidScreenToBufferBlt` have the
  left pixel in the high nibble.
- **No `HalResetDisplay`.** The original calls it (through `HalPrivateDispatchTable`) in
  `VidInitialize` and `VidResetDisplay`; on halmacpi it runs INT 10h AX=0012h in real mode, which
  would only make the BIOS change what it scans out.
- **Handoffs.** CSMWrap's B8000 text scan-out redraws only changed cells and the kernel never writes
  B8000, so they do not interfere (none seen on Gen2). Once a display driver owns the screen the
  kernel stops calling bootvid; a bug check takes the display back through `VidResetDisplay`, which
  restores the default palette and redraws the whole frame buffer.

## Installing

Only for firmware that leaves a coreboot frame buffer record (CSMWrap on a GOP frame buffer); keep
XP's own where there is a real VGA.

- Text-mode setup: `hvkit hvfb-cd ... --bootvid out/bootvid.dll` replaces the CD's
  `I386\BOOTVID.DL_` with the uncompressed DLL, which SETUPLDR loads for the setup kernel and setup
  copies to `system32` (`[SourceDisksFiles]` unchanged).
- Installed system: offline, replace `system32\bootvid.dll` and, since Windows File Protection
  lists it, `system32\dllcache\bootvid.dll`. `migrate/inject.ps1 -Bootvid` does both and keeps
  XP's DLL as `system32\bootvid.xp`.

## Files

```
bootvid.c    the Vid* exports: 640x480 colour index screen, palette, text, bitmaps
bootvid.h    interface and screen/font constants
font.c       8x13 code page 437 font, generated from X.Org's 8x13.bdf by tools/mkfont.py
bootvid.def  export names and ordinals; linked as a DLL with --kill-at (undecorated names)
```

bootvid prints the frame buffer it found through `DbgPrint` ("bootvid: frame buffer 1024x768, 32
bpp, ..."). With a kernel debugger attached, XP breaks in before drawing the bug check screen;
continue (`g`) to see it.
