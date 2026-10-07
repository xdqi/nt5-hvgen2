# Testing

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
