# predev.exe: pre-installs VMBus devices that turn up only after setup (run from $OEM$\cmdlines.txt of
# a setup CD made by hvkit setup-cd; see predev.c).

ALL += $(OUT)/predev.exe

$(OUT)/predev.exe: guest/predev/predev.c Makefile
	@mkdir -p $(OUT)
	$(CC) $(UM_CFLAGS) -o $@ $< $(UM_LDFLAGS) -lsetupapi
