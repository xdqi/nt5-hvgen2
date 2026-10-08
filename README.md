# nt5-hvgen2

Drivers and tools that run Windows XP and Server 2003 (x86) in a Hyper-V Generation 2 VM, booting
through [CSMWrap](https://github.com/CSMWrap/CSMWrap), a UEFI application that provides a legacy
BIOS (SeaBIOS). A Gen2 VM has no VGA (the screen stays the UEFI GOP frame buffer, on Hyper-V
1024x768x32 at 0xF8000000) and no PCI bus, and its disk is on the VMBus SCSI controller; XP's own
`vga.sys`, `bootvid.dll` and boot sequence cannot cope with that. [w9x](w9x/README.md) does the same
for Windows 98 SE.

## Components

- [drivers/hvfb](drivers/hvfb/README.md): hvfb.sys, a linear frame buffer display miniport (found
  through VBE or the coreboot table) for text-mode setup and, with XP's `framebuf.dll`, the
  installed system.
- [drivers/bootvid](drivers/bootvid/README.md): bootvid.dll, the boot screen, `/sos` text and bug
  check screens on the frame buffer.
- [drivers/bootwait](drivers/bootwait/README.md): bootwait.sys, holds the boot until the VMBus disk
  and its volume exist (otherwise bug check 0x7B, and 0xC000021A on a new installation).
- [drivers/mdlex](drivers/mdlex/README.md): mdlex.sys, the two ntoskrnl routines the Integration
  Services' Dynamic Memory driver needs on NT 5.x.
- [drivers/vmbecho](drivers/vmbecho/README.md): VMBus pipes offered by a host program, with a test
  driver.
- [drivers/vmbaud](drivers/vmbaud/README.md): vmbaud.sys, a sound card that plays through a host
  program over such a pipe.
- [hvkit](hvkit/README.md): one Rust tool for the patches of Microsoft files (among them
  [NTLDR's mode 12h menus](hvkit/docs/ntldr.md)), registry hives, setup CDs, the CSMWrap boot CD,
  disk images and offline changes to an installed system.
- [migrate](migrate/README.md): moving an XP installed on Gen1 to Gen2.
- `guest/`: predev.exe, which pre-installs VMBus devices that appear only after setup (run by the
  CDs `hvkit setup-cd` makes and by `hvkit nlite`'s predev addon), and vsstest.c, a VSS requester
  probe.

## Status

On Hyper-V Gen2:

- XP SP3 (also nLite'd) and Server 2003 SP2 install from a CD made by `hvkit setup-cd` onto an
  empty disk, with CSMWrap booting from a CD of its own (`hvkit csmwrap-cd`). They run with Dynamic
  Memory, production checkpoints (VSS), Copy-VMFile (Guest Service Interface) and SynthVid at up to
  32 bpp; XP also with the sound card.
- An XP installed on Gen1 with the Integration Services 6.3 moves over with
  `migrate/Convert-XPToGen2.ps1` (`hvkit migrate` offline); `hvkit inject` adds the same components
  to an installed system offline.
- Windows 98 SE installs and runs, see [w9x](w9x/README.md).

In QEMU/KVM (CSMWrap + OVMF, QEMU's VGA with VBE), XP SP3 from a CD made by `hvkit hvfb-cd` installs
and runs on hvfb and `framebuf.dll`.

Open points:

- `hvkit inject`'s SynthVid patch is undone on the disk's first Gen2 boot, when Plug and Play
  reinstalls SynthVid from its INF's source files.
- Firmware that scans out a text buffer and watches the BDA video mode does not see XP's mode
  switches (see [drivers/hvfb](drivers/hvfb/README.md)).
- Safe Mode and `/basevideo` use only VgaSave and stay black.

## Quick start

```
make                                                    # drivers and guest programs into out/
hvkit setup-cd XP.iso OUT.iso --files DIR --ic DIR      # the install CD
hvkit csmwrap-cd csmwrap.iso --efi csmwrap.efi --ini csmwrap.ini --dsdt dsdt.aml
```

`--files` holds our drivers from `out/` and Microsoft's from the user's own media (the list is in
[hvkit](hvkit/docs/setup-cd.md#setup-cd)), `--ic` the Integration Services 6.3 packages.
csmwrap.efi comes from the `hyperv-gen2` branch of xdqi/CSMWrap, dsdt.aml from `acpi/build.sh`. The
VM: Generation 2, Secure Boot off, an empty disk, the install CD, and the CSMWrap CD at a later SCSI
location as the first boot device, left in. Enable Dynamic Memory and the Guest Service Interface
before installing: setup installs drivers only for devices that exist.

## Layout

```
Makefile    toolchain, pattern rules, entry points; each component's rules are in a *.mk next to it,
            its version resource in a *.rc
drivers/    the NT 5.x drivers, one directory each with its INF, host side and test client;
            drivers/common/ holds what several of them share
guest/      programs that run in the guest
hvkit/      the Rust tool
migrate/    the Gen1-to-Gen2 converter (PowerShell 5.1, with hvkit.exe)
acpi/       the DSDT CSMWrap gives the guest
w9x/        Windows 98 SE
tools/      PE checks, cdb symbol check, font generator, converter packaging
```

## Building

The build uses clang and lld from the [msys2-cross](https://github.com/xdqi/msys-cross) toolchain
for `i686-w64-mingw32`, with the mingw-w64 DDK headers and import libraries. Install its bootstrap
into `/opt/msys2-cross` ([its README](https://github.com/xdqi/msys-cross#install), steps 1-3), then:

```
/opt/msys2-cross/bin/msys-pacman -Sy msys-cross-clang msys-cross-mingw32-gcc  # once
/opt/msys2-cross/bin/msys-pacman -Sy msys-cross-mingw64-gcc   # once, for the x64 host programs
make              # everything into out/, with PDBs and maps
make check        # PE checks; XPBIN=<dir with XP's binaries> adds import/export checks
make cdb-check    # resolve hvfb!* and bootvid!* with the Windows cdb.exe (WSL only; CDB=...)
make w9x          # Windows 98 SE, see w9x/README.md
```

The gcc packages provide only the sysroots; the compiler is clang. The sound card's host program is
x64 because `vmbuspiper.dll` exists only in System32. `MSYS2_CROSS`, `LLVM_DIR` and `SYSROOT`
override the paths. hvkit builds with cargo, see [its README](hvkit/README.md#building).

The drivers are NT native 5.01 images (base 0x10000, relocations kept, PE checksum set as the loader
requires for boot drivers) that import undecorated names from kernel modules only. clang writes
CodeView and lld a PDB, referenced by file name only (the image is stripped), so WinDbg/KD get full
symbols, types and lines. `tools/pecheck.py` checks this after every link; `--against DIR` resolves
the imports against XP's own modules and `--exports-like` compares bootvid's exports with XP's
(`make check XPBIN=DIR`). In the kernel debugger, `.sympath+ <out>`: the PDBs match the images by
GUID and age.

A trap in the mingw-w64 headers: WDK-built x86 drivers default to `__stdcall` (`/Gz`), so function
pointer typedefs without an explicit convention, such as `PINTERFACE_REFERENCE` in `miniport.h`, are
stdcall in Microsoft's headers but cdecl here; calling one unbalances the stack.

## License

MIT, see [LICENSE](LICENSE), except `drivers/vmbaud/`, which is under the Microsoft Public License
(MS-PL, [drivers/vmbaud/LICENSE](drivers/vmbaud/LICENSE)): parts of it derive from Scream (MS-PL),
which is based on Microsoft's MSVAD sample. `drivers/bootvid/font.c` is generated from the X.Org
misc-misc font `8x13.bdf` (public domain).

Apart from the MSVAD-derived parts of `drivers/vmbaud/`, this repository contains no Microsoft source
code and redistributes no Microsoft files. The drivers are written from public documentation (the
WDK, ACPI and VESA specifications) and from analysing the interfaces of XP's own binaries. The
Microsoft drivers the tools install, such as the Integration Services, come from the user's own
media.
