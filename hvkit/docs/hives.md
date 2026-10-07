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

No `reg load`, no administrator; needs the `hive` feature: hivex, or on Windows the system's
offreg.dll (Offline Registry Library).

- `export` writes byte for byte what `reg.exe export` writes (UTF-16, CR LF), including keys whose
  ACL keeps administrators out; `--utf8` writes UTF-8 instead.
- `import` takes REGEDIT4 and version 5.00 files, including `[-key]` and `"v"=-` deletions. The
  root of the .reg file (by default the first two components of its first key) stands for the
  hive's root key. A dirty hive (log not written back) needs `--force`.
- hivex changes the hive file in place; offreg writes it anew (compacted) in the regf version it
  had. A new key gets its parent's security descriptor (hivex) or its parent's inheritable entries
  (offreg, as Windows); the two agree below the control sets of XP's SYSTEM hive.
- A hive inside a disk image (`IMAGE:N:PATH`, `IMAGE::PATH`) is read from its FAT volume, and
  `import` writes it back there.
