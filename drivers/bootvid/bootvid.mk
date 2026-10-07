# bootvid.dll: the boot video DLL for a frame buffer, a kernel-mode DLL imported by ntoskrnl that exports
# the undecorated names listed in bootvid.def.

HEADERS += drivers/bootvid/bootvid.h
BOOTVID_OBJS := $(addprefix $(OBJ)/drivers/,bootvid/bootvid.o bootvid/font.o common/cbtable.o \
	bootvid/bootvid.res)
ALL += $(OUT)/bootvid.dll
CHECK += check-bootvid

$(OUT)/bootvid.dll: $(BOOTVID_OBJS) drivers/bootvid/bootvid.def
	$(CC) $(LDFLAGS) -shared -Wl,--kill-at drivers/bootvid/bootvid.def \
		-Wl,--pdb=$(OUT)/bootvid.pdb -Wl,-Map=$(OUT)/bootvid.map \
		-o $@ $(BOOTVID_OBJS) -lntoskrnl
	$(PECHECK) --quiet --dll --exports-def drivers/bootvid/bootvid.def \
		--map $(OUT)/bootvid.map --entry _DriverEntry@8 $@

$(OUT)/bootvid.pdb: $(OUT)/bootvid.dll

check-bootvid: $(OUT)/bootvid.dll
	$(PECHECK) $(PECHECK_XP) --dll --exports-def drivers/bootvid/bootvid.def \
		$(if $(XPBIN),--exports-like $(XPBIN)/bootvid.dll) \
		--map $(OUT)/bootvid.map --entry _DriverEntry@8 $(OUT)/bootvid.dll

# The same DLL for NT 5.2 x64 (XP Professional x64 / Server 2003 x64), whose own bootvid.dll draws
# on VGA hardware a Generation 2 VM does not have: no boot screen, and bug checks are not seen.
BOOTVID64_OBJS := $(addprefix $(K64_OBJ)/drivers/,bootvid/bootvid.o bootvid/font.o common/cbtable.o \
	bootvid/bootvid.res)
ALL += $(OUT)/bootvid64.dll

$(OUT)/bootvid64.dll: $(BOOTVID64_OBJS) drivers/bootvid/bootvid64.def
	$(CC) $(K64_LDFLAGS) -shared drivers/bootvid/bootvid64.def \
		-Wl,--pdb=$(OUT)/bootvid64.pdb -Wl,-Map=$(OUT)/bootvid64.map \
		-o $@ $(BOOTVID64_OBJS) -lntoskrnl
	$(PECHECK) --quiet --dll --exports-def drivers/bootvid/bootvid64.def \
		--map $(OUT)/bootvid64.map --entry DriverEntry $@

$(OUT)/bootvid64.pdb: $(OUT)/bootvid64.dll

# make font BDF=8x13.bdf: regenerate the font from a BDF file.
font:
	@test -n "$(BDF)" || { echo "usage: make font BDF=path/to/8x13.bdf" >&2; exit 1; }
	$(PYTHON) tools/mkfont.py $(BDF) > drivers/bootvid/font.c

.PHONY: font
