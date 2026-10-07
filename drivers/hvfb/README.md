# hvfb.sys

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
