# Build the Windows XP (x86) drivers with clang + lld from msys2-cross.
#
#   make            out/hvfb.sys, out/hvfb.pdb, out/hvfb.inf, out/bootvid.dll, out/bootvid.pdb,
#                   out/bootwait.sys, out/bootwait.pdb, out/bootwait.inf
#   make check      PE sanity checks (subsystem, imports, relocations, checksum, entry);
#                   XPBIN=dir also checks imports and bootvid's exports against XP's binaries
#   make cdb-check  load the drivers and PDBs into the Windows cdb.exe (WSL interop)
#   make font BDF=8x13.bdf   regenerate bootvid/font.c (see tools/mkfont.py)
#
# CodeView debug info (-gcodeview) goes into PDBs written by lld, so WinDbg/KD
# can resolve hvfb!*, bootvid!* and bootwait!* symbols; the images themselves are stripped.

MSYS2_CROSS ?= /opt/msys2-cross
LLVM_DIR    ?= $(MSYS2_CROSS)/libexec/msys-cross-clang
SYSROOT     ?= $(MSYS2_CROSS)/mingw32
CC          := $(LLVM_DIR)/clang
RC          := $(LLVM_DIR)/llvm-rc
CLANG_INC   := $(firstword $(wildcard $(LLVM_DIR)/lib/clang/*/include))
PYTHON      ?= python3

OUT := out
OBJ := $(OUT)/obj

CFLAGS := --target=i686-w64-mingw32 --sysroot=$(SYSROOT) \
	-nostdinc -isystem $(CLANG_INC) -isystem $(SYSROOT)/include -isystem $(SYSROOT)/include/ddk \
	-std=gnu11 -O2 -march=i686 -ffreestanding -mgeneral-regs-only \
	-mno-stack-arg-probe -fno-stack-protector -fno-omit-frame-pointer \
	-fno-asynchronous-unwind-tables -fno-unwind-tables \
	-g -gcodeview \
	-Wall -Wextra -Wno-unused-parameter -Werror

# When the tree is on a Windows drive, record source paths as Windows paths
# so that WinDbg opens the sources by itself.  clang records $PWD, which may
# be a symlinked path, while make's CURDIR is the physical one: map both.
SRCDIR_WIN := $(shell wslpath -w "$(CURDIR)" 2>/dev/null)
ifneq ($(SRCDIR_WIN),)
CFLAGS += '-fdebug-prefix-map=$(CURDIR)=$(SRCDIR_WIN)'
ifneq ($(PWD),$(CURDIR))
CFLAGS += '-fdebug-prefix-map=$(PWD)=$(SRCDIR_WIN)'
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

HEADERS := hvfb/hvfb.h bootvid/bootvid.h common/cbtable.h

HVFB_SRCS := hvfb/hvfb.c hvfb/modes.c common/cbtable.c
HVFB_OBJS := $(HVFB_SRCS:%.c=$(OBJ)/%.o) $(OBJ)/hvfb/hvfb.res
HVFB_LIBS := -lvideoprt -lntoskrnl

# bootvid.dll is a kernel-mode DLL imported by ntoskrnl; it exports the
# undecorated names listed in bootvid.def.
BOOTVID_SRCS := bootvid/bootvid.c bootvid/font.c common/cbtable.c
BOOTVID_OBJS := $(BOOTVID_SRCS:%.c=$(OBJ)/%.o) $(OBJ)/bootvid/bootvid.res
BOOTVID_LIBS := -lntoskrnl

BOOTWAIT_SRCS := bootwait/bootwait.c
BOOTWAIT_OBJS := $(BOOTWAIT_SRCS:%.c=$(OBJ)/%.o) $(OBJ)/bootwait/bootwait.res
BOOTWAIT_LIBS := -lntoskrnl

# Optional directory with XP's own binaries (bootvid.dll, ntoskrnl.exe,
# hal.dll, videoprt.sys) for `make check`.
XPBIN ?=
PECHECK_XP = $(if $(XPBIN),--against $(XPBIN))

all: $(OUT)/hvfb.sys $(OUT)/hvfb.inf $(OUT)/bootvid.dll $(OUT)/bootwait.sys $(OUT)/bootwait.inf

$(OBJ)/%.o: %.c $(HEADERS) Makefile
	@mkdir -p $(dir $@)
	$(CC) $(CFLAGS) -c $< -o $@

$(OBJ)/%.res: %.rc
	@mkdir -p $(dir $@)
	$(RC) -no-preprocess /fo $@ $<

# The PDB and the map are by-products of the link.
$(OUT)/hvfb.sys: $(HVFB_OBJS)
	$(CC) $(LDFLAGS) -Wl,--pdb=$(OUT)/hvfb.pdb -Wl,-Map=$(OUT)/hvfb.map \
		-o $@ $(HVFB_OBJS) $(HVFB_LIBS)
	$(PYTHON) tools/pecheck.py --quiet --map $(OUT)/hvfb.map --entry _DriverEntry@8 $@

$(OUT)/hvfb.pdb: $(OUT)/hvfb.sys

$(OUT)/bootvid.dll: $(BOOTVID_OBJS) bootvid/bootvid.def
	$(CC) $(LDFLAGS) -shared -Wl,--kill-at bootvid/bootvid.def \
		-Wl,--pdb=$(OUT)/bootvid.pdb -Wl,-Map=$(OUT)/bootvid.map \
		-o $@ $(BOOTVID_OBJS) $(BOOTVID_LIBS)
	$(PYTHON) tools/pecheck.py --quiet --dll --exports-def bootvid/bootvid.def \
		--map $(OUT)/bootvid.map --entry _DriverEntry@8 $@

$(OUT)/bootvid.pdb: $(OUT)/bootvid.dll

$(OUT)/bootwait.sys: $(BOOTWAIT_OBJS)
	$(CC) $(LDFLAGS) -Wl,--pdb=$(OUT)/bootwait.pdb -Wl,-Map=$(OUT)/bootwait.map \
		-o $@ $(BOOTWAIT_OBJS) $(BOOTWAIT_LIBS)
	$(PYTHON) tools/pecheck.py --quiet --map $(OUT)/bootwait.map --entry _DriverEntry@8 $@

$(OUT)/bootwait.pdb: $(OUT)/bootwait.sys

# INF files are shipped with CRLF line endings.
$(OUT)/hvfb.inf: hvfb/hvfb.inf
	@mkdir -p $(OUT)
	sed 's/\r*$$/\r/' $< > $@

$(OUT)/bootwait.inf: bootwait/bootwait.inf
	@mkdir -p $(OUT)
	sed 's/\r*$$/\r/' $< > $@

check: $(OUT)/hvfb.sys $(OUT)/bootvid.dll $(OUT)/bootwait.sys
	$(PYTHON) tools/pecheck.py $(PECHECK_XP) --map $(OUT)/hvfb.map --entry _DriverEntry@8 $(OUT)/hvfb.sys
	$(PYTHON) tools/pecheck.py $(PECHECK_XP) --dll --exports-def bootvid/bootvid.def \
		$(if $(XPBIN),--exports-like $(XPBIN)/bootvid.dll) \
		--map $(OUT)/bootvid.map --entry _DriverEntry@8 $(OUT)/bootvid.dll
	$(PYTHON) tools/pecheck.py $(PECHECK_XP) --map $(OUT)/bootwait.map --entry _DriverEntry@8 $(OUT)/bootwait.sys

cdb-check: $(OUT)/hvfb.sys $(OUT)/bootvid.dll $(OUT)/bootwait.sys
	tools/cdb-check.sh $(OUT)/hvfb.sys hvfb
	tools/cdb-check.sh $(OUT)/bootvid.dll bootvid

font:
	@test -n "$(BDF)" || { echo "usage: make font BDF=path/to/8x13.bdf" >&2; exit 1; }
	$(PYTHON) tools/mkfont.py $(BDF) > bootvid/font.c

# Include flags for clangd and other editor tooling (not tracked).
compile_flags.txt: Makefile
	printf '%s\n' $(filter-out -Werror,$(CFLAGS)) > $@

clean:
	rm -rf $(OUT)

.PHONY: all check cdb-check font clean
