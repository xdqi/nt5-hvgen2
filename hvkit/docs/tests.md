# Tests

```
cargo test
```

The recipe and hive tests compare with the scripts' and reg.exe's output byte for byte. They need
Microsoft files, so they read them from the directory named by `HVKIT_TESTDATA` and do nothing
without it:

| `in/` | `expected/` (made by the script in migrate/) |
|---|---|
| `ntldr-zh`, `ntldr-en`, `ntldr-2k3`: I386\NTLDR of the zh-hans and en XP SP3 CDs and of Server 2003 SP2 | the same names, `Patch-Ntldr.ps1 -InFile in\X -OutFile expected\X` |
| `setupldr-zh`, `setupldr-en`, `setupldr-2k3`: I386\SETUPLDR.BIN of the same CDs | as above |
| `dmvsc.sys`: Integration Services 6.3.9600.16384 | `dmvsc.sys`, `Patch-Dmvsc.ps1` |
| `icsvc.dll`: Integration Services 6.3.9600.16384 | `icsvc-gsi.dll`, `Install-IcSvcGuestInterfacePatch` on a copy |
| `icsvc.dll` (the same file) | `icsvc-vss.dll`, `Patch-IcSvcVss.ps1` |
| `VMBusVideoM.sys`, `VMBusVideoD.dll`: Integration Services 6.3.9600.16384 | the same names, `vid32/patch.py in out --table` (CSMWrap testbed) |
| `dmvsc.inf`, `vmic.inf`: Integration Services 6.3.9600.16384 | none: the media tests check the CD's edits of them line by line |
| `system-xpv1.hiv`, `system-xpvss.hiv`: SYSTEM hives of XP installations | `system-xpv1.reg`, `system-xpvss.reg`: `reg.exe export` of the hive loaded as HKLM\SPK, run as SYSTEM |
| `keyboard.drv`: KEYBOARD.DRV of the zh-hans Windows 98 SE CD's MINI.CAB | `keyboard.drv`, `w98/patch-kbd.py` |
| `vpicd.vxd`, `vtd.vxd`, `vkd.vxd` (BASE5.CAB); `gen2leg-vectors.txt` (the vectors.txt used) | the same names, `w98/patch-io.py patch` |
| `sysdetmg.dll`: SYSDETMG.DLL of the zh-hans Windows 98 SE CD's PRECOPY2.CAB | `sysdetmg.dll`, `w98/patch-sysdetmg.py` |
| `wlink-type2.vxd`: a VxD with wlink's type 2 DDB entry | `wlink-type2.vxd`, `gen2leg-ow/fixentry.py` |
| `setupreg-in.hiv`: SETUPREG.HIV of the zh-hans XP SP3 CD; `setupreg.reg`: zhcd/build.sh's edits | `setupreg-in.reg` (export as above); `setupreg-out.hiv`: reg.exe's import of `setupreg.reg` into it, and its export `setupreg-out.reg` |

The setup CD builders were compared with the scripts they replace (the CSMWrap testbed's
zhcd/build.sh on the zh-hans XP SP3, WinLite and zh-hans Server 2003 R2 SP2 CDs, and tools/xp-iso.sh)
by building the same CDs both ways: the trees are identical except for SETUPREG.HIV, whose keys and
values are (hivex writes other bytes than reg.exe), and the first comment of WINNT.SIF; booted on
Hyper-V Generation 2, both CDs show the same screens up to the partition list. That needs gigabytes of
CDs and Microsoft files, so it is not part of `cargo test`.
