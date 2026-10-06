# xp-hyperv-gen2

Windows XP (x86) drivers for running XP in a Hyper-V Generation 2 VM that
boots through [CSMWrap](https://github.com/CSMWrap/CSMWrap), a UEFI
application that provides a legacy BIOS (SeaBIOS) on UEFI-only machines.

A Gen2 VM has no VGA hardware. The VGA ports read 0xFF, 0xA0000-0xBFFFF is
plain RAM, and there is no PCI bus. After boot the screen keeps showing the
UEFI GOP frame buffer (on Hyper-V: 0xF8000000, 1024x768, 32 bpp, pitch 4096).
XP's own `vga.sys` and `bootvid.dll` need real VGA, so text-mode setup and the
GUI stay black. This repository supplies the missing drivers.

So far it has one driver:

- **hvfb.sys**: a linear frame buffer display miniport. It finds the frame
  buffer through VESA BIOS Extensions (SeaVGABIOS's coreboot frame buffer back
  end in CSMWrap, or any VBE 2.0+ BIOS). If VBE is unavailable it falls back
  to the coreboot table that CSMWrap leaves in low memory. It works both as
  the display miniport of XP text-mode setup (replacing `vga.sys`) and, with
  XP's in-box `framebuf.dll`, as the display driver of the installed system.

## Status

Tested in QEMU/KVM only (CSMWrap + OVMF, `pc` machine, QEMU's VGA with VBE),
with an XP SP3 CD repacked by `tools/xp-iso.sh`:

- text-mode setup draws through hvfb (VBE mode, 640x480x32), through
  partitioning, formatting and file copy;
- GUI-mode setup and the installed system draw through hvfb and
  `framebuf.dll` at 1024x768x32 (`DEFAULT_MODE=1024x768x32`).

Not yet tried on Hyper-V Gen2. Open points:

- Firmware that scans out a text buffer and watches the BDA video mode does
  not see XP's mode switches (see "The INT 10h environment on XP" below).
- Safe Mode and `/basevideo` use only VgaSave and stay black.

## Layout

```
hvfb/hvfb.c        VideoPort entry points and IOCTL handling
hvfb/modes.c       mode discovery: VBE 4F00/4F01 via INT 10h, coreboot table fallback
hvfb/hvfb.h        shared definitions
hvfb/hvfb.rc       version resource
hvfb/hvfb.inf      installs hvfb as a legacy display driver on an installed XP
tools/pecheck.py   checks that a .sys is a valid XP kernel driver (and fixes the checksum if asked)
tools/cdb-check.sh loads the driver and PDB into the Windows debugger (cdb.exe) from WSL
tools/xp-iso.sh    repacks an XP CD so that setup uses hvfb
tools/qemu-xp.sh   boots an XP CD through CSMWrap in QEMU/KVM and takes screendumps
Makefile           builds everything into out/
```

## Building

The build uses clang and lld from the msys2-cross toolchain (`/opt/msys2-cross`),
targeting `i686-w64-mingw32` with the mingw-w64 DDK headers and import
libraries:

```
/opt/msys2-cross/bin/msys-pacman -S msys-cross-clang msys-cross-mingw32-gcc   # once
make              # out/hvfb.sys, out/hvfb.pdb, out/hvfb.inf (+ out/hvfb.map)
make check        # PE checks, see below
make cdb-check    # resolve hvfb!* with the Windows cdb.exe (WSL only; CDB=... to override)
```

`msys-cross-mingw32-gcc` is only needed for the mingw32 sysroot (headers and
import libraries); the compiler is clang. Override `MSYS2_CROSS`, `LLVM_DIR`
or `SYSROOT` if your toolchain lives elsewhere.

clang emits CodeView debug info (`-gcodeview`) and lld writes a PDB
(`--pdb`). WinDbg/KD therefore get full symbols, types and line numbers,
which they cannot get from gcc's DWARF. The linked image:

- is an NT native subsystem 5.01 image with OS version 5.1, base 0x10000 and
  relocations kept (`.reloc`);
- has `DriverEntry@8` as entry point and imports undecorated names from
  kernel modules only (currently just `videoprt.sys`);
- has its PE checksum filled by lld (`/release`), which the NT loader requires
  for boot drivers;
- references its PDB by file name only (`/pdbaltpath:%_PDB%`), so the debugger
  finds it on the symbol path; the `.sys` itself is stripped.

`tools/pecheck.py` checks all of the above after every link. Use
`--against DIR` to also resolve every import against XP's own
`videoprt.sys`/`ntoskrnl.exe`/`hal.dll`.

One trap with the mingw-w64 headers: WDK-built x86 drivers default to
`__stdcall` (`/Gz`), so function pointer typedefs without an explicit
convention are stdcall in Microsoft's headers but cdecl here.
`PINTERFACE_REFERENCE` in `miniport.h` is one of them. Calling such a pointer
from clang/gcc unbalances the stack.

## hvfb.sys

A legacy (non-PnP) VideoPort miniport, like `vga.sys`. It has no power or
child-device callbacks, so videoprt treats it as a legacy driver: HwFindAdapter
is called from DriverEntry and the device appears as `Root\LEGACY_HVFB`.
That is the only model that works in text-mode setup, where there is no
devnode to bind to. AdapterInterfaceType is tried in the order Isa, Internal,
PCIBus. A Gen2 VM has no PCI bus, but NTDETECT always reports an ISA
adapter on PCs; this still has to be confirmed on Gen2.

- **Discovery.** VBE needs INT 10h with a real-mode buffer, so hvfb uses the
  `VideoPortQueryServices(VideoPortServicesInt10)` interface (XP and later).
  videoprt only allows INT 10h after the device's first open, so 4F00/4F01
  run in HwInitialize, not HwFindAdapter. A mode is kept if it is supported,
  graphics, has a linear frame buffer, is direct colour (or packed pixel with
  15 bpp or more) at 15/16/24/32 bpp, and fits in VRAM. The pitch
  (LinBytesPerScanLine for VBE 3.0) and the masks come from VBE. 15-bit
  modes are reported as 16 bpp with 5:5:5 masks and are dropped if a 5:6:5
  mode of the same size exists. If VBE gives nothing, the coreboot table
  frame buffer (`LBIO` header in physical 0-4 KiB, optional `CB_TAG_FORWARD`,
  `CB_TAG_FRAMEBUFFER`) becomes the only mode.
- **Mode order.** Text-mode setup always uses mode 0 (see below), so mode 0
  is chosen for setup. It is the smallest mode of at least 640x480 that gives
  setup exactly 80 text columns, and it is never 24 bpp; deeper colour wins.
  That is normally 640x480x32, which Hyper-V Gen2's SeaVGABIOS offers next
  to 800x600 and 1024x768. `framebuf.dll` also starts in mode 0 when the
  registry has no `DefaultSettings`, so the installed system needs
  `DefaultSettings.*` (or a change in Display Properties) to run at the
  native resolution.
- **IOCTLs.**
  - Mode queries: `QUERY_NUM_AVAIL_MODES`, `QUERY_AVAIL_MODES` and
    `QUERY_CURRENT_MODE` return fully filled `VIDEO_MODE_INFORMATION`.
  - `SET_CURRENT_MODE` calls VBE 4F02 with the linear frame buffer bit.
  - `RESET_DEVICE` sets text mode 3 through INT 10h.
  - Memory mapping: `MAP_VIDEO_MEMORY`/`UNMAP_VIDEO_MEMORY` and
    `SHARE_VIDEO_MEMORY`/`UNSHARE_VIDEO_MEMORY`.
  - No-ops: `SET_COLOR_REGISTERS` (direct colour only), `QUERY_PUBLIC_ACCESS_RANGES`
    (none) and `QUERY_POINTER_CAPABILITIES` (no hardware cursor).
- **Firmware contract.**
  - Graphics modes are set with VBE 4F02 and text mode with INT 10h AH=00h.
    A BIOS that scans a text buffer out to the frame buffer can stop when a
    graphics mode is set.
  - Both calls ask the BIOS not to clear memory (4F02 bit 15, mode 0x83), and
    hvfb clears the frame buffer and the 0xB8000 text page itself.
    SeaVGABIOS's coreboot back end clears a high frame buffer through
    INT 15h AH=87h, which switches to protected mode and cannot run in
    videoprt's virtual-8086 monitor.
- **The INT 10h environment on XP.** videoprt runs INT 10h in V86 mode inside
  the process that first opened the device: CSRSS on an installed system, the
  setup process in text-mode setup. At that open it builds the first megabyte
  of that process:
  - IVT/BDA, EBDA and the ROM areas (C0000-FFFFF) are **private copies**
    taken at that moment.
  - Only the range the miniport reports in
    `VdmPhysicalVideoMemoryAddress` (A0000-BFFFF, as for VGA) is mapped live
    from physical memory.
  - Without that range videoprt skips the setup and every INT 10h fails.
  - So BIOS state written during these calls, such as the BDA video mode byte
    at 0x449, never reaches physical memory. Firmware cannot learn about OS
    mode switches from the physical BDA.

Debug output goes through `VideoPortDebugPrint`. Discovery results print at
level Error, so a free-build kernel debugger shows them by default; per-request
traces print at level Trace.

## How XP loads a legacy display miniport

These findings come from the XP SP3 binaries (setupldr, setupdd.sys,
videoprt.sys, framebuf.dll) and its INF/SIF files.

### Text-mode setup

- On x86, setupldr always uses the display id `vga` and loads the miniport
  listed under that key in `[Display.Load]` of `TXTSETUP.SIF`
  (`vga = vga.sys`). If the key is missing it falls back to `vga.sys`.
  `[Map.Display]` and `[Display]` describe the same id
  (`vga = "Auto Detect",files.none`). The file itself is listed in
  `[SourceDisksFiles]`:

  ```
  vga.sys = 100,,,,,,4_,4,0,0,,1,4
  ```

  | Field | Value | Meaning |
  |---|---|---|
  | 1 | 100 | source disk, `[SourceDisksNames.x86] 100` = `\i386` on the CD |
  | 2-6 | (empty) | subdirectory, size, checksum, unused |
  | 7 | `4_` | file is on boot floppy 4, i.e. setupldr can load it |
  | 8 | 4 | target directory, `[WinntDirectories] 4` = `system32\drivers` |
  | 9 | 0 | upgrade disposition: always copy |
  | 10 | 0 | fresh-install disposition: always copy |
  | 11 | (empty) | new file name |
  | 12-13 | 1,4 | WinPE source disk and directory |

  `[files.vga]` lists `vga.sys` and the VGA display DLLs (including
  `framebuf.dll`) that setup copies for the base video service.
- setupdd.sys opens `\Device\Video0` and queries the modes. It uses its VGA
  text back end only if the miniport offers a non-graphics 720x400 mode.
  Otherwise it uses the frame buffer back end, apparently a remnant of NT's
  RISC ports and of SGI's VGA-less x86 Visual Workstations (`TXTSETUP.SIF`
  still lists `sglfb = "Cobalt"` under `[Display]`).
- On a PC, setup has no configured resolution and always takes **mode 0**.
  Mode 0 must be a graphics mode with at least 8 bpp and at least 640x480.
  The back end draws with 1, 2 or 4 bytes per pixel, so a 24 bpp mode 0
  renders wrongly. Colours come from the masks and `Number*Bits`.
- Setup uses an 8x16 font (16 pixels wide from 160 columns up) and lays its
  screens out for 80 columns. With more columns it crashes:
  `SpvidClearScreenRegion` mirrors cleared cells into an 80-column buffer on
  its stack and computes `79 - x` without a bounds check. At 1024x768
  (128 columns), clearing a region that starts at column 80 or later while
  formatting causes bug check 0x50 in setupdd.sys. Mode 0 must therefore be
  640 pixels wide (or 1280-1295, which also gives 80 columns).
- The IOCTLs setupdd uses are `QUERY_NUM_AVAIL_MODES`, `QUERY_AVAIL_MODES`,
  `SET_CURRENT_MODE` and `MAP_VIDEO_MEMORY`/`UNMAP_VIDEO_MEMORY`. It sends
  `SET_COLOR_REGISTERS` only for palette modes.
- If anything fails, setupdd prints error 0x239c ("Setup encountered an error
  while initializing your computer's video") through bootvid and stops. On a
  machine without VGA this looks like a hang.
- The optional third field of a `[Display]` entry names a service, for
  example `vga = "Auto Detect",files.none,hvfb`. setupdd then writes the mode
  it used to `Services\<svc>\Device0\DefaultSettings.*` in the new system.

So text-mode setup uses hvfb after two `TXTSETUP.SIF` changes:
`[Display.Load] vga = hvfb.sys`, and `hvfb.sys` in `[SourceDisksFiles]` with
the same attributes as `vga.sys`. Setupldr loads uncompressed files from
`\I386` as well, so the driver needs no `makecab`.

### Installed system

- XP's in-box legacy display miniports are declared in `HIVESYS.INF`. For
  example, VgaSave has
  `Start=1, Type=1, Group="Video Save", ImagePath=\SystemRoot\System32\drivers\vga.sys`,
  and its `Device0` has `InstalledDisplayDrivers` and `VgaCompatible`.
- videoprt handles a legacy miniport at boot as follows:
  - It calls `IoQueryDeviceDescription` for the requested bus type. If no such
    bus exists, HwFindAdapter is never called.
  - It reports the device as `Root\LEGACY_<SVC>`.
  - On the first boot it creates `Services\<svc>\Video\VideoID`, copies
    `Services\<svc>\Device0` to `Control\Video\{VideoID}\0000`, and points
    `HARDWARE\DEVICEMAP\VIDEO\Device\VideoN` there. The copy is made only once.
- Legacy initialisation is refused once win32k has opened a display device. A
  legacy miniport is therefore a boot-start (`Start=1`) service in group
  `Video`, and it takes effect after a reboot.
- win32k uses the first display device that is not VgaSave as the primary.
  `framebuf.dll` is named by `InstalledDisplayDrivers`. `VgaCompatible=0`
  marks the device as not VGA. On Gen2, VgaSave's HwFindAdapter is expected
  to find no VGA and fail harmlessly (`ErrorControl=0`).
- Safe Mode and `/basevideo` load only VgaSave (`vga.sys`), so they stay black
  on Gen2.

`hvfb.inf` installs hvfb this way on a running XP. It is a `DefaultInstall`
INF, because there is no PnP device to match. Right-click it and choose
Install, or run:

```
rundll32 setupapi.dll,InstallHinfSection DefaultInstall 132 <dir>\hvfb.inf
```

Then reboot. The INF copies `hvfb.sys`, creates the service and writes
`Device0` (`InstalledDisplayDrivers=framebuf`, `VgaCompatible=0`).
`framebuf.dll` already ships in `system32`. There are no `DefaultSettings`,
so the first boot uses mode 0 (640x480). Choose another resolution in Display
Properties, or uncomment the `DefaultSettings.*` lines in the INF.

To have setup install hvfb into the new system, `tools/xp-iso.sh` with
`INSTALL=1` (the default) does three things:

1. keeps the `[SourceDisksFiles]` entry, which copies `hvfb.sys` to
   `system32\drivers`;
2. adds the hvfb service and `Device0` values to `HIVESYS.INF` next to
   VgaSave;
3. sets the initial resolution. With `DEFAULT_MODE=1024x768x32`, it writes
   `DefaultSettings.*` into `HIVESYS.INF`. Without it, it sets the
   `[Display]` service field, so GUI-mode setup and the installed system
   start in the mode text-mode setup used (640x480).

## Testing

`tools/xp-iso.sh BASE.iso OUT.iso` repacks an XP CD with hvfb.
`tools/qemu-xp.sh` boots it through CSMWrap in QEMU/KVM. QEMU has a real VGA
with VBE, so the test exercises the VBE path; Hyper-V Gen2 differs in having
no VGA at all. For example:

```
make
tools/xp-iso.sh ~/Projects/CSMWrap/hyperv/WinLite_En_mp.iso /tmp/hvfb-iso/xp-hvfb.iso
ISO=/tmp/hvfb-iso/xp-hvfb.iso EFI=.../csmwrap.efi GROW_MB=3072 tools/qemu-xp.sh
```

`PRODUCT_KEY_FILE=<file>` puts a product key into the copy's `WINNT.SIF`;
keep key files out of this repository. Screendumps land in `$Q` (default
`/tmp/hvfb-qemu`). Send keys through the
QEMU monitor socket `$Q/mon.sock` (`sendkey ret`), or schedule them with
`KEYS=file`.

For kernel debugging, put `out/` on the symbol path (`.sympath+ <dir>`).
The PDB matches `out/hvfb.sys` by GUID and age.

## License

TODO: not chosen yet. Until a license is added, all rights are reserved by
the authors.
