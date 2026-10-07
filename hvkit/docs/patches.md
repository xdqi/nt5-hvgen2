# Patch recipes

```
hvkit patch --list
hvkit patch ntldr     I386/NTLDR          # NTLDR / SETUPLDR.BIN: menu highlight in single-plane mode 12h
hvkit patch dmvsc     dmvsc.sys -o out/dmvsc.sys   # rebind XP-missing imports to mdlex.sys
hvkit patch icsvc-gsi icsvc.dll -o icsvcgsi.dll    # Guest Service Interface on XP
hvkit patch icsvc-vss icsvc.dll -o icsvcvss.dll    # VSS (Backup) service on XP: production checkpoints
hvkit patch synthvid  VMBusVideoM.sys -o out/VMBusVideoM.sys   # SynthVid at 32 bpp, 56 modes:
hvkit patch synthvid  VMBusVideoD.dll -o out/VMBusVideoD.dll   #   both files, installed together
hvkit patch win98-keyboard KEYBOARD.DRV  # Win98 setup's keyboard driver: scancode from the BDA, not port 60h
hvkit patch ntldr     NTLDR --check       # only tell whether it is stock, patched or not patchable
```

Without `-o` the file is patched in place. How the NTLDR recipe works: [ntldr.md](ntldr.md). A recipe leaves a file it patched before alone and
refuses files it does not know. The recipes give the same bytes as the scripts they were ported
from: `migrate/Patch-Ntldr.ps1`, `Patch-Dmvsc.ps1`, `IcSvcGuestInterface.ps1`, `Patch-IcSvcVss.ps1`,
and the CSMWrap testbed's `vid32/patch.py --table` and `w98/patch-kbd.py`. The patched SynthVid files
no longer match the Integration Services catalog's signature. No Microsoft file is in this
repository: the recipes patch the user's own copies.
