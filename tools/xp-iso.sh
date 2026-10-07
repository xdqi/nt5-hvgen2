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
#      PRODUCT_KEY_FILE  file holding a product key (one line) to put into
#              WINNT.SIF [UserData] ProductKey; the key is never printed
#      BOOTVID bootvid.dll to use instead of the CD's (e.g. out/bootvid.dll,
#              for machines without VGA such as Hyper-V Gen2).  The kernel
#              of text-mode setup loads it and setup copies it to system32
#              like the original.  Unset: keep XP's VGA bootvid.dll
#
#      HVKIT   the hvkit binary (default: hvkit on PATH; build it with
#              cargo install --path tools/hvkit/hvkit)
#
# The work is done by `hvkit hvfb-cd` (tools/hvkit/crates/media/src/hvfb_cd.rs).
# The CD has El Torito no-emulation boot and no Rock Ridge, like Microsoft's;
# BASE.iso is only read.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
[ $# = 2 ] || { sed -n '2,32p' "$0"; exit 2; }
BASE=$1
OUT=$2
SYS=${SYS:-$here/../out/hvfb.sys}
WORK=${WORK:-/tmp/hvfb-iso}
[ -f "$SYS" ] || { echo "xp-iso: $SYS not found (run make)" >&2; exit 1; }
args=(hvfb-cd "$BASE" "$OUT" --hvfb "$SYS" --work "$WORK/root")
[ "${INSTALL:-1}" = 1 ] || args+=(--no-install)
[ -n "${DEFAULT_MODE:-}" ] && args+=(--default-mode "$DEFAULT_MODE")
[ -n "${VOLID:-}" ] && args+=(--volume-id "$VOLID")
[ -n "${PRODUCT_KEY_FILE:-}" ] && args+=(--product-key-file "$PRODUCT_KEY_FILE")
[ -n "${BOOTVID:-}" ] && args+=(--bootvid "$BOOTVID")
exec "${HVKIT:-hvkit}" "${args[@]}"
