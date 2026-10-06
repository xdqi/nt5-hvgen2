#!/usr/bin/env python3
"""Sanity checks for Windows XP x86 kernel-mode drivers.

Checks that a .sys is something the NT 5.1 loader accepts as a driver:
i386, native subsystem 5.01, relocations present, no writable+executable
sections, imports by undecorated name from kernel modules only, a correct
PE checksum and (with --map) the expected entry symbol.

  pecheck.py [--map FILE --entry SYMBOL] [--against DIR] [--fix-checksum] [--quiet]
             [--dll [--exports-def FILE.def] [--exports-like XP.dll]] image

--against DIR additionally resolves every import against the export tables
of the real modules (e.g. videoprt.sys, ntoskrnl.exe, hal.dll from XP).
--dll expects a kernel-mode DLL (IMAGE_FILE_DLL set) such as bootvid.dll.
--exports-def checks that the export table has exactly the names and
ordinals of a .def file (stdcall @N suffixes stripped); --exports-like
compares names and ordinals with another DLL, e.g. XP's own.
--fix-checksum rewrites the checksum field if the linker left it wrong.
Exit status is non-zero if any check fails.
"""
import argparse
import os
import re
import struct
import sys

KERNEL_MODULES = {"videoprt.sys", "ntoskrnl.exe", "hal.dll"}

IMAGE_FILE_MACHINE_I386 = 0x14C
IMAGE_FILE_RELOCS_STRIPPED = 0x0001
IMAGE_FILE_DLL = 0x2000
IMAGE_SUBSYSTEM_NATIVE = 1
SCN_EXECUTE = 0x20000000
SCN_WRITE = 0x80000000
SCN_DISCARDABLE = 0x02000000

DIR_NAMES = ["export", "import", "resource", "exception", "security", "basereloc",
             "debug", "architecture", "globalptr", "tls", "loadconfig", "boundimport",
             "iat", "delayimport", "clr", "reserved"]


class PE:
    def __init__(self, data):
        self.data = data
        if data[:2] != b"MZ":
            raise ValueError("no MZ header")
        self.pe_off = struct.unpack_from("<I", data, 0x3C)[0]
        if data[self.pe_off:self.pe_off + 4] != b"PE\0\0":
            raise ValueError("no PE signature")
        coff = self.pe_off + 4
        (self.machine, nsec, self.timestamp, _, _, opt_size,
         self.characteristics) = struct.unpack_from("<HHIIIHH", data, coff)
        self.opt_off = coff + 20
        magic = struct.unpack_from("<H", data, self.opt_off)[0]
        if magic != 0x10B:
            raise ValueError("not a PE32 image (magic %#x)" % magic)
        o = self.opt_off
        self.entry_rva = struct.unpack_from("<I", data, o + 16)[0]
        self.image_base = struct.unpack_from("<I", data, o + 28)[0]
        self.section_alignment, self.file_alignment = struct.unpack_from("<II", data, o + 32)
        (self.os_major, self.os_minor, _, _, self.subsys_major,
         self.subsys_minor) = struct.unpack_from("<HHHHHH", data, o + 40)
        self.size_of_image, self.size_of_headers = struct.unpack_from("<II", data, o + 56)
        self.checksum_off = o + 64
        self.checksum = struct.unpack_from("<I", data, self.checksum_off)[0]
        self.subsystem, self.dll_characteristics = struct.unpack_from("<HH", data, o + 68)
        ndirs = struct.unpack_from("<I", data, o + 92)[0]
        self.dirs = [struct.unpack_from("<II", data, o + 96 + 8 * i) for i in range(ndirs)]
        self.sections = []
        s = self.opt_off + opt_size
        for i in range(nsec):
            name = data[s:s + 8].rstrip(b"\0").decode("latin-1")
            vsize, va, rsize, rptr = struct.unpack_from("<IIII", data, s + 8)
            chars = struct.unpack_from("<I", data, s + 36)[0]
            self.sections.append((name, va, vsize, rptr, rsize, chars))
            s += 40

    def rva_to_off(self, rva):
        for name, va, vsize, rptr, rsize, chars in self.sections:
            if va <= rva < va + max(vsize, rsize):
                if rva - va >= rsize:
                    return None
                return rptr + rva - va
        if rva < self.size_of_headers:
            return rva
        return None

    def cstr(self, rva):
        off = self.rva_to_off(rva)
        end = self.data.index(b"\0", off)
        return self.data[off:end].decode("latin-1")

    def directory(self, name):
        i = DIR_NAMES.index(name)
        return self.dirs[i] if i < len(self.dirs) else (0, 0)

    def imports(self):
        rva, size = self.directory("import")
        result = []
        if not rva:
            return result
        off = self.rva_to_off(rva)
        while True:
            ilt, _, _, name_rva, iat = struct.unpack_from("<IIIII", self.data, off)
            if not (ilt or name_rva or iat):
                break
            dll = self.cstr(name_rva)
            funcs = []
            t = self.rva_to_off(ilt or iat)
            while True:
                entry = struct.unpack_from("<I", self.data, t)[0]
                if not entry:
                    break
                if entry & 0x80000000:
                    funcs.append(("#%d" % (entry & 0xFFFF), True))
                else:
                    funcs.append((self.cstr(entry + 2), False))
                t += 4
            result.append((dll, funcs))
            off += 20
        return result

    def exports(self):
        return set(self.export_ordinals())

    def export_ordinals(self):
        """Exported names mapped to their ordinals."""
        rva, size = self.directory("export")
        if not rva:
            return {}
        off = self.rva_to_off(rva)
        base, _, nnames, _, names_rva, ords_rva = struct.unpack_from("<IIIIII", self.data, off + 16)
        names_off = self.rva_to_off(names_rva)
        ords_off = self.rva_to_off(ords_rva)
        result = {}
        for i in range(nnames):
            name = self.cstr(struct.unpack_from("<I", self.data, names_off + 4 * i)[0])
            result[name] = base + struct.unpack_from("<H", self.data, ords_off + 2 * i)[0]
        return result

    def export_names_sorted(self):
        """True if the name table is sorted, as the loader's binary search requires."""
        rva, size = self.directory("export")
        if not rva:
            return True
        off = self.rva_to_off(rva)
        nnames, _, names_rva = struct.unpack_from("<III", self.data, off + 24)
        names_off = self.rva_to_off(names_rva)
        names = [self.cstr(struct.unpack_from("<I", self.data, names_off + 4 * i)[0]).encode("latin-1")
                 for i in range(nnames)]
        return names == sorted(names)

    def codeview(self):
        rva, size = self.directory("debug")
        if not rva:
            return None
        off = self.rva_to_off(rva)
        for i in range(size // 28):
            (_, _, _, _, dtype, dsize, _, dptr) = struct.unpack_from("<IIHHIIII", self.data, off + 28 * i)
            if dtype == 2 and self.data[dptr:dptr + 4] == b"RSDS":
                g = self.data[dptr + 4:dptr + 20]
                age = struct.unpack_from("<I", self.data, dptr + 20)[0]
                name = self.data[dptr + 24:dptr + dsize].split(b"\0")[0].decode("latin-1")
                d1, d2, d3 = struct.unpack_from("<IHH", g)
                guid = "%08X-%04X-%04X-%s-%s" % (d1, d2, d3, g[8:10].hex().upper(), g[10:].hex().upper())
                return guid, age, name
        return None

    def compute_checksum(self):
        data = self.data
        total = 0
        n = len(data)
        padded = data + (b"\0" if n & 1 else b"")
        for i, (w,) in enumerate(struct.iter_unpack("<H", padded)):
            if i * 2 in (self.checksum_off, self.checksum_off + 2):
                continue
            total += w
            total = (total & 0xFFFF) + (total >> 16)
        total = (total & 0xFFFF) + (total >> 16)
        return (total + n) & 0xFFFFFFFF


def def_exports(path):
    """Names (stdcall suffix stripped) and ordinals from the EXPORTS of a .def file."""
    result = {}
    in_exports = False
    with open(path, encoding="latin-1") as f:
        for line in f:
            line = line.split(";")[0].strip()
            if not line:
                continue
            if line.upper() == "EXPORTS":
                in_exports = True
                continue
            if not in_exports:
                continue
            m = re.match(r"^(\S+?)(@\d+)?\s+@(\d+)", line) or re.match(r"^(\S+?)(@\d+)?$", line)
            result[m.group(1)] = int(m.group(3)) if m.lastindex and m.lastindex >= 3 else None
    return result


def entry_from_map(path, symbol):
    """Find the RVA of `symbol` in an lld map file (Address Size Align Symbol)."""
    pat = re.compile(r"^([0-9a-fA-F]{8})\s+([0-9a-fA-F]{8})\s+\d+\s+(\S+)\s*$")
    with open(path, encoding="latin-1") as f:
        for line in f:
            m = pat.match(line)
            if m and m.group(3) == symbol:
                return int(m.group(1), 16)
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("image")
    ap.add_argument("--map")
    ap.add_argument("--entry", default="_DriverEntry@8")
    ap.add_argument("--against", help="directory with the real kernel modules")
    ap.add_argument("--dll", action="store_true", help="expect a kernel-mode DLL")
    ap.add_argument("--exports-def", help=".def file whose exports the image must match")
    ap.add_argument("--exports-like", help="DLL whose export names and ordinals the image must match")
    ap.add_argument("--fix-checksum", action="store_true")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    data = bytearray(open(args.image, "rb").read())
    pe = PE(bytes(data))
    failures = []
    lines = []

    def check(ok, what):
        lines.append(("ok   " if ok else "FAIL ") + what)
        if not ok:
            failures.append(what)

    check(pe.machine == IMAGE_FILE_MACHINE_I386, "machine i386 (%#x)" % pe.machine)
    check(pe.subsystem == IMAGE_SUBSYSTEM_NATIVE, "subsystem native (%d)" % pe.subsystem)
    check((pe.subsys_major, pe.subsys_minor) == (5, 1),
          "subsystem version %d.%02d" % (pe.subsys_major, pe.subsys_minor))
    check((pe.os_major, pe.os_minor) <= (5, 1),
          "OS version %d.%d" % (pe.os_major, pe.os_minor))
    if args.dll:
        check(not pe.characteristics & IMAGE_FILE_RELOCS_STRIPPED and pe.characteristics & IMAGE_FILE_DLL,
              "file characteristics %#06x (relocs kept, DLL)" % pe.characteristics)
    else:
        check(not pe.characteristics & IMAGE_FILE_RELOCS_STRIPPED and not pe.characteristics & IMAGE_FILE_DLL,
              "file characteristics %#06x (relocs kept, not a DLL)" % pe.characteristics)
    lines.append("     DLL characteristics %#06x, image base %#x, size %#x, timestamp %#x"
                 % (pe.dll_characteristics, pe.image_base, pe.size_of_image, pe.timestamp))

    rel_rva, rel_size = pe.directory("basereloc")
    has_reloc_sec = any(s[0] == ".reloc" for s in pe.sections)
    check(rel_rva != 0 and rel_size > 0 and has_reloc_sec,
          ".reloc present (dir rva %#x size %#x)" % (rel_rva, rel_size))
    for name, va, vsize, rptr, rsize, chars in pe.sections:
        lines.append("     section %-8s va %#07x vsize %#07x raw %#06x chars %#010x"
                     % (name, va, vsize, rsize, chars))
        check(not (chars & SCN_EXECUTE and chars & SCN_WRITE), "section %s not writable+executable" % name)
    for d in ("tls", "exception", "delayimport"):
        check(pe.directory(d)[0] == 0, "no %s directory" % d)

    if args.map:
        sym_rva = entry_from_map(args.map, args.entry)
        check(sym_rva is not None and sym_rva == pe.entry_rva,
              "entry point %#x = %s (%s)" % (pe.entry_rva, args.entry,
                                            "%#x in map" % sym_rva if sym_rva is not None else "not in map"))
    else:
        lines.append("     entry point %#x (no map given)" % pe.entry_rva)

    if args.exports_def or args.exports_like:
        have = pe.export_ordinals()
        lines.append("     exports: " + ", ".join("%s @%d" % (n, o) for n, o in
                                                   sorted(have.items(), key=lambda kv: kv[1])))
        check(pe.export_names_sorted(), "export name table sorted")
        for what, want in (("%s" % args.exports_def, def_exports(args.exports_def) if args.exports_def else None),
                           ("%s" % args.exports_like,
                            PE(open(args.exports_like, "rb").read()).export_ordinals() if args.exports_like else None)):
            if want is None:
                continue
            same = set(have) == set(want) and all(o is None or have[n] == o for n, o in want.items())
            check(same, "exports (names and ordinals) match %s" % what)
            for n in sorted(set(want) - set(have)):
                lines.append("     missing export %s" % n)
            for n in sorted(set(have) - set(want)):
                lines.append("     extra export %s" % n)

    exports = {}
    if args.against:
        for fn in os.listdir(args.against):
            if fn.lower() in KERNEL_MODULES:
                exports[fn.lower()] = PE(open(os.path.join(args.against, fn), "rb").read()).exports()
    for dll, funcs in pe.imports():
        check(dll.lower() in KERNEL_MODULES, "import module %s" % dll)
        names = []
        for fname, by_ord in funcs:
            undecorated = not by_ord and "@" not in fname and not fname.startswith(("_", "?")) \
                or fname in ("_except_handler3", "_alldiv", "_allmul", "_aulldiv", "_allshl", "_aullshr")
            check(undecorated, "  %s: imported by undecorated name" % fname)
            if args.against and dll.lower() in exports:
                check(fname in exports[dll.lower()], "  %s exported by XP %s" % (fname, dll))
            names.append(fname)
        lines.append("     %s: %s" % (dll, ", ".join(names)))

    computed = pe.compute_checksum()
    if pe.checksum != computed and args.fix_checksum:
        struct.pack_into("<I", data, pe.checksum_off, computed)
        open(args.image, "wb").write(data)
        lines.append("     checksum rewritten: %#x -> %#x" % (pe.checksum, computed))
        pe.checksum = computed
    check(pe.checksum == computed and computed != 0,
          "PE checksum %#x (computed %#x)" % (pe.checksum, computed))

    cv = pe.codeview()
    check(cv is not None, "CodeView RSDS record" + (": %s age %d -> %s" % cv if cv else ""))

    if failures or not args.quiet:
        print("%s:" % args.image)
        for l in lines:
            if not args.quiet or l.startswith("FAIL"):
                print("  " + l)
    if failures:
        print("%d check(s) failed" % len(failures), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
