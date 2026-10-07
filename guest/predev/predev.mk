# predev.exe: pre-installs VMBus devices that turn up only after setup (run from $OEM$\cmdlines.txt of
# a setup CD made by hvkit setup-cd; see predev.c).

ALL += $(OUT)/predev.exe

$(OUT)/predev.exe: guest/predev/predev.c Makefile
	@mkdir -p $(OUT)
	$(CC) $(UM_CFLAGS) -o $@ $< $(UM_LDFLAGS) -lsetupapi

# The same for XP Professional x64, given to setup-cd as predev.exe in the x64 --files: SetupAPI
# does not let a 32-bit process install devices on 64-bit Windows (ERROR_IN_WOW64).
ALL += $(OUT)/predev64.exe

$(OUT)/predev64.exe: guest/predev/predev.c Makefile
	@mkdir -p $(OUT)
	$(CC) $(UM64_CFLAGS) -o $@ $< $(UM64_LDFLAGS) -lsetupapi
