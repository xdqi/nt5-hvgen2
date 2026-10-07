#!/bin/bash
# Build gen2leg.vxd the way smp.vxd (github.com/JHRobotics/smp.vxd) is built: Open Watcom's wcc386 and
# wlink (`system win_vxd`), then fixlink -vxd32, then `hvkit vxd fix-entry` (this wlink exports the DDB
# as a type 2 entry, which Windows 98 refuses as "damaged").
# Usage: build.sh [out.vxd]        (default: out/w9x/gen2leg.vxd at the top of the repository)
# Env: WATCOM    Open Watcom 2 (default: ~/opt/watcom, whose env.sh is sourced)
#      FIXLINK   fixlink (gcc -std=c89 fixlink.c from github.com/JHRobotics/fixlink; default
#                ~/opt/fixlink/fixlink, else from PATH)
#      HVKIT     hvkit (default: from PATH)
#      EXTRA_CFLAGS  e.g. -DEXIT_TEST
set -euo pipefail
cd "$(dirname "$0")"
top=$(cd ../.. && pwd)
out=${1:-$top/out/w9x/gen2leg.vxd}
if [ -z "${WATCOM:-}" ]; then source "$HOME/opt/watcom/env.sh"; fi
FIXLINK=${FIXLINK:-$( [ -x "$HOME/opt/fixlink/fixlink" ] && echo "$HOME/opt/fixlink/fixlink" || echo fixlink )}
HVKIT=${HVKIT:-hvkit}
O=$(mktemp -d /tmp/gen2leg.XXXX)
trap 'rm -rf "$O"' EXIT
# vectors.txt: the IDT vectors of the shim's operations, also read by `hvkit vxd patch-io` and
# `hvkit vxd patch-sysdetmg`; VMM's IDT has 96 gates.
v=$(tr -s ' \t\r\n' ' ' < vectors.txt | sed 's/^ //; s/ $//')
set -- $v
[ $# = 22 ] || { echo "vectors.txt: 22 vectors expected, got $#" >&2; exit 1; }
[ "$(printf '%s\n' "$@" | sort -u | wc -l)" = 22 ] || { echo "vectors.txt: vectors not distinct" >&2; exit 1; }
for x in "$@"; do [ $((x)) -lt 96 ] || { echo "vectors.txt: $x is not below 0x60" >&2; exit 1; }; done
echo "static const u8 vec_table[NVEC] = { $(printf '%s\n' "$@" | paste -sd, | sed 's/,/, /g') };" > "$O/vectors.h"
CF="-q -wx -s -zls -6s -fp6 -mf -DVXD32 -fpi87 -ei -oeatxhn -za99 ${EXTRA_CFLAGS:-}"
wcc386 $CF -I"$WATCOM/h/win" -I"$O" -I. -I../vmbc -fo="$O/gen2leg.obj" -fr="$O/gen2leg.err" gen2leg.c
cat > "$O/gen2leg.lnk" <<LNK
system win_vxd dynamic
option map=gen2leg.map
option nodefaultlibs
name gen2leg.vxd
file gen2leg.obj
segment '_TEXT'  PRELOAD NONDISCARDABLE IOPL
segment '_DATA'  PRELOAD NONDISCARDABLE IOPL
segment 'CONST'  PRELOAD NONDISCARDABLE IOPL
segment 'CONST2' PRELOAD NONDISCARDABLE IOPL
segment '_BSS'   PRELOAD NONDISCARDABLE IOPL
export VXD_DDB.1
LNK
(cd "$O" && wlink op quiet @gen2leg.lnk && "$FIXLINK" -vxd32 gen2leg.vxd)
"$HVKIT" vxd fix-entry "$O/gen2leg.vxd" >/dev/null
mkdir -p "$(dirname "$out")"
cp "$O/gen2leg.vxd" "$out"
cp "$O/gen2leg.map" "${out%.vxd}.map"
echo "$out $(stat -c %s "$out") bytes"
