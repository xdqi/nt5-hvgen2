# bootwait.sys: the boot driver that waits for the VMBus boot disk (and keeps a few registry values and
# device bindings), and bootwait.inf, which installs it on an installed XP.

BOOTWAIT_OBJS := $(addprefix $(OBJ)/drivers/bootwait/,bootwait.o bootwait.res)
ALL += $(OUT)/bootwait.sys $(OUT)/bootwait.inf
CHECK += check-bootwait

$(OUT)/bootwait.sys: $(BOOTWAIT_OBJS)
	$(CC) $(LDFLAGS) -Wl,--pdb=$(OUT)/bootwait.pdb -Wl,-Map=$(OUT)/bootwait.map \
		-o $@ $(BOOTWAIT_OBJS) -lntoskrnl
	$(PECHECK) --quiet --map $(OUT)/bootwait.map --entry _DriverEntry@8 $@

$(OUT)/bootwait.pdb: $(OUT)/bootwait.sys

# The same driver for NT 5.2 x64 (XP Professional x64 / Server 2003 x64): its text-mode
# setup needs it too, for the VMBus boot disk to be there when the boot partition is
# marked. No check: pecheck's XP set is x86.
BOOTWAIT64_OBJS := $(addprefix $(K64_OBJ)/drivers/bootwait/,bootwait.o bootwait.res)
ALL += $(OUT)/bootwait64.sys

$(OUT)/bootwait64.sys: $(BOOTWAIT64_OBJS)
	$(CC) $(K64_LDFLAGS) -Wl,--pdb=$(OUT)/bootwait64.pdb -Wl,-Map=$(OUT)/bootwait64.map \
		-o $@ $(BOOTWAIT64_OBJS) -lntoskrnl
	$(PECHECK) --quiet --map $(OUT)/bootwait64.map --entry DriverEntry $@

$(OUT)/bootwait64.pdb: $(OUT)/bootwait64.sys

$(OUT)/bootwait.inf: drivers/bootwait/bootwait.inf
	@mkdir -p $(OUT)
	$(CRLF)

check-bootwait: $(OUT)/bootwait.sys
	$(PECHECK) $(PECHECK_XP) --map $(OUT)/bootwait.map --entry _DriverEntry@8 $(OUT)/bootwait.sys
