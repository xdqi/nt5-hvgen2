#!/bin/bash
# Assembles the package of the XP Generation 1 -> 2 converter (migrate/Convert-XPToGen2.*).
#
#   tools/mkdist.sh CSMWRAP_EFI=<csmwrap.efi> [MS_DIR=<dir>] [OUT=out/dist]
#
#   CSMWRAP_EFI  a release build of CSMWrap with Hyper-V Generation 2 support
#   MS_DIR       a directory with storvsc.sys, storport.sys and diskdump.sys (see migrate/README.txt).
#                The package then needs nothing else, and it is named ...-private: it contains
#                Microsoft files, so it must not be published.
#   OUT          where the package goes (default out/dist)
#
# Builds the drivers (make) and the DSDT (acpi/build.sh) first. Result: $OUT/<name>/ and
# $OUT/<name>.zip, <name> = nt5-hvgen2-migrate or nt5-hvgen2-migrate-private.
set -euo pipefail
cd "$(dirname "$0")/.."
for a in "$@"; do
    case $a in
        CSMWRAP_EFI=*|MS_DIR=*|OUT=*) export "${a?}" ;;
        *) echo "unknown argument: $a" >&2; exit 1 ;;
    esac
done
: "${CSMWRAP_EFI:?CSMWRAP_EFI=<csmwrap.efi> is required}"
MS_DIR=${MS_DIR:-}
OUT=${OUT:-out/dist}
NAME=nt5-hvgen2-migrate${MS_DIR:+-private}
D=$OUT/$NAME

file -b "$CSMWRAP_EFI" | grep -q 'EFI (application), x86-64' || { echo "$CSMWRAP_EFI is not an x64 EFI application" >&2; exit 1; }
if [ -n "$MS_DIR" ]; then
    for f in storvsc.sys storport.sys diskdump.sys; do
        [ -f "$MS_DIR/$f" ] || { echo "$MS_DIR/$f is missing" >&2; exit 1; }
    done
fi

make
acpi/build.sh

rm -rf "$D" "$D.zip"
mkdir -p "$D/resources"
crlf() { sed 's/\r*$/\r/' "$1" > "$2"; }
for f in Convert-XPToGen2.cmd Convert-XPToGen2.ps1 inject.ps1; do
    crlf "migrate/$f" "$D/$f"
done
{
    if [ -n "$MS_DIR" ]; then
        echo 'PRIVATE PACKAGE: resources\ contains Microsoft files (storvsc.sys,'
        echo 'storport.sys, diskdump.sys). Do not publish or redistribute it.'
        echo
    fi
    cat migrate/README.txt
} > "$D/README.tmp"
crlf "$D/README.tmp" "$D/README.txt"
rm "$D/README.tmp"
cp "$CSMWRAP_EFI" "$D/resources/csmwrap.efi"
cp acpi/out/dsdt.aml out/hvfb.sys out/bootwait.sys out/bootvid.dll "$D/resources/"
if [ -n "$MS_DIR" ]; then
    cp "$MS_DIR/storvsc.sys" "$MS_DIR/storport.sys" "$MS_DIR/diskdump.sys" "$D/resources/"
fi
ver=$(strings -n 8 "$CSMWRAP_EFI" | grep -o 'CSMWrap-[^ ]*' | sort -u | tr '\n' ' ')
{
    echo "nt5-hvgen2 $(git describe --always --dirty)"
    echo "csmwrap.efi ${ver:-(no version string)}"
} | sed 's/$/\r/' > "$D/BUILD.txt"
(cd "$D" && find . -type f ! -name SHA256SUMS -printf '%P\n' | sort | xargs sha256sum > SHA256SUMS)
(cd "$OUT" && zip -qr "$NAME.zip" "$NAME")
echo "$D/"
(cd "$D" && find . -type f -printf '  %P (%s bytes)\n' | sort)
echo "$OUT/$NAME.zip"
