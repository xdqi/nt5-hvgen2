# Windows 98 (make w9x): gen2leg.vxd and vmdisp9x's VESA driver, built by their scripts (Open Watcom,
# fixlink, hvkit; vmdisp9x's source and release come through gh). Not part of `make`.

W9X := $(OUT)/w9x
W9X_GEN2LEG := $(addprefix w9x/gen2leg/,build.sh gen2leg.c compat.h vmm.h vxd.h vectors.txt) \
	w9x/vmbc/vmbc.c w9x/vmbc/vmbc.h
VMDISP9X_DEBUG ?=

w9x: $(W9X)/gen2leg.vxd $(W9X)/vesamini.vxd

$(W9X)/gen2leg.vxd: $(W9X_GEN2LEG)
	w9x/gen2leg/build.sh $(abspath $@)

$(W9X)/vesamini.vxd: $(addprefix w9x/vmdisp9x/,build.sh fixes.patch build-linux.patch)
	DEBUG=$(VMDISP9X_DEBUG) w9x/vmdisp9x/build.sh $(abspath $(W9X))

.PHONY: w9x
