# nLite addons and driver folders

```
hvkit nlite XP.iso OUTDIR --files DIR --ic DIR [--partition fat|ntfs|none] [--kd]
    [--product-key-file FILE] [--time-zone 210]
    [--no-dynamic-memory] [--no-vss] [--no-gsi] [--no-synthvid] [--vmbaud] [--no-patch-ntldr]
```

For a CD that is processed with nLite 1.4.9.3 anyway: the changes `setup-cd` makes (the list is
`crates/media/src/nt5.rs`), as nLite addons and folders for nLite's Drivers page. The CD is only
read, for its version and for the exact text of the lines the addons replace. `--files` and `--ic`
are setup-cd's. The addons carry your Microsoft files: they are for you only.

| OUTDIR | |
|---|---|
| `addons\hvgen2-core.cab` | text-mode drivers (KMDF, VMBus, storvsc with the KB943295 storport, bootwait, hyperkbd), their TXTSETUP.SIF sections and lines, the keyboard map, HIVESYS.INF lines, the kernel options |
| `addons\hvgen2-mphal.cab` | text mode with the multiprocessor HAL; keeps nLite's Multi-Processor Support |
| `addons\hvgen2-display.cab` | hvfb.sys, the frame buffer bootvid.dll, its HIVESYS.INF lines |
| `addons\hvgen2-ntldr.cab` | NTLDR and SETUPLDR.BIN with the mode 12h highlight patch (not with `--no-patch-ntldr`) |
| `addons\hvgen2-partition.cab` | `--partition fat` or `ntfs` as for setup-cd, by changing the WINNT.SIF of nLite's Unattended page |
| `addons\hvgen2-predev.cab` | with the Guest Service Interface or `--vmbaud`: predev.exe in `\I386\SVCPACK` and a line per device in SVCPACK.INF, which runs it at T-13 of GUI-mode setup (setup-cd's CDs run it from cmdlines.txt); it finds the INFs by name in DevicePath, where nLite's driver folders are |
| `drivers\<package>\` | the Integration Services packages, prepared as for setup-cd's `$OEM$`, for the Drivers page (PnP) |
| `hvgen2.ini`, `hvgen2_u.ini` | an nLite preset with all of it (written when Windows can see OUTDIR: hvkit.exe, or under `/mnt/<drive>` in WSL) |
| `README.txt` | what to select in nLite |

In nLite: insert every `.cab` as an add-on, every package's `.inf` as a PnP driver, turn the
Unattended page on (its `DriverSigningPolicy=Ignore` lets setup install the changed drivers), keep
Multi-Processor Support. Or load `hvgen2.ini` on the Presets page: it has the add-ons, the drivers
and a fully unattended setup (UnattendMode FullUnattended, no Windows Welcome, `--time-zone` by its
index under the name the CD's HIVESFT.INF gives it, the product key, a server's licensing mode).
`nLite.exe /path:<CD folder> /preset:<OUTDIR>\hvgen2.ini` (run as administrator) processes the
folder with it and exits; nLite makes no ISO then (`hvkit iso build --nt5-setup`). The preset keeps
nLite from copying itself, product key included, into the CD's root (NoISOPreset). The VM boots
CSMWrap from its own CD, as with setup-cd's CDs.

Verified 2026-10-08 with nLite 1.4.9.3 on a host with code page 1252: zh-hans XP SP3 and Server 2003
R2 SP2 from the preset to the desktop without a key press (the GBK TXTSETUP.SIF comes through byte
for byte); the vmbus, storvsc and synthkbd packages work through the Drivers page although their
files are text-mode files too.

Not (yet) as with setup-cd: `--kd` only reaches text mode
(nLite's WINNT.SIF has no place for the installed system's boot options); an nLite'd CD without the
multiprocessor HALs (setup-cd's `--mp-source`).
