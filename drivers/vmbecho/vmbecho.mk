# vmbecho.sys: the test driver for a VMBus pipe offered by a host program, its INF and that host
# program (vmbecho-host.ps1).

VMBECHO_OBJS := $(addprefix $(OBJ)/drivers/vmbecho/,vmbecho.o vmbecho.res)
ALL += $(OUT)/vmbecho.sys $(OUT)/vmbecho.inf $(OUT)/vmbecho-host.ps1
CHECK += check-vmbecho

$(OUT)/vmbecho.sys: $(VMBECHO_OBJS)
	$(CC) $(LDFLAGS) -Wl,--pdb=$(OUT)/vmbecho.pdb -Wl,-Map=$(OUT)/vmbecho.map \
		-o $@ $(VMBECHO_OBJS) -lntoskrnl
	$(PECHECK) --quiet --map $(OUT)/vmbecho.map --entry _DriverEntry@8 $@

$(OUT)/vmbecho.pdb: $(OUT)/vmbecho.sys

$(OUT)/vmbecho.inf: drivers/vmbecho/vmbecho.inf
	@mkdir -p $(OUT)
	$(CRLF)

$(OUT)/vmbecho-host.ps1: drivers/vmbecho/vmbecho-host.ps1
	@mkdir -p $(OUT)
	$(CRLF)

check-vmbecho: $(OUT)/vmbecho.sys
	$(PECHECK) $(PECHECK_XP) --map $(OUT)/vmbecho.map --entry _DriverEntry@8 $(OUT)/vmbecho.sys
