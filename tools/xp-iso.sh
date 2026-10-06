#!/bin/bash
# Repack a Windows XP x86 CD image so that text-mode setup uses hvfb.sys as
# its display miniport instead of vga.sys, and the installed system gets
# hvfb as a boot-started display driver.
#
#   tools/xp-iso.sh BASE.iso OUT.iso
#
# Env: SYS     hvfb.sys to integrate (default out/hvfb.sys)
#      WORK    scratch directory (default /tmp/hvfb-iso)
#      INSTALL 1 (default): also install hvfb into the new system (copy to
#              system32\drivers, service entries in HIVESYS.INF); 0: text-mode
#              setup only
#      DEFAULT_MODE  WxHxBPP the installed system starts in (e.g. 1024x768x32,
#              the Hyper-V Gen2 native mode).  Unset: setup records the mode
#              it ran in (hvfb's mode 0, normally 640x480x32)
#      VOLID   ISO volume id (default: keep the base image's)
#
# The boot image and the xorriso options follow a plain El Torito
# no-emulation XP CD; BASE.iso is only read.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
BASE=$1
OUT=$2
SYS=${SYS:-$here/../out/hvfb.sys}
WORK=${WORK:-/tmp/hvfb-iso}
INSTALL=${INSTALL:-1}
VOLID=${VOLID:-$(xorriso -indev "$BASE" -pvd_info 2>/dev/null | sed -n 's/^Volume Id *: *//p')}

[ -f "$SYS" ] || { echo "xp-iso: $SYS not found (run make)" >&2; exit 1; }
rm -rf "$WORK/root"
mkdir -p "$WORK/root"
7z x -y -o"$WORK/root" "$BASE" >/dev/null
mv "$WORK/root/[BOOT]/Boot-NoEmul.img" "$WORK/root/boot.img"
rmdir "$WORK/root/[BOOT]"
I386=$WORK/root/I386
cp "$SYS" "$I386/HVFB.SYS"

python3 - "$I386/TXTSETUP.SIF" "$I386/HIVESYS.INF" "$INSTALL" "${DEFAULT_MODE:-}" <<'EOF'
import re, sys

sif_path, hive_path, install, default_mode = sys.argv[1], sys.argv[2], sys.argv[3] == "1", sys.argv[4]
if default_mode and not re.fullmatch(r"\d+x\d+x\d+", default_mode):
    sys.exit("DEFAULT_MODE must look like 1024x768x32")

def load(p):
    return open(p, encoding="latin-1", newline="").read().split("\r\n")

def save(p, lines):
    open(p, "w", encoding="latin-1", newline="").write("\r\n".join(lines))

def section(lines, name):
    """[start, end) of the first section called `name` (body only)."""
    start = None
    for i, l in enumerate(lines):
        m = re.match(r"\s*\[([^\]]+)\]", l)
        if m:
            if start is not None:
                return start, i
            if m.group(1).lower() == name.lower():
                start = i + 1
    if start is None:
        sys.exit("section [%s] not found" % name)
    return start, len(lines)

def key(l):
    return l.split("=")[0].strip().lower() if "=" in l and not l.lstrip().startswith(";") else None

sif = load(sif_path)

# Text-mode setup loads the miniport named under the "vga" display id.
s, e = section(sif, "Display.Load")
idx = [i for i in range(s, e) if key(sif[i]) == "vga"]
if len(idx) != 1:
    sys.exit("[Display.Load] has no single vga entry")
sif[idx[0]] = "vga      = hvfb.sys"

# Same source/target attributes as vga.sys: on the boot media, copied to
# system32\drivers (directory 4) on every fresh install and upgrade.
s, e = section(sif, "SourceDisksFiles")
vga = [i for i in range(s, e) if key(sif[i]) == "vga.sys"]
if not vga:
    sys.exit("vga.sys missing from [SourceDisksFiles]")
if not any(key(sif[i]) == "hvfb.sys" for i in range(s, e)):
    sif.insert(vga[0] + 1, "hvfb.sys = 100,,,,,,4_,4,0,0,,1,4")

if install and not default_mode:
    # Third field of a [Display] entry: the service whose Device0 key setupdd
    # writes the text-mode display settings to (DefaultSettings.*), so the
    # installed system starts in the mode text-mode setup used.
    s, e = section(sif, "Display")
    for i in range(s, e):
        if key(sif[i]) == "vga":
            sif[i] = 'vga      = "Auto Detect",files.none,hvfb'
save(sif_path, sif)

if install:
    hive = load(hive_path)
    s, e = section(hive, "AddReg")
    svc = r'HKLM,"SYSTEM\CurrentControlSet\Services\hvfb'
    add = [
        svc + r'","ErrorControl",0x00010003,0',
        svc + r'","Group",0x00000000,"Video"',
        svc + r'","ImagePath",0x00020000,"\SystemRoot\System32\drivers\hvfb.sys"',
        svc + r'","Start",0x00010001,1',
        svc + r'","Type",0x00010001,1',
        svc + r'\Device0","InstalledDisplayDrivers",0x00010000,"framebuf"',
        svc + r'\Device0","VgaCompatible",0x00010001,0',
    ]
    if default_mode:
        w, h, bpp = default_mode.split("x")
        for name, value in (("XResolution", w), ("YResolution", h), ("BitsPerPel", bpp),
                            ("VRefresh", "60"), ("Flags", "0"), ("XPanning", "0"),
                            ("YPanning", "0")):
            add.append(svc + r'\Device0","DefaultSettings.%s",0x00010001,%s' % (name, value))
    if not any("Services\\hvfb" in l for l in hive):
        # Next to the VgaSave service, the other legacy display miniport.
        last = max(i for i in range(s, e) if "Services\\VgaSave" in hive[i])
        hive[last + 1:last + 1] = add
    save(hive_path, hive)
EOF

grep -n -i 'hvfb' "$I386/TXTSETUP.SIF" "$I386/HIVESYS.INF" | tr -d '\r'
rm -f "$OUT"
xorriso -as mkisofs -quiet -iso-level 2 -J -joliet-long -l -D -N -relaxed-filenames \
    -V "$VOLID" -b boot.img -no-emul-boot -boot-load-size 4 -hide boot.img -hide boot.catalog \
    -o "$OUT" "$WORK/root"
ls -la "$OUT"
