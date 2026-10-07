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
  [Hyper-V Dynamic Memory](drivers/mdlex/README.md).
- **vmbecho.sys**: a test driver for a VMBus channel that a program on the
  host offers to the VM, the transport for paravirtual devices whose back end
  is a host program. See
  [VMBus pipes from a host program](drivers/vmbecho/README.md).
- **vmbaud.sys**: a sound card. A PortCls WaveCyclic render driver that sends
  the PCM over such a VMBus pipe to a program on the host, which plays it.
  Hyper-V has no sound device for a Windows XP guest otherwise. See
  [Sound (vmbaud.sys)](drivers/vmbaud/README.md). Under the MS-PL (see
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
(see [Moving an installed XP to Gen2](migrate/README.md)).

bootvid.dll is tested on Hyper-V Gen2 through CSMWrap, in text-mode setup
of a CD repacked with it: the kernel's bug check screen (0x7B, no storage
driver yet) is readable, and without `/noguiboot` the boot screen appears
(caught fading in when the 0x7B stops the boot). A full boot with the
progress bar running is not tried yet.

Open points:

- Firmware that scans out a text buffer and watches the BDA video mode does
  not see XP's mode switches (see "The INT 10h environment on XP" in [drivers/hvfb](drivers/hvfb/README.md)).
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

## Components

Each component has its own README:

- [drivers/hvfb](drivers/hvfb/README.md): the frame buffer display miniport, and how XP loads a legacy
  display miniport;
- [drivers/bootvid](drivers/bootvid/README.md): the frame buffer boot video DLL;
- [drivers/bootwait](drivers/bootwait/README.md): the boot driver that waits for the VMBus disk;
- [drivers/mdlex](drivers/mdlex/README.md): Hyper-V Dynamic Memory on XP;
- [drivers/vmbecho](drivers/vmbecho/README.md): VMBus pipes from a host program;
- [drivers/vmbaud](drivers/vmbaud/README.md): the sound card;
- [migrate](migrate/README.md): moving an installed XP to Gen2;
- [docs/ntldr-mode12h.md](docs/ntldr-mode12h.md): NTLDR's and SETUPLDR's mode 12h screens;
- [docs/testing.md](docs/testing.md): testing in QEMU;
- [hvkit](hvkit/README.md), [w9x](w9x/README.md).

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
