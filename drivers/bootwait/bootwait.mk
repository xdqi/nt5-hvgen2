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

$(OUT)/bootwait.inf: drivers/bootwait/bootwait.inf
	@mkdir -p $(OUT)
	$(CRLF)

check-bootwait: $(OUT)/bootwait.sys
	$(PECHECK) $(PECHECK_XP) --map $(OUT)/bootwait.map --entry _DriverEntry@8 $(OUT)/bootwait.sys
