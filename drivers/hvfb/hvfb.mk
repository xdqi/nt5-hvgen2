# hvfb.sys: the VideoPort miniport for a linear frame buffer (VBE, or the coreboot table CSMWrap leaves),
# and hvfb.inf, which installs it on an installed XP.

HEADERS += drivers/hvfb/hvfb.h
HVFB_OBJS := $(addprefix $(OBJ)/drivers/,hvfb/hvfb.o hvfb/modes.o common/cbtable.o hvfb/hvfb.res)
ALL += $(OUT)/hvfb.sys $(OUT)/hvfb.inf
CHECK += check-hvfb

# The PDB and the map are by-products of the link.
$(OUT)/hvfb.sys: $(HVFB_OBJS)
	$(CC) $(LDFLAGS) -Wl,--pdb=$(OUT)/hvfb.pdb -Wl,-Map=$(OUT)/hvfb.map \
		-o $@ $(HVFB_OBJS) -lvideoprt -lntoskrnl
	$(PECHECK) --quiet --map $(OUT)/hvfb.map --entry _DriverEntry@8 $@

$(OUT)/hvfb.pdb: $(OUT)/hvfb.sys

$(OUT)/hvfb.inf: drivers/hvfb/hvfb.inf
	@mkdir -p $(OUT)
	$(CRLF)

check-hvfb: $(OUT)/hvfb.sys
	$(PECHECK) $(PECHECK_XP) --map $(OUT)/hvfb.map --entry _DriverEntry@8 $(OUT)/hvfb.sys
