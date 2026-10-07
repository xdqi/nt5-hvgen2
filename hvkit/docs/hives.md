# Registry hives

```
hvkit hive info   SYSTEM                       # sequence numbers, version, dirty or not
hvkit hive export SYSTEM -o system.reg --root 'HKEY_LOCAL_MACHINE\XPMIG'   # = reg.exe export
hvkit hive export SYSTEM --key 'ControlSet001\Services\hvfb'              # to stdout, UTF-8
hvkit hive import SYSTEM edits.reg             # = reg.exe import with the hive loaded at the .reg's root
hvkit hive show   SYSTEM 'Services\\vmbus$' 'Control\\Video'           # regdump.py-style view
hvkit hive export 'xp.vhdx::\WINDOWS\system32\config\system' --utf8 -o system.txt
hvkit hive import 'xp.vhdx:1:\WINDOWS\system32\config\system' edits.reg   # in place, in the image
```

No `reg load`, no Windows, no administrator. `export` writes the file `reg.exe export` writes (UTF-16
with CR LF), byte for byte, including keys whose ACL keeps administrators out; `--utf8` gives the
text hive-dump.sh made of it. `import` takes REGEDIT4 and version 5.00 files, `[-key]` and `"v"=-`
deletions included; the root of the .reg file (by default the first two components of its first
key) stands for the hive's root key. It refuses a dirty hive (log not written back) without
`--force`. A hive can also be named inside a FAT volume of a disk image: `IMAGE:N:PATH` for
partition N, `IMAGE::PATH` for the one FAT partition that has PATH; `import` writes it back there.
Hives are read and written with [hivex](https://libguestfs.org/hivex.3.html) (LGPL-2.1),
linked dynamically: install it (`pacman -S hivex`, `apt install libhivex-dev`) or build without the
`hive` feature.
