# hvkit

One Rust tool for the patches and media this repository needs, in place of the earlier bash, Python
and PowerShell scripts. The converter in `migrate/` does its offline work through hvkit.exe, the
Windows build.

- [`patch`](docs/patches.md): patch recipes for Microsoft files (NTLDR, dmvsc, icsvc, SynthVid,
  Windows 98's KEYBOARD.DRV);
- [`hive`](docs/hives.md): offline registry hives, also inside disk images;
- [`setup-cd`, `hvfb-cd`, `csmwrap-cd`, `iso`](docs/setup-cd.md): XP and Server 2003 setup CDs for
  Hyper-V Generation 2, the CD that boots CSMWrap, ISO images;
- [`migrate`, `inject`](docs/inject.md): an installed XP moved from Generation 1 to 2, and the same
  Integration Services components for an installed system, offline;
- [`disk`, `fat`](docs/disks.md): raw and VHDX disk images, FAT file systems;
- [`cab`](docs/cab.md): cabinets;
- [`w98-disk`, `vxd`](docs/win98.md): the Windows 98 SE install disk and the VxD patches.

An argument `@FILE` stands for the arguments in FILE, one per line, unquoted; blank lines and lines
starting with `#` are skipped, paths are taken as written (so use absolute ones). That keeps the
settings of one CD or disk in a file: `hvkit setup-cd @zh.args --kd`, with zh.args holding
`/path/XP.iso`, `/path/OUT.iso`, `--files=/path/files`, ... (`--opt=value`, or option and value on
two lines). A file inside a disk image is named `IMAGE:N:PATH` (partition N) or `IMAGE::PATH` (the
one FAT partition that has PATH).

## Building

Rust 1.85 or later (edition 2024). From this directory:

```
cargo build --release      # target/release/hvkit
cargo install --path hvkit # the same into ~/.cargo/bin
cargo build --release -p hvkit --no-default-features   # without hives and ISO mastering: no C libraries
cargo build --release -p hvkit --no-default-features --features inject \
    --target x86_64-pc-windows-gnu                     # hvkit.exe: hives through offreg.dll, migrate
```

Without `-p hvkit` the workspace builds every crate, the C shims included. The Windows build needs
`x86_64-w64-mingw32-gcc` and `-dlltool` on PATH (e.g. `/opt/msys2-cross/bin`) and
`CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc`.

Two C libraries are linked dynamically, each behind a feature: [hivex](https://libguestfs.org/hivex.3.html)
(LGPL-2.1; feature `hive`; `pacman -S hivex`, `apt install libhivex-dev`; on Windows the feature
uses the system's offreg.dll instead) and
[libisofs](https://dev.lovelyhq.com/libburnia/libisofs) (GPL-2.0-or-later; feature `iso`;
`pacman -S libisofs`, `apt install libisofs-dev`). A binary with libisofs falls under the GPL.
Feature `inject` (hives) gives `migrate` and `inject`; `setup-cd` (both libraries) adds `setup-cd`,
`hvfb-cd`, `csmwrap-cd` and `w98-disk`. Reading ISO images needs neither.

## Tests

`cargo test`. The tests that compare with the scripts' and reg.exe's output need Microsoft files
from `HVKIT_TESTDATA`, see [docs/tests.md](docs/tests.md).

Crates: `formats` (PE, LE and NE images, byte patterns, setup text files, ISO 9660 reading, cabinets,
MBRs), `recipes` (the patches), `hive` (hivex or offreg, .reg files), `iso` (libisofs), `disk` (raw
and VHDX images, FAT), `media` (setup CDs, `migrate`, `inject`, the Windows 98 disk); `hvkit` is the
command line.
