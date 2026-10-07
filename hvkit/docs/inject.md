# Changing an installed system offline

```
hvkit inject xp.vhdx --files out          # XP: Dynamic Memory, VSS, Guest Service Interface, SynthVid
hvkit inject xp.vhdx:1 --files out --no-synthvid --vmbaud
```

The same components as on the setup CD, with the same defaults and switches, for an XP or Server 2003
on a FAT volume (default: the one with `\WINDOWS\system32\config\system`) that already boots on
Generation 2 and has the Integration Services 6.3; the VM must be off. The Integration Services'
INFs are installed there already, so their NULL drivers stay and the services, Dynamic Memory's
Critical Device Database entry and bootwait entries that keep them (Parameters\Devices,
Parameters\Values) are written into the registry, as `migrate/inject.ps1` does with `-Dmvsc -Mdlex
-VssPatch -GuestInterfacePatch`. Running it again changes nothing. dmvsc.sys and dmvscres.dll come
from `--ic` or the Integration Services' copy in the system's Program Files, icsvc.dll and the
SynthVid files from the system; a file the recipe does not know stops it before anything is written,
and the message names the switch that leaves it out. vmbaud goes to `\Drivers\HV\vmbaud`, added to
DevicePath.
