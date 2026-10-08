# NT 6.x setup DVDs

```
hvkit setup-dvd WIN7.iso OUT.iso [--driver DIR]... [--replace-inbox] [--copy SRC=DEST]...
    [--reg [HIVE:]FILE]... [--hvfb out/hvfb.sys [--mode 1024x768x32] [--deny-synthvid]]
    [--poll-boot-partition MS] [--unattend [--product-key-file F] [--computer-name N] [--arch A]]
    [--boot-index 2] [--install-index 1] [--no-boot-wim] [--no-install-wim] [--work DIR] [--hook SCRIPT]
```

Makes a Windows NT 6.x (Vista and later; tried with Windows 7 SP1 and Windows Thin PC, x86) DVD
that installs on Hyper-V Generation 2 through CSMWrap's CD. It changes image 2 of
`sources/boot.wim` (Windows Setup) and an image of `sources/install.wim` with the `wim` crate,
without DISM and without Windows: added files have no security descriptor of their own, replaced
files keep theirs, and every unchanged resource is copied as it is (a Thin PC DVD takes about 25
seconds). The reason for each change is in `crates/media/src/setup_dvd.rs`.

- `--driver DIR`: every INF below DIR is a driver package. They go in `$WinPEDriver$` on the DVD,
  which Setup loads into Windows PE and adds to the installed system, except boot-critical ones
  (a boot-start service or a storage controller class): Setup stops on those because it does not
  install them unsigned.
- `--replace-inbox`: where an image has its own package of a `--driver` package's name
  (`Windows\inf\NAME.inf`), it becomes that package: both copies of the INF, the files of its
  driver store folder, its files in `System32\drivers` (.sys) and `System32` (the rest), and the
  INF cache (`System32\DriverStore\INFCACHE.1`) goes so that SetupAPI rebuilds it with the new
  INFs' hardware IDs. Windows 7's own signed Integration Services packages outrank the staged
  unsigned ones at device install, so without this the system runs its own `vmbus.sys` beside the
  package's `hyperkbd.sys` (keyboard code 10), and Windows PE its own VMBus and storage drivers,
  which do not start on Generation 2. This is how the IC 6.3 VMBus and storage drivers get in at
  all. Not a standard interface: the image's own package is no longer what its catalog signed.
- `--hvfb`: the display (Generation 2 has no VGA, so the screen stays black without it), with the
  VideoPort registry and `framebuf.dll` at `--mode`. `--deny-synthvid` sets the device
  installation policy that keeps SynthVid from starting mid-setup and moving the VRAM away from
  hvfb (Hyper-V event 18570 on an MMX access). Without `--hvfb`, SynthVid is the display.
- `--poll-boot-partition` (default 30000 ms): `PollBootPartitionTimeout`. Without it
  `nt!PnpBootDeviceWait` does not wait for the VMBus boot disk at all.
- `--copy SRC=DEST` puts a file in both images (DEST below the volume root), `--reg` applies a .reg
  file to a hive of both (default SYSTEM; the first key names the root, e.g.
  `HKEY_LOCAL_MACHINE\W7\...`).
- `--unattend` writes `autounattend.xml`: one active NTFS partition over disk 0, the built-in
  Administrator with an empty password and auto-logon, OOBE skipped.

Windows Thin PC, verified on Generation 2 (VM with Dynamic Memory and the Guest Service Interface)
from an empty disk to the desktop without a key press in about 5½ minutes, with the KB3063109
packages (IC 6.3.9600.17903; dmvsc.sys patched with `hvkit patch dmvsc`) and mdlex.sys:

```
hvkit setup-dvd en_windows_thin_pc_x86_697681.iso OUT.iso --driver ic17903 --replace-inbox \
    --copy out/mdlex.sys=Windows/System32/drivers/mdlex.sys \
    --hvfb out/hvfb.sys --deny-synthvid --unattend
```

The keyboard and mouse, storvsc, Dynamic Memory and every Integration Services component work; the
only devices without a driver are SynthVid (denied) and the two Automatic VM Activation devices.

The work tree (`--work`, default OUT.d) holds hard links to the extracted DVD where both are on one
filesystem; keep it beside `--cache`.
