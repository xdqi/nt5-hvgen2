# Changing an installed system offline

```
hvkit inject xp.vhdx --files out          # XP: Dynamic Memory, VSS, Guest Service Interface, SynthVid
hvkit inject xp.vhdx:1 --files out --no-synthvid --vmbaud
```

Adds the components of [setup-cd](setup-cd.md#setup-cd), with the same defaults and switches, to an
XP or Server 2003 that already boots on Generation 2 and has the Integration Services 6.3. The VM must
be off. The system is on a FAT volume of the image: partition N with `IMAGE:N`, by default the one
with `\WINDOWS\system32\config\system`.

The Integration Services' INFs are installed already and their NULL drivers stay. Instead, the
services, Dynamic Memory's Critical Device Database entry and the bootwait entries that keep them
(`Parameters\Devices`, `Parameters\Values`) are written into the registry, as `migrate/inject.ps1
-Dmvsc -Mdlex -VssPatch -GuestInterfacePatch` does. A bootwait.sys too old for those tables is
replaced by the one in `--files`. Running inject again changes nothing.

dmvsc.sys and dmvscres.dll come from `--ic` or the system's Program Files copy of the Integration
Services, icsvc.dll and the SynthVid files from the system. A file a recipe does not know stops
inject before anything is written; the message names the switch that leaves it out. vmbaud goes to
`\Drivers\HV\vmbaud`, which is added to DevicePath; predev cannot run offline, so the sound card's
first appearance brings the Found New Hardware wizard.

Not solved yet: the SynthVid patch is undone on the disk's first Gen2 boot, when Plug and Play
reinstalls SynthVid from its INF's source files (setupapi.log).
