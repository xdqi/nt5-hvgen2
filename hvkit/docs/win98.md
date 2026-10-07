# Windows 98

```
hvkit w98-disk w98se.iso install.vhdx --files out/w9x --vectors w9x/gen2leg/vectors.txt \
    --product-key-file KEY --efi csmwrap.efi [--ini csmwrap.ini] [--patcher9x patcher9x] \
    [--owner NAME] [--org ORG] [--size 1600M]
hvkit vxd info VPICD.VXD                  # header, objects, entries, DDB (FILE@0xOFF: an LE inside a W3)
hvkit vxd scan VPICD.VXD VTD.VXD          # port I/O to the PIC, PIT, port 61h, i8042
hvkit vxd show VPICD.VXD 1 17c0 17f0      # disassembly of part of an object, fixups marked
hvkit vxd patch-io VPICD.VXD out.vxd --vectors w9x/gen2leg/vectors.txt
hvkit vxd patch-sysdetmg SYSDETMG.DLL [-o OUT] --vectors w9x/gen2leg/vectors.txt [--check]
hvkit vxd fix-entry gen2leg.vxd           # wlink's type 2 DDB export -> type 3
```

`w98-disk` makes a disk that installs Windows 98 SE on Hyper-V Generation 2 by itself (see
`w9x/README.md`; `make w9x` builds `--files`): an MBR, a FAT16 EFI system partition with CSMWrap,
and an active FAT16 C: with the CD's DOS, an AUTOEXEC.BAT that runs setup with MSBATCH.INF, and
C:\WIN98, the CD's WIN98 directory with setup's MINI.CAB set rebuilt around the patched
KEYBOARD.DRV, the patched SYSDETMG.DLL, VPICD.VXD, VTD.VXD and VKD.VXD as loose files (setup prefers
them to the cabinets' copies and builds VMM32.VXD from the VxDs), the files of `--files`
(GEN2LEG.VXD, VESAMINI.DRV, VESAMINI.VXD) and `w9x/setup`'s MSBATCH.INF, GEN2DISP.INF and
GEN2MON.INF. `--patcher9x` runs [patcher9x](https://github.com/JHRobotics/patcher9x) on that
directory (menu choice 4, its install-media mode), which leaves VMM32.VXD, NDIS.VXD and VCACHE.VXD
with its fixes for current CPUs there. The CD's extracted copy is kept in `~/.cache/hvkit/cd`.

`patch-io` redirects VPICD's, VTD's and VKD's port I/O into the GEN2LEG shim VxD (an `int vv` per
port and direction, the vectors from the shim's build) and is the CSMWrap testbed's `w98/patch-io.py`,
with the same output; `patch-sysdetmg` does the same for the port I/O of SYSDETMG.DLL, hardware
detection (its `w98/patch-sysdetmg.py`); `fix-entry` is its `gen2leg-ow/fixentry.py`. The sweep uses
iced-x86 instead of ndisasm; it finds the same sites in all 266 VxDs of the CD except in data inside
code objects.
