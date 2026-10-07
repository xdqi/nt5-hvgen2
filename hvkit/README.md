# hvkit

One Rust tool for the binary patches and media this repository needs, replacing the scattered bash,
Python and PowerShell scripts step by step. So far it has the patch recipes for Microsoft files,
offline registry hives, ISO images, Windows setup CDs, disk images with FAT file systems, cabinets,
and the Windows 98 pieces of the Gen2 work (VxDs, setup's keyboard driver and hardware detection,
the install disk).

Any argument `@FILE` stands for the arguments in FILE, one per line, without quoting; blank lines and
lines starting with `#` are left out. Paths in it are taken as written, so such files use absolute
paths. That keeps the settings of one CD or disk in a file, e.g. `hvkit setup-cd @zh.args --kd`, where
zh.args holds `/path/XP.iso`, `/path/OUT.iso`, `--files=/path/files`, ... (an option and its value
either as `--opt=value` or on two lines).

## Commands

- [`patch`](docs/patches.md): patch recipes for Microsoft files;
- [`hive`](docs/hives.md): registry hives;
- [`setup-cd`, `hvfb-cd`, `csmwrap-cd`, `iso`](docs/setup-cd.md): setup CDs and ISO images;
- [`inject`](docs/inject.md): changing an installed system offline;
- [`disk`, `fat`](docs/disks.md): disk images and FAT;
- [`cab`](docs/cab.md): cabinets;
- [`w98-disk`, `vxd`](docs/win98.md): Windows 98.

## Building

Rust 1.85 or later (edition 2024). From this directory:

```
cargo build --release      # target/release/hvkit (needs hivex and libisofs, see docs/hives.md and docs/setup-cd.md)
cargo install --path hvkit # the same into ~/.cargo/bin
cargo build --release --no-default-features   # without hives and ISO mastering: no C libraries, MIT only
cargo build --release --target x86_64-pc-windows-gnu --no-default-features   # hvkit.exe
```

## Tests

```
cargo test
```

The test data and how it was made: [docs/tests.md](docs/tests.md).

Layout: `crates/formats` (PE, LE and NE images, byte patterns, setup text files, ISO 9660 reading,
cabinets, MBRs), `crates/recipes` (the patches), `crates/hive` (hivex and .reg files), `crates/iso` (libisofs),
`crates/disk` (raw and VHDX images, FAT), `crates/media` (the setup CDs), `hvkit` (the command line).
The design, including the steps still to come, is in the CSMWrap testbed's `docs/rust-toolkit-design.md`.
