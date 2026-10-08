# nLite addons and driver folders

```
hvkit nlite XP.iso OUTDIR --files DIR --ic DIR [--partition fat|ntfs|none] [--kd]
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
| `drivers\<package>\` | the Integration Services packages, prepared as for setup-cd's `$OEM$`, for the Drivers page (PnP) |
| `README.txt` | what to select in nLite |

In nLite: insert every `.cab` as an add-on, every package's `.inf` as a PnP driver, turn the
Unattended page on (its `DriverSigningPolicy=Ignore` lets setup install the changed drivers), keep
Multi-Processor Support. The VM boots CSMWrap from its own CD, as with setup-cd's CDs.

Not (yet) as with setup-cd: predev.exe (vmbaud and the Guest Service Interface get the Found New
Hardware wizard once when their device first turns up after setup); `--kd` only reaches text mode
(nLite's WINNT.SIF has no place for the installed system's boot options); an nLite'd CD without the
multiprocessor HALs (setup-cd's `--mp-source`).
