# The vmbaud sound card (MS-PL, like everything in drivers/vmbaud/):
# - vmbaud.sys: PortCls WaveCyclic render miniport streaming PCM over a VMBus pipe (C++), linked against
#   an import library for XP's portcls.sys made from drivers/common/portcls.def; vmbaud.inf;
# - testplay.exe: a user-mode console program for XP that plays a tone through winmm (the test client);
# - on the host (x64): vmbaudtray.exe, the tray program, vmbaudcli.exe, the same without the UI, and the
#   older script vmbaud-host.ps1.

VMBAUD_OBJS := $(addprefix $(OBJ)/drivers/vmbaud/,adapter.o common.o helpers.o minwave.o \
	minstream.o mintopo.o vmbaud.res)
VMBAUD_LIBS := $(OBJ)/libportcls.a -lksguid -luuid -lntoskrnl -lhal
# The vmbaud objects share class layouts through these headers: one object built against an older
# layout allocates a smaller object than another fills in.
CXX_HEADERS += $(wildcard drivers/vmbaud/*.h)
ALL += $(OUT)/vmbaud.sys $(OUT)/vmbaud.inf $(OUT)/vmbaud-host.ps1 $(OUT)/testplay.exe \
	$(OUT)/vmbaudtray.exe $(OUT)/vmbaudcli.exe
CHECK += check-vmbaud

$(OBJ)/libportcls.a: drivers/common/portcls.def
	@mkdir -p $(dir $@)
	$(DLLTOOL) -m i386 -k -d $< -l $@

$(OUT)/vmbaud.sys: $(VMBAUD_OBJS) $(OBJ)/libportcls.a
	$(CC) $(LDFLAGS) -Wl,--pdb=$(OUT)/vmbaud.pdb -Wl,-Map=$(OUT)/vmbaud.map \
		-o $@ $(VMBAUD_OBJS) $(VMBAUD_LIBS)
	$(PECHECK) --quiet --map $(OUT)/vmbaud.map --entry _DriverEntry@8 $@

$(OUT)/vmbaud.pdb: $(OUT)/vmbaud.sys

$(OUT)/vmbaud.inf: drivers/vmbaud/vmbaud.inf
	@mkdir -p $(OUT)
	$(CRLF)

$(OUT)/vmbaud-host.ps1: drivers/vmbaud/vmbaud-host.ps1
	@mkdir -p $(OUT)
	$(CRLF)

$(OUT)/testplay.exe: drivers/vmbaud/testplay.c Makefile
	@mkdir -p $(OUT)
	$(CC) $(UM_CFLAGS) -o $@ $< $(UM_LDFLAGS) -lwinmm

check-vmbaud: $(OUT)/vmbaud.sys
	$(PECHECK) $(PECHECK_XP) --map $(OUT)/vmbaud.map --entry _DriverEntry@8 $(OUT)/vmbaud.sys

# The host programs. The part without the UI is shared by vmbaudtray.exe and vmbaudcli.exe.
VMBAUD_TRAY := drivers/vmbaud/tray
VMBAUD_HOST_OBJS := $(addprefix $(HOST64_OBJ)/$(VMBAUD_TRAY)/,hvhost.o pipechannel.o vmsession.o \
	audioout.o log.o)
VMBAUD_TRAY_OBJS := $(VMBAUD_HOST_OBJS) $(addprefix $(HOST64_OBJ)/$(VMBAUD_TRAY)/,trayui.o main.o \
	vmbaudtray.res)
VMBAUD_CLI_OBJS := $(VMBAUD_HOST_OBJS) $(HOST64_OBJ)/$(VMBAUD_TRAY)/cli.o
VMBAUD_HOST_LIBS := -lole32 -loleaut32 -luuid -lwbemuuid -lavrt
HOST64_HEADERS += $(addprefix $(VMBAUD_TRAY)/,hvhost.h pipechannel.h vmsession.h audioout.h \
	trayui.h log.h resource.h) drivers/vmbaud/vmbaud.h

# The tray's .rc is UTF-8 (it has a Chinese string table) and includes resource.h, so it is
# preprocessed and read with code page 65001.
$(HOST64_OBJ)/$(VMBAUD_TRAY)/vmbaudtray.res: $(addprefix $(VMBAUD_TRAY)/,vmbaudtray.rc resource.h \
		vmbaudtray.manifest vmbaudtray.ico)
	@mkdir -p $(dir $@)
	$(RC) /C 65001 /I drivers/vmbaud/tray /fo $@ $<

$(OUT)/vmbaudtray.exe: $(VMBAUD_TRAY_OBJS)
	@mkdir -p $(OUT)
	$(CC) $(HOST64_LDFLAGS) -mwindows -o $@ $(VMBAUD_TRAY_OBJS) \
		$(VMBAUD_HOST_LIBS) -ltaskschd -lcomctl32 -lshell32

$(OUT)/vmbaudcli.exe: $(VMBAUD_CLI_OBJS)
	@mkdir -p $(OUT)
	$(CC) $(HOST64_LDFLAGS) -mconsole -o $@ $(VMBAUD_CLI_OBJS) $(VMBAUD_HOST_LIBS)
