#!/bin/bash
# Boot a Windows XP CD through CSMWrap (UEFI -> legacy BIOS) in QEMU/KVM and
# take periodic screendumps; modelled on CSMWrap's hyperv/qemu-xp.sh but with
# its own scratch directory.  QEMU has a real VGA with VBE, so this exercises
# hvfb's VBE path (Hyper-V Gen2 has no VGA, which QEMU cannot reproduce).
#
#   ISO=/tmp/hvfb-iso/xp-hvfb.iso tools/qemu-xp.sh
#
# Env: ISO      CD image (required; "none" for no CD drive)
#      CSMWRAP  CSMWrap checkout (for hyperv/mkimg.sh, the OVMF build, MBRs)
#      EFI      csmwrap.efi to boot (default $CSMWRAP/bin-x86_64/csmwrap.efi)
#      Q        scratch directory (default /tmp/hvfb-qemu)
#      SECS     run time in seconds (240), STEP screendump interval (20)
#      MACHINE  pc (IDE, default; XP has no AHCI driver) or q35
#      GROW_MB  grow the boot disk to this size (0 = keep 64 MiB), leaving
#               unpartitioned space after the CSMWrap ESP to install XP to.
#               (A second IDE disk does not work: CSMWrap's SeaBIOS then has
#               no room left to register the CD drive.)
#      BOOT_RAW use this existing disk image instead of building one (e.g. to
#               continue an installation); it is used in place, not copied
#      KEYS     file with "<seconds> <qemu sendkey argument>" lines, sent
#               through the monitor at those times (e.g. "60 ret")
#      EXTRA    more QEMU arguments
# Output: $Q/sNNN.png screendumps, $Q/serial.log (CSMWrap/SeaBIOS COM1).
set -u
CSMWRAP=${CSMWRAP:-/home/kosaka/Projects/CSMWrap}
Q=${Q:-/tmp/hvfb-qemu}
ISO=${ISO:?set ISO=}
EFI=${EFI:-$CSMWRAP/bin-x86_64/csmwrap.efi}
SECS=${SECS:-240}
STEP=${STEP:-20}
GROW_MB=${GROW_MB:-0}
mkdir -p "$Q"
rm -f "$Q"/mon.sock "$Q"/serial.log "$Q"/qemu.log "$Q"/s[0-9]*.p*

# Boot disk: FAT ESP with CSMWrap; its MBR just does INT 18h so the BIOS
# moves on to the CD.
if [ -z "${BOOT_RAW:-}" ]; then
    WORK=$Q/img EFI=$EFI MBR=$CSMWRAP/hyperv/mbr/int18.bin OUT=$Q/boot.vhdx \
        "$CSMWRAP/hyperv/mkimg.sh" >/dev/null
    BOOT_RAW=$Q/boot.raw
    cp "$Q/img/disk.raw" "$BOOT_RAW"
    [ "$GROW_MB" -gt 0 ] && truncate -s "${GROW_MB}M" "$BOOT_RAW"
fi
disks=(-drive file="$BOOT_RAW",format=raw,if=none,id=d0 -device ide-hd,drive=d0,bus=ide.0,unit=0)
[ "$ISO" = none ] ||
    disks+=(-drive file="$ISO",format=raw,if=none,id=cd0,media=cdrom,readonly=on -device ide-cd,drive=cd0,bus=ide.1)

monitor() {
    python3 - "$Q/mon.sock" "$1" <<'PY'
import socket, sys, time
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); time.sleep(0.2); s.recv(65536)
s.sendall((sys.argv[2] + "\n").encode()); time.sleep(0.5); s.close()
PY
}

(
    t=0
    while sleep 1; do
        t=$((t + 1))
        [ -S "$Q/mon.sock" ] || { [ $t -gt 10 ] && break; continue; }
        if [ -n "${KEYS:-}" ]; then
            while read -r at key; do
                [ "$at" = "$t" ] && monitor "sendkey $key"
            done < "$KEYS"
        fi
        [ $((t % STEP)) -eq 0 ] && { monitor "screendump $Q/s$(printf %04d $t).ppm" || break; }
    done
) &

timeout "$SECS" qemu-system-x86_64 -M "${MACHINE:-pc}" -accel kvm -cpu host -m 1G -smp 2 \
    -drive if=pflash,unit=0,format=raw,file="$CSMWRAP/edk2-ovmf/ovmf-code-x86_64.fd",readonly=on \
    "${disks[@]}" \
    -nic none -display none -serial file:"$Q/serial.log" -d cpu_reset,guest_errors -D "$Q/qemu.log" \
    -monitor unix:"$Q/mon.sock",server,nowait ${EXTRA:-} > "$Q/stdout.log" 2>&1
echo "qemu exit=$? (124 = still running at timeout)"
wait
python3 - "$Q" <<'PY'
import glob, sys
from PIL import Image
for f in sorted(glob.glob(sys.argv[1] + "/s[0-9]*.ppm")):
    Image.open(f).save(f[:-4] + ".png"); print(f[:-4] + ".png")
PY
