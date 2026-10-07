#!/bin/bash
# vmdisp9x's VESA display driver (github.com/JHRobotics/vmdisp9x, MIT) for Windows 98 on Gen2:
#   vesamini.vxd  built from the v1.2025.0.119 source with fixes.patch:
#                 - Device_Init: the control procedure popped registers it had not pushed
#                 - VESA_setmode_phy: no back buffer when the VBE memory cannot hold a second frame
#                   (SeaVGABIOS reports 3 MiB; 1024x768 at pitch 4096 is 3 MiB already)
#   vesamini.drv  the release's binary (a build of it with this toolchain fails to load)
# build-linux.patch makes the makefile work with Open Watcom on Linux and sends the VxD's debug output
# to COM1, the port a Gen2 VM has.
# Usage: build.sh [outdir]        (default: out/w9x at the top of the repository)
# Env: VMDISP9X  checkout of v1.2025.0.119 to build (default: cloned into ~/.cache/w9x/vmdisp9x)
#      WATCOM    Open Watcom 2 (default: ~/opt/watcom, whose env.sh is sourced)
#      DEBUG=1   debug output on COM1 (install.sh in the CSMWrap testbed waits for its "SET success")
set -euo pipefail
cd "$(dirname "$0")"
here=$(pwd); top=$(cd ../.. && pwd)
outdir=${1:-$top/out/w9x}
TAG=v1.2025.0.119
ZIP=vmdisp9x-1.2025.0.119b-driver-2d.zip
src=${VMDISP9X:-$HOME/.cache/w9x/vmdisp9x}
if [ ! -d "$src/.git" ]; then
    gh repo clone JHRobotics/vmdisp9x "$src" -- -q --depth 1 --branch "$TAG" --recurse-submodules --shallow-submodules
fi
if [ -z "${WATCOM:-}" ]; then source "$HOME/opt/watcom/env.sh"; fi
B=$(mktemp -d /tmp/vmdisp9x.XXXX)
trap 'rm -rf "$B"' EXIT
(cd "$src" && tar --exclude=.git -cf - .) | tar -xf - -C "$B"
git -C "$B" apply --whitespace=nowarn "$here/fixes.patch" "$here/build-linux.patch"
(cd "$B" && wmake -h ${DEBUG:+DBGPRINT=1} vesamini.vxd >wmake.log 2>&1) || { tail -20 "$B/wmake.log"; exit 1; }
mkdir -p "$outdir"
cp "$B/vesamini.vxd" "$outdir/vesamini.vxd"
cache=$HOME/.cache/w9x
[ -f "$cache/$ZIP" ] || gh release download "$TAG" -R JHRobotics/vmdisp9x -p "$ZIP" -D "$cache"
unzip -o -q -j "$cache/$ZIP" vesamini.drv -d "$outdir"
ls -la "$outdir/vesamini.vxd" "$outdir/vesamini.drv"
