/* predev.exe {Instance} {InterfaceType} INF [...]  - pre-install VMBus devices that turn up later.
 *
 * For each triple: create the (non-present, "phantom") device node that vmbus.sys will report when
 * the host offers the channel, VMBUS\{instance}\<ParentIdPrefix>&{instance}, and install the INF's
 * best driver on it.  When the device does appear, Plug and Play finds an installed device node and
 * just starts the driver: no install, so no server-side signature block and no Found New Hardware
 * wizard.  Meant for GUI-mode setup ($OEM$\cmdlines.txt, as SYSTEM, where DriverSigningPolicy=Ignore
 * holds), or from svcpack.inf on an nLite CD.  GUIDs any case, with or without braces; environment
 * variables in the INF path are expanded; an INF named without a directory is the first one of that
 * name in the DevicePath directories (nLite's driver folders, %SystemRoot%\NLDRV\NNN, are numbered
 * as the user added them).  Log: stdout and %SystemRoot%\predev.log; exit code = number of failed
 * triples. */
#define UNICODE
#define _UNICODE
#include <windows.h>
#include <setupapi.h>
#include <cfgmgr32.h>
#include <shellapi.h>
#ifndef CONFIGFLAG_REINSTALL
#define CONFIGFLAG_REINSTALL 0x20
#define CONFIGFLAG_FAILEDINSTALL 0x40
#define CONFIGFLAG_FINISH_INSTALL 0x400
#endif

static HANDLE logf = INVALID_HANDLE_VALUE;

static void out(const WCHAR *fmt, ...)
{
    WCHAR w[1024];
    char a[2048];
    DWORD n;
    va_list ap;
    va_start(ap, fmt);
    wvsprintfW(w, fmt, ap);
    va_end(ap);
    n = WideCharToMultiByte(CP_ACP, 0, w, -1, a, sizeof a, NULL, NULL);
    if (n)
        n--;
    WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), a, n, &n, NULL);
    if (logf != INVALID_HANDLE_VALUE)
        WriteFile(logf, a, n, &n, NULL);
}

/* "{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}" in lower case, as vmbus.sys reports it */
static BOOL guid_arg(const WCHAR *s, WCHAR *g)
{
    int i, j = 0;
    if (*s == L'{')
        s++;
    g[j++] = L'{';
    for (i = 0; i < 36; i++) {
        WCHAR c = s[i];
        if (i == 8 || i == 13 || i == 18 || i == 23) {
            if (c != L'-')
                return FALSE;
        } else if (c >= L'A' && c <= L'F')
            c += 32;
        else if (!((c >= L'0' && c <= L'9') || (c >= L'a' && c <= L'f')))
            return FALSE;
        g[j++] = c;
    }
    if (s[36] && !(s[36] == L'}' && !s[37]))
        return FALSE;
    g[j++] = L'}';
    g[j] = 0;
    return TRUE;
}

static LONG enum_value(const WCHAR *inst, const WCHAR *name, WCHAR *buf, DWORD cb)
{
    WCHAR path[400] = L"SYSTEM\\CurrentControlSet\\Enum\\";
    HKEY k;
    LONG r;
    DWORD type;
    lstrcatW(path, inst);
    r = RegOpenKeyExW(HKEY_LOCAL_MACHINE, path, 0, KEY_QUERY_VALUE, &k);
    if (r)
        return r;
    r = RegQueryValueExW(k, name, NULL, &type, (BYTE *)buf, &cb);
    RegCloseKey(k);
    return r;
}

/* ParentIdPrefix of the VMBus bus device (the present device whose service is vmbus): vmbus.sys
 * does not report unique instance IDs, so Plug and Play puts this prefix in front of its children's. */
static BOOL vmbus_prefix(WCHAR *prefix, DWORD cch)
{
    HDEVINFO s = SetupDiGetClassDevsW(NULL, NULL, NULL, DIGCF_ALLCLASSES | DIGCF_PRESENT);
    SP_DEVINFO_DATA d;
    DWORD i;
    BOOL ok = FALSE;
    if (s == INVALID_HANDLE_VALUE)
        return FALSE;
    d.cbSize = sizeof d;
    for (i = 0; !ok && SetupDiEnumDeviceInfo(s, i, &d); i++) {
        WCHAR svc[64], id[MAX_DEVICE_ID_LEN], child[MAX_DEVICE_ID_LEN];
        DEVINST c;
        LONG r;
        if (!SetupDiGetDeviceRegistryPropertyW(s, &d, SPDRP_SERVICE, NULL, (BYTE *)svc, sizeof svc,
                                               NULL) ||
            lstrcmpiW(svc, L"vmbus") ||
            !SetupDiGetDeviceInstanceIdW(s, &d, id, MAX_DEVICE_ID_LEN, NULL))
            continue;
        r = enum_value(id, L"ParentIdPrefix", prefix, cch * sizeof(WCHAR));
        out(L"VMBus bus device %s: ParentIdPrefix %s (%d)\r\n", id, r ? L"?" : prefix, r);
        if (CM_Get_Child(&c, d.DevInst, 0) == CR_SUCCESS &&
            CM_Get_Device_IDW(c, child, MAX_DEVICE_ID_LEN, 0) == CR_SUCCESS)
            out(L"  a child for comparison: %s\r\n", child);
        ok = !r;
    }
    SetupDiDestroyDeviceInfoList(s);
    return ok;
}

/* The INF `name` (no directory) in the first DevicePath directory that has it, into full[MAX_PATH]. */
static BOOL device_path_inf(const WCHAR *name, WCHAR *full)
{
    WCHAR raw[4096], dirs[4096], *d = dirs, *e;
    DWORD cb = sizeof raw - sizeof(WCHAR), type, n;
    HKEY k;
    LONG r;
    if (RegOpenKeyExW(HKEY_LOCAL_MACHINE, L"SOFTWARE\\Microsoft\\Windows\\CurrentVersion", 0,
                      KEY_QUERY_VALUE, &k))
        return FALSE;
    r = RegQueryValueExW(k, L"DevicePath", NULL, &type, (BYTE *)raw, &cb);
    RegCloseKey(k);
    if (r || (type != REG_SZ && type != REG_EXPAND_SZ))
        return FALSE;
    raw[cb / sizeof(WCHAR)] = 0;
    n = ExpandEnvironmentStringsW(raw, dirs, 4096);
    if (!n || n > 4096)
        return FALSE;
    for (;;) {
        for (e = d; *e && *e != L';'; e++)
            ;
        if (e > d && (e - d) + 1 + lstrlenW(name) < MAX_PATH) {
            lstrcpynW(full, d, (int)(e - d) + 1);
            if (e[-1] != L'\\')
                lstrcatW(full, L"\\");
            lstrcatW(full, name);
            if (GetFileAttributesW(full) != INVALID_FILE_ATTRIBUTES)
                return TRUE;
        }
        if (!*e)
            return FALSE;
        d = e + 1;
    }
}

static BOOL bare_name(const WCHAR *s)
{
    for (; *s; s++)
        if (*s == L'\\' || *s == L'/' || *s == L':')
            return FALSE;
    return TRUE;
}

static BOOL dif(DI_FUNCTION f, const WCHAR *name, HDEVINFO s, SP_DEVINFO_DATA *d, BOOL optional)
{
    DWORD e;
    if (SetupDiCallClassInstaller(f, s, d))
        return TRUE;
    e = GetLastError();
    out(L"  %s: error 0x%08x%s\r\n", name, e, optional ? L" (ignored)" : L"");
    return optional;
}

static BOOL predev(const WCHAR *inst, const WCHAR *type, const WCHAR *prefix, const WCHAR *inf)
{
    WCHAR id[MAX_DEVICE_ID_LEN], hw[200], compat[100], path[400];
    DEVINST root, dn;
    CONFIGRET cr;
    HDEVINFO s;
    SP_DEVINFO_DATA d;
    SP_DEVINSTALL_PARAMS_W p;
    SP_DRVINFO_DATA_W drv;
    SP_DRVINSTALL_PARAMS dp;
    ULONG flags, cb;
    HKEY k;
    BOOL ok = FALSE;
    int n;

    wsprintfW(id, L"VMBUS\\%s\\%s&%s", inst, prefix, inst);
    out(L"%s: %s\r\n", id, inf);
    /* A phantom device node (registry only, no PDO): CM_Create_DevInst with CM_CREATE_DEVINST_PHANTOM
     * takes any legal device ID under the root.  Not SetupDiCreateDeviceInfo + DIF_REGISTERDEVICE:
     * registering makes a root-enumerated device of it (NtPlugPlayControl), and the unregistered
     * phantom SetupDiCreateDeviceInfo makes is deleted again by SetupDiDestroyDeviceInfoList. */
    cr = CM_Locate_DevNodeW(&root, NULL, CM_LOCATE_DEVNODE_NORMAL);
    if (cr == CR_SUCCESS)
        cr = CM_Create_DevNodeW(&dn, id, root, CM_CREATE_DEVNODE_PHANTOM);
    if (cr == CR_ALREADY_SUCH_DEVNODE) {
        /* The device was there during setup (Plug and Play installed it), or this ran before. */
        WCHAR drv[200];
        if (!enum_value(id, L"Driver", drv, sizeof drv)) {
            out(L"  installed already (Driver %s), left alone\r\n", drv);
            return TRUE;
        }
        out(L"  device node exists already, without a driver\r\n");
    } else if (cr != CR_SUCCESS) {
        out(L"  CM_Create_DevInst: CR 0x%x\r\n", cr);
        return FALSE;
    }
    /* Such a "private phantom" has the value Phantom=1, and umpnpmgr hides it from everyone but the
     * creator's handle (PNP_ValidateDeviceInstance fails CM_LOCATE_DEVNODE_PHANTOM for it, so
     * SetupDiOpenDeviceInfo cannot find it).  Without the value it is an ordinary non-present device
     * node, like one that has been present before.  The Enum keys are writable only by SYSTEM. */
    wsprintfW(path, L"SYSTEM\\CurrentControlSet\\Enum\\%s", id);
    if (RegOpenKeyExW(HKEY_LOCAL_MACHINE, path, 0, KEY_SET_VALUE, &k) == ERROR_SUCCESS) {
        LONG r = RegDeleteValueW(k, L"Phantom");
        RegCloseKey(k);
        if (r && r != ERROR_FILE_NOT_FOUND) {
            out(L"  Phantom value: error %d\r\n", r);
            return FALSE;
        }
    } else {
        out(L"  Enum key not writable (run as SYSTEM)\r\n");
        return FALSE;
    }
    s = SetupDiCreateDeviceInfoList(NULL, NULL);
    if (s == INVALID_HANDLE_VALUE)
        return FALSE;
    d.cbSize = sizeof d;
    if (!SetupDiOpenDeviceInfoW(s, id, NULL, 0, &d)) {
        out(L"  SetupDiOpenDeviceInfo: error 0x%08x\r\n", GetLastError());
        goto done;
    }
    /* what vmbus.sys reports: hardware IDs VMBUS\{type}, VMBUS\{instance}; compatible VMBUS\{type} */
    n = wsprintfW(hw, L"VMBUS\\%s", type) + 1;
    n += wsprintfW(hw + n, L"VMBUS\\%s", inst) + 1;
    hw[n++] = 0;
    if (!SetupDiSetDeviceRegistryPropertyW(s, &d, SPDRP_HARDWAREID, (BYTE *)hw, n * sizeof(WCHAR)))
        out(L"  HardwareID: error 0x%08x\r\n", GetLastError());
    n = wsprintfW(compat, L"VMBUS\\%s", type) + 1;
    compat[n++] = 0;
    if (!SetupDiSetDeviceRegistryPropertyW(s, &d, SPDRP_COMPATIBLEIDS, (BYTE *)compat,
                                           n * sizeof(WCHAR)))
        out(L"  CompatibleIDs: error 0x%08x\r\n", GetLastError());

    p.cbSize = sizeof p;
    if (!SetupDiGetDeviceInstallParamsW(s, &d, &p))
        goto fail;
    lstrcpynW(p.DriverPath, inf, MAX_PATH);
    p.Flags |= DI_ENUMSINGLEINF | DI_QUIETINSTALL;
    if (!SetupDiSetDeviceInstallParamsW(s, &d, &p) ||
        !SetupDiBuildDriverInfoList(s, &d, SPDIT_COMPATDRIVER))
        goto fail;
    if (!dif(DIF_SELECTBESTCOMPATDRV, L"DIF_SELECTBESTCOMPATDRV", s, &d, FALSE))
        goto done;
    drv.cbSize = sizeof drv;
    dp.cbSize = sizeof dp;
    if (SetupDiGetSelectedDriverW(s, &d, &drv) &&
        SetupDiGetDriverInstallParamsW(s, &d, &drv, &dp))
        out(L"  driver \"%s\" (%s), rank 0x%x\r\n", drv.Description, drv.ProviderName, dp.Rank);
    if (!dif(DIF_ALLOW_INSTALL, L"DIF_ALLOW_INSTALL", s, &d, TRUE) ||
        !dif(DIF_INSTALLDEVICEFILES, L"DIF_INSTALLDEVICEFILES", s, &d, FALSE) ||
        !dif(DIF_REGISTER_COINSTALLERS, L"DIF_REGISTER_COINSTALLERS", s, &d, FALSE) ||
        !dif(DIF_INSTALLINTERFACES, L"DIF_INSTALLINTERFACES", s, &d, FALSE) ||
        !dif(DIF_INSTALLDEVICE, L"DIF_INSTALLDEVICE", s, &d, FALSE))
        goto done;
    ok = TRUE;
    if (SetupDiGetDeviceInstallParamsW(s, &d, &p))
        out(L"  installed; install flags 0x%08x\r\n", p.Flags);

    /* Plug and Play installs a device again when it turns up with CONFIGFLAG_REINSTALL or
     * CONFIGFLAG_FINISH_INSTALL (or FAILEDINSTALL); none of them may stay on. */
    cb = sizeof flags;
    cr = CM_Get_DevNode_Registry_PropertyW(d.DevInst, CM_DRP_CONFIGFLAGS, NULL, &flags, &cb, 0);
    if (cr == CR_SUCCESS) {
        ULONG bad = CONFIGFLAG_REINSTALL | CONFIGFLAG_FAILEDINSTALL | CONFIGFLAG_FINISH_INSTALL;
        out(L"  ConfigFlags 0x%x\r\n", flags);
        if (flags & bad) {
            flags &= ~bad;
            cr = CM_Set_DevNode_Registry_PropertyW(d.DevInst, CM_DRP_CONFIGFLAGS, &flags,
                                                   sizeof flags, 0);
            out(L"  ConfigFlags -> 0x%x (CR 0x%x)\r\n", flags, cr);
        }
    } else
        out(L"  ConfigFlags: CR 0x%x\r\n", cr);

    {
        WCHAR v[200];
        if (!enum_value(id, L"Service", v, sizeof v))
            out(L"  Service %s\r\n", v);
        if (!enum_value(id, L"Driver", v, sizeof v))
            out(L"  Driver %s\r\n", v);
        if (!enum_value(id, L"ClassGUID", v, sizeof v))
            out(L"  ClassGUID %s\r\n", v);
    }
    goto done;
fail:
    out(L"  error 0x%08x\r\n", GetLastError());
done:
    SetupDiDestroyDeviceInfoList(s);
    return ok;
}

int main(void)
{
    int argc, i, fails = 0;
    WCHAR **argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    WCHAR log[MAX_PATH], prefix[64];

    if (GetEnvironmentVariableW(L"SystemRoot", log, MAX_PATH - 16)) {
        lstrcatW(log, L"\\predev.log");
        logf = CreateFileW(log, FILE_APPEND_DATA, FILE_SHARE_READ, NULL, OPEN_ALWAYS,
                           FILE_ATTRIBUTE_NORMAL, NULL);
    }
    if (!argv || argc < 4 || (argc - 1) % 3) {
        out(L"usage: predev {Instance} {InterfaceType} INF [...]\r\n");
        return 1;
    }
    if (!vmbus_prefix(prefix, 64)) {
        out(L"no VMBus bus device with a ParentIdPrefix\r\n");
        return (argc - 1) / 3;
    }
    for (i = 1; i + 2 < argc; i += 3) {
        WCHAR inst[40], type[40], inf[MAX_PATH], full[MAX_PATH], *part;
        if (!guid_arg(argv[i], inst) || !guid_arg(argv[i + 1], type)) {
            out(L"%s %s: not GUIDs\r\n", argv[i], argv[i + 1]);
            fails++;
            continue;
        }
        if (!ExpandEnvironmentStringsW(argv[i + 2], inf, MAX_PATH) ||
            !(bare_name(inf) ? device_path_inf(inf, full)
                             : GetFullPathNameW(inf, MAX_PATH, full, &part) &&
                                   GetFileAttributesW(full) != INVALID_FILE_ATTRIBUTES)) {
            out(L"%s: no such INF%s\r\n", argv[i + 2], bare_name(inf) ? L" in DevicePath" : L"");
            fails++;
            continue;
        }
        if (!predev(inst, type, prefix, full))
            fails++;
    }
    out(L"%d failed\r\n", fails);
    if (logf != INVALID_HANDLE_VALUE)
        CloseHandle(logf);
    return fails;
}
