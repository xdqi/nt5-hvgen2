# hvkit

One Rust tool for the binary patches and media this repository needs, replacing the scattered bash,
Python and PowerShell scripts step by step. So far it has the patch recipes for Microsoft files and
offline registry hives.

## Patch recipes

```
hvkit patch --list
hvkit patch ntldr     I386/NTLDR          # NTLDR / SETUPLDR.BIN: menu highlight in single-plane mode 12h
hvkit patch dmvsc     dmvsc.sys -o out/dmvsc.sys   # rebind XP-missing imports to mdlex.sys
hvkit patch icsvc-gsi icsvc.dll -o icsvcgsi.dll    # Guest Service Interface on XP
hvkit patch ntldr     NTLDR --check       # only tell whether it is stock, patched or not patchable
```

Without `-o` the file is patched in place. Every recipe recognises a file it patched before and leaves
it alone, and refuses files it does not know how to patch. The recipes are ports of
`migrate/Patch-Ntldr.ps1`, `migrate/Patch-Dmvsc.ps1` and `migrate/IcSvcGuestInterface.ps1` and give
the same bytes; the scripts stay until the converter uses hvkit. No Microsoft file is in this
repository: the recipes patch the user's own copies.

## Registry hives

```
hvkit hive info   SYSTEM                       # sequence numbers, version, dirty or not
hvkit hive export SYSTEM -o system.reg --root 'HKEY_LOCAL_MACHINE\XPMIG'   # = reg.exe export
hvkit hive export SYSTEM --key 'ControlSet001\Services\hvfb'              # to stdout, UTF-8
hvkit hive import SYSTEM edits.reg             # = reg.exe import with the hive loaded at the .reg's root
hvkit hive show   SYSTEM 'Services\\vmbus$' 'Control\\Video'           # regdump.py-style view
```

No `reg load`, no Windows, no administrator. `export` writes the file `reg.exe export` writes (UTF-16
with CR LF), byte for byte, including keys whose ACL keeps administrators out; `--utf8` gives the
text hive-dump.sh made of it. `import` takes REGEDIT4 and version 5.00 files, `[-key]` and `"v"=-`
deletions included; the root of the .reg file (by default the first two components of its first
key) stands for the hive's root key. It refuses a dirty hive (log not written back) without
`--force`. Hives are read and written with [hivex](https://libguestfs.org/hivex.3.html) (LGPL-2.1),
linked dynamically: install it (`pacman -S hivex`, `apt install libhivex-dev`) or build without the
`hive` feature.

## Building

Rust 1.85 or later (edition 2024). From this directory:

```
cargo build --release      # target/release/hvkit (needs hivex, see above)
cargo build --release --target x86_64-pc-windows-gnu --no-default-features   # hvkit.exe without hives
```

## Tests

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
| `system-xpv1.hiv`, `system-xpvss.hiv`: SYSTEM hives of XP installations | `system-xpv1.reg`, `system-xpvss.reg`: `reg.exe export` of the hive loaded as HKLM\SPK, run as SYSTEM |
| `setupreg-in.hiv`: SETUPREG.HIV of the zh-hans XP SP3 CD; `setupreg.reg`: zhcd/build.sh's edits | `setupreg-in.reg` (export as above); `setupreg-out.hiv`: reg.exe's import of `setupreg.reg` into it, and its export `setupreg-out.reg` |

Layout: `crates/formats` (PE images, byte patterns), `crates/recipes` (the patches), `crates/hive`
(hivex and .reg files), `hvkit` (the command line). The design, including the steps still to come,
is in the CSMWrap testbed's `docs/rust-toolkit-design.md`.
