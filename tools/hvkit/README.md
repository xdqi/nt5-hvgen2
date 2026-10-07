# hvkit

One Rust tool for the binary patches and media this repository needs, replacing the scattered bash,
Python and PowerShell scripts step by step. So far it has the patch recipes for Microsoft files:

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

## Building

Rust 1.85 or later (edition 2024). From this directory:

```
cargo build --release      # target/release/hvkit
cargo build --release --target x86_64-pc-windows-gnu   # hvkit.exe (needs a mingw-w64 linker)
```

## Tests

```
cargo test
```

The recipe tests compare with the scripts' output byte for byte. They need Microsoft files, so they
read them from the directory named by `HVKIT_TESTDATA` and do nothing without it:

| `in/` | `expected/` (made by the script in migrate/) |
|---|---|
| `ntldr-zh`, `ntldr-en`, `ntldr-2k3`: I386\NTLDR of the zh-hans and en XP SP3 CDs and of Server 2003 SP2 | the same names, `Patch-Ntldr.ps1 -InFile in\X -OutFile expected\X` |
| `setupldr-zh`, `setupldr-en`, `setupldr-2k3`: I386\SETUPLDR.BIN of the same CDs | as above |
| `dmvsc.sys`: Integration Services 6.3.9600.16384 | `dmvsc.sys`, `Patch-Dmvsc.ps1` |
| `icsvc.dll`: Integration Services 6.3.9600.16384 | `icsvc-gsi.dll`, `Install-IcSvcGuestInterfacePatch` on a copy |

Layout: `crates/formats` (PE images, byte patterns), `crates/recipes` (the patches), `hvkit` (the
command line). The design, including the steps still to come, is in the CSMWrap testbed's
`docs/rust-toolkit-design.md`.
