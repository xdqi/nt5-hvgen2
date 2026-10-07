/*
 * hvhost.cpp: Hyper-V WMI helpers.
 *
 * Settings live in each VM's host-only KVP (Msvm_KvpExchangeDataItem with
 * Source = 4), written through Msvm_VirtualSystemManagementService and read
 * from Msvm_KvpExchangeComponentSettingData.HostOnlyItems (embedded CIM XML).
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#include <windows.h>
#include <oleauto.h>
#include <wbemidl.h>

#include "hvhost.h"
#include "log.h"

/* Local replacements for shlwapi; keeps the import list to system COM DLLs. */
static const wchar_t* wcs_find(const wchar_t* hay, const wchar_t* needle)
{
    size_t n;
    if (!hay || !needle || !*needle)
        return hay;
    n = 0;
    while (needle[n])
        n++;
    for (; *hay; hay++) {
        size_t i = 0;
        while (i < n && hay[i] == needle[i])
            i++;
        if (i == n)
            return hay;
    }
    return 0;
}

static int wcs_nicmp(const wchar_t* a, const wchar_t* b, size_t n)
{
    size_t i;
    for (i = 0; i < n; i++) {
        wchar_t ca = a[i], cb = b[i];
        if (ca >= L'A' && ca <= L'Z') ca = (wchar_t)(ca - L'A' + L'a');
        if (cb >= L'A' && cb <= L'Z') cb = (wchar_t)(cb - L'A' + L'a');
        if (ca != cb)
            return (ca < cb) ? -1 : 1;
        if (!ca)
            return 0;
    }
    return 0;
}

static int wcs_toi(const wchar_t* s)
{
    int sign = 1, v = 0;
    if (!s)
        return 0;
    if (*s == L'-') { sign = -1; s++; }
    else if (*s == L'+') s++;
    while (*s >= L'0' && *s <= L'9') {
        v = v * 10 + (*s - L'0');
        s++;
    }
    return sign * v;
}

static IWbemServices*  g_svc;
static IWbemObjectSink* g_sink;

/* ------------------------------------------------------------------ */
/* Small string helpers (no C++ stdlib)                               */
/* ------------------------------------------------------------------ */

/* Unbraced, matching Msvm_ComputerSystem.Name. */
static void guid_to_str(const GUID* g, wchar_t* out, int cap)
{
    wsprintfW(out,
              L"%08X-%04X-%04X-%02X%02X-%02X%02X%02X%02X%02X%02X",
              (unsigned)g->Data1, (unsigned)g->Data2, (unsigned)g->Data3,
              (unsigned)g->Data4[0], (unsigned)g->Data4[1],
              (unsigned)g->Data4[2], (unsigned)g->Data4[3],
              (unsigned)g->Data4[4], (unsigned)g->Data4[5],
              (unsigned)g->Data4[6], (unsigned)g->Data4[7]);
    (void)cap;
}

/* Extract <VALUE> of the property called `name` from a CIM DTD 2.0 instance. */
static int xml_prop(const wchar_t* xml, const wchar_t* name, wchar_t* out, int cap)
{
    wchar_t key[80];
    const wchar_t* p;
    const wchar_t* v0;
    const wchar_t* v1;
    int n;

    if (!xml || !name || !out || cap <= 0)
        return 0;
    wsprintfW(key, L"NAME=\"%s\"", name);
    p = xml;
    for (;;) {
        p = wcs_find(p, key);
        if (!p)
            return 0;
        /* NAME="Name" must not match NAME="NameX": the quote is part of key. */
        v0 = wcs_find(p, L"<VALUE>");
        v1 = wcs_find(p, L"</PROPERTY>");
        if (v0 && (!v1 || v0 < v1))
            break;
        p += 1;
    }
    v0 += 7; /* strlen("<VALUE>") */
    v1 = wcs_find(v0, L"</VALUE>");
    if (!v1)
        return 0;
    n = (int)(v1 - v0);
    if (n >= cap)
        n = cap - 1;
    /* Copy and unescape the few entities WMI emits. */
    {
        int i = 0, o = 0;
        while (i < n && o < cap - 1) {
            if (v0[i] == L'&') {
                if (wcs_nicmp(v0 + i, L"lt;", 3) == 0)   { out[o++] = L'<';  i += 3; continue; }
                if (wcs_nicmp(v0 + i, L"gt;", 3) == 0)   { out[o++] = L'>';  i += 3; continue; }
                if (wcs_nicmp(v0 + i, L"amp;", 4) == 0)  { out[o++] = L'&';  i += 4; continue; }
                if (wcs_nicmp(v0 + i, L"quot;", 5) == 0) { out[o++] = L'"';  i += 5; continue; }
                if (wcs_nicmp(v0 + i, L"apos;", 5) == 0) { out[o++] = L'\''; i += 5; continue; }
            }
            out[o++] = v0[i++];
        }
        out[o] = 0;
        return o;
    }
}

/* A BSTR argument that lives until the end of the full expression. */
struct Bstr {
    BSTR b;
    explicit Bstr(const wchar_t* s) : b(SysAllocString(s)) {}
    ~Bstr() { SysFreeString(b); }
    operator BSTR() const { return b; }
};

static HRESULT exec_query(const wchar_t* wql, IEnumWbemClassObject** en)
{
    return g_svc->ExecQuery(Bstr(L"WQL"), Bstr(wql),
                            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                            0, en);
}

static IWbemClassObject* query_first(const wchar_t* wql)
{
    IEnumWbemClassObject* en = 0;
    IWbemClassObject* obj = 0;
    ULONG n = 0;

    if (FAILED(exec_query(wql, &en)) || !en)
        return 0;
    en->Next(WBEM_INFINITE, 1, &obj, &n);
    en->Release();
    return (n == 1) ? obj : 0;
}

static void variant_clear(VARIANT* v)
{
    VariantClear(v);
}

/* Reads one string property into out. */
static int get_str(IWbemClassObject* obj, const wchar_t* prop, wchar_t* out, int cap)
{
    VARIANT v;
    int n = 0;

    VariantInit(&v);
    if (SUCCEEDED(obj->Get(prop, 0, &v, 0, 0))) {
        if (v.vt == VT_BSTR && v.bstrVal) {
            lstrcpynW(out, v.bstrVal, cap);
            n = lstrlenW(out);
        }
    }
    variant_clear(&v);
    return n;
}

static int get_uint(IWbemClassObject* obj, const wchar_t* prop, int def)
{
    VARIANT v;
    int r = def;

    VariantInit(&v);
    if (SUCCEEDED(obj->Get(prop, 0, &v, 0, 0))) {
        if (v.vt == VT_I4)       r = (int)v.lVal;
        else if (v.vt == VT_UI4) r = (int)v.ulVal;
        else if (v.vt == VT_I2)  r = (int)v.iVal;
        else if (v.vt == VT_UI2) r = (int)v.uiVal;
        else if (v.vt == VT_UI1) r = (int)v.bVal;
    }
    variant_clear(&v);
    return r;
}

/* ------------------------------------------------------------------ */
/* Connect / list                                                     */
/* ------------------------------------------------------------------ */

/* g_svc belongs to the apartment of the thread that opened it (vmbaudtray's
 * UI thread is an STA).  Another thread that calls in here gets a connection
 * of its own, kept in TLS until HvHost_ThreadDone: marshalling g_svc to it
 * would run its calls on the UI thread, which may be waiting for it. */
static DWORD g_svcThread;
static DWORD g_tls = TLS_OUT_OF_INDEXES;

static IWbemServices* connect_wmi(void)
{
    IWbemLocator* loc = 0;
    IWbemServices* svc = 0;

    if (FAILED(CoCreateInstance(CLSID_WbemLocator, 0, CLSCTX_INPROC_SERVER,
                                IID_IWbemLocator, (void**)&loc)) || !loc)
        return 0;
    if (FAILED(loc->ConnectServer(Bstr(L"ROOT\\virtualization\\v2"),
                                  0, 0, 0, 0, 0, 0, &svc)))
        svc = 0;
    loc->Release();
    if (svc)
        CoSetProxyBlanket(svc, RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, 0,
                          RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
                          0, EOAC_NONE);
    return svc;
}

/* The connection for the calling thread, or 0. */
static IWbemServices* svc_here(void)
{
    IWbemServices* svc;

    if (GetCurrentThreadId() == g_svcThread || g_tls == TLS_OUT_OF_INDEXES)
        return g_svc;
    svc = (IWbemServices*)TlsGetValue(g_tls);
    if (!svc) {
        svc = connect_wmi();
        TlsSetValue(g_tls, svc);
    }
    return svc;
}

BOOL HvHost_Open(void)
{
    if (g_svc)
        return TRUE;
    g_svc = connect_wmi();
    if (!g_svc)
        return FALSE;
    g_svcThread = GetCurrentThreadId();
    g_tls = TlsAlloc();
    return TRUE;
}

void HvHost_ThreadDone(void)
{
    IWbemServices* svc;

    if (g_tls == TLS_OUT_OF_INDEXES)
        return;
    svc = (IWbemServices*)TlsGetValue(g_tls);
    if (svc) {
        svc->Release();
        TlsSetValue(g_tls, 0);
    }
}

void HvHost_Close(void)
{
    HvHost_StopEvents();
    if (g_svc) { g_svc->Release(); g_svc = 0; }
    if (g_tls != TLS_OUT_OF_INDEXES) {
        TlsFree(g_tls);
        g_tls = TLS_OUT_OF_INDEXES;
    }
}

BOOL HvHost_IsOpen(void)
{
    return g_svc != 0;
}

/* ------------------------------------------------------------------ */
/* Host-only KVP                                                      */
/* ------------------------------------------------------------------ */

/* Calls cb for each of the VM's host-only KVP items.
 *
 * WQL ASSOCIATORS OF does not work in root\virtualization\v2 ("Invalid
 * object path"); the live object is Msvm_KvpExchangeComponentSettingData
 * whose InstanceID is "Microsoft:<VM GUID>\<component GUID>".  HostOnlyItems
 * is a SAFEARRAY of BSTR, each a CIM DTD 2.0 Msvm_KvpExchangeDataItem.
 */
typedef void (*KVP_CB)(void* ctx, const wchar_t* key, const wchar_t* value);

static void kvp_each(const GUID* vmId, KVP_CB cb, void* ctx)
{
    wchar_t g[48], wql[160];
    IWbemClassObject* kvp;
    VARIANT v;

    guid_to_str(vmId, g, 48);
    wsprintfW(wql,
              L"SELECT * FROM Msvm_KvpExchangeComponentSettingData "
              L"WHERE InstanceID LIKE '%%%s%%'",
              g);
    kvp = query_first(wql);
    if (!kvp)
        return;
    VariantInit(&v);
    if (SUCCEEDED(kvp->Get(L"HostOnlyItems", 0, &v, 0, 0))
        && v.vt == (VT_ARRAY | VT_BSTR) && v.parray) {
        LONG lo = 0, hi = -1, i;
        SafeArrayGetLBound(v.parray, 1, &lo);
        SafeArrayGetUBound(v.parray, 1, &hi);
        for (i = lo; i <= hi; i++) {
            BSTR item = 0;
            wchar_t key[128], val[64];
            if (FAILED(SafeArrayGetElement(v.parray, &i, &item)) || !item)
                continue;
            if (xml_prop(item, L"Name", key, 128)) {
                if (!xml_prop(item, L"Data", val, 64))
                    val[0] = 0;
                cb(ctx, key, val);
            }
            SysFreeString(item);
        }
    }
    variant_clear(&v);
    kvp->Release();
}

static void settings_cb(void* ctx, const wchar_t* key, const wchar_t* val)
{
    HV_VM* out = (HV_VM*)ctx;

    if (lstrcmpW(key, L"vmbaud.enabled") == 0)
        out->SoundEnabled = (val[0] == L'1') ? 1 : 0;
    else if (lstrcmpW(key, L"vmbaud.volume") == 0)
        out->Volume = wcs_toi(val);
    else if (lstrcmpW(key, L"vmbaud.mute") == 0)
        out->Mute = (val[0] == L'1') ? 1 : 0;
}

/* Fills enabled/volume/mute; absent keys keep the defaults. */
static void read_kvp_into(const GUID* vmId, HV_VM* out)
{
    out->SoundEnabled = 0;
    out->Volume = 100;
    out->Mute = 0;
    kvp_each(vmId, settings_cb, out);
    if (out->Volume < 0)
        out->Volume = 0;
    if (out->Volume > 100)
        out->Volume = 100;
}

typedef struct _KVP_LOOKUP {
    const wchar_t* key;
    wchar_t* val;
    int cap;
    int found;
} KVP_LOOKUP;

static void lookup_cb(void* ctx, const wchar_t* key, const wchar_t* val)
{
    KVP_LOOKUP* l = (KVP_LOOKUP*)ctx;

    if (lstrcmpW(key, l->key) == 0) {
        lstrcpynW(l->val, val, l->cap);
        l->found = 1;
    }
}

static int kvp_lookup(const GUID* vmId, const wchar_t* key, wchar_t* val, int cap)
{
    KVP_LOOKUP l = { key, val, cap, 0 };
    kvp_each(vmId, lookup_cb, &l);
    return l.found;
}

BOOL HvHost_ReadSettings(const GUID* vmId, HV_VM* out)
{
    if (!g_svc || !vmId || !out)
        return FALSE;
    read_kvp_into(vmId, out);
    return TRUE;
}

/* A CIM DTD 2.0 Msvm_KvpExchangeDataItem with Source = 4 (host only). */
static BSTR make_kvp_xml(const wchar_t* key, const wchar_t* value)
{
    IWbemClassObject* cls = 0;
    IWbemClassObject* inst = 0;
    BSTR text = 0;
    VARIANT v;

    if (FAILED(g_svc->GetObject(Bstr(L"Msvm_KvpExchangeDataItem"), 0, 0, &cls, 0)) || !cls)
        return 0;
    if (FAILED(cls->SpawnInstance(0, &inst)) || !inst) {
        cls->Release();
        return 0;
    }
    VariantInit(&v);
    v.vt = VT_BSTR;
    v.bstrVal = SysAllocString(key);
    inst->Put(L"Name", 0, &v, 0);
    variant_clear(&v);

    VariantInit(&v);
    v.vt = VT_BSTR;
    v.bstrVal = SysAllocString(value);
    inst->Put(L"Data", 0, &v, 0);
    variant_clear(&v);

    VariantInit(&v);
    v.vt = VT_I4;
    v.lVal = 4;
    inst->Put(L"Source", 0, &v, 0);
    variant_clear(&v);

    /* IWbemClassObject::GetObjectText only makes MOF; the methods want the
     * CIM XML that IWbemObjectTextSrc makes (PowerShell's GetText(1)). */
    {
        IWbemObjectTextSrc* src = 0;
        if (SUCCEEDED(CoCreateInstance(CLSID_WbemObjectTextSrc, 0, CLSCTX_INPROC_SERVER,
                                       IID_IWbemObjectTextSrc, (void**)&src)) && src) {
            if (FAILED(src->GetText(0, inst, WMI_OBJ_TEXT_CIM_DTD_2_0, 0, &text)))
                text = 0;
            src->Release();
        }
    }
    inst->Release();
    cls->Release();
    return text;
}

/* The __PATH of the first object the query returns (caller frees). */
static BSTR query_path(const wchar_t* wql)
{
    IWbemClassObject* obj = query_first(wql);
    BSTR path = 0;
    VARIANT v;

    if (!obj)
        return 0;
    VariantInit(&v);
    if (SUCCEEDED(obj->Get(L"__PATH", 0, &v, 0, 0)) && v.vt == VT_BSTR) {
        path = v.bstrVal;
        v.vt = VT_EMPTY;    /* ownership moves to path */
    }
    variant_clear(&v);
    obj->Release();
    return path;
}

/* Waits for a Msvm_ConcreteJob.  0 = completed, else its ErrorCode. */
static DWORD wait_job(const wchar_t* jobPath)
{
    int i;

    for (i = 0; i < 200; i++) {        /* 10 s */
        IWbemClassObject* job = 0;
        int state, code;

        if (FAILED(g_svc->GetObject(Bstr(jobPath), 0, 0, &job, 0)) || !job)
            return (DWORD)E_FAIL;
        state = get_uint(job, L"JobState", 0);
        code = get_uint(job, L"ErrorCode", 0);
        job->Release();
        if (state == 7)                 /* Completed */
            return 0;
        if (state > 7)                  /* Terminated, Killed, Exception */
            return code ? (DWORD)code : (DWORD)state;
        Sleep(50);
    }
    return WAIT_TIMEOUT;
}

/* Msvm_VirtualSystemManagementService.<method>(TargetSystem, DataItems[1]),
 * waiting for the job.  0 = done, else ReturnValue / job error / HRESULT. */
static DWORD kvp_call(BSTR vmPath, const wchar_t* method, BSTR item)
{
    BSTR svcPath;
    IWbemClassObject* cls = 0;
    IWbemClassObject* inSig = 0;
    IWbemClassObject* in = 0;
    IWbemClassObject* out = 0;
    DWORD rc = (DWORD)E_FAIL;
    VARIANT v;
    HRESULT hr;

    svcPath = query_path(L"SELECT * FROM Msvm_VirtualSystemManagementService");
    if (!svcPath)
        return rc;
    if (SUCCEEDED(g_svc->GetObject(Bstr(L"Msvm_VirtualSystemManagementService"), 0, 0, &cls, 0)) && cls) {
        cls->GetMethod(method, 0, &inSig, 0);
        cls->Release();
    }
    if (inSig && SUCCEEDED(inSig->SpawnInstance(0, &in)) && in) {
        SAFEARRAY* sa;

        VariantInit(&v);
        v.vt = VT_BSTR;
        v.bstrVal = SysAllocString(vmPath);
        in->Put(L"TargetSystem", 0, &v, 0);
        variant_clear(&v);

        sa = SafeArrayCreateVector(VT_BSTR, 0, 1);
        if (sa) {
            LONG idx = 0;
            SafeArrayPutElement(sa, &idx, item);    /* copies item */
            VariantInit(&v);
            v.vt = VT_ARRAY | VT_BSTR;
            v.parray = sa;
            in->Put(L"DataItems", 0, &v, 0);        /* copies the array */
            variant_clear(&v);
        }

        /* An instance method: call it on the service object's path. */
        hr = g_svc->ExecMethod(svcPath, Bstr(method), 0, 0, in, &out, 0);
        if (SUCCEEDED(hr) && out) {
            rc = (DWORD)get_uint(out, L"ReturnValue", -1);
            if (rc == 4096) {           /* job started */
                wchar_t job[512];
                rc = get_str(out, L"Job", job, 512) ? wait_job(job) : 0;
            }
        } else {
            rc = (DWORD)hr;
        }
    }
    if (out) out->Release();
    if (in) in->Release();
    if (inSig) inSig->Release();
    SysFreeString(svcPath);
    return rc;
}

static BSTR vm_path(const GUID* vmId)
{
    wchar_t g[48], wql[120];

    guid_to_str(vmId, g, 48);
    wsprintfW(wql, L"SELECT * FROM Msvm_ComputerSystem WHERE Name='%s'", g);
    return query_path(wql);
}

BOOL HvHost_WriteSetting(const GUID* vmId, const wchar_t* key, const wchar_t* value)
{
    wchar_t cur[64];
    BSTR path, item;
    DWORD rc;
    int had;
    BOOL ok;

    if (!g_svc || !vmId || !key || !value)
        return FALSE;
    path = vm_path(vmId);
    item = path ? make_kvp_xml(key, value) : 0;
    if (!item) {
        SysFreeString(path);
        return FALSE;
    }
    had = kvp_lookup(vmId, key, cur, 64);
    rc = kvp_call(path, had ? L"ModifyKvpItems" : L"AddKvpItems", item);
    /* On a running VM the job can end with an error from the guest's KVP
     * integration service although the host-only item is stored: the read
     * back decides. */
    ok = kvp_lookup(vmId, key, cur, 64) && lstrcmpW(cur, value) == 0;
    guid_to_str(vmId, cur, 64);
    Log_Printf(L"KVP %s %s=%s: %s, rc %u, %s", cur, key, value,
               had ? L"modify" : L"add", (unsigned)rc, ok ? L"stored" : L"NOT stored");
    SysFreeString(item);
    SysFreeString(path);
    return ok;
}

BOOL HvHost_RemoveSetting(const GUID* vmId, const wchar_t* key)
{
    wchar_t cur[64];
    BSTR path, item;
    DWORD rc;
    BOOL ok;

    if (!g_svc || !vmId || !key)
        return FALSE;
    if (!kvp_lookup(vmId, key, cur, 64))
        return TRUE;
    path = vm_path(vmId);
    item = path ? make_kvp_xml(key, L"") : 0;
    if (!item) {
        SysFreeString(path);
        return FALSE;
    }
    rc = kvp_call(path, L"RemoveKvpItems", item);
    ok = !kvp_lookup(vmId, key, cur, 64);
    guid_to_str(vmId, cur, 64);
    Log_Printf(L"KVP %s remove %s: rc %u, %s", cur, key, (unsigned)rc,
               ok ? L"removed" : L"NOT removed");
    SysFreeString(item);
    SysFreeString(path);
    return ok;
}

/* ------------------------------------------------------------------ */
/* Integration services                                               */
/* ------------------------------------------------------------------ */

/* OperationalStatus is an array of uint16; 2 = OK.  FALSE if the query
 * failed. */
static BOOL read_ic(IWbemServices* svc, const wchar_t* wql, int* okFlag, int* seenFlag)
{
    IEnumWbemClassObject* en = 0;
    IWbemClassObject* obj = 0;
    ULONG n = 0;

    if (FAILED(svc->ExecQuery(Bstr(L"WQL"), Bstr(wql),
                              WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                              0, &en)) || !en)
        return FALSE;
    while (en->Next(WBEM_INFINITE, 1, &obj, &n) == S_OK && n == 1) {
        VARIANT v;
        *seenFlag = 1;
        VariantInit(&v);
        if (SUCCEEDED(obj->Get(L"OperationalStatus", 0, &v, 0, 0))) {
            if ((v.vt & VT_ARRAY) && v.parray) {
                LONG lo = 0, hi = -1;
                SafeArrayGetLBound(v.parray, 1, &lo);
                SafeArrayGetUBound(v.parray, 1, &hi);
                for (LONG i = lo; i <= hi; i++) {
                    LONG s = 0;
                    if (SUCCEEDED(SafeArrayGetElement(v.parray, &i, &s)) && s == 2)
                        *okFlag = 1;
                }
            } else if (v.vt == VT_I4 || v.vt == VT_UI4 || v.vt == VT_I2 || v.vt == VT_UI2) {
                if (get_uint(obj, L"OperationalStatus", 0) == 2)
                    *okFlag = 1;
            }
        }
        variant_clear(&v);
        obj->Release();
        obj = 0;
    }
    en->Release();
    return TRUE;
}

BOOL HvHost_ReadIntegrationServices(const GUID* vmId, HV_VM* out)
{
    IWbemServices* svc = svc_here();
    wchar_t g[48], wql[200];
    BOOL ok;

    if (!svc || !vmId || !out)
        return FALSE;
    out->IcHeartbeatOk = 0;
    out->IcKvpOk = 0;
    out->IcSeen = 0;
    guid_to_str(vmId, g, 48);

    /* Both components carry SystemName = the VM GUID. */
    wsprintfW(wql,
              L"SELECT * FROM Msvm_HeartbeatComponent WHERE SystemName='%s'",
              g);
    ok = read_ic(svc, wql, &out->IcHeartbeatOk, &out->IcSeen);

    wsprintfW(wql,
              L"SELECT * FROM Msvm_KvpExchangeComponent WHERE SystemName='%s'",
              g);
    ok = read_ic(svc, wql, &out->IcKvpOk, &out->IcSeen) && ok;
    return ok;
}

/* ------------------------------------------------------------------ */
/* List                                                               */
/* ------------------------------------------------------------------ */

int HvHost_ListVms(HV_VM* out, int max)
{
    IEnumWbemClassObject* en = 0;
    IWbemClassObject* obj = 0;
    ULONG n = 0;
    int count = 0;

    if (!g_svc || !out || max <= 0)
        return -1;
    if (FAILED(exec_query(L"SELECT * FROM Msvm_ComputerSystem", &en)) || !en)
        return -1;
    while (count < max && en->Next(WBEM_INFINITE, 1, &obj, &n) == S_OK && n == 1) {
        wchar_t name[64];
        wchar_t caption[64];
        GUID id;
        int haveId = 0;

        get_str(obj, L"Name", name, 64);
        get_str(obj, L"Caption", caption, 64);
        /* The hosting computer system is not a VM. */
        if (lstrcmpW(name, L"Hosting Computer System") == 0) {
            obj->Release();
            obj = 0;
            continue;
        }
        /* Name is the VM GUID, with or without braces (Hyper-V omits them). */
        if (lstrlenW(name) >= 36) {
            wchar_t guidStr[48];
            int n = lstrlenW(name);
            if (n > 44)
                n = 44;
            if (name[0] == L'{') {
                lstrcpynW(guidStr, name, 48);
            } else {
                guidStr[0] = L'{';
                CopyMemory(guidStr + 1, name, (SIZE_T)n * sizeof(wchar_t));
                guidStr[1 + n] = L'}';
                guidStr[2 + n] = 0;
            }
            if (IIDFromString(guidStr, &id) == S_OK)
                haveId = 1;
        }
        if (!haveId) {
            obj->Release();
            obj = 0;
            continue;
        }
        ZeroMemory(&out[count], sizeof(out[count]));
        out[count].Id = id;
        {
            VARIANT v;
            VariantInit(&v);
            if (SUCCEEDED(obj->Get(L"ElementName", 0, &v, 0, 0)) && v.vt == VT_BSTR && v.bstrVal)
                lstrcpynW(out[count].Name, v.bstrVal, 64);
            variant_clear(&v);
        }
        if (!out[count].Name[0])
            lstrcpynW(out[count].Name, name, 64);
        out[count].EnabledState = get_uint(obj, L"EnabledState", 0);
        read_kvp_into(&id, &out[count]);
        count++;
        obj->Release();
        obj = 0;
    }
    en->Release();
    return count;
}

/* ------------------------------------------------------------------ */
/* State-change events                                                */
/* ------------------------------------------------------------------ */

class EventSink : public IWbemObjectSink {
public:
    LONG refs;
    HWND hwnd;
    UINT msg;

    EventSink() : refs(0), hwnd(0), msg(0) {}

    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void** ppv) {
        if (!ppv)
            return E_POINTER;
        if (riid == IID_IUnknown || riid == IID_IWbemObjectSink) {
            *ppv = static_cast<IWbemObjectSink*>(this);
            AddRef();
            return S_OK;
        }
        *ppv = 0;
        return E_NOINTERFACE;
    }
    ULONG STDMETHODCALLTYPE AddRef(void) {
        return (ULONG)InterlockedIncrement(&refs);
    }
    ULONG STDMETHODCALLTYPE Release(void) {
        /* Static instance; never driven to zero. */
        return (ULONG)InterlockedDecrement(&refs);
    }
    /* On a WMI thread.  The UI re-lists the VMs, so which one changed does
     * not matter. */
    HRESULT STDMETHODCALLTYPE Indicate(LONG count, IWbemClassObject** objs) {
        (void)objs;
        if (count > 0 && hwnd)
            PostMessageW(hwnd, msg, 0, 0);
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE SetStatus(LONG, HRESULT, BSTR, IWbemClassObject*) {
        return S_OK;
    }
};

static EventSink g_eventSink;

BOOL HvHost_StartEvents(HWND hwnd, UINT msg)
{
    HRESULT hr;

    if (!g_svc)
        return FALSE;
    if (g_sink) {
        g_eventSink.hwnd = hwnd;
        g_eventSink.msg = msg;
        return TRUE;
    }
    g_eventSink.hwnd = hwnd;
    g_eventSink.msg = msg;
    g_eventSink.AddRef();
    hr = g_svc->ExecNotificationQueryAsync(
        Bstr(L"WQL"),
        /* Only state changes: other properties (OnTimeInMilliseconds)
         * change all the time. */
        Bstr(L"SELECT * FROM __InstanceModificationEvent WITHIN 2 "
             L"WHERE TargetInstance ISA 'Msvm_ComputerSystem' "
             L"AND TargetInstance.EnabledState <> PreviousInstance.EnabledState"),
        0, 0, &g_eventSink);
    if (FAILED(hr)) {
        g_eventSink.Release();
        return FALSE;
    }
    g_sink = &g_eventSink;
    return TRUE;
}

void HvHost_StopEvents(void)
{
    if (g_svc && g_sink) {
        g_svc->CancelAsyncCall(g_sink);
        g_sink = 0;
    }
}
