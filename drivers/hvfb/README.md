# hvfb.sys

hvfb.sys is an XP display miniport for a linear frame buffer without VGA, as on Hyper-V Gen2
through CSMWrap. It replaces `vga.sys` in text-mode setup and drives the installed system's desktop
through `framebuf.dll`.

Like `vga.sys` it is a legacy (non-PnP) VideoPort miniport, the only kind text-mode setup can use
(there is no devnode to bind to): with no power or child-device callbacks, videoprt calls
HwFindAdapter from DriverEntry and the device appears as `Root\LEGACY_HVFB`. AdapterInterfaceType
is tried as Isa, Internal, PCIBus. Gen2 has no PCI bus; NTDETECT always reports an ISA adapter on
PCs, which still has to be confirmed on Gen2.

## Modes

hvfb calls VBE 4F00/4F01 through `VideoPortQueryServices(VideoPortServicesInt10)` (XP and later)
in HwInitialize, as videoprt allows INT 10h only after the device's first open. It keeps supported
linear graphics modes, direct colour or packed pixel, at 15/16/24/32 bpp, that fit in VRAM, with
pitch (LinBytesPerScanLine for VBE 3.0) and masks from VBE. 15-bit modes become 16 bpp with 5:5:5
masks, dropped if a 5:6:5 mode of the same size exists. Without VBE modes, the coreboot table's
frame buffer (`LBIO` header in physical 0-4 KiB, optional `CB_TAG_FORWARD`, `CB_TAG_FRAMEBUFFER`)
is the only mode.

Mode 0, which text-mode setup always takes, is the smallest mode of at least 640x480 with exactly
80 text columns, never 24 bpp, deeper colour first (see [Text-mode setup](#text-mode-setup)). On
Gen2 (640x480, 800x600 and 1024x768) that is 640x480x32. The installed system would start in it
too, so `hvfb.inf` and `migrate/inject.ps1` set 1024x768x32.

Hyper-V scans the GOP frame buffer out at its native size, and CSMWrap's SeaVGABIOS emulates VBE on
it, so 4F02 only changes what the BIOS draws. When VBE looks like this (video memory no larger than
the coreboot table's frame buffer, same base, pitch and depth), hvfb centres smaller modes in that
frame buffer with its pitch, as bootvid.dll does, and reports no off-screen memory (all of it is
visible). Each mode set clears the border, and the picture unless `VIDEO_MODE_NO_ZERO_MEMORY`. A
VGA BIOS that programs the display per mode (QEMU's) is left alone.

## IOCTLs

- `QUERY_NUM_AVAIL_MODES`, `QUERY_AVAIL_MODES`, `QUERY_CURRENT_MODE`: fully filled
  `VIDEO_MODE_INFORMATION`.
- `SET_CURRENT_MODE`: VBE 4F02 with the linear frame buffer bit. `RESET_DEVICE`: INT 10h mode 3.
- `MAP_VIDEO_MEMORY`/`UNMAP_VIDEO_MEMORY`, `SHARE_VIDEO_MEMORY`/`UNSHARE_VIDEO_MEMORY`: mappings
  start at the picture's first pixel, so `VideoRamBase` equals `FrameBufferBase` as `framebuf.dll`
  expects (not page aligned for a centred mode, which VideoPort handles).
- No-ops: `SET_COLOR_REGISTERS` (direct colour only), `QUERY_PUBLIC_ACCESS_RANGES` (none),
  `QUERY_POINTER_CAPABILITIES` (no hardware cursor).

Debug output (`VideoPortDebugPrint`): discovery results at level Error, which a free-build kernel
debugger shows by default; per-request traces at level Trace.

## Firmware contract

- Graphics modes are set with VBE 4F02, text mode with INT 10h AH=00h. A BIOS that scans a text
  buffer out to the frame buffer can stop when a graphics mode is set.
- Both calls ask the BIOS not to clear memory (4F02 bit 15, mode 0x83); hvfb clears the frame
  buffer and the 0xB8000 text page itself. SeaVGABIOS's coreboot back end would clear a high frame
  buffer through INT 15h AH=87h, which switches to protected mode and cannot run in videoprt's V86
  monitor.
- **The INT 10h environment on XP.** videoprt runs INT 10h in V86 mode in the process that first
  opened the device (CSRSS, or setup in text-mode setup), with a first megabyte built at that open:
  IVT, BDA, EBDA and the ROM areas (C0000-FFFFF) are private copies, and only the range reported in
  `VdmPhysicalVideoMemoryAddress` (A0000-BFFFF, as for VGA) is mapped live; without that range,
  every INT 10h fails. BIOS state written by these calls, such as the BDA video mode byte at 0x449,
  never reaches physical memory, so firmware cannot learn about OS mode switches from the BDA.

## Installing

On a running XP, right-click `hvfb.inf` and choose Install, or run the following, then reboot:

```
rundll32 setupapi.dll,InstallHinfSection DefaultInstall 132 <dir>\hvfb.inf
```

It is a `DefaultInstall` INF, as there is no PnP device to match. It copies `hvfb.sys` and writes
the service with `Device0` (`InstalledDisplayDrivers=framebuf`, `VgaCompatible=0`; `framebuf.dll`
is in-box), a fixed VideoID `{449ECA2B-4408-4A8C-979B-72B866C035D8}` with its device key, and
`DefaultSettings.*` for 1024x768x32 in the hardware profile's key, so every boot starts at
1024x768. An existing VideoID and a mode chosen there are kept. `migrate/inject.ps1 -Hvfb` writes
the same keys offline (also keeping an existing VideoID).

`hvkit hvfb-cd` makes text-mode setup use hvfb and, without `--no-install`, installs it through
`[SourceDisksFiles]` and `HIVESYS.INF` (service and `Device0` next to VgaSave).
`--default-mode 1024x768x32` adds `DefaultSettings.*` there; without it, the `[Display]` service
field carries text-mode setup's 640x480 over to GUI-mode setup and the installed system. Both only
reach `Services\hvfb\Device0`, i.e. the device key; whether a fresh installation keeps the mode
after its first boot is not tested yet.

## How XP loads a legacy display miniport

From the XP SP3 binaries (setupldr, setupdd.sys, videoprt.sys, framebuf.dll) and INF/SIF files.

### Text-mode setup

On x86, setupldr always uses the display id `vga`: it loads the miniport named in `[Display.Load]`
of `TXTSETUP.SIF` (`vga = vga.sys`, also the default), and setup copies `[files.vga]`, the VGA
display DLLs including `framebuf.dll`. hvfb therefore needs `[Display.Load] vga = hvfb.sys` and a
`[SourceDisksFiles]` entry like that of `vga.sys`; setupldr also loads uncompressed files from
`\I386`, so no `makecab` is needed.

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

setupdd.sys then:

- uses its VGA text back end only if there is a non-graphics 720x400 mode, otherwise its frame
  buffer back end (mode queries, `SET_CURRENT_MODE`, `MAP_VIDEO_MEMORY`/`UNMAP_VIDEO_MEMORY`;
  `SET_COLOR_REGISTERS` only for palette modes);
- on a PC always takes **mode 0**: a graphics mode of at least 8 bpp and 640x480, not 24 bpp (it
  draws with 1, 2 or 4 bytes per pixel), with colours from the masks and `Number*Bits`;
- lays out 80 columns of an 8x16 font (16 pixels wide from 160 columns up), so mode 0 must be 640
  (or 1280-1295) pixels wide. More columns crash formatting with bug check 0x50:
  `SpvidClearScreenRegion` mirrors cleared cells into an 80-column stack buffer at `79 - x`
  without a bounds check;
- on any failure prints error 0x239c ("Setup encountered an error while initializing your
  computer's video") through bootvid and stops, which without VGA looks like a hang;
- writes the mode it used to `Services\<svc>\Device0\DefaultSettings.*` of the new system if the
  `[Display]` entry's third field names a service (`vga = "Auto Detect",files.none,hvfb`).

### Installed system

- videoprt calls HwFindAdapter only if `IoQueryDeviceDescription` finds the requested bus type, and
  points `HARDWARE\DEVICEMAP\VIDEO\Device\VideoN` at the device key `Control\Video\{VideoID}\0000`
  (VideoID from `Services\<svc>\Video\VideoID`). Without a VideoID, the first boot creates one and
  copies `Services\<svc>\Device0` to the device key, only once. `HIVESYS.INF` instead gives VgaSave
  a fixed VideoID (`{23A77BF7-ED96-40EC-AF06-9B1F4867732A}`) and writes its keys up front.
- win32k reads the mode from the hardware profile's copy of the device key,
  `Hardware Profiles\Current\System\CurrentControlSet\Control\Video\{VideoID}\0000` (HKCC), where
  Display Properties stores it. The device key's `DefaultSettings.*` count only on the first boot,
  which creates that copy with just `Attach.ToDesktop`, so later boots start at 640x480. On Gen2,
  `DefaultSettings.*` in the profile key give 1024x768 from the first boot on.
- Legacy initialisation is refused once win32k has opened a display device, so a legacy miniport is
  a boot-start (`Start=1`) service in group `Video` and takes effect after a reboot.
- win32k's primary is the first display device other than VgaSave, drawn by the DLL in
  `InstalledDisplayDrivers`; `VgaCompatible=0` marks it as not VGA. On Gen2, VgaSave's
  HwFindAdapter is expected to find no VGA and fail harmlessly (`ErrorControl=0`).
- Safe Mode and `/basevideo` load only VgaSave, so they stay black on Gen2.

## Files

```
hvfb.c     VideoPort entry points and IOCTL handling
modes.c    mode discovery: VBE 4F00/4F01 through INT 10h, coreboot table fallback
hvfb.h     shared definitions
hvfb.inf   installs hvfb as a legacy display driver on an installed XP
../common/cbtable.c  coreboot table frame buffer lookup, shared with bootvid
```
