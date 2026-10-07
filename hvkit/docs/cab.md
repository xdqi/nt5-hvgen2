# Cabinets

```
hvkit cab ls BASE5.CAB                    # with the rest of its set (linked cabinets next to it)
hvkit cab extract BASE5.CAB out vpicd.vxd vtd.vxd
hvkit cab create OUT.CAB files... [--set 0x9898] [--date 1999-05-05 --time 22:22:00]
hvkit cab create-set MINI.CAB MINI1.CAB --first 11 --set 0x6101 files...
```

Reading handles MSZIP, LZX and stored folders, and sets whose folders continue from cabinet to
cabinet (Windows 98's BASE5.CAB is in the middle of a set of 77); for all four sets on the Windows 98
SE CD it gives the same files as 7z. Writing makes MSZIP cabinets. `create-set` lays out a set of two
like Windows 98 setup's MINI.CAB/MINI1.CAB, whose extractor moves on to the next cabinet only where a
folder continues: the first `--first` files form a folder that continues into the second cabinet
inside the last of them, the other files a second folder of the second cabinet. Windows' expand.exe
treats such a set like the original.
