#!/bin/bash
# Build vmbcprob.com, a DOS test of vmbc on SeaBIOS's VMBus connection (stub.asm switches to flat
# 32-bit protected mode and calls probe.c; output on COM1).
# Usage: build.sh [out.com]      (default: out/w9x/vmbcprob.com at the top of the repository)
# Needs clang, ld.lld, llvm-objcopy (or objcopy) and nasm.
set -euo pipefail
cd "$(dirname "$0")"
top=$(cd ../../.. && pwd)
out=${1:-$top/out/w9x/vmbcprob.com}
O=$(mktemp -d /tmp/vmbcprob.XXXX)
trap 'rm -rf "$O"' EXIT
CF="-m32 -march=i686 -ffreestanding -fno-pic -fno-pie -Os -Wall -mno-sse -mno-mmx -fno-asynchronous-unwind-tables
    -fno-stack-protector -fno-builtin-memcpy -fno-builtin-memset"
clang $CF -c probe.c -o $O/probe.o
clang $CF -c ../vmbc.c -o $O/vmbc.o
ld.lld -m elf_i386 -T probe.ld -o $O/probe.elf $O/probe.o $O/vmbc.o
llvm-objcopy -O binary $O/probe.elf $O/probe.bin 2>/dev/null || objcopy -O binary $O/probe.elf $O/probe.bin
end=$(nm $O/probe.elf | awk '$3 == "_end" { print $1 }')
[ $((0x$end)) -lt $((0xc000)) ] || { echo "image too big (end $end), stack is at fff0" >&2; exit 1; }
mkdir -p "$(dirname "$out")"
nasm -f bin -I $O/ -o "$out" stub.asm
echo "$out $(stat -c %s "$out") bytes, image end $end"
