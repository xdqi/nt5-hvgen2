#!/bin/bash
# Build the Windows XP compatible Hyper-V Gen2 DSDT: out/dsdt.aml (+ out/dsdt.lst listing).
# Env: NCPU=128 number of Processor() objects, ACPI processor IDs 1..NCPU (Hyper-V's MADT numbers
#               its processors from 1; Gen1's DSDT declares 128). Declaring more than the VM has is
#               harmless: Windows only starts the processors in the MADT.
#      IASL=iasl
#
# XP compatibility comes from the source rather than from iasl options: DefinitionBlock compliance
# revision 1 makes the table use 32-bit integers (iasl then rejects constants above 0xFFFFFFFF), and
# dsdt.asl only uses ACPI 1.0b operators (iasl emits ASL+ "=", "==" as Store/LEqual).
# iasl flags: -we warnings are errors, except
#   -vw 3168  "Legacy Processor() keyword": Processor() is exactly what ACPI 1.0 OSes need
#   -vw 6033  "_HID string must be exactly 7 or 8 characters": _HID "VMBus" is what XP's vmbus.inf wants
#   -vw 3141  "Device has a _DIS, missing a _SRS": VMBS keeps Gen1's _DIS/_PS0 bookkeeping
# -l writes the listing out/dsdt.lst, -p the output prefix.
set -euo pipefail
cd "$(dirname "$0")"
NCPU=${NCPU:-128}
IASL=${IASL:-iasl}
[ "$NCPU" -ge 1 ] && [ "$NCPU" -le 255 ] || { echo "NCPU must be 1..255" >&2; exit 1; }
mkdir -p out
for i in $(seq 1 "$NCPU"); do
    printf 'Processor (P%03d, 0x%02X, 0x00000000, 0x00) {}\n' "$i" "$i"
done > out/cpus.asl
"$IASL" -we -vw 3168 -vw 6033 -vw 3141 -l -I out -p out/dsdt dsdt.asl
# Ensure the result is what XP expects.
python3 - out/dsdt.aml <<'PY'
import struct, sys
d = open(sys.argv[1], 'rb').read()
sig, length, rev = d[:4], struct.unpack_from('<I', d, 4)[0], d[8]
assert sig == b'DSDT' and length == len(d) and rev == 1, (sig, length, rev)
assert sum(d) & 0xff == 0, 'bad checksum'
print(f'out/dsdt.aml: DSDT revision {rev}, {length} bytes, checksum ok')
PY
