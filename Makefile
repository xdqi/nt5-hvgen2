# Build the Windows XP (x86) drivers with clang + lld from msys2-cross
# (https://github.com/xdqi/msys-cross; see its README for installation).
#
#   make            out/hvfb.sys, out/hvfb.pdb, out/hvfb.inf, out/bootvid.dll, out/bootvid.pdb,
#                   out/bootwait.sys, out/bootwait.pdb, out/bootwait.inf,
#                   out/vmbecho.sys, out/vmbecho.pdb, out/vmbecho.inf, out/vmbecho-host.ps1,
#                   out/vmbaud.sys, out/vmbaud.pdb, out/vmbaud.inf, out/vmbaud-host.ps1,
#                   out/testplay.exe
#   make check      PE sanity checks (subsystem, imports, relocations, checksum, entry);
#                   XPBIN=dir also checks imports and bootvid's exports against XP's binaries
#   make cdb-check  load the drivers and PDBs into the Windows cdb.exe (WSL interop)
#   make font BDF=8x13.bdf   regenerate bootvid/font.c (see tools/mkfont.py)
#   make w9x        out/w9x/gen2leg.vxd, out/w9x/vesamini.vxd, out/w9x/vesamini.drv for Windows 98
#                   (Open Watcom 2, see w9x/README.md); VMDISP9X_DEBUG=1: vesamini.vxd logs to COM1
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
	-I. \
	-std=gnu++17 -O2 -march=i686 -ffreestanding -mgeneral-regs-only \
	-mno-stack-arg-probe -fno-stack-protector -fno-omit-frame-pointer \
	-fno-asynchronous-unwind-tables -fno-unwind-tables \
	-fno-exceptions -fno-rtti -fno-threadsafe-statics -fno-use-cxa-atexit \
	-g -gcodeview \
	-Wall -Wextra -Wno-unused-parameter -Werror \
	-Wno-unknown-pragmas -Wno-pragma-pack -Wno-missing-braces

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

HEADERS := hvfb/hvfb.h bootvid/bootvid.h common/cbtable.h common/ddk_compat.h

# vmbaud.sys: PortCls WaveCyclic render miniport streaming PCM over a VMBus
# pipe (C++).  It links against an import library for XP's portcls.sys made
# from common/portcls.def.
VMBAUD_SRCS := vmbaud/adapter.cpp vmbaud/common.cpp vmbaud/helpers.cpp \
	vmbaud/minwave.cpp vmbaud/minstream.cpp vmbaud/mintopo.cpp
VMBAUD_OBJS := $(VMBAUD_SRCS:%.cpp=$(OBJ)/%.o) $(OBJ)/vmbaud/vmbaud.res
VMBAUD_LIBS := $(OBJ)/libportcls.a -lksguid -luuid -lntoskrnl -lhal

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

# mdlex.sys is a kernel-mode export DLL (like bootvid.dll) that exports the
# one routine XP lacks, MmAllocatePagesForMdlEx, for the import-patched
# dmvsc.sys.  The @36 stdcall suffix in mdlex.def is stripped by --kill-at.
MDLEX_SRCS := mdlex/mdlex.c
MDLEX_OBJS := $(MDLEX_SRCS:%.c=$(OBJ)/%.o) $(OBJ)/mdlex/mdlex.res
MDLEX_LIBS := -lntoskrnl

VMBECHO_SRCS := vmbecho/vmbecho.c
VMBECHO_OBJS := $(VMBECHO_SRCS:%.c=$(OBJ)/%.o) $(OBJ)/vmbecho/vmbecho.res
VMBECHO_LIBS := -lntoskrnl

# Optional directory with XP's own binaries (bootvid.dll, ntoskrnl.exe,
# hal.dll, videoprt.sys) for `make check`.
XPBIN ?=
PECHECK_XP = $(if $(XPBIN),--against $(XPBIN))

all: $(OUT)/hvfb.sys $(OUT)/hvfb.inf $(OUT)/bootvid.dll $(OUT)/bootwait.sys $(OUT)/bootwait.inf \
	$(OUT)/mdlex.sys $(OUT)/vmbecho.sys $(OUT)/vmbecho.inf $(OUT)/vmbecho-host.ps1 \
	$(OUT)/vmbaud.sys $(OUT)/vmbaud.inf $(OUT)/vmbaud-host.ps1 $(OUT)/testplay.exe

$(OBJ)/%.o: %.c $(HEADERS) Makefile
	@mkdir -p $(dir $@)
	$(CC) $(CFLAGS) -c $< -o $@

# The vmbaud objects share class layouts through these headers: one object
# built against an older layout allocates a smaller object than another
# fills in.
VMBAUD_HEADERS := $(wildcard vmbaud/*.h) common/ddk_compat.h

$(OBJ)/%.o: %.cpp $(HEADERS) $(VMBAUD_HEADERS) Makefile
	@mkdir -p $(dir $@)
	$(CC) $(CXXFLAGS) -c $< -o $@

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

# mdlex.sys: an export DLL image (-shared), native subsystem, DriverEntry as
# entry point.  The kernel loads it as a dependency of the import-patched
# dmvsc.sys and snaps MmAllocatePagesForMdlEx to it.
$(OUT)/mdlex.sys: $(MDLEX_OBJS) mdlex/mdlex.def
	$(CC) $(LDFLAGS) -shared -Wl,--kill-at mdlex/mdlex.def \
		-Wl,--pdb=$(OUT)/mdlex.pdb -Wl,-Map=$(OUT)/mdlex.map \
		-o $@ $(MDLEX_OBJS) $(MDLEX_LIBS)
	$(PYTHON) tools/pecheck.py --quiet --dll --exports-def mdlex/mdlex.def \
		--map $(OUT)/mdlex.map --entry _DriverEntry@8 $@

$(OUT)/mdlex.pdb: $(OUT)/mdlex.sys

$(OUT)/vmbecho.sys: $(VMBECHO_OBJS)
	$(CC) $(LDFLAGS) -Wl,--pdb=$(OUT)/vmbecho.pdb -Wl,-Map=$(OUT)/vmbecho.map \
		-o $@ $(VMBECHO_OBJS) $(VMBECHO_LIBS)
	$(PYTHON) tools/pecheck.py --quiet --map $(OUT)/vmbecho.map --entry _DriverEntry@8 $@

$(OUT)/vmbecho.pdb: $(OUT)/vmbecho.sys

$(OBJ)/libportcls.a: common/portcls.def
	@mkdir -p $(dir $@)
	$(DLLTOOL) -m i386 -k -d $< -l $@

$(OUT)/vmbaud.sys: $(VMBAUD_OBJS) $(OBJ)/libportcls.a
	$(CC) $(LDFLAGS) -Wl,--pdb=$(OUT)/vmbaud.pdb -Wl,-Map=$(OUT)/vmbaud.map \
		-o $@ $(VMBAUD_OBJS) $(VMBAUD_LIBS)
	$(PYTHON) tools/pecheck.py --quiet --map $(OUT)/vmbaud.map --entry _DriverEntry@8 $@

$(OUT)/vmbaud.pdb: $(OUT)/vmbaud.sys

# INF files are shipped with CRLF line endings.
$(OUT)/hvfb.inf: hvfb/hvfb.inf
	@mkdir -p $(OUT)
	sed 's/\r*$$/\r/' $< > $@

$(OUT)/bootwait.inf: bootwait/bootwait.inf
	@mkdir -p $(OUT)
	sed 's/\r*$$/\r/' $< > $@

$(OUT)/vmbecho.inf: vmbecho/vmbecho.inf
	@mkdir -p $(OUT)
	sed 's/\r*$$/\r/' $< > $@

$(OUT)/vmbaud.inf: vmbaud/vmbaud.inf
	@mkdir -p $(OUT)
	sed 's/\r*$$/\r/' $< > $@

$(OUT)/vmbecho-host.ps1: vmbecho/vmbecho-host.ps1
	@mkdir -p $(OUT)
	sed 's/\r*$$/\r/' $< > $@

$(OUT)/vmbaud-host.ps1: vmbaud/vmbaud-host.ps1
	@mkdir -p $(OUT)
	sed 's/\r*$$/\r/' $< > $@

# testplay.exe: a user-mode console program for XP that plays a tone through
# winmm (the vmbaud test client).
$(OUT)/testplay.exe: vmbaud/testplay.c Makefile
	@mkdir -p $(OUT)
	$(CC) --target=i686-w64-mingw32 --sysroot=$(SYSROOT) -fuse-ld=lld \
		-nostdinc -isystem $(CLANG_INC) -isystem $(SYSROOT)/include \
		-O2 -Wall -Wextra -Werror -o $@ $< -L$(SYSROOT)/lib -lwinmm \
		-Wl,--subsystem,console:5.01 -Wl,--major-os-version,5 -Wl,--minor-os-version,1

check: $(OUT)/hvfb.sys $(OUT)/bootvid.dll $(OUT)/bootwait.sys $(OUT)/mdlex.sys $(OUT)/vmbecho.sys $(OUT)/vmbaud.sys
	$(PYTHON) tools/pecheck.py $(PECHECK_XP) --map $(OUT)/hvfb.map --entry _DriverEntry@8 $(OUT)/hvfb.sys
	$(PYTHON) tools/pecheck.py $(PECHECK_XP) --dll --exports-def bootvid/bootvid.def \
		$(if $(XPBIN),--exports-like $(XPBIN)/bootvid.dll) \
		--map $(OUT)/bootvid.map --entry _DriverEntry@8 $(OUT)/bootvid.dll
	$(PYTHON) tools/pecheck.py $(PECHECK_XP) --map $(OUT)/bootwait.map --entry _DriverEntry@8 $(OUT)/bootwait.sys
	$(PYTHON) tools/pecheck.py $(PECHECK_XP) --dll --exports-def mdlex/mdlex.def \
		--map $(OUT)/mdlex.map --entry _DriverEntry@8 $(OUT)/mdlex.sys
	$(PYTHON) tools/pecheck.py $(PECHECK_XP) --map $(OUT)/vmbecho.map --entry _DriverEntry@8 $(OUT)/vmbecho.sys
	$(PYTHON) tools/pecheck.py $(PECHECK_XP) --map $(OUT)/vmbaud.map --entry _DriverEntry@8 $(OUT)/vmbaud.sys

# Windows 98: gen2leg.vxd and vmdisp9x's VESA driver, built by their scripts (Open Watcom, fixlink,
# hvkit; vmdisp9x's source and release come through gh).
W9X := $(OUT)/w9x
W9X_GEN2LEG := $(addprefix w9x/gen2leg/,build.sh gen2leg.c compat.h vmm.h vxd.h vectors.txt) \
	w9x/vmbc/vmbc.c w9x/vmbc/vmbc.h
VMDISP9X_DEBUG ?=

w9x: $(W9X)/gen2leg.vxd $(W9X)/vesamini.vxd

$(W9X)/gen2leg.vxd: $(W9X_GEN2LEG)
	w9x/gen2leg/build.sh $(abspath $@)

$(W9X)/vesamini.vxd: $(addprefix w9x/vmdisp9x/,build.sh fixes.patch build-linux.patch)
	DEBUG=$(VMDISP9X_DEBUG) w9x/vmdisp9x/build.sh $(abspath $(W9X))

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

.PHONY: all check cdb-check font clean w9x
