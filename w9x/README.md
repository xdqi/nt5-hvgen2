# Windows 98 SE on Hyper-V Generation 2

Windows 98 SE installs and runs in a Generation 2 VM that boots through CSMWrap, with the
keyboard and mouse on VMBus and a 1024x768 32 bpp display. A Gen2 VM has none of the legacy PC
devices Windows 9x programs directly: no 8259 PICs, no 8254 PIT, nothing at port 61h, no i8042,
no VGA. The pieces here fill the gaps without a hypervisor of their own:

- **gen2leg.vxd** (`gen2leg/`): a VxD that emulates the 8259 pair, the 8254 and port 61h on the
  local APIC and Hyper-V's synthetic timer, and an i8042 with an AT keyboard and PS/2 mouse. The
  port I/O of Windows' own VPICD, VTD and VKD (and of SYSDETMG.DLL, hardware detection) is
  rewritten into `int` instructions to the vectors in `gen2leg/vectors.txt`, which gen2leg owns.
  It also takes over the VMBus keyboard channel SeaBIOS opened and opens the synthetic mouse
  (absolute, through VMOUSE's `VMD_Post_Absolute_Pointer_Message`), turns the 8042 reset into a
  VM reset, and puts the BIOS's timer, APIC and VMBus state back when Windows exits to DOS.
- **vmbc** (`vmbc/`): a small VMBus client that works on the connection SeaBIOS made, found
  through the `$HvVmBus` handoff block in the F segment. `vmbc/dos/` is a DOS test program for it.
- **Display**: vmdisp9x's VESA driver (`vmdisp9x/`, [JHRobotics/vmdisp9x], MIT) over SeaVGABIOS's
  VBE modes, with two fixes, and `setup/gen2disp.inf`, which hardware detection picks for the
  "VGA" device it assumes (`*PNP0917`). `setup/gen2mon.inf` lets the display go up to 1024x768.
- **Setup** (`setup/`): `msbatch.inf` answers setup and copies the pieces above in (its
  `[Install]` section), `autoexec.bat` starts setup, `mbr.asm` boots the disk.
- The Microsoft files are patched by [hvkit](../hvkit): `hvkit vxd patch-io` (VPICD, VTD,
  VKD), `hvkit vxd patch-sysdetmg` (SYSDETMG.DLL), `hvkit patch win98-keyboard` (KEYBOARD.DRV of
  setup's mini-Windows), and `hvkit w98-disk` builds the install disk with all of it.

CSMWrap must come from the `hyperv-gen2-win9x` branches of xdqi/CSMWrap and
xdqi/seabios-csmwrap: SeaBIOS there turns HIMEM's #GP into flat real mode, raises int 09h for
the synthetic keyboard (setup's keyboard driver reads the scancode from the BDA) and
publishes its VMBus connection.

[JHRobotics/vmdisp9x]: https://github.com/JHRobotics/vmdisp9x

## Building

`make w9x` builds `out/w9x/gen2leg.vxd`, `out/w9x/vesamini.vxd` and `out/w9x/vesamini.drv`
(`VMDISP9X_DEBUG=1`: vesamini.vxd logs to COM1). It needs [Open Watcom 2] (`WATCOM`, or
`~/opt/watcom`), fixlink ([JHRobotics/fixlink]), hvkit and `gh`:

- `gen2leg/build.sh`: wcc386, wlink, fixlink, `hvkit vxd fix-entry`.
- `vmdisp9x/build.sh`: clones v1.2025.0.119, applies `fixes.patch` and `build-linux.patch` and
  builds `vesamini.vxd`; `vesamini.drv` is the release's binary.

hvkit: `cargo build` in `hvkit`. [patcher9x] (MIT) has the fixes for current CPUs (TLB
invalidation, CPU speed) that Windows 98 needs at least on Intel 12th/13th generation hosts;
`hvkit w98-disk --patcher9x` runs it on the disk's WIN98 directory (its install-media mode).

[Open Watcom 2]: https://github.com/open-watcom/open-watcom-v2
[JHRobotics/fixlink]: https://github.com/JHRobotics/fixlink
[patcher9x]: https://github.com/JHRobotics/patcher9x

## Installing

```
make w9x
hvkit w98-disk w98se.iso install.vhdx --files out/w9x --vectors w9x/gen2leg/vectors.txt \
    --patcher9x patcher9x --product-key-file key.txt --efi csmwrap.efi --ini csmwrap.ini
```

Attach the disk to a Generation 2 VM (Secure Boot off, 2 or more virtual processors, 512 MB of
static memory, no DVD needed) and start it. Setup copies the files without questions, then:

1. "Restart now": press Enter. Its 15 s countdown does not run (no timer ticks reach setup's
   standard-mode Windows).
2. The OEM first-run wizard (name, license, product key, finish) always shows; its fields are
   filled in from MSBATCH.INF: Enter, Alt+A Enter, Enter, Enter.

Hardware detection follows, Windows restarts and comes up at 1024x768 32 bpp. Tested with the
zh-hans OEM CD of Windows 98 SE (4.10.2222): the patches check the bytes they replace and say so
for other builds.

## Limits

- The disk is reached through the BIOS (INT 13h compatibility mode); there are no network, sound
  or storage drivers.
- Restart resets the VM with a triple fault (it works; Hyper-V logs it).
- The mouse wheel is ignored; DOS boxes under Windows do not get the keyboard yet.
- The VBE frame buffer is 3 MiB: 2D only, no off-screen surfaces at 1024x768.

## License

MIT, like the rest of this repository. `gen2leg/vmm.h` comes from [JHRobotics/smp.vxd]
(MIT, `gen2leg/LICENSE.smp.vxd`); the vmdisp9x patches apply to MIT code.

[JHRobotics/smp.vxd]: https://github.com/JHRobotics/smp.vxd
