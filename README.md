# nt5-hvgen2

Windows XP (x86) drivers for running XP in a Hyper-V Generation 2 VM that
boots through [CSMWrap](https://github.com/CSMWrap/CSMWrap), a UEFI
application that provides a legacy BIOS (SeaBIOS) on UEFI-only machines.

A Gen2 VM has no VGA hardware. The VGA ports read 0xFF, 0xA0000-0xBFFFF is
plain RAM, and there is no PCI bus. After boot the screen keeps showing the
UEFI GOP frame buffer (on Hyper-V: 0xF8000000, 1024x768, 32 bpp, pitch 4096).
XP's own `vga.sys` and `bootvid.dll` need real VGA, so text-mode setup and the
GUI stay black. This repository supplies the missing drivers.

Drivers:

- **hvfb.sys**: a linear frame buffer display miniport. It finds the frame
  buffer through VESA BIOS Extensions (SeaVGABIOS's coreboot frame buffer back
  end in CSMWrap, or any VBE 2.0+ BIOS). If VBE is unavailable it falls back
  to the coreboot table that CSMWrap leaves in low memory. It works both as
  the display miniport of XP text-mode setup (replacing `vga.sys`) and, with
  XP's in-box `framebuf.dll`, as the display driver of the installed system.
- **bootvid.dll**: a replacement for the kernel's boot video DLL, which draws
  the boot screen, the `/sos` text and bug check ("blue") screens. It draws
  XP's 640x480 16-colour screen on the frame buffer from the coreboot table
  instead of programming a VGA.
- **bootwait.sys**: a boot-start helper that holds the boot until the boot
  partition exists and the mount manager knows its volume (in text-mode
  setup booted from a CD: until there is a CD-ROM drive). On Gen2 the boot
  disk is on the VMBus SCSI controller, which XP's boot sequence would
  otherwise never get to see (bug check 0x7B), and on the first boots of a
  new installation nobody would give its volume a drive letter (0xC000021A).
- **mdlex.sys**: a one-function kernel export driver that gives XP the two
  ntoskrnl routines Microsoft's Dynamic Memory driver `dmvsc.sys` needs but XP
  does not provide, so that an import-patched `dmvsc.sys` runs on XP and
  Hyper-V Dynamic Memory (balloon) works. See
  [Hyper-V Dynamic Memory](#hyper-v-dynamic-memory-mdlexsys-and-dmvsc).
- **vmbecho.sys**: a test driver for a VMBus channel that a program on the
  host offers to the VM, the transport for paravirtual devices whose back end
  is a host program. See
  [VMBus pipes from a host program](#vmbus-pipes-from-a-host-program-vmbechosys).
- **vmbaud.sys**: a sound card. A PortCls WaveCyclic render driver that sends
  the PCM over such a VMBus pipe to a program on the host, which plays it.
  Hyper-V has no sound device for a Windows XP guest otherwise. See
  [Sound (vmbaud.sys)](#sound-vmbaudsys). Under the MS-PL (see
  [License](#license)).

`w9x/` does the same for Windows 98 SE: gen2leg.vxd (the legacy devices a
Gen2 VM lacks, and the VMBus keyboard and mouse), a display driver, and a disk
that installs Windows with them; see [w9x/README.md](w9x/README.md).

## Status

Tested in QEMU/KVM only (CSMWrap + OVMF, `pc` machine, QEMU's VGA with VBE),
with an XP SP3 CD repacked by `hvkit hvfb-cd`:

- text-mode setup draws through hvfb (VBE mode, 640x480x32), through
  partitioning, formatting and file copy;
- GUI-mode setup and the installed system draw through hvfb and
  `framebuf.dll` at 1024x768x32 (`--default-mode 1024x768x32`).

On Hyper-V Gen2, an installed XP SP3 (moved over from a Gen1 VM with the
2012 R2 Integration Services, booting from the VMBus SCSI disk through
storvsc.sys, a Server 2003 storport update and bootwait.sys) reaches the
desktop: on its first Gen2 boot with hvfb and `framebuf.dll` at
1024x768x32, afterwards with the Integration Services' Hyper-V Video
driver. `migrate/Convert-XPToGen2.ps1` makes such a disk from the Gen1 one
(see [Moving an installed XP to Gen2](#moving-an-installed-xp-to-gen2)).

bootvid.dll is tested on Hyper-V Gen2 through CSMWrap, in text-mode setup
of a CD repacked with it: the kernel's bug check screen (0x7B, no storage
driver yet) is readable, and without `/noguiboot` the boot screen appears
(caught fading in when the 0x7B stops the boot). A full boot with the
progress bar running is not tried yet.

Open points:

- Firmware that scans out a text buffer and watches the BDA video mode does
  not see XP's mode switches (see "The INT 10h environment on XP" below).
- Safe Mode and `/basevideo` use only VgaSave and stay black.

## Layout

```
Makefile           the toolchain, shared pattern rules and entry points (make, make check, make w9x),
                   building everything into out/; each component's rules are in a *.mk next to it
drivers/           the NT 5.x drivers, one directory each, with their INF, host side and test client
drivers/*/*.mk     each driver's build rules
drivers/hvfb/hvfb.c        VideoPort entry points and IOCTL handling
drivers/hvfb/modes.c       mode discovery: VBE 4F00/4F01 via INT 10h, coreboot table fallback
drivers/hvfb/hvfb.h        shared definitions
drivers/hvfb/hvfb.rc       version resource
drivers/hvfb/hvfb.inf      installs hvfb as a legacy display driver on an installed XP
drivers/bootvid/bootvid.c  the Vid* exports: 640x480 colour index screen, palette, text, bitmaps
drivers/bootvid/bootvid.h  interface and screen/font constants
drivers/bootvid/font.c     8x13 code page 437 font (generated by tools/mkfont.py)
drivers/bootvid/bootvid.def export names and ordinals
drivers/bootvid/bootvid.rc version resource
drivers/bootwait/bootwait.c boot driver reinitialization routine that waits for the boot partition
drivers/bootwait/bootwait.rc version resource
drivers/bootwait/bootwait.inf installs bootwait as a boot-start service on an installed XP
drivers/mdlex/mdlex.c      the two ntoskrnl routines dmvsc.sys needs that XP lacks (MmAllocatePagesForMdlEx, MmAddPhysicalMemory)
drivers/mdlex/mdlex.def    export names/ordinals (undecorated, --kill-at)
drivers/mdlex/mdlex.rc     version resource
drivers/vmbecho/vmbecho.c  function driver for a host-offered VMBus pipe: echoes what the host writes
drivers/vmbecho/vmbecho.rc version resource
drivers/vmbecho/vmbecho.inf installs vmbecho for VMBUS\{39868fad-8ee5-403c-9d09-2ac377fe9889}
drivers/vmbecho/vmbecho-host.ps1  offers the pipe from the host and talks to vmbecho (PS 5.1, elevated)
drivers/vmbaud/adapter.cpp DriverEntry, AddDevice and StartDevice: the PortCls adapter with a wave and a topology filter
drivers/vmbaud/minwave.cpp the WaveCyclic miniport and its filter description (wavtable.h)
drivers/vmbaud/minstream.cpp the render stream: IMiniportWaveCyclicStream and IDmaChannel, the pipe I/O and the stream clock
drivers/vmbaud/mintopo.cpp the topology miniport: one volume node (toptable.h)
drivers/vmbaud/common.cpp  adapter common object, power management stub, CUnknown, operator new and 64-bit division helpers
drivers/vmbaud/helpers.cpp property helpers
drivers/vmbaud/vmbaud.h    wire protocol, interface GUID, clock constants
drivers/vmbaud/vmbaud.inf  installs vmbaud as a sound device for VMBUS\{8b57f4e3-2a3c-4f6e-9c8d-1e5a70b9c4d2}
drivers/vmbaud/vmbaud.rc   version resource
drivers/vmbaud/vmbaud-host.ps1  offers the sound device from the host and plays its PCM through winmm (PS 5.1, elevated)
drivers/vmbaud/LICENSE     the MS-PL, which covers everything in drivers/vmbaud/
drivers/vmbaud/testplay.c  XP console program that plays a tone through winmm: the test client
drivers/vmbaud/tray/       the host program (x64): vmbaudtray.exe, the tray application, and vmbaudcli.exe
drivers/vmbaud/tray/hvhost.cpp      Hyper-V WMI: VMs, their state changes, the host-only KVP settings
drivers/vmbaud/tray/pipechannel.cpp vmbuspiper.dll: offer, connect, read, write
drivers/vmbaud/tray/vmsession.cpp   one worker thread per VM: offers, wire protocol, CONSUMED, reconnecting
drivers/vmbaud/tray/audioout.cpp    one WASAPI stream (and volume mixer entry) per VM
drivers/vmbaud/tray/trayui.cpp      tray icon, settings dialog, autostart task; main.cpp, vmbaudtray.rc
drivers/vmbaud/tray/cli.cpp         vmbaudcli.exe: settings and a session from the command line
drivers/common/portcls.def import library definition for XP's portcls.sys
drivers/common/ddk_compat.h definitions the toolchain's portcls.h needs but does not get under C++
drivers/common/cbtable.c   coreboot table frame buffer lookup, shared by hvfb and bootvid
guest/predev/predev.c  pre-installs VMBus devices that appear only after setup (vmbaud, the Guest Service
                   Interface): run from a setup CD's cmdlines.txt; guest/predev/predev.mk
guest/vsstest/vsstest.c  XP VSS requester probe for diagnosing the Integration Services' backup path
migrate/Patch-Dmvsc.ps1  rebinds dmvsc.sys's two missing ntoskrnl imports to mdlex.sys (import-table patch, PS 5.1)
migrate/IcSvcGuestInterface.ps1  patches icsvc.dll so that the Guest Service Interface (Copy-VMFile) works on XP (PS 5.1)
migrate/Convert-XPToGen2.ps1  copies an installed XP's Gen1 disk into a Gen2 disk (and optionally a VM)
migrate/Convert-XPToGen2.cmd  double-click / drag-and-drop wrapper for it
migrate/README.txt  the converter package's instructions
migrate/inject.ps1 prepares an installed XP's disk (Gen1, Integration Services 6.3) for Gen2, offline
migrate/Patch-Ntldr.ps1  makes NTLDR and SETUPLDR.BIN (XP SP3, Server 2003 SP2) show reverse video in a single-plane mode 12h (PS 5.1)
hvkit/             one Rust tool for patches, registry hives, setup CDs, disks and offline changes (see
                   hvkit/README.md), replacing scripts step by step
acpi/dsdt.asl      the DSDT CSMWrap gives the guest (acpi/build.sh compiles it)
tools/pecheck.py   checks that a .sys/.dll is a valid XP kernel image (and fixes the checksum if asked)
tools/cdb-check.sh loads the driver and PDB into the Windows debugger (cdb.exe) from WSL
tools/mkfont.py    converts a BDF font into drivers/bootvid/font.c
tools/qemu-xp.sh   boots an XP CD through CSMWrap in QEMU/KVM and takes screendumps
tools/mkdist.sh    assembles the converter package (zip)
w9x/gen2leg/       gen2leg.vxd (Windows 98): legacy PIC/PIT/i8042 for the patched VxDs, VMBus keyboard and mouse
w9x/vmbc/          VMBus client on the connection SeaBIOS made (gen2leg's; dos/ is a DOS test of it)
w9x/vmdisp9x/      builds vmdisp9x's VESA driver with the fixes Windows 98 on Gen2 needs
w9x/setup/         MSBATCH.INF template, display and monitor INFs, AUTOEXEC.BAT, MBR of the install disk
w9x/w9x.mk         make w9x
```

## Building

The build uses clang and lld from the
[msys2-cross](https://github.com/xdqi/msys-cross) toolchain, targeting
`i686-w64-mingw32` with the mingw-w64 DDK headers and import libraries.
Install the msys2-cross bootstrap into `/opt/msys2-cross` as described in
[its README](https://github.com/xdqi/msys-cross#install) (steps 1–3), then
add the two packages this build needs:

```
/opt/msys2-cross/bin/msys-pacman -Sy msys-cross-clang msys-cross-mingw32-gcc  # once
/opt/msys2-cross/bin/msys-pacman -Sy msys-cross-mingw64-gcc   # once, for the x64 host programs
make              # out/hvfb.sys, out/bootvid.dll, out/bootwait.sys, out/mdlex.sys, PDBs, INFs (+ .map),
                  # vmbaud.sys, testplay.exe, and the host's vmbaudtray.exe and vmbaudcli.exe
make check        # PE checks, see below; XPBIN=<dir with XP's binaries> adds import/export checks
make cdb-check    # resolve hvfb!* and bootvid!* with the Windows cdb.exe (WSL only; CDB=... to override)
```

`msys-cross-mingw32-gcc` is only needed for the mingw32 sysroot (headers and
import libraries); the compiler is clang. `msys-cross-mingw64-gcc` is the
same for x86_64: the sound card's host program must be 64-bit, as
`vmbuspiper.dll` exists only in System32. Override `MSYS2_CROSS`, `LLVM_DIR`
or `SYSROOT` if your toolchain lives elsewhere.

clang emits CodeView debug info (`-gcodeview`) and lld writes a PDB
(`--pdb`). WinDbg/KD therefore get full symbols, types and line numbers,
which they cannot get from gcc's DWARF. The linked image:

- is an NT native subsystem 5.01 image with OS version 5.1, base 0x10000 and
  relocations kept (`.reloc`);
- has `DriverEntry@8` as entry point and imports undecorated names from
  kernel modules only (`videoprt.sys` for hvfb, `ntoskrnl.exe` for bootvid);
- has its PE checksum filled by lld (`/release`), which the NT loader requires
  for boot drivers;
- references its PDB by file name only (`/pdbaltpath:%_PDB%`), so the debugger
  finds it on the symbol path; the `.sys` itself is stripped.

`bootvid.dll` is linked as a DLL (`IMAGE_FILE_DLL`) whose exports are the
undecorated names and ordinals of `drivers/bootvid/bootvid.def` (`--kill-at` strips
the `@N` stdcall suffixes).

`tools/pecheck.py` checks all of the above after every link. Use
`--against DIR` to also resolve every import against XP's own
`videoprt.sys`/`ntoskrnl.exe`/`hal.dll`, and `--exports-like` to compare
bootvid's export table with XP's `bootvid.dll` (`make check XPBIN=DIR` does
both).

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
  to 800x600 and 1024x768. Without `DefaultSettings` win32k also starts the
  installed system at 640x480, so `hvfb.inf` and `migrate/inject.ps1` set
  1024x768x32 (see [Installed system](#installed-system) for where).
- **Centred modes.** CSMWrap's SeaVGABIOS emulates VBE on the UEFI GOP frame
  buffer, which Hyper-V always scans out at its native size: every mode
  shares the GOP base address and pitch, and 4F02 only changes what the BIOS
  draws. A 640x480 mode would sit in the top left corner of the 1024x768
  screen, so hvfb centres modes that are smaller than the frame buffer of
  the coreboot table record when the firmware looks like this (VBE video
  memory no larger than that frame buffer, same base, pitch and depth). The
  picture then starts at an offset into the frame buffer and keeps the
  native pitch, as in bootvid.dll. Every mode set clears the border (and the
  picture unless `VIDEO_MODE_NO_ZERO_MEMORY`); a centred mode reports no
  off-screen memory, because everything around the picture is visible. A VGA
  BIOS that programs the display per mode (QEMU's) is left alone.
- **IOCTLs.**
  - Mode queries: `QUERY_NUM_AVAIL_MODES`, `QUERY_AVAIL_MODES` and
    `QUERY_CURRENT_MODE` return fully filled `VIDEO_MODE_INFORMATION`.
  - `SET_CURRENT_MODE` calls VBE 4F02 with the linear frame buffer bit.
  - `RESET_DEVICE` sets text mode 3 through INT 10h.
  - Memory mapping: `MAP_VIDEO_MEMORY`/`UNMAP_VIDEO_MEMORY` and
    `SHARE_VIDEO_MEMORY`/`UNSHARE_VIDEO_MEMORY`. Mappings start at the first
    pixel of the picture, so `VideoRamBase` equals `FrameBufferBase` as
    `framebuf.dll` expects; for a centred mode that address is not page
    aligned, which VideoPort handles.
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

## bootvid.dll

The kernel imports `bootvid.dll` directly: `InbvDriverInitialize` calls
`VidInitialize` early in phase 1, and the `Inbv*` functions draw the boot
screen, the `/sos` driver list and bug check screens through the other
`Vid*` exports. The display driver (hvfb or `vga.sys`) is not involved. XP's
own `bootvid.dll` programs the VGA registers for planar mode 12h and writes
to 0xA0000, so on Gen2 even a bug check leaves the screen as it was.

### Interface

The exports, their ordinals and calling conventions come from XP SP3's own
`bootvid.dll` (version 5.1.2600.0, which SP3 still ships; the stack bytes are
the `ret N` of each function) and from the import tables of `ntoskrnl.exe`,
`ntkrnlmp.exe` and `ntkrpamp.exe`. All are `__stdcall`, exported by
undecorated name:

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

The kernel imports all of them except `VidDisplayStringXY`. mingw-w64's
`libbootvid.a` declares `VidInitialize@8`, but XP's takes one argument.
Rectangles are inclusive, coordinates are pixels of a 640x480 screen and
colours are indices into a 16-entry palette.

### Design

- **Frame buffer.** `VidInitialize` looks up the frame buffer in the
  coreboot table that CSMWrap leaves at physical 0x500 (`drivers/common/cbtable.c`,
  the same code as hvfb's fallback) and maps it uncached with
  `MmMapIoSpace`. VBE is no option: bootvid runs before videoprt's INT 10h
  support exists, and the bug check path runs at `HIGH_LEVEL`. Without a
  usable record (15, 16, 24 or 32 bpp, at least 640x480) `VidInitialize`
  returns FALSE and the kernel runs without a boot video driver.
- **Screen.** bootvid keeps the colour index of every pixel of the 640x480
  screen in memory, and each export ends by copying the rectangle it changed
  to the frame buffer. Nothing is read back from the frame buffer. The screen
  is scaled by the largest integer factor that fits (1 at Gen2's 1024x768)
  and centred. The area around it shows the colour of the last fill of the
  whole screen, so a bug check screen is blue to the edges.
- **Palette.** Indices map to colours through 6-bit DAC values, as on the
  VGA. The default is Windows' 16-colour order, as in the original: 4 is the
  dark blue (#000080) of the bug check screen, 15 is white. `VidBitBlt` loads
  the bitmap's colour table before drawing, and a palette change recolours
  the pixels already on the screen. The kernel depends on that: it draws the
  boot screen with an all-black colour table and fades the real colours in
  by drawing a 1x1 bitmap at y = 480 once per progress bar tick. Only pixels
  whose palette entry changed are rewritten.
- **Text.** An 8x13 font with code page 437 glyphs on 14-pixel lines (34
  lines in the bug check's scroll region 0-475), drawn transparently in the
  text colour. `font.c` is generated by `tools/mkfont.py` from `8x13.bdf` of
  the X.Org misc-misc fonts, which is in the public domain (`make font
  BDF=...`). As in the original, `VidDisplayString` saves the background of
  each new line; a CR not followed by LF restores it before the next
  character (so a line can be rewritten), scrolling restores it into the
  freed line, and the saved line sits right below the screen, where
  scrolling reads from.
- **Bitmaps.** `VidBitBlt` draws 4 bpp `BI_RGB` (bottom-up or top-down) and
  `BI_RLE4` bitmaps; the kernel's boot screen bitmaps are all 4 bpp. The
  4 bpp buffers of `VidBufferToScreenBlt`/`VidScreenToBufferBlt` have the
  left pixel in the high nibble.
- **No HAL display reset.** The original calls the HAL's `HalResetDisplay`
  (through `HalPrivateDispatchTable`) in `VidInitialize` and
  `VidResetDisplay`. On halmacpi that switches to real mode and calls INT 10h
  with AX=0012h. bootvid never calls it: it does nothing useful for a frame
  buffer and would only make the BIOS change what it scans out.
- **Handoffs.** CSMWrap's B8000 text scan-out only redraws cells that
  change, and the kernel never writes B8000, so the two do not interfere (none
  was seen on Gen2). Once a display driver owns the screen, the kernel stops
  calling bootvid; for a bug check it takes the display back and calls
  `VidResetDisplay`, which restores the default palette and redraws the
  whole frame buffer.

### Installing

- Text-mode setup: `hvkit hvfb-cd ... --bootvid out/bootvid.dll` replaces
  the CD's `I386\BOOTVID.DL_` with the uncompressed DLL. SETUPLDR loads it
  for the setup kernel, and setup copies it to `system32` like the original
  (`[SourceDisksFiles]` needs no change).
- Installed system: replace `%SystemRoot%\system32\bootvid.dll` while the
  system is offline. Windows File Protection lists `bootvid.dll`, so also
  replace `system32\dllcache\bootvid.dll`. `migrate/inject.ps1 -Bootvid`
  does both and keeps XP's DLL as `system32\bootvid.xp`.
- This bootvid only works where the firmware leaves a coreboot frame buffer
  record (CSMWrap on a GOP frame buffer). Keep XP's own on machines with a
  real VGA.

## bootwait.sys

XP looks for the boot partition (`IopMarkBootPartition`, bug check 0x7B
`INACCESSIBLE_BOOT_DEVICE` if it is missing) as soon as the boot drivers
have run. On Hyper-V Gen2 the boot disk sits behind the VMBus SCSI
controller, and the kernel debugger shows why it is not there by then:

- `vmbus.sys` starts during the boot driver phase and answers the first
  bus relations query with no children. It learns about its devices from
  the host's channel offers, which it processes in work items
  (`XPartReceiveMessageWorkItem`), and reports them later with
  `IoInvalidateDeviceRelations`.
- One of those work items binds itself to processor 0. The boot thread
  (`Phase1Initialization`) runs the whole boot driver phase on processor 0
  at priority 31, so the work item stays ready but never runs; the other
  processors are idle. At the bug check, the VMBus device node has no
  children at all and `\Driver\storvsc` has no device objects.
- Until all boot drivers are initialized, Plug and Play only processes
  device changes synchronously on the boot thread, after each boot driver.

On Gen1 the same XP boots from the emulated IDE disk, so nothing waits for
VMBus devices.

Apart from the registry repairs described below, which `DriverEntry` does,
bootwait only registers a boot driver
reinitialization routine (`IoRegisterBootDriverReinitialization`). The I/O
manager calls these routines after all boot drivers are initialized, when
Plug and Play already handles device changes on worker threads, and before
it creates the ARC names and looks for the boot partition. The routine
polls every 100 ms and sleeps in between, which frees processor 0. vmbus
then reports its children, and Plug and Play starts the storvsc adapter and
the disk while the boot thread waits. The routine returns as soon as the
boot partition exists and the mount manager knows its volume (see
[The boot volume's drive letter](#the-boot-volumes-drive-letter)), or after
`Services\bootwait\Parameters\TimeoutSeconds` (default 30, at most 600); on
a machine whose boot disk is already there it returns at once.

The boot partition is the one in `Control\SystemBootDevice` (e.g.
`multi(0)disk(0)rdisk(0)partition(1)`). Like the kernel's ARC name code,
bootwait finds the disk by its MBR signature, taken from the BIOS disk's
`Identifier` under `HARDWARE\DESCRIPTION\System\MultifunctionAdapter`
or from a `signature()` ARC path; without one it uses
`\Device\Harddisk<rdisk>`. It opens devices with `FILE_READ_ATTRIBUTES`
only, so it never mounts a volume.

On the Gen2 test VM (under the kernel debugger, before the mount manager
check below existed) it waited about 0.8 s:

```
bootwait: boot device multi(0)disk(0)rdisk(0)partition(1)
bootwait: BIOS disk 0 identifier ed9575aa-dc6edc6e-A
bootwait: waiting up to 30 s for partition 1 of the disk with signature dc6edc6e
bootwait: 0 disk(s) at 0 ms
bootwait: 1 disk(s) at 687 ms
bootwait: boot partition is \Device\Harddisk0\Partition1 (after 828 ms)
```

Text-mode setup booted from a CD has a `cdrom(<n>)` ARC path, and the boot
device the kernel looks for is the CD-ROM drive (on Gen2 a DVD drive on the
same VMBus SCSI controller). For such a path bootwait waits until there is
a CD-ROM device (`IoGetConfigurationInformation()->CdRomCount`) instead of
a partition, and logs `bootwait: a CD-ROM is there (after <n> ms)` or
`bootwait: no CD-ROM after <n> s, giving up`. The mount manager step below
only concerns a boot partition.

Install it with `bootwait.inf` (DefaultInstall), or offline as a kernel
service with `Type=1`, `Start=0` (boot) and
`ImagePath=system32\DRIVERS\bootwait.sys`. The load order group does not
matter: the routine runs after all boot drivers.

### The boot volume's drive letter

Right after the boot drivers, the I/O manager gives the boot partition its
drive letter (`IoAssignDriveLetters` asks the mount manager with
`IOCTL_MOUNTMGR_NEXT_DRIVE_LETTER`) and sets `NtSystemRoot` to it. The mount
manager hears of a volume when the volume manager registers the volume's
mounted device interface. For a volume that turns up after the boot drivers,
as the VMBus disk does, and whose device node is not installed yet (the
first boots of a new installation), that registration waits until Plug and
Play has installed the node. The boot volume then gets no drive letter at
all, `NtSystemRoot` stays at the default `C:\WINDOWS`, which does not exist,
and smss stops the boot with 0xC000021A (`STATUS_OBJECT_PATH_NOT_FOUND`).
This happens whichever partition XP is on.

So once the boot partition is there, bootwait asks the mount manager with
`IOCTL_MOUNTMGR_QUERY_POINTS` whether it knows
`\Device\Harddisk<n>\Partition<m>`. If it does, the routine returns. If not,
the routine announces the volume once with the mount manager's documented
`IOCTL_MOUNTMGR_VOLUME_ARRIVAL_NOTIFICATION` and polls until the mount
manager has registered it. The last line of its log is then
`bootwait: the mount manager has the boot volume (after <n> ms)`, with
`, announced by bootwait` if it had to announce the volume. A timeout in
this step is logged as
`bootwait: the mount manager does not know the boot volume after <n> s, giving up`,
one before the partition exists as `no boot partition after <n> s`.

### RepairStorvsc

An XP moved over from Gen1 has a second problem with the SCSI controller.
On the first Gen2 boot the controller is a new device node, which the kernel
binds to storvsc through the CriticalDeviceDatabase. User-mode Plug and Play
then finishes the installation with the best driver it finds, the
Integration Services' `storvsc.inf`, whose section for Windows XP installs a
NULL driver. That deletes the device's `Service` value; the running boot is
not affected, but the next one stops with 0x7B. XP prefers signed drivers
over unsigned ones regardless of how well they match, so an INF of our own
cannot take the device over, and the controller's VMBus instance GUID
differs from VM to VM, so the device node cannot be prepared in advance.

With `Services\bootwait\Parameters\RepairStorvsc` (REG_DWORD) set to
nonzero, `DriverEntry` walks `Enum\VMBUS\<device>\<instance>` and writes
`Service=storvsc` into every instance whose first hardware ID is
`VMBUS\{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f}` (the SCSI controller class)
and that has no service. `ConfigFlags`, `Driver` and the driver key stay as
the NULL installation left them. The NULL INF also names the device
"Microsoft Hyper-V SCSI Controller (not supported)", so the repair writes a
`FriendlyName` too, which Device Manager shows instead. This runs before
vmbus.sys reports its children, so Plug and Play reads the repaired value:

```
bootwait: loaded, timeout 30 s, 1 device class(es) to repair
bootwait: Service=storvsc restored on VMBUS\{8b693a5d-...}\4&22ffa449&0&{8b693a5d-...} (status 00000000)
bootwait: FriendlyName "Microsoft Hyper-V SCSI Controller" set on VMBUS\{8b693a5d-...}\4&22ffa449&0&{8b693a5d-...} (status 00000000)
bootwait: boot partition is \Device\Harddisk0\Partition1 (after 328 ms)
```

The NULL installation also deletes the `Service` value of the controller
class's `CriticalDeviceDatabase\vmbus#{ba6163d9-...}` entry, through which
the kernel binds a device node it has not seen before to storvsc. Without
that value a disk that has booted once stops with 0x7B in any VM whose
controller is a new device node (another VM, an imported copy; the
controller's instance GUID is per VM). So bootwait writes the entry's
`Service` back as well, on every boot.

The device node does not exist before the first Gen2 boot, so the name
appears from the second boot on (the first one asks for a restart anyway).
The default is 0, and `bootwait.inf` leaves it off; the converter turns it
on.

### Other VMBus devices

The Integration Services' INFs install NULL drivers for more devices than
the SCSI controller on Windows XP, and name them "(not supported)". For those
that work with a driver or service that is installed by other means,
`Parameters\Devices` holds a table. Each subkey is one device class:

| Value          | Type   | Meaning |
|----------------|--------|---------|
| `HardwareID`   | REG_SZ | first hardware ID of the device, `VMBUS\{<type guid>}` |
| `Service`      | REG_SZ | optional: service written into nodes that have none |
| `FriendlyName` | REG_SZ | optional: name written into the node |
| `ClassGUID`, `Class` | REG_SZ | optional: class (and its name) written into nodes that have none |

The Integration Services leave a few devices of a Generation 2 VM without
any driver, among them the Activation component and the two Remote Desktop
channels of an Enhanced Session. XP installs its own NULL driver on them,
which leaves them nameless in the class "Other devices". The converter gives
them the names and the class "System" of the Windows 8 INF (`wvmic.inf`)
through this table. An INF of our own would do it properly, but it is
unsigned: XP lowers its rank to 0x8000, which makes Plug and Play show the
Found New Hardware wizard instead of installing it quietly, whatever the
driver signing policy says.

`RepairStorvsc` is the entry for the SCSI controller; an entry in the table
with the same hardware ID replaces it. An entry with a `Service` repairs
the Critical Device Database entry of its class too (see above).
`migrate/inject.ps1 -DeviceFix` writes the table. The strings have fixed
sizes: a `HardwareID` of at most 79 characters, `Service`, `ClassGUID` and
`Class` of at most 39, `FriendlyName` of at most 99. A longer string is not
shortened but left out, and logged to the kernel debugger: an entry with a
longer `HardwareID` is ignored, a longer `Service` or `FriendlyName` is not
written while the rest of the entry still is, and a longer `ClassGUID` or
`Class` leaves out both.

### Registry values

Plug and Play installs the Integration Services' devices again on the first
boot of a new VM, and the INF of each service writes its values again, for
example `Parameters\ServiceDll` of the services that run `icsvc.dll`.
Whatever was changed offline there is lost. `Parameters\Values\<n>` holds a
table of string values that bootwait writes on every boot, before the
service control manager reads them:

| Value  | Type   | Meaning |
|--------|--------|---------|
| `Key`  | REG_SZ | key below `CurrentControlSet`, e.g. `Services\vmicguestinterface\Parameters`; it has to exist |
| `Name` | REG_SZ | name of the value |
| `Data` | REG_SZ | the string; the value becomes REG_EXPAND_SZ if it contains a `%`, REG_SZ otherwise |

A value is only written if it differs. `Key` and `Data` are limited to 99
characters and `Name` to 39; an entry with longer strings is ignored.
`migrate/inject.ps1` fills this table for the Guest Service Interface's
`ServiceDll`.

## NTLDR's and SETUPLDR's mode 12h screens

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

## Hyper-V Dynamic Memory (mdlex.sys and dmvsc)

Hyper-V Dynamic Memory lets the host grow and shrink a VM's RAM at runtime
through a balloon driver. Microsoft's balloon driver for the guest is
`dmvsc.sys`, shipped in the Integration Services. The last IC release that
still supports XP (6.3.9600.16384) contains a `dmvsc.sys` built for Windows
Server 2003 SP1, and its INF deliberately installs a "(not supported)" NULL
driver on XP. With two small adjustments that `dmvsc.sys` runs on XP SP3 and
Dynamic Memory works (balloon only; XP cannot hot-add memory).

`dmvsc.sys` is a KMDF driver that talks the Dynamic Memory protocol over its
VMBus channel (`{525074dc-8985-46e2-8057-a307dc18a502}`) using
`vmbkmcl.sys`/`wdfldr.sys`, which the Integration Services already put on the
disk. The only things XP's kernel does not give it are two ntoskrnl exports:

- **`MmAllocatePagesForMdlEx`** (Server 2003 SP1+). XP does not export it at
  all, so `dmvsc.sys` will not even load. It is the balloon-inflate allocator:
  `dmvsc` calls it to take pages away from the guest and hand the page runs to
  the host. XP has the older `MmAllocatePagesForMdl`, whose first four
  arguments are identical.
- **`MmAddPhysicalMemory`**. XP exports it, but XP cannot usefully hot-add
  memory, and - crucially - the host does **not** issue balloon requests
  unless the guest advertises the hot-add capability (the Linux and macOS
  balloon drivers advertise hot-add for exactly this reason and then refuse
  the host's hot-add requests). `dmvsc` decides whether to advertise hot-add
  from a start-time probe: it "adds" one already-present page with
  `MmAddPhysicalMemory` and advertises hot-add only if that returns success.
  On XP the real routine does not, so `dmvsc` advertises hot-add = 0, the host
  rejects the capabilities (STATUS `0xC000A013`), and the device fails to
  start (`CM_PROB_FAILED_POST_START`).

**mdlex.sys** (this repository, MIT) is a tiny kernel export driver that
provides both routines:

- `MmAllocatePagesForMdlEx` is implemented over XP's `MmAllocatePagesForMdl`.
  It honours `MM_ALLOCATE_FULLY_REQUIRED` (frees a short allocation and returns
  NULL rather than a partial MDL) and `MM_DONT_ZERO_ALLOCATION` (zeroes the
  pages otherwise); Dynamic Memory always passes `MM_DONT_ZERO_ALLOCATION`.
- `MmAddPhysicalMemory` returns `STATUS_SUCCESS` for the one-page capability
  probe (so `dmvsc` advertises hot-add and the host accepts the capabilities)
  and `STATUS_INVALID_PARAMETER_1` for any larger request - a real hot-add,
  which the host issues only when Maximum > Startup. XP cannot add physical
  memory. `dmvsc` maps exactly that status to "zero pages added" and answers
  the host's request with it, as a balloon-only guest should; any other failure
  status, `STATUS_NOT_SUPPORTED` for one, makes it send no answer and stop its
  message loop, which ends Dynamic Memory in the guest. (Found by reading
  `dmvsc`'s code. The probe only treats `STATUS_NOT_SUPPORTED` as "no
  hot-add", so it is not affected.)

`migrate/Patch-Dmvsc.ps1` rebinds those two imports in a caller-supplied
`dmvsc.sys` from ntoskrnl to `mdlex.sys`, touching no code: it appends a new
`.dmx` section with a fresh import-descriptor array (the four original
descriptors verbatim plus one per rebound import whose `FirstThunk` reuses the
existing IAT slot the code already calls), repoints each rebound ntoskrnl INT
entry to an already-imported export so the loader can still snap ntoskrnl's
thunk (the `mdlex` descriptors, processed afterwards, overwrite those two IAT
slots), repoints the import directory and fixes the PE checksum. The four
original IAT slots keep their addresses, so every other import and all code are
unchanged. The script verifies the input SHA-256 (IC 6.3.9600.16384
`dmvsc.sys`) before patching and never redistributes the Microsoft binary -
the caller supplies its own copy and keeps the output locally. It runs under
Windows PowerShell 5.1, where the converter runs.

```
# on the host, against your own IC 6.3.9600.16384 dmvsc.sys:
.\migrate\Patch-Dmvsc.ps1 -InputPath <IS>\dmvsc\dmvsc.sys -OutputPath out\dmvsc.sys
```

### Installing

`hvkit setup-cd` installs Dynamic Memory with XP from a setup CD, and `hvkit
inject` adds it to an installed XP offline (see hvkit/README.md); enable
Dynamic Memory on the VM before installing from the CD, or the device is not
there for setup to install. What they do to an installed system, by hand while
experimenting:

- Files: the patched `dmvsc.sys` and `mdlex.sys` into `%SystemRoot%\system32\
  drivers\`, and the Integration Services' `dmvscres.dll` into
  `%SystemRoot%\system32\` (the event-log message resource).
- Service `dmvsc` (`HKLM\SYSTEM\CurrentControlSet\Services\dmvsc`):
  `Type=1` (kernel), `Start=3` (demand), `ErrorControl=1`,
  `ImagePath=system32\DRIVERS\dmvsc.sys`; optional event-log source under
  `Services\EventLog\System\dmvsc` (`EventMessageFile=%SystemRoot%\System32\
  IoLogMsg.dll;%SystemRoot%\System32\dmvscres.dll`, `TypesSupported=7`).
  No service is needed for `mdlex.sys`: the kernel loads it automatically as a
  dependency of the import-patched `dmvsc.sys` and snaps the two imports to it.
- Bind the Dynamic Memory device to `dmvsc`. Its first hardware ID is
  `VMBUS\{525074dc-8985-46e2-8057-a307dc18a502}`; the IC INF left the devnode's
  `Service` empty (the NULL install) and the Critical Device Database entry
  `Control\CriticalDeviceDatabase\vmbus#{525074dc-8985-46e2-8057-a307dc18a502}`
  without a `Service`. Write `Service=dmvsc` into that CDDB key (so a never-seen
  instance binds on first boot) and into the existing `Enum\VMBUS\{525074dc-
  ...}\<instance>` node. User-mode Plug and Play may re-apply the NULL section
  on a later boot, so the generalized `bootwait.sys` re-asserts the `Service`
  (and the friendly name `Microsoft Hyper-V Dynamic Memory`) each boot, like it
  does for the SCSI controller.

### What works

Tested on Hyper-V Gen2 through CSMWrap, XP SP3, Integration Services
6.3.9600.16384, Dynamic Memory enabled (startup 2 GB, minimum 512 MB):
`dmvsc.sys` loads and starts (no yellow bang, no bug check), the host reports
`MemoryStatus = OK` and a live `MemoryDemand`, and the assigned memory tracks
demand - it balloons down to the 512 MB minimum while XP idles (demand
~90 MB) and rises again when a workload raises demand (e.g. ~770 MB assigned
at ~610 MB demand). XP stays stable. With Maximum > Startup the VM boots and
balloons normally; a real hot-add is answered with "no pages added" (XP cannot
add RAM), so the VM never grows above its startup size - it is balloon-only.
That answer to a real hot-add request follows from `dmvsc`'s code but has not
been exercised on a VM; the converter sets Maximum = Startup, which never
produces such a request.

## VMBus pipes from a host program (vmbecho.sys)

Hyper-V has no interface for third parties to add a VMBus device to a VM,
but the host's `vmbuspiper.dll` exports `VmbusPipeServerOfferChannel`, which
the Hyper-V stack itself uses for its pipe channels. It is not documented.
An elevated program on the host can call it with a VM's ID and an interface
type GUID of its own; the running VM then gets a VMBus channel offer, and the
program gets a handle on which `ReadFile` and `WriteFile` exchange messages
with the guest. Closing the handle rescinds the offer.

The offer (0xAC bytes, layout read from the host's `vmbuspiper.dll`):

| Offset | Size | Field |
|---|---|---|
| 0x00 | 16 | VM ID (`(Get-VM).Id`) |
| 0x10 | 4 | interrupt latency in ms (0) |
| 0x14 | 16 | interface type GUID: the guest's hardware ID is `VMBUS\{type}` |
| 0x24 | 16 | interface instance GUID |
| 0x34 | 4 | interface revision (0) |
| 0x38 | 2 | MMIO megabytes (0) |
| 0x3A | 2 | flags (0; `0x1` = open per file object, see below) |
| 0x3C | 112 | user-defined bytes |

`HANDLE VmbusPipeServerOfferChannel(offer, openMode, pipeMode)` returns -1
and sets the last error on failure (`ERROR_ACCESS_DENIED` without a full
administrator token, a Hyper-V Administrators member is not enough).
`VmbusPipeServerConnectPipe(handle, overlapped)` completes when the guest
opens the channel. Use an overlapped handle (`openMode` =
`FILE_FLAG_OVERLAPPED`): a synchronous ConnectPipe still waiting in another
thread makes `CloseHandle` block.

On XP, the offer appears as a new device right away ("Found New Hardware").
Its hardware IDs are `VMBUS\{type}` and `VMBUS\{instance}`, so an INF
matches the type; keeping the instance GUID the same across offers keeps the
device node, and the driver is installed once.

The channel is in named pipe mode, and the Integration Services' `vmbus.sys`
(6.3.9600) handles such channels itself; its function driver does not use
the KMCL library. When the device starts, `vmbus.sys` opens the channel (5
ring buffer pages each way) and from then on serves `IRP_MJ_READ` and
`IRP_MJ_WRITE` sent to the device's PDO, with direct I/O (the buffer is the
IRP's MDL). It adds and strips the pipe's packet headers: a write in the
guest is one message to the host, and a read returns what the host wrote.
(With flag `0x1` in the offer, `vmbus.sys` opens the channel on
`IRP_MJ_CREATE` instead, one open at a time, so that a user-mode program
could use it.) Opening the channel with KMCL as well fails
(`VmbChannelEnable` returns `STATUS_UNSUCCESSFUL`): `vmbus.sys` already has it.

`vmbecho.sys` is the smallest function driver for such a device: on start it
runs a thread that writes a hello message to the PDO, then reads, and
answers every read with its length and first bytes. The thread ends when a
read fails (the host closed the pipe: the read is cancelled when the device
goes away) or when the device stops.

Tested on Hyper-V Gen2 through CSMWrap, XP SP3, Integration Services
6.3.9600.16384, Windows 11 host:

```
# on the host, elevated:
.\vmbecho-host.ps1 -VMName <vm> -Seconds 10 -IntervalMs 100
```

The first offer shows the Found New Hardware wizard; install from a CD or
folder with `vmbecho.inf` and `vmbecho.sys`. 100 messages at 100 ms
intervals each reached the guest intact and were answered, about 12 ms per
round trip (with a kernel debugger attached); after the host closes its
handle the device is removed, and a new offer starts the driver again. Not
tried yet: the `pipeMode` argument (0 here), offering to a VM that is off or
restarts, and large messages.

## Sound (vmbaud.sys)

Hyper-V gives a guest no sound device; Windows guests get sound only through
an enhanced session (RDP), which needs a newer Windows inside. vmbaud.sys is
a sound card for XP whose back end is a program on the host: the guest's
audio stack plays into a PortCls WaveCyclic render device, the driver sends
the PCM over a VMBus pipe (see the previous section), and `vmbaudtray.exe`
plays it on the host's default output.

The host program offers the device (interface type
`{8b57f4e3-2a3c-4f6e-9c8d-1e5a70b9c4d2}`, a fixed instance) to each running
VM that has sound switched on. On the first offer XP shows Found New
Hardware; install `vmbaud.inf` and `vmbaud.sys` from a CD or folder.
A device that first appears after setup can't be installed without that
wizard on XP when its driver is unsigned: Plug and Play's non-interactive
install refuses unsigned files whatever the signing policy. So `hvkit
setup-cd --vmbaud` installs it ahead of time instead: `guest/predev`, run
from the CD's `cmdlines.txt` near the end of GUI-mode setup, creates the
device node and installs `vmbaud.inf` on it, and the first offer just
starts the driver. `hvkit inject --vmbaud` only puts the files into
DevicePath, where the wizard finds them. The device is "VMBus PCM
Audio"; winmm lists it as `VMbaud_Wave`. It is there while the host program
serves the VM: exiting the program removes it.

The render pin takes PCM, 16 bits, 1 or 2 channels, 8 to 48 kHz. There is no
capture pin yet.

### The host program

`vmbaudtray.exe` runs elevated (offering a pipe needs a full administrator
token) with an icon in the notification area. Its settings window lists the
VMs in three groups, sound on, running and off; the checkbox in front of a
VM switches its sound on, and the slider and Mute below act on the selected
VM. Each VM gets its own entry in the Windows volume mixer, named after it.
"Start with Windows" registers a Task Scheduler task that starts the program
at logon with the highest privileges, so without a UAC prompt. Closing the
window hides it; Exit is in the icon's menu.

The settings are kept with the VM as host-only KVP items (not exchanged with
the guest; they move with an export):

| item | values | absent |
|---|---|---|
| `vmbaud.enabled` | `0`, `1` | `0` |
| `vmbaud.volume` | `0` to `100` | `100` |
| `vmbaud.mute` | `0`, `1` | `0` |

The program offers the device as soon as such a VM runs; XP picks it up when
vmbaud.sys starts (an offer made while the firmware runs is fine). When the
channel breaks, after a guest reboot for one, it offers again; a new offer
the guest does not open within 10 s (doubling up to 60 s) is withdrawn and
made again, except while the VM is rebooting. `--wait-ic` holds each offer
until the Heartbeat or KVP integration service reports OK. The program logs
one line per event to `%TEMP%\vmbaudtray.log`.

`vmbaudcli.exe` does the same from a console:

```
vmbaudcli list                                     # VMs, state, settings
vmbaudcli set <vm> enabled|volume|mute <value>     # writes the KVP item
vmbaudcli unset <vm> enabled|volume|mute
vmbaudcli run <vm> [seconds] [--mute] [--wav prefix]   # elevated; not while vmbaudtray serves the VM
```

`run` serves one VM in the foreground and logs to the console. With
`--mute` the stream plays on the real device, clock included, with its
session muted; `--wav` writes each stream as the guest sent it to
`prefix-<n>.wav`. `vmbaud-host.ps1` is the earlier PowerShell host (winmm,
one VM, `-LogFile` capture), kept for scripting:

```
.\vmbaud-host.ps1 -VMName <vm> [-Volume 0..100] [-LogFile capture.wav]   # elevated
```

### Wire protocol

One pipe message per write, an 8-byte header (`u32 type`, `u32 size` of the
payload that follows), little endian:

| type | name | direction | payload |
|---|---|---|---|
| 1 | FORMAT | guest to host | `u32 rate, u32 channels, u32 bits`: a stream starts |
| 2 | PCM_OUT | guest to host | interleaved PCM |
| 3 | CONSUMED | host to guest | `u64 played, u32 queued`: bytes of this stream played and still queued on the host |

The guest sends FORMAT when a stream goes from KSSTATE_STOP to ACQUIRE; both
sides count the stream's bytes from there. The host sends CONSUMED every
10 ms while a stream plays.

### The clock

The play position that the driver reports (`GetPosition`) decides how fast
the guest's audio stack hands it data, so it is the clock of the stream. It
must advance smoothly: XP's WaveCyclic port keeps only about 40 ms written
ahead of the position, and a position that moves in steps of tens of
milliseconds makes kmixer use up its client's data and fill the gaps with
silence. It must also follow the host's sound card in the long run, or the
host's queue grows or runs dry.

So the position is a clock on the guest's performance counter at the nominal
byte rate, corrected by up to 0.5%. The host reports how much PCM it has
queued; the driver smooths that over about 16 reports and slows its clock
when the queue is deeper than 60 ms, and speeds it up when it is shallower.
The audio is not resampled. The clock stops at what has been sent, as the
host cannot play data it does not have.

Two other models were tried first. A position that follows only what the
host has played lags by the whole round trip (the host's own buffering and
the reports' interval): the guest could send only a third of real time and
the host kept running dry. A position that follows what the host has
received moves in steps, with the kmixer silence described above.

The host queues the guest's PCM, starts playing once 60 ms are queued (again
after running dry), and starts anyway when the guest stops sending for
30 ms. vmbaudtray plays through a shared-mode WASAPI stream with a 50 ms
device buffer, refilled on every wake-up of its MMCSS ("Pro Audio") worker;
vmbaud-host.ps1 joins the messages into 20 ms winmm buffers.

### Notes on the implementation

- Data path: the port calls `IDmaChannel::CopyTo` with the client's PCM, and
  the driver sends it as is (the MSVAD/Scream approach); the DMA buffer is
  allocated only because the port asks for one. XP's port calls `CopyTo`
  already in KSSTATE_PAUSE, while the client fills the buffer before it
  starts the stream.
- The stream object is both the `IMiniportWaveCyclicStream` and the
  `IDmaChannel` the port gets from `NewStream`.
- The counts and the write list are used at PASSIVE_LEVEL (the thread that
  reads CONSUMED) and at DISPATCH_LEVEL (the port), so they are under a spin
  lock that raises IRQL. Stopping a stream cancels its pending writes outside
  that lock: vmbus.sys completes a cancelled pipe write inside `IoCancelIrp`,
  and the completion routine takes the lock.
- Formats: the render pin takes only PCM in a WAVEFORMATEX(TENSIBLE), and
  `NewStream`, `SetFormat` and the proposed-format property all check it.
  DirectSound offers the pin its hardware buffer formats
  (`KSDATAFORMAT_SPECIFIER_DSOUND`) at rates down to 100 Hz; refused, it
  mixes in software through kmixer, which converts any format to one the pin
  takes. kmixer also changes the format after STOP -> ACQUIRE (DirectSound
  opens at 48 kHz, then plays 44.1 kHz), so `SetFormat` past STOP sends a new
  FORMAT and the counts restart on both sides.
- If the host stops reading the pipe, at most 64 writes (about 640 ms) stay
  pending; after that PCM is dropped and the stream clock keeps running, like
  a card with nothing plugged in, so the guest's programs do not hang.
- C++ with this toolchain: `ddk/portcls.h` needs `DECLSPEC_NOVTABLE`,
  `DECLSPEC_NOTHROW`, `TCHAR` and the `KSRTAUDIO_*` structures under C++
  (`drivers/common/ddk_compat.h`, included after `ntddk.h` and before `portcls.h`),
  and `-fno-exceptions` is required: with exceptions `STDMETHOD` is
  `noexcept` and `STDMETHODIMP_` is not. `operator delete` comes from
  `stdunk.h`; `operator new` and the 64-bit division helpers are in
  `common.cpp`.
- The toolchain has no import library for portcls.sys; the Makefile makes
  one from `drivers/common/portcls.def`. XP's forwarding routine is
  `PcForwardIrpSynchronous`, and `PcTerminateAdapterDriver` does not exist.

### Testing

Tested on Hyper-V Gen2 through CSMWrap, XP SP3, Integration Services
6.3.9600.16384, Windows 11 host. `testplay.exe` (built by `make`) plays a sine
tone through winmm and prints one line per buffer:

```
c:\testplay -s 20              (20 s, 4 buffers of 20 ms queued)
c:\testplay -s 5 -n 8 -m 50     (5 s, 8 buffers of 50 ms queued)
c:\testplay -T                  (the clock tick; XP's default is 15.6 ms)
```

Like a player, testplay waits on an event for finished buffers. `-p` makes
it poll with `Sleep(2)` instead, which sleeps a whole clock tick: with
4 buffers of 20 ms kmixer then runs dry and inserts 10 ms of silence every
quarter second or so, which is the client's doing, not the card's. `-t`
raises the clock to 1 ms while playing.

Checked with `vmbaudcli run --mute --wav` captures (a sine is easy to check
for silence and discontinuities): with 8 buffers of 50 ms the PCM that
reaches the host is a clean sine; with 4 buffers of 20 ms it is too, but for
10 ms of silence 20 ms into each stream. Windows Media Player 9 (DirectSound,
44.1 kHz) plays the sample music through without a gap. The host's queue stays between 45
and 90 ms, and its device buffer runs empty only at the end of a stream.
Open points:

- XP's logon sound has a gap about 0.3 s in: during logon the guest sends
  late.
- A client that hangs holding the stream keeps the device from being removed
  when the host program exits; the next offer then does not start the device
  until XP restarts.
- The trim is proportional only, so the host's queue settles above the
  60 ms target by the clock difference.

## Moving an installed XP to Gen2

`migrate/Convert-XPToGen2.ps1` converts the disk of a Windows XP
Professional SP3 x86 that runs on a Gen1 VM with the Integration Services
of Windows Server 2012 R2 (6.3.9600) into a new disk for a Gen2 VM, and
with `-VMName` also creates the VM. It runs in Windows PowerShell on the
Hyper-V host, elevated; `migrate/Convert-XPToGen2.cmd` starts it from
Explorer. The source (`.vhd`, `.vhdx` or a checkpoint's `.avhdx`) is only
read. Usage, options and limits are in [migrate/README.txt](migrate/README.txt);
the comments at the top of the script list where each file comes from.

What the new disk gets (through `migrate/inject.ps1`):

- CSMWrap as `\EFI\BOOT\BOOTX64.EFI` on the XP partition, which therefore
  has to be FAT32, with `madt_pcat_compat = true` and this repository's
  DSDT (`acpi_dsdt`);
- storvsc.sys from the Integration Services and storport.sys/diskdump.sys
  from KB943295 (SP2 QFE branch; the SP2 RTM storport rejects this storvsc)
  as the boot storage stack, with CriticalDeviceDatabase entries for the
  VMBus devices XP needs before its first Gen2 logon;
- hvfb, bootvid.dll and bootwait (with `RepairStorvsc`, the device table for
  the Activation component and the Remote Desktop channels, and the value
  table for the Guest Service Interface's `ServiceDll`);
- `system32\icsvcgsi.dll`, a copy of the Integration Services' `icsvc.dll`
  patched by `migrate/IcSvcGuestInterface.ps1`, run by the
  `vmicguestinterface` service only, so that `Copy-VMFile` works. The
  service logs on `NT AUTHORITY\SYSTEM` for every file it receives, with an
  empty password (`LOGON32_LOGON_SERVICE`), which only Windows Vista and later
  allow; the patch hands it the service's own token instead, so the files
  are written as SYSTEM. The copy is separate because Plug and Play puts the
  original `icsvc.dll` back from the driver store on the first boot of a new
  VM, and the driver store's file cannot be patched (its catalog signature is
  checked); the VM gets that integration service turned on;
- with `-DynamicMemory`: `dmvsc.sys` patched by `migrate/Patch-Dmvsc.ps1`,
  `mdlex.sys` and the `dmvsc` service (see Hyper-V Dynamic Memory above), and
  a VM with dynamic memory (minimum 512 MB, startup and maximum equal).

The Hyper-V Video driver (SynthVid) needs care on the first boot. The
Gen2 VMBus is a new parent device, so the video channel is a new device
node. Left alone, user-mode Plug and Play installed and started SynthVid in
the middle of the first session while hvfb drew the desktop; hvfb's drawing
then slowed to a crawl and the display watchdog stopped the system (0xEA in
`framebuf`). Bound through a CriticalDeviceDatabase entry alone, SynthVid
became `\Device\Video0` before its installation was finished and win32k
enabled no display driver at all (the screen kept showing autochk's
output). With the entry and the SynthVid service disabled, the first boot
runs on hvfb at 1024x768x32, Plug and Play installs SynthVid (its INF sets
the service back to demand start) and asks for a restart, and from the
second boot on SynthVid is the primary display with hvfb detached.

The Microsoft files are not part of this repository. The script takes
storvsc.sys from the disk itself and KB943295 from the package
(`-Kb943295`) or from vmguest.iso (`-VmGuestIso`), and checks their
versions. `tools/mkdist.sh` builds a package with the scripts and the
built files in `resources\`:

```
tools/mkdist.sh CSMWRAP_EFI=<release csmwrap.efi>            # out/dist/nt5-hvgen2-migrate.zip
tools/mkdist.sh CSMWRAP_EFI=<...> MS_DIR=<dir>               # ...-private.zip, needs nothing else
```

`MS_DIR` holds storvsc.sys, storport.sys and diskdump.sys; a package made
with it contains Microsoft files and is for private use only. The scripts
and text files in a package have CRLF line endings.

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
  - It points `HARDWARE\DEVICEMAP\VIDEO\Device\VideoN` at the device key
    `Control\Video\{VideoID}\0000`, where `VideoID` comes from
    `Services\<svc>\Video\VideoID`. If there is none, the first boot creates
    a new GUID and copies `Services\<svc>\Device0` to the device key; that
    copy is made only once. `HIVESYS.INF` instead gives VgaSave a fixed
    VideoID (`{23A77BF7-ED96-40EC-AF06-9B1F4867732A}`) and writes its keys
    up front.
- win32k takes the display mode from the hardware profile's copy of the
  device key, `Hardware Profiles\Current\System\CurrentControlSet\Control\Video\{VideoID}\0000`
  (HKCC), which is also where Display Properties stores the user's choice.
  The device key's `DefaultSettings.*` only counted on the first boot, before
  that profile key existed. That boot created the profile key with
  `Attach.ToDesktop` alone, and every later boot started at 640x480
  (seen on Hyper-V Gen2; deleting `DefaultSettings.*` from the profile key
  brings 640x480 back, writing them there gives 1024x768 from the first boot
  on).
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
`framebuf.dll` already ships in `system32`. Like `HIVESYS.INF` for VgaSave,
the INF gives hvfb a fixed VideoID, `{449ECA2B-4408-4A8C-979B-72B866C035D8}`,
writes the device key, and puts `DefaultSettings.*` for 1024x768x32 into the
current hardware profile's key, so the desktop starts at 1024x768 on every
boot. An existing VideoID and a mode already chosen there are kept.
`migrate/inject.ps1 -Hvfb` writes the same keys offline (keeping an existing
VideoID).

To have setup install hvfb into the new system, `hvkit hvfb-cd` (without
`--no-install`) does three things:

1. keeps the `[SourceDisksFiles]` entry, which copies `hvfb.sys` to
   `system32\drivers`;
2. adds the hvfb service and `Device0` values to `HIVESYS.INF` next to
   VgaSave;
3. sets the initial resolution. With `--default-mode 1024x768x32`, it writes
   `DefaultSettings.*` into `HIVESYS.INF`. Without it, it sets the
   `[Display]` service field, so GUI-mode setup and the installed system
   start in the mode text-mode setup used (640x480).

   Both only reach `Services\hvfb\Device0`, i.e. the device key; whether a
   fresh installation keeps the mode after its first boot (see
   [Installed system](#installed-system)) has not been tested yet.

## Testing

`hvkit hvfb-cd BASE.iso OUT.iso --hvfb out/hvfb.sys` repacks an XP CD with hvfb.
`tools/qemu-xp.sh` boots it through CSMWrap in QEMU/KVM. QEMU has a real VGA
with VBE, so the test exercises the VBE path; Hyper-V Gen2 differs in having
no VGA at all. For example:

```
make
hvkit hvfb-cd ~/Projects/CSMWrap/hyperv/WinLite_En_mp.iso /tmp/hvfb-iso/xp-hvfb.iso --hvfb out/hvfb.sys
ISO=/tmp/hvfb-iso/xp-hvfb.iso EFI=.../csmwrap.efi GROW_MB=3072 tools/qemu-xp.sh
```

`--product-key-file <file>` puts a product key into the copy's `WINNT.SIF`;
keep key files out of this repository. Screendumps land in `$Q` (default
`/tmp/hvfb-qemu`). Send keys through the
QEMU monitor socket `$Q/mon.sock` (`sendkey ret`), or schedule them with
`KEYS=file`.

For kernel debugging, put `out/` on the symbol path (`.sympath+ <dir>`).
The PDBs match `out/hvfb.sys`, `out/bootvid.dll` and `out/bootwait.sys`
by GUID and age. bootvid
reports the frame buffer it found through `DbgPrint` ("bootvid: frame buffer
1024x768, 32 bpp, ..."). In a bug check with a debugger attached, XP breaks in
before it draws the blue screen; continue (`g`) to see it.

## License

MIT, see [LICENSE](LICENSE), except `drivers/vmbaud/`, which is under the Microsoft
Public License (MS-PL), see [drivers/vmbaud/LICENSE](drivers/vmbaud/LICENSE): parts of it are
derived from Scream (MS-PL), which is based on Microsoft's MSVAD sample.

`drivers/bootvid/font.c` is generated from the X.Org misc-misc font `8x13.bdf`,
which is in the public domain.

Apart from the MSVAD-derived parts of `drivers/vmbaud/`, this repository contains
no Microsoft source code, and it redistributes no Microsoft files. The
drivers are written from public documentation (the Windows Driver Kit, ACPI
and VESA specifications) and from analysing the interfaces of Windows XP's
own binaries (for example the exports and callers of `bootvid.dll`). The Microsoft drivers the tools install, such as
the Hyper-V Integration Services, come from the user's own media.
