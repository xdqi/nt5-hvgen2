#!/bin/sh
# Load a driver image and its PDB into the Windows console debugger (cdb.exe)
# through WSL interop and list the module's symbols.  Fails unless the PDB
# matched and DriverEntry resolved.
#
#   tools/cdb-check.sh out/hvfb.sys hvfb
#
# CDB: path to cdb.exe (Linux path; default: cdb.exe found on PATH).
set -eu
IMAGE=$1
MODULE=$2
CDB=${CDB:-$(command -v cdb.exe || true)}

command -v wslpath >/dev/null || { echo "cdb-check: needs WSL (wslpath)" >&2; exit 1; }
[ -n "$CDB" ] && [ -x "$CDB" ] || { echo "cdb-check: cdb.exe not found (set CDB=)" >&2; exit 1; }

dir=$(cd "$(dirname "$IMAGE")" && pwd -P)
case $dir in
/mnt/*) ;;
*) echo "cdb-check: $dir must be on a Windows drive (/mnt/...)" >&2; exit 1 ;;
esac
wimg=$(wslpath -w "$dir/$(basename "$IMAGE")")
wdir=$(wslpath -w "$dir")

log=$(mktemp)
trap 'rm -f "$log"' EXIT
"$CDB" -z "$wimg" -y "$wdir" -c ".reload /f; lm v m $MODULE; x $MODULE!*; q" \
    2>&1 | tr -d '\r' | grep -v -e '^NatVis script' -e '^$' | tee "$log"

grep -q "$MODULE.*(private pdb symbols)" "$log" || { echo "cdb-check: PDB not loaded" >&2; exit 1; }
grep -q "$MODULE!DriverEntry" "$log" || { echo "cdb-check: DriverEntry not resolved" >&2; exit 1; }
echo "cdb-check: OK"
