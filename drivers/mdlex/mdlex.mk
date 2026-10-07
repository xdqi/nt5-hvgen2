# mdlex.sys: a kernel-mode export DLL (like bootvid.dll; -shared, native subsystem, DriverEntry as entry
# point) with the routines XP lacks for the import-patched dmvsc.sys, which the kernel loads it for.
# The @36 stdcall suffix in mdlex.def is stripped by --kill-at.

MDLEX_OBJS := $(addprefix $(OBJ)/drivers/mdlex/,mdlex.o mdlex.res)
ALL += $(OUT)/mdlex.sys
CHECK += check-mdlex

$(OUT)/mdlex.sys: $(MDLEX_OBJS) drivers/mdlex/mdlex.def
	$(CC) $(LDFLAGS) -shared -Wl,--kill-at drivers/mdlex/mdlex.def \
		-Wl,--pdb=$(OUT)/mdlex.pdb -Wl,-Map=$(OUT)/mdlex.map \
		-o $@ $(MDLEX_OBJS) -lntoskrnl
	$(PECHECK) --quiet --dll --exports-def drivers/mdlex/mdlex.def \
		--map $(OUT)/mdlex.map --entry _DriverEntry@8 $@

$(OUT)/mdlex.pdb: $(OUT)/mdlex.sys

check-mdlex: $(OUT)/mdlex.sys
	$(PECHECK) $(PECHECK_XP) --dll --exports-def drivers/mdlex/mdlex.def \
		--map $(OUT)/mdlex.map --entry _DriverEntry@8 $(OUT)/mdlex.sys

# The same driver for XP Professional x64 (NT 5.2 x64), installed there as mdlex.sys as well. x64
# has no stdcall decoration, hence its own .def (and no --kill-at). No check: pecheck's XP set is x86.
MDLEX64_OBJS := $(addprefix $(K64_OBJ)/drivers/mdlex/,mdlex.o mdlex.res)
ALL += $(OUT)/mdlex64.sys

$(OUT)/mdlex64.sys: $(MDLEX64_OBJS) drivers/mdlex/mdlex64.def
	$(CC) $(K64_LDFLAGS) -shared drivers/mdlex/mdlex64.def \
		-Wl,--pdb=$(OUT)/mdlex64.pdb -Wl,-Map=$(OUT)/mdlex64.map \
		-o $@ $(MDLEX64_OBJS) -lntoskrnl
	$(PECHECK) --quiet --dll --exports-def drivers/mdlex/mdlex64.def \
		--map $(OUT)/mdlex64.map --entry DriverEntry $@

$(OUT)/mdlex64.pdb: $(OUT)/mdlex64.sys
