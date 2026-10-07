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

# The same miniport for NT 5.2 x64 (XP Professional x64 / Server 2003 x64), whose text-mode
# setup otherwise sits in vga.sys waiting for VGA hardware a Generation 2 VM does not have.
HVFB64_OBJS := $(addprefix $(K64_OBJ)/drivers/,hvfb/hvfb.o hvfb/modes.o common/cbtable.o hvfb/hvfb.res)
ALL += $(OUT)/hvfb64.sys

$(K64_OBJ)/libvideoprt.a: drivers/common/videoprt64.def
	@mkdir -p $(dir $@)
	$(DLLTOOL) -m i386:x86-64 -d $< -l $@

$(OUT)/hvfb64.sys: $(HVFB64_OBJS) $(K64_OBJ)/libvideoprt.a
	$(CC) $(K64_LDFLAGS) -Wl,--pdb=$(OUT)/hvfb64.pdb -Wl,-Map=$(OUT)/hvfb64.map \
		-o $@ $(HVFB64_OBJS) $(K64_OBJ)/libvideoprt.a -lntoskrnl
	$(PECHECK) --quiet --map $(OUT)/hvfb64.map --entry DriverEntry $@

$(OUT)/hvfb64.pdb: $(OUT)/hvfb64.sys

$(OUT)/hvfb.inf: drivers/hvfb/hvfb.inf
	@mkdir -p $(OUT)
	$(CRLF)

check-hvfb: $(OUT)/hvfb.sys
	$(PECHECK) $(PECHECK_XP) --map $(OUT)/hvfb.map --entry _DriverEntry@8 $(OUT)/hvfb.sys
