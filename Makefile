# Build the Windows XP (x86) drivers, their host and test programs with clang + lld from msys2-cross
# (https://github.com/xdqi/msys-cross; see its README for installation).
#
#   make            out/hvfb.sys, out/hvfb.pdb, out/hvfb.inf, out/bootvid.dll, out/bootvid.pdb,
#                   out/bootwait.sys, out/bootwait.pdb, out/bootwait.inf, out/mdlex.sys,
#                   out/vmbecho.sys, out/vmbecho.pdb, out/vmbecho.inf, out/vmbecho-host.ps1,
#                   out/vmbaud.sys, out/vmbaud.pdb, out/vmbaud.inf, out/vmbaud-host.ps1,
#                   out/testplay.exe, out/vmbaudtray.exe, out/vmbaudcli.exe (x64, host side),
#                   out/predev.exe
#   make check      PE sanity checks (subsystem, imports, relocations, checksum, entry);
#                   XPBIN=dir also checks imports and bootvid's exports against XP's binaries
#   make cdb-check  load the drivers and PDBs into the Windows cdb.exe (WSL interop)
#   make font BDF=8x13.bdf   regenerate drivers/bootvid/font.c (see tools/mkfont.py)
#   make w9x        out/w9x/gen2leg.vxd, out/w9x/vesamini.vxd, out/w9x/vesamini.drv for Windows 98
#                   (Open Watcom 2, see w9x/README.md); VMDISP9X_DEBUG=1: vesamini.vxd logs to COM1
#
# This file has the toolchain, the shared pattern rules and the entry points. Each component's rules
# are next to its sources (drivers/*/*.mk, w9x/w9x.mk); a component adds its outputs to ALL, its
# `make check` targets to CHECK and its headers to HEADERS (C), CXX_HEADERS or HOST64_HEADERS.
#
# CodeView debug info (-gcodeview) goes into PDBs written by lld, so WinDbg/KD
# can resolve hvfb!*, bootvid!* and bootwait!* symbols; the images themselves are stripped.

MSYS2_CROSS ?= /opt/msys2-cross
LLVM_DIR    ?= $(MSYS2_CROSS)/libexec/msys-cross-clang
SYSROOT     ?= $(MSYS2_CROSS)/mingw32
CC          := $(LLVM_DIR)/clang
RC          := $(LLVM_DIR)/llvm-rc
DLLTOOL     := $(LLVM_DIR)/llvm-dlltool
CLANG_INC   := $(firstword $(wildcard $(LLVM_DIR)/lib/clang/*/include))
PYTHON      ?= python3
PECHECK     := $(PYTHON) tools/pecheck.py

OUT := out
OBJ := $(OUT)/obj

CFLAGS := --target=i686-w64-mingw32 --sysroot=$(SYSROOT) \
	-nostdinc -isystem $(CLANG_INC) -isystem $(SYSROOT)/include -isystem $(SYSROOT)/include/ddk \
	-std=gnu11 -O2 -march=i686 -ffreestanding -mgeneral-regs-only \
	-mno-stack-arg-probe -fno-stack-protector -fno-omit-frame-pointer \
	-fno-asynchronous-unwind-tables -fno-unwind-tables \
	-g -gcodeview \
	-Wall -Wextra -Wno-unused-parameter -Werror

# C++ for the PortCls COM miniports.  -fno-exceptions is required: with
# exceptions on, STDMETHOD is noexcept and STDMETHODIMP_ is not, which clang
# rejects under -Werror.  -fno-rtti/-fno-threadsafe-statics/-fno-use-cxa-atexit
# keep __cxa_* / typeinfo / atexit out of the image.
CXXFLAGS := --target=i686-w64-mingw32 --sysroot=$(SYSROOT) \
	-nostdinc -isystem $(CLANG_INC) -isystem $(SYSROOT)/include -isystem $(SYSROOT)/include/ddk \
	-std=gnu++17 -O2 -march=i686 -ffreestanding -mgeneral-regs-only \
	-mno-stack-arg-probe -fno-stack-protector -fno-omit-frame-pointer \
	-fno-asynchronous-unwind-tables -fno-unwind-tables \
	-fno-exceptions -fno-rtti -fno-threadsafe-statics -fno-use-cxa-atexit \
	-g -gcodeview \
	-Wall -Wextra -Wno-unused-parameter -Werror \
	-Wno-unknown-pragmas -Wno-pragma-pack -Wno-missing-braces

# User-mode console programs for XP (test clients and the like): no DDK, NT 5.1 subsystem version.
UM_CFLAGS := --target=i686-w64-mingw32 --sysroot=$(SYSROOT) -fuse-ld=lld \
	-nostdinc -isystem $(CLANG_INC) -isystem $(SYSROOT)/include \
	-O2 -Wall -Wextra -Werror
UM_LDFLAGS := -L$(SYSROOT)/lib \
	-Wl,--subsystem,console:5.01 -Wl,--major-os-version,5 -Wl,--minor-os-version,1

# x64 user-mode programs that run on the Hyper-V host (vmbuspiper.dll lives only in System32, so they
# are 64-bit and use the mingw64 sysroot). C++ without the standard library, Win32 + COM only.
HOST64_SYSROOT ?= $(MSYS2_CROSS)/mingw64
HOST64_OBJ     := $(OUT)/obj64
HOST64_CXXFLAGS := --target=x86_64-w64-mingw32 --sysroot=$(HOST64_SYSROOT) \
	-nostdinc -isystem $(CLANG_INC) -isystem $(HOST64_SYSROOT)/include \
	-DUNICODE -D_UNICODE \
	-std=gnu++17 -O2 -fno-exceptions -fno-rtti \
	-Wall -Wextra -Wno-unused-parameter -Wno-pragma-pack -Werror
HOST64_LDFLAGS := --target=x86_64-w64-mingw32 --sysroot=$(HOST64_SYSROOT) \
	-fuse-ld=lld -municode -static-libgcc -L$(HOST64_SYSROOT)/lib

# When the tree is on a Windows drive, record source paths as Windows paths
# so that WinDbg opens the sources by itself.  clang records $PWD, which may
# be a symlinked path, while make's CURDIR is the physical one: map both.
SRCDIR_WIN := $(shell wslpath -w "$(CURDIR)" 2>/dev/null)
ifneq ($(SRCDIR_WIN),)
CFLAGS += '-fdebug-prefix-map=$(CURDIR)=$(SRCDIR_WIN)'
CXXFLAGS += '-fdebug-prefix-map=$(CURDIR)=$(SRCDIR_WIN)'
ifneq ($(PWD),$(CURDIR))
CFLAGS += '-fdebug-prefix-map=$(PWD)=$(SRCDIR_WIN)'
CXXFLAGS += '-fdebug-prefix-map=$(PWD)=$(SRCDIR_WIN)'
endif
endif

# Kernel-mode image for NT 5.1: native subsystem 5.01, relocatable, checksum
# filled (/release), PDB referenced by file name only (/pdbaltpath).
LDFLAGS := --target=i686-w64-mingw32 --sysroot=$(SYSROOT) -fuse-ld=lld -nostdlib \
	-L$(SYSROOT)/lib \
	-Wl,--subsystem,native:5.01 -Wl,--major-os-version,5 -Wl,--minor-os-version,1 \
	-Wl,--entry,_DriverEntry@8 -Wl,--image-base,0x10000 \
	-Wl,--file-alignment,0x200 -Wl,--section-alignment,0x1000 \
	-Wl,--dynamicbase -Wl,--disable-nxcompat -Wl,--disable-tsaware \
	-Wl,--strip-all \
	-Wl,--Xlink=-driver -Wl,--Xlink=-release -Wl,--Xlink=-pdbaltpath:%_PDB%

# Optional directory with XP's own binaries (bootvid.dll, ntoskrnl.exe,
# hal.dll, videoprt.sys) for `make check`.
XPBIN ?=
PECHECK_XP = $(if $(XPBIN),--against $(XPBIN))

# INF files and host scripts are shipped with CRLF line endings.
CRLF = sed 's/\r*$$/\r/' $< > $@

.DEFAULT_GOAL := all
ALL :=
CHECK :=
HEADERS := drivers/common/cbtable.h drivers/common/ddk_compat.h
CXX_HEADERS :=
HOST64_HEADERS :=

include drivers/hvfb/hvfb.mk
include drivers/bootvid/bootvid.mk
include drivers/bootwait/bootwait.mk
include drivers/mdlex/mdlex.mk
include drivers/vmbecho/vmbecho.mk
include drivers/vmbaud/vmbaud.mk
include guest/predev/predev.mk
include w9x/w9x.mk

# The pattern rules come after the components, whose headers they depend on.
$(OBJ)/%.o: %.c $(HEADERS) Makefile
	@mkdir -p $(dir $@)
	$(CC) $(CFLAGS) -c $< -o $@

$(OBJ)/%.o: %.cpp $(HEADERS) $(CXX_HEADERS) Makefile
	@mkdir -p $(dir $@)
	$(CC) $(CXXFLAGS) -c $< -o $@

$(OBJ)/%.res: %.rc
	@mkdir -p $(dir $@)
	$(RC) -no-preprocess /fo $@ $<

# x64 objects live under $(HOST64_OBJ) so the i686 rules cannot pick them up
# (make would otherwise match out/obj/%.o with a bogus stem).
$(HOST64_OBJ)/%.o: %.cpp $(HOST64_HEADERS) Makefile
	@mkdir -p $(dir $@)
	$(CC) $(HOST64_CXXFLAGS) -c $< -o $@

all: $(ALL)

check: $(CHECK)

cdb-check: $(OUT)/hvfb.sys $(OUT)/bootvid.dll $(OUT)/bootwait.sys
	tools/cdb-check.sh $(OUT)/hvfb.sys hvfb
	tools/cdb-check.sh $(OUT)/bootvid.dll bootvid

# Include flags for clangd and other editor tooling (not tracked).
compile_flags.txt: Makefile
	printf '%s\n' $(filter-out -Werror,$(CFLAGS)) > $@

clean:
	rm -rf $(OUT)

.PHONY: all check cdb-check clean $(CHECK)
