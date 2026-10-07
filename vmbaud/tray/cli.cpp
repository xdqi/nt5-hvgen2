/*
 * cli.cpp: vmbaudcli.exe, the console counterpart of vmbaudtray, for
 * scripts and tests.
 *
 *   vmbaudcli list
 *   vmbaudcli set   <VM> enabled|volume|mute <value>
 *   vmbaudcli unset <VM> enabled|volume|mute
 *   vmbaudcli run   <VM> [seconds] [--mute] [--wav PREFIX]
 *
 * <VM> is the VM name or its GUID.  set/unset write the host-only KVP items
 * vmbaudtray reads (a Hyper-V Administrators member may do that).  run
 * serves one VM in the foreground, the way vmbaudtray does, logging to the
 * console until Ctrl+C or the given time; it needs an elevated prompt (the
 * offer needs a full administrator token) and must not run while vmbaudtray
 * serves the same VM.  --mute: the stream runs on the real device (its
 * clock), its session muted.  --wav: each stream the guest sends is also
 * written to PREFIX-<n>.wav as received.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#include <windows.h>
#include <shellapi.h>
#include <stdarg.h>

#include "hvhost.h"
#include "pipechannel.h"
#include "audioout.h"
#include "vmsession.h"
#include "log.h"

enum { kMaxVms = 64 };

static HANDLE g_out;
static HANDLE g_quit;

static void out(const wchar_t* fmt, ...)
{
    wchar_t line[1024 + 2];
    char utf8[3 * (1024 + 2)];
    va_list ap;
    DWORD written;
    int n;

    va_start(ap, fmt);
    wvsprintfW(line, fmt, ap);
    va_end(ap);
    n = lstrlenW(line);
    line[n++] = L'\r';
    line[n++] = L'\n';
    n = WideCharToMultiByte(CP_UTF8, 0, line, n, utf8, (int)sizeof(utf8), 0, 0);
    if (n > 0)
        WriteFile(g_out, utf8, (DWORD)n, &written, 0);
}

static int usage(void)
{
    out(L"usage: vmbaudcli list");
    out(L"       vmbaudcli set   <VM> enabled|volume|mute <value>");
    out(L"       vmbaudcli unset <VM> enabled|volume|mute");
    out(L"       vmbaudcli run   <VM> [seconds] [--mute] [--wav PREFIX]   (elevated)");
    return 2;
}

static const wchar_t* state_name(int state)
{
    switch (state) {
    case 2:  return L"Running";
    case 3:  return L"Off";
    case 6:  return L"Saved";
    case 9:  return L"Paused";
    case 10: return L"Starting";
    case 4:  return L"Stopping";
    default: return L"Other";
    }
}

static void guid_text(const GUID* g, wchar_t* buf)
{
    wsprintfW(buf, L"%08X-%04X-%04X-%02X%02X-%02X%02X%02X%02X%02X%02X",
              (unsigned)g->Data1, (unsigned)g->Data2, (unsigned)g->Data3,
              g->Data4[0], g->Data4[1], g->Data4[2], g->Data4[3],
              g->Data4[4], g->Data4[5], g->Data4[6], g->Data4[7]);
}

/* By name (case-insensitive) or GUID. */
static int find_vm(const wchar_t* which, HV_VM* vm)
{
    static HV_VM vms[kMaxVms];
    int n, i;

    n = HvHost_ListVms(vms, kMaxVms);
    for (i = 0; i < n; i++) {
        wchar_t g[40];
        guid_text(&vms[i].Id, g);
        if (lstrcmpiW(vms[i].Name, which) == 0 || lstrcmpiW(g, which) == 0) {
            *vm = vms[i];
            return 1;
        }
    }
    out(L"no VM called %s", which);
    return 0;
}

static const wchar_t* kvp_key(const wchar_t* name)
{
    if (lstrcmpiW(name, L"enabled") == 0) return L"vmbaud.enabled";
    if (lstrcmpiW(name, L"volume") == 0)  return L"vmbaud.volume";
    if (lstrcmpiW(name, L"mute") == 0)    return L"vmbaud.mute";
    out(L"unknown setting %s (enabled, volume, mute)", name);
    return 0;
}

static int cmd_list(void)
{
    static HV_VM vms[kMaxVms];
    int n, i;

    n = HvHost_ListVms(vms, kMaxVms);
    if (n < 0) {
        out(L"listing the VMs failed");
        return 1;
    }
    out(L"%-24s %-9s %-7s %-6s %s", L"NAME", L"STATE", L"SOUND", L"VOLUME", L"MUTE");
    for (i = 0; i < n; i++)
        out(L"%-24s %-9s %-7s %-6d %s", vms[i].Name, state_name(vms[i].EnabledState),
            vms[i].SoundEnabled ? L"on" : L"off", vms[i].Volume,
            vms[i].Mute ? L"yes" : L"no");
    return 0;
}

static int cmd_set(const wchar_t* which, const wchar_t* name, const wchar_t* value)
{
    HV_VM vm;
    const wchar_t* key = kvp_key(name);

    if (!key || !find_vm(which, &vm))
        return 1;
    if (value) {
        if (!HvHost_WriteSetting(&vm.Id, key, value)) {
            out(L"%s: writing %s=%s failed", vm.Name, key, value);
            return 1;
        }
        out(L"%s: %s=%s", vm.Name, key, value);
    } else {
        if (!HvHost_RemoveSetting(&vm.Id, key)) {
            out(L"%s: removing %s failed", vm.Name, key);
            return 1;
        }
        out(L"%s: %s removed", vm.Name, key);
    }
    return 0;
}

static BOOL WINAPI on_ctrl(DWORD type)
{
    (void)type;
    SetEvent(g_quit);
    return TRUE;
}

static int cmd_run(int argc, wchar_t** argv)
{
    HV_VM vm;
    VM_SESSION* s;
    DWORD ms = INFINITE;
    int mute = 0, i;

    if (argc < 3 || !find_vm(argv[2], &vm))
        return argc < 3 ? usage() : 1;
    for (i = 3; i < argc; i++) {
        if (lstrcmpiW(argv[i], L"--mute") == 0) {
            mute = 1;
        } else if (lstrcmpiW(argv[i], L"--wav") == 0 && i + 1 < argc) {
            VmSession_SetCapture(argv[++i]);
        } else if (argv[i][0] >= L'0' && argv[i][0] <= L'9') {
            const wchar_t* p = argv[i];
            DWORD n = 0;
            while (*p >= L'0' && *p <= L'9')
                n = n * 10 + (DWORD)(*p++ - L'0');
            ms = n * 1000;
        } else {
            return usage();
        }
    }
    if (mute)
        vm.Mute = 1;
    if (!PipeChannel_Load()) {
        out(L"vmbuspiper.dll cannot be loaded (error %u)", (unsigned)GetLastError());
        return 1;
    }
    if (!AudioOut_GlobalInit())
        out(L"no audio device enumerator: the clock is estimated");

    g_quit = CreateEventW(0, TRUE, FALSE, 0);
    SetConsoleCtrlHandler(on_ctrl, TRUE);
    /* No window: the session reports through the log only. */
    s = VmSession_Start(&vm, 0, 0, 0);
    if (!s) {
        out(L"cannot start the session");
        return 1;
    }
    WaitForSingleObject(g_quit, ms);
    VmSession_Stop(s);
    AudioOut_GlobalShutdown();
    PipeChannel_Unload();
    return 0;
}

int wmain(int argc, wchar_t** argv)
{
    int rc;

    g_out = GetStdHandle(STD_OUTPUT_HANDLE);
    if (argc < 2)
        return usage();

    CoInitializeEx(0, COINIT_MULTITHREADED);
    /* Also a file: over SSH the console output arrives in blocks. */
    Log_Open(L"vmbaudcli.log", g_out);
    if (!HvHost_Open()) {
        out(L"cannot connect to Hyper-V WMI (root\\virtualization\\v2)");
        CoUninitialize();
        return 1;
    }

    if (lstrcmpiW(argv[1], L"list") == 0 && argc == 2)
        rc = cmd_list();
    else if (lstrcmpiW(argv[1], L"set") == 0 && argc == 5)
        rc = cmd_set(argv[2], argv[3], argv[4]);
    else if (lstrcmpiW(argv[1], L"unset") == 0 && argc == 4)
        rc = cmd_set(argv[2], argv[3], 0);
    else if (lstrcmpiW(argv[1], L"run") == 0)
        rc = cmd_run(argc, argv);
    else
        rc = usage();

    HvHost_Close();
    Log_Close();
    CoUninitialize();
    return rc;
}
