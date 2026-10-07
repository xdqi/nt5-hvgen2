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
