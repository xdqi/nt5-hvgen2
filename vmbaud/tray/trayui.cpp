/*
 * trayui.cpp: tray icon, settings dialog, per-VM session ownership.
 *
 * The settings dialog lists the VMs in three collapsible groups: sound on,
 * running, off.  The row's checkbox switches sound on (host-only KVP
 * vmbaud.enabled); the volume trackbar and mute act on the selected VM.
 *
 * Closing the settings window only hides it; Exit lives in the tray menu and
 * tears every pipe down (the sound card disappears from the guests).
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#include <windows.h>
#include <shellapi.h>
#include <commctrl.h>
#include <taskschd.h>
#include <oleauto.h>

#include "trayui.h"
#include "hvhost.h"
#include "vmsession.h"
#include "log.h"
#include "resource.h"

#define WM_TRAY         (WM_APP + 10)
#define WM_APP_RELIST   (WM_APP + 11)   /* dialog: regroup; wParam = uid to select */

static const wchar_t kWndClass[] = L"vmbaudtray.main";
static const wchar_t kTaskName[] = L"vmbaudtray";

enum { kGroupSound = 1, kGroupRunning = 2, kGroupOff = 3 };
enum { kMaxVms = 64 };

static HINSTANCE g_inst;
static HWND g_mainWnd;
static HWND g_dlg;
static HWND g_list;
static HWND g_volume;
static HWND g_mute;
static HWND g_autostart;
static HICON g_iconBig;
static HICON g_iconSmall;
static NOTIFYICONDATAW g_nid;
static int g_haveNid;
static UINT g_taskbarCreated;

typedef struct _UI_VM {
    HV_VM        vm;
    int          uid;           /* ListView lParam, stable while the VM exists */
    VM_SESSION*  session;
    UINT_PTR     cookie;        /* identifies session in WM_APP_STATUS */
    int          code;          /* last kVs* status, 0 = none */
    DWORD        err;
    int          kvpVolume;     /* last value read from / written to the KVP */
    int          kvpMute;
} UI_VM;

static UI_VM g_vms[kMaxVms];    /* sorted by name */
static int   g_vmCount;
static int   g_nextUid = 1;
static UINT_PTR g_nextCookie = 1;
static int   g_inFill;          /* list changes made by us, not by the user */

/* ------------------------------------------------------------------ */
/* Helpers                                                            */
/* ------------------------------------------------------------------ */

static void load_str(UINT id, wchar_t* buf, int cap)
{
    if (!LoadStringW(g_inst, id, buf, cap) && cap > 0)
        buf[0] = 0;
}

static int find_uid(int uid)
{
    int i;
    for (i = 0; i < g_vmCount; i++) {
        if (g_vms[i].uid == uid)
            return i;
    }
    return -1;
}

/* Msvm_ComputerSystem.EnabledState: 2 Running, 3 Off, 4 Stopping, 6 Saved,
 * 9 Paused, 10 Starting (plus the 327xx values older hosts report). */
static int vm_is_off(int state)
{
    return state == 0 || state == 3 || state == 6 || state == 32769;
}

static UINT state_string(int state)
{
    switch (state) {
    case 2:     return IDS_STATE_RUNNING;
    case 3:     return IDS_STATE_OFF;
    case 6:
    case 32769: return IDS_STATE_SAVED;
    case 9:
    case 32768: return IDS_STATE_PAUSED;
    case 10:
    case 32770: return IDS_STATE_STARTING;
    case 4:
    case 32774: return IDS_STATE_STOPPING;
    default:    return IDS_STATE_OTHER;
    }
}

static int vm_group(const UI_VM* v)
{
    if (v->vm.SoundEnabled)
        return kGroupSound;
    return vm_is_off(v->vm.EnabledState) ? kGroupOff : kGroupRunning;
}

static int vm_playing(const UI_VM* v)
{
    return v->session && (v->code == kVsPlaying || v->code == kVsNoDevice);
}

static void status_text(const UI_VM* v, wchar_t* buf, int cap)
{
    wchar_t fmt[128];
    UINT id;

    if (!v->vm.SoundEnabled) {
        load_str(IDS_ST_OFF, buf, cap);
        return;
    }
    if (!v->session && v->code != kVsError) {
        load_str(IDS_ST_WAITVM, buf, cap);
        return;
    }
    switch (v->code) {
    case kVsWaitIc:      id = IDS_ST_WAITIC; break;
    case kVsOffering:    id = IDS_ST_OFFERING; break;
    case kVsOfferFailed: id = IDS_ST_OFFERFAIL; break;
    case kVsConnected:   id = IDS_ST_CONNECTED; break;
    case kVsPlaying:     id = IDS_ST_PLAYING; break;
    case kVsNoDevice:    id = IDS_ST_NODEVICE; break;
    case kVsReoffer:     id = IDS_ST_REOFFER; break;
    case kVsError:       id = IDS_ST_ERROR; break;
    case kVsBadFormat:   id = IDS_ST_BADFORMAT; break;
    default:             id = IDS_ST_STARTING; break;
    }
    load_str(id, fmt, 128);
    if (id == IDS_ST_OFFERFAIL || id == IDS_ST_ERROR || id == IDS_ST_REOFFER
        || id == IDS_ST_BADFORMAT || id == IDS_ST_PLAYING)
        wsprintfW(buf, fmt, (unsigned)v->err);
    else
        lstrcpynW(buf, fmt, cap);
}

static void write_kvp(const GUID* id, const wchar_t* key, int value)
{
    wchar_t buf[16];
    wsprintfW(buf, L"%d", value);
    HvHost_WriteSetting(id, key, buf);
}

/* ------------------------------------------------------------------ */
/* Sessions                                                           */
/* ------------------------------------------------------------------ */

static void stop_all_sessions(void)
{
    int i;
    for (i = 0; i < g_vmCount; i++) {
        if (g_vms[i].session) {
            VmSession_Stop(g_vms[i].session);
            g_vms[i].session = 0;
        }
    }
}

/* A session runs while sound is on and the VM is not off: a paused VM keeps
 * its channel, so the guest's device does not go away and come back. */
static void sync_sessions(void)
{
    int i;
    for (i = 0; i < g_vmCount; i++) {
        UI_VM* v = &g_vms[i];
        int want = v->vm.SoundEnabled && !vm_is_off(v->vm.EnabledState);

        if (want && !v->session) {
            v->cookie = g_nextCookie++;
            v->session = VmSession_Start(&v->vm, g_mainWnd, WM_APP_STATUS, v->cookie);
            v->code = v->session ? kVsStarting : kVsError;
            v->err = v->session ? 0 : ERROR_NOT_ENOUGH_MEMORY;
        } else if (!want) {
            if (v->session) {
                VmSession_Stop(v->session);
                v->session = 0;
            }
            v->code = 0;
            v->err = 0;
        } else {
            VmSession_SetVolume(v->session, v->vm.Volume, v->vm.Mute);
        }
    }
}

static void update_tooltip(void)
{
    wchar_t tip[128];
    int i, n = 0;

    for (i = 0; i < g_vmCount; i++) {
        if (vm_playing(&g_vms[i]))
            n++;
    }
    if (n == 0)
        load_str(IDS_TIP_NONE, tip, 128);
    else if (n == 1)
        load_str(IDS_TIP_ONE, tip, 128);
    else {
        wchar_t fmt[64];
        load_str(IDS_TIP_MANY, fmt, 64);
        wsprintfW(tip, fmt, n);
    }
    if (g_haveNid && lstrcmpW(tip, g_nid.szTip) != 0) {
        lstrcpynW(g_nid.szTip, tip, (int)(sizeof(g_nid.szTip) / sizeof(g_nid.szTip[0])));
        g_nid.uFlags = NIF_TIP;
        Shell_NotifyIconW(NIM_MODIFY, &g_nid);
    }
}

/* ------------------------------------------------------------------ */
/* ListView                                                           */
/* ------------------------------------------------------------------ */

static void list_add_group(int id, int collapsed)
{
    LVGROUP g;

    ZeroMemory(&g, sizeof(g));
    g.cbSize = sizeof(g);
    g.mask = LVGF_HEADER | LVGF_GROUPID | LVGF_STATE;
    g.iGroupId = id;
    g.pszHeader = (LPWSTR)L"";
    g.stateMask = LVGS_COLLAPSIBLE | LVGS_COLLAPSED;
    g.state = LVGS_COLLAPSIBLE | (collapsed ? LVGS_COLLAPSED : 0);
    ListView_InsertGroup(g_list, -1, &g);
}

/* Dialog units to pixels for the settings dialog (follows its DPI). */
static int dux(int n)
{
    RECT r = { 0, 0, n, 0 };
    MapDialogRect(g_dlg, &r);
    return r.right;
}

static int duy(int n)
{
    RECT r = { 0, 0, 0, n };
    MapDialogRect(g_dlg, &r);
    return r.bottom;
}

/* The status column takes what the name and state columns leave. */
static void list_fit_columns(void)
{
    RECT rc;
    int w;

    GetClientRect(g_list, &rc);
    w = rc.right - ListView_GetColumnWidth(g_list, 0) - ListView_GetColumnWidth(g_list, 1);
    if (!(GetWindowLongW(g_list, GWL_STYLE) & WS_VSCROLL))
        w -= GetSystemMetrics(SM_CXVSCROLL);    /* room for it when it comes */
    if (w < dux(60))
        w = dux(60);
    ListView_SetColumnWidth(g_list, 2, w);
}

static void list_init(void)
{
    static const UINT cols[3] = { IDS_COL_NAME, IDS_COL_STATE, IDS_COL_STATUS };
    static const int widths[3] = { 110, 55, 100 };  /* dialog units */
    wchar_t text[64];
    LVCOLUMNW col;
    int i;

    ListView_SetExtendedListViewStyle(g_list,
        LVS_EX_CHECKBOXES | LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER);

    ZeroMemory(&col, sizeof(col));
    col.mask = LVCF_TEXT | LVCF_WIDTH | LVCF_SUBITEM;
    for (i = 0; i < 3; i++) {
        load_str(cols[i], text, 64);
        col.iSubItem = i;
        col.cx = dux(widths[i]);
        col.pszText = text;
        ListView_InsertColumn(g_list, i, &col);
    }
    list_fit_columns();

    ListView_EnableGroupView(g_list, TRUE);
    list_add_group(kGroupSound, 0);
    list_add_group(kGroupRunning, 0);
    list_add_group(kGroupOff, 1);
}

static void list_set_group_count(int id, int count)
{
    static const UINT names[4] = { 0, IDS_GRP_SOUND, IDS_GRP_RUNNING, IDS_GRP_OFF };
    wchar_t fmt[64], text[80];
    LVGROUP g;

    load_str(names[id], fmt, 64);
    wsprintfW(text, fmt, count);
    ZeroMemory(&g, sizeof(g));
    g.cbSize = sizeof(g);
    g.mask = LVGF_HEADER;
    g.pszHeader = text;
    ListView_SetGroupInfo(g_list, id, &g);
}

static int list_find(int uid)
{
    LVFINDINFOW fi;

    ZeroMemory(&fi, sizeof(fi));
    fi.flags = LVFI_PARAM;
    fi.lParam = uid;
    return ListView_FindItem(g_list, -1, &fi);
}

/* Inserts or updates the row of g_vms[idx]. */
static void list_sync_item(int idx)
{
    UI_VM* v = &g_vms[idx];
    wchar_t text[128];
    LVITEMW it;
    int item;

    if (!g_list)
        return;
    g_inFill++;
    item = list_find(v->uid);
    ZeroMemory(&it, sizeof(it));
    it.mask = LVIF_TEXT | LVIF_GROUPID;
    it.pszText = v->vm.Name;
    it.iGroupId = vm_group(v);
    if (item < 0) {
        it.mask |= LVIF_PARAM;
        it.iItem = ListView_GetItemCount(g_list);
        it.lParam = v->uid;
        item = ListView_InsertItem(g_list, &it);
    } else {
        it.iItem = item;
        ListView_SetItem(g_list, &it);
    }
    if (item >= 0) {
        ListView_SetCheckState(g_list, item, v->vm.SoundEnabled);
        load_str(state_string(v->vm.EnabledState), text, 128);
        ListView_SetItemText(g_list, item, 1, text);
        status_text(v, text, 128);
        ListView_SetItemText(g_list, item, 2, text);
    }
    g_inFill--;
}

static int CALLBACK list_compare(LPARAM a, LPARAM b, LPARAM)
{
    return find_uid((int)a) - find_uid((int)b);
}

static void list_fill(void)
{
    int counts[4] = { 0, 0, 0, 0 };
    int i;

    if (!g_list)
        return;
    g_inFill++;
    SendMessageW(g_list, WM_SETREDRAW, FALSE, 0);

    /* Rows of VMs that are gone. */
    for (i = ListView_GetItemCount(g_list) - 1; i >= 0; i--) {
        LVITEMW it;
        ZeroMemory(&it, sizeof(it));
        it.mask = LVIF_PARAM;
        it.iItem = i;
        if (ListView_GetItem(g_list, &it) && find_uid((int)it.lParam) < 0)
            ListView_DeleteItem(g_list, i);
    }
    for (i = 0; i < g_vmCount; i++) {
        list_sync_item(i);
        counts[vm_group(&g_vms[i])]++;
    }
    for (i = kGroupSound; i <= kGroupOff; i++)
        list_set_group_count(i, counts[i]);
    /* Within a group, rows are in item order: keep it the name order. */
    ListView_SortItems(g_list, list_compare, 0);

    SendMessageW(g_list, WM_SETREDRAW, TRUE, 0);
    InvalidateRect(g_list, 0, TRUE);
    g_inFill--;
}

static int selected_vm(void)
{
    LVITEMW it;
    int item;

    if (!g_list)
        return -1;
    item = ListView_GetNextItem(g_list, -1, LVNI_SELECTED);
    if (item < 0)
        return -1;
    ZeroMemory(&it, sizeof(it));
    it.mask = LVIF_PARAM;
    it.iItem = item;
    if (!ListView_GetItem(g_list, &it))
        return -1;
    return find_uid((int)it.lParam);
}

static void select_uid(int uid)
{
    int item = list_find(uid);
    if (item < 0)
        return;
    ListView_SetItemState(g_list, item, LVIS_SELECTED | LVIS_FOCUSED,
                          LVIS_SELECTED | LVIS_FOCUSED);
    ListView_EnsureVisible(g_list, item, FALSE);
}

/* ------------------------------------------------------------------ */
/* Volume panel                                                       */
/* ------------------------------------------------------------------ */

static void volume_value_text(int volume)
{
    wchar_t text[16];
    wsprintfW(text, L"%d%%", volume);
    SetDlgItemTextW(g_dlg, IDC_VOL_VALUE, text);
}

static void volume_panel_update(void)
{
    int idx = selected_vm();
    int on = (idx >= 0 && g_vms[idx].vm.SoundEnabled);
    wchar_t text[160];

    if (!g_dlg)
        return;
    if (on) {
        wchar_t fmt[64];
        load_str(IDS_VOLUME_FOR, fmt, 64);
        wsprintfW(text, fmt, g_vms[idx].vm.Name);
    } else {
        load_str(IDS_VOLUME_HINT, text, 160);
    }
    SetDlgItemTextW(g_dlg, IDC_VOL_LABEL, text);
    EnableWindow(g_volume, on);
    EnableWindow(g_mute, on);
    EnableWindow(GetDlgItem(g_dlg, IDC_VOL_VALUE), on);
    if (on) {
        SendMessageW(g_volume, TBM_SETPOS, TRUE, g_vms[idx].vm.Volume);
        CheckDlgButton(g_dlg, IDC_MUTE, g_vms[idx].vm.Mute ? BST_CHECKED : BST_UNCHECKED);
        volume_value_text(g_vms[idx].vm.Volume);
    } else {
        SetDlgItemTextW(g_dlg, IDC_VOL_VALUE, L"");
    }
}

/* Applies to the running session at once; the KVP is written when asked
 * and only if it differs from what is stored. */
static void apply_volume(int idx, int volume, int mute, int writeKvp)
{
    UI_VM* v = &g_vms[idx];

    v->vm.Volume = volume;
    v->vm.Mute = mute;
    if (v->session)
        VmSession_SetVolume(v->session, volume, mute);
    if (writeKvp && volume != v->kvpVolume) {
        write_kvp(&v->vm.Id, L"vmbaud.volume", volume);
        v->kvpVolume = volume;
    }
    if (writeKvp && mute != v->kvpMute) {
        write_kvp(&v->vm.Id, L"vmbaud.mute", mute);
        v->kvpMute = mute;
    }
}

static void set_sound(int idx, int on)
{
    Log_Printf(L"[%s] sound switched %s", g_vms[idx].vm.Name, on ? L"on" : L"off");
    g_vms[idx].vm.SoundEnabled = on;
    write_kvp(&g_vms[idx].vm.Id, L"vmbaud.enabled", on);
    sync_sessions();
    update_tooltip();
}

/* ------------------------------------------------------------------ */
/* VM list                                                            */
/* ------------------------------------------------------------------ */

static void refresh_vms(void)
{
    /* UI thread only; static to keep ~25 KB off the stack. */
    static HV_VM fresh[kMaxVms];
    static UI_VM next[kMaxVms];
    int n, i, j;

    n = HvHost_ListVms(fresh, kMaxVms);
    if (n < 0)
        return;     /* WMI hiccup: keep what we have */

    for (i = 1; i < n; i++) {
        HV_VM t = fresh[i];
        for (j = i; j > 0 && lstrcmpiW(fresh[j - 1].Name, t.Name) > 0; j--)
            fresh[j] = fresh[j - 1];
        fresh[j] = t;
    }

    for (i = 0; i < n; i++) {
        UI_VM* old = 0;
        for (j = 0; j < g_vmCount; j++) {
            if (g_vms[j].uid && IsEqualGUID(g_vms[j].vm.Id, fresh[i].Id)) {
                old = &g_vms[j];
                break;
            }
        }
        if (old) {
            if (old->vm.EnabledState != fresh[i].EnabledState
                || old->vm.SoundEnabled != fresh[i].SoundEnabled)
                Log_Printf(L"[%s] state %d -> %d, sound %d", fresh[i].Name,
                           old->vm.EnabledState, fresh[i].EnabledState,
                           fresh[i].SoundEnabled);
            if (old->vm.EnabledState != fresh[i].EnabledState && old->session)
                VmSession_NotifyStateChange(old->session);
            next[i] = *old;
            old->uid = 0;       /* taken over */
        } else {
            if (fresh[i].SoundEnabled)
                Log_Printf(L"[%s] state %d, sound on", fresh[i].Name,
                           fresh[i].EnabledState);
            ZeroMemory(&next[i], sizeof(next[i]));
            next[i].uid = g_nextUid++;
        }
        next[i].vm = fresh[i];
        next[i].kvpVolume = fresh[i].Volume;
        next[i].kvpMute = fresh[i].Mute;
    }
    /* VMs that were deleted. */
    for (j = 0; j < g_vmCount; j++) {
        if (g_vms[j].uid && g_vms[j].session)
            VmSession_Stop(g_vms[j].session);
    }
    CopyMemory(g_vms, next, (SIZE_T)n * sizeof(UI_VM));
    g_vmCount = n;

    sync_sessions();
    list_fill();
    volume_panel_update();
    update_tooltip();
}

/* ------------------------------------------------------------------ */
/* Autostart (Task Scheduler)                                         */
/* ------------------------------------------------------------------ */

static ITaskService* task_service(void)
{
    ITaskService* svc = 0;
    VARIANT empty;
    HRESULT hr;

    hr = CoCreateInstance(CLSID_TaskScheduler, 0, CLSCTX_INPROC_SERVER,
                          IID_ITaskService, (void**)&svc);
    if (FAILED(hr) || !svc)
        return 0;
    VariantInit(&empty);
    hr = svc->Connect(empty, empty, empty, empty);
    if (FAILED(hr)) {
        svc->Release();
        return 0;
    }
    return svc;
}

static ITaskFolder* task_root(ITaskService* svc)
{
    ITaskFolder* folder = 0;
    BSTR root = SysAllocString(L"\\");

    if (FAILED(svc->GetFolder(root, &folder)))
        folder = 0;
    SysFreeString(root);
    return folder;
}

BOOL TrayUi_Autostart(void)
{
    ITaskService* svc = task_service();
    ITaskFolder* folder;
    IRegisteredTask* task = 0;
    BOOL on = FALSE;

    if (!svc)
        return FALSE;
    folder = task_root(svc);
    if (folder) {
        BSTR name = SysAllocString(kTaskName);
        if (SUCCEEDED(folder->GetTask(name, &task)) && task) {
            on = TRUE;
            task->Release();
        }
        SysFreeString(name);
        folder->Release();
    }
    svc->Release();
    return on;
}

/* Logon trigger for the current user, run with highest privileges: the
 * elevated tray starts without a UAC prompt. */
static HRESULT register_task(ITaskService* svc, ITaskFolder* folder, BSTR name)
{
    ITaskDefinition* def = 0;
    IRegistrationInfo* info = 0;
    IPrincipal* principal = 0;
    ITaskSettings* settings = 0;
    ITriggerCollection* triggers = 0;
    ITrigger* trigger = 0;
    IActionCollection* actions = 0;
    IAction* action = 0;
    IExecAction* exec = 0;
    IRegisteredTask* task = 0;
    wchar_t path[MAX_PATH];
    BSTR b;
    VARIANT empty;
    HRESULT hr;

    hr = svc->NewTask(0, &def);
    if (FAILED(hr) || !def)
        return FAILED(hr) ? hr : E_FAIL;

    if (SUCCEEDED(def->get_RegistrationInfo(&info)) && info) {
        b = SysAllocString(L"nt5-hvgen2");
        info->put_Author(b);
        SysFreeString(b);
        info->Release();
    }
    if (SUCCEEDED(def->get_Principal(&principal)) && principal) {
        principal->put_LogonType(TASK_LOGON_INTERACTIVE_TOKEN);
        principal->put_RunLevel(TASK_RUNLEVEL_HIGHEST);
        principal->Release();
    }
    if (SUCCEEDED(def->get_Settings(&settings)) && settings) {
        /* Runs for the whole session; on battery too. */
        b = SysAllocString(L"PT0S");
        settings->put_ExecutionTimeLimit(b);
        SysFreeString(b);
        settings->put_DisallowStartIfOnBatteries(VARIANT_FALSE);
        settings->put_StopIfGoingOnBatteries(VARIANT_FALSE);
        settings->Release();
    }
    if (SUCCEEDED(def->get_Triggers(&triggers)) && triggers) {
        if (SUCCEEDED(triggers->Create(TASK_TRIGGER_LOGON, &trigger)) && trigger) {
            ILogonTrigger* logon = 0;
            if (SUCCEEDED(trigger->QueryInterface(IID_ILogonTrigger, (void**)&logon)) && logon) {
                wchar_t user[256];
                DWORD cap = 256;
                if (GetUserNameW(user, &cap)) {
                    b = SysAllocString(user);
                    logon->put_UserId(b);
                    SysFreeString(b);
                }
                logon->Release();
            }
            trigger->Release();
        }
        triggers->Release();
    }
    GetModuleFileNameW(0, path, MAX_PATH);
    if (SUCCEEDED(def->get_Actions(&actions)) && actions) {
        if (SUCCEEDED(actions->Create(TASK_ACTION_EXEC, &action)) && action) {
            if (SUCCEEDED(action->QueryInterface(IID_IExecAction, (void**)&exec)) && exec) {
                b = SysAllocString(path);
                exec->put_Path(b);
                SysFreeString(b);
                exec->Release();
            }
            action->Release();
        }
        actions->Release();
    }

    VariantInit(&empty);
    hr = folder->RegisterTaskDefinition(name, def, TASK_CREATE_OR_UPDATE,
                                        empty, empty, TASK_LOGON_INTERACTIVE_TOKEN,
                                        empty, &task);
    if (task)
        task->Release();
    def->Release();
    return hr;
}

BOOL TrayUi_SetAutostart(BOOL on)
{
    ITaskService* svc = task_service();
    ITaskFolder* folder;
    BSTR name;
    HRESULT hr = E_FAIL;

    if (!svc)
        return FALSE;
    folder = task_root(svc);
    if (folder) {
        name = SysAllocString(kTaskName);
        if (on)
            hr = register_task(svc, folder, name);
        else {
            hr = folder->DeleteTask(name, 0);
            if (hr == HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND))
                hr = S_OK;
        }
        SysFreeString(name);
        folder->Release();
    }
    svc->Release();
    return SUCCEEDED(hr);
}

/* ------------------------------------------------------------------ */
/* Dialog                                                             */
/* ------------------------------------------------------------------ */

/* Lays the dialog out for its client size, in dialog units as in
 * vmbaudtray.rc: the list takes what the volume box and the autostart
 * checkbox at the bottom leave. */
static void dialog_layout(void)
{
    RECT c;
    int w, h, autoTop, boxTop, boxBottom;
    HDWP dwp;

    GetClientRect(g_dlg, &c);
    w = c.right;
    h = c.bottom;
    autoTop = h - duy(7 + 11);
    boxBottom = autoTop - duy(8);
    boxTop = boxBottom - duy(34);

    dwp = BeginDeferWindowPos(6);
#define PLACE(id, x, y, cx, cy) \
    dwp = DeferWindowPos(dwp, GetDlgItem(g_dlg, id), 0, x, y, cx, cy, \
                         SWP_NOZORDER | SWP_NOACTIVATE)
    PLACE(IDC_LIST, dux(7), duy(7), w - dux(14), boxTop - duy(5) - duy(7));
    PLACE(IDC_VOL_LABEL, dux(7), boxTop, w - dux(14), duy(34));
    PLACE(IDC_VOLUME, dux(12), boxTop + duy(13), w - dux(12 + 82), duy(14));
    PLACE(IDC_VOL_VALUE, w - dux(80), boxTop + duy(16), dux(24), duy(9));
    PLACE(IDC_MUTE, w - dux(50), boxTop + duy(15), dux(38), duy(10));
    PLACE(IDC_AUTOSTART, dux(7), autoTop, dux(200), duy(10));
#undef PLACE
    if (dwp)
        EndDeferWindowPos(dwp);
    list_fit_columns();
    /* Group boxes leave trails when they move. */
    RedrawWindow(g_dlg, 0, 0, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN);
}

static void dialog_init(HWND dlg)
{
    wchar_t text[64];

    g_dlg = dlg;
    g_list = GetDlgItem(dlg, IDC_LIST);
    g_volume = GetDlgItem(dlg, IDC_VOLUME);
    g_mute = GetDlgItem(dlg, IDC_MUTE);
    g_autostart = GetDlgItem(dlg, IDC_AUTOSTART);

    /* A dialog has no class icon: without these the caption and the taskbar
     * show the generic one. */
    SendMessageW(dlg, WM_SETICON, ICON_BIG, (LPARAM)g_iconBig);
    SendMessageW(dlg, WM_SETICON, ICON_SMALL, (LPARAM)g_iconSmall);

    load_str(IDS_APP_TITLE, text, 64);
    SetWindowTextW(dlg, text);
    load_str(IDS_MUTE, text, 64);
    SetWindowTextW(g_mute, text);
    load_str(IDS_MENU_AUTOSTART, text, 64);
    SetWindowTextW(g_autostart, text);

    SendMessageW(g_volume, TBM_SETRANGE, FALSE, MAKELONG(0, 100));
    SendMessageW(g_volume, TBM_SETTICFREQ, 10, 0);
    SendMessageW(g_volume, TBM_SETPAGESIZE, 0, 10);

    list_init();
}

static INT_PTR CALLBACK dialog_proc(HWND dlg, UINT msg, WPARAM wParam, LPARAM lParam)
{
    switch (msg) {
    case WM_INITDIALOG:
        dialog_init(dlg);
        return TRUE;

    case WM_SIZE:
        if (g_list && wParam != SIZE_MINIMIZED)
            dialog_layout();
        return FALSE;

    case WM_GETMINMAXINFO: {
        /* No smaller than the template. */
        MINMAXINFO* mm = (MINMAXINFO*)lParam;
        RECT r = { 0, 0, 300, 222 };
        MapDialogRect(dlg, &r);
        AdjustWindowRectEx(&r, (DWORD)GetWindowLongW(dlg, GWL_STYLE), FALSE,
                           (DWORD)GetWindowLongW(dlg, GWL_EXSTYLE));
        mm->ptMinTrackSize.x = r.right - r.left;
        mm->ptMinTrackSize.y = r.bottom - r.top;
        return TRUE;
    }

    case WM_APP_RELIST:
        list_fill();
        select_uid((int)wParam);
        volume_panel_update();
        return TRUE;

    case WM_NOTIFY: {
        NMHDR* hdr = (NMHDR*)lParam;
        if (hdr->idFrom == IDC_LIST && hdr->code == LVN_ITEMCHANGED && !g_inFill) {
            NMLISTVIEW* lv = (NMLISTVIEW*)lParam;
            UINT changed = lv->uNewState ^ lv->uOldState;
            if (!(lv->uChanged & LVIF_STATE))
                return FALSE;
            if ((changed & LVIS_STATEIMAGEMASK) && lv->iItem >= 0) {
                /* The checkbox: state image 2 = checked. */
                int idx = find_uid((int)lv->lParam);
                int on = ((lv->uNewState & LVIS_STATEIMAGEMASK) >> 12) == 2;
                if (idx >= 0 && on != g_vms[idx].vm.SoundEnabled) {
                    set_sound(idx, on);
                    /* Moving the row to its new group inside this
                     * notification confuses the ListView: do it after. */
                    PostMessageW(dlg, WM_APP_RELIST, (WPARAM)g_vms[idx].uid, 0);
                }
            }
            if (changed & LVIS_SELECTED)
                volume_panel_update();
        }
        return FALSE;
    }

    case WM_HSCROLL:
        if ((HWND)lParam == g_volume) {
            int idx = selected_vm();
            if (idx >= 0) {
                int pos = (int)SendMessageW(g_volume, TBM_GETPOS, 0, 0);
                /* Dragging changes the session live; the KVP is written
                 * once the thumb is released (and on keyboard steps). */
                apply_volume(idx, pos, g_vms[idx].vm.Mute,
                             LOWORD(wParam) != TB_THUMBTRACK);
                volume_value_text(pos);
            }
            return TRUE;
        }
        return FALSE;

    case WM_COMMAND:
        switch (LOWORD(wParam)) {
        case IDC_MUTE: {
            int idx = selected_vm();
            if (idx >= 0)
                apply_volume(idx, g_vms[idx].vm.Volume,
                             IsDlgButtonChecked(dlg, IDC_MUTE) == BST_CHECKED, TRUE);
            return TRUE;
        }
        case IDC_AUTOSTART: {
            BOOL on = IsDlgButtonChecked(dlg, IDC_AUTOSTART) == BST_CHECKED;
            if (!TrayUi_SetAutostart(on))
                CheckDlgButton(dlg, IDC_AUTOSTART, on ? BST_UNCHECKED : BST_CHECKED);
            return TRUE;
        }
        case IDCANCEL:      /* Esc */
            ShowWindow(dlg, SW_HIDE);
            return TRUE;
        }
        return FALSE;

    case WM_CLOSE:
        ShowWindow(dlg, SW_HIDE);
        return TRUE;

    default:
        return FALSE;
    }
}

void TrayUi_Show(void)
{
    if (!g_dlg)
        return;
    /* Picks up KVP edits made from outside, and fresh VM states. */
    refresh_vms();
    if (selected_vm() < 0) {
        /* Start on a VM whose volume can be set. */
        int i;
        for (i = 0; i < g_vmCount; i++) {
            if (g_vms[i].vm.SoundEnabled) {
                select_uid(g_vms[i].uid);
                break;
            }
        }
        volume_panel_update();
    }
    CheckDlgButton(g_dlg, IDC_AUTOSTART,
                   TrayUi_Autostart() ? BST_CHECKED : BST_UNCHECKED);
    if (IsIconic(g_dlg))
        ShowWindow(g_dlg, SW_RESTORE);
    else
        ShowWindow(g_dlg, SW_SHOW);
    SetForegroundWindow(g_dlg);
}

/* ------------------------------------------------------------------ */
/* Tray + main window                                                 */
/* ------------------------------------------------------------------ */

static void tray_add(void)
{
    ZeroMemory(&g_nid, sizeof(g_nid));
    g_nid.cbSize = sizeof(g_nid);
    g_nid.hWnd = g_mainWnd;
    g_nid.uID = 1;
    g_nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    g_nid.uCallbackMessage = WM_TRAY;
    g_nid.hIcon = g_iconSmall;
    load_str(IDS_TIP_NONE, g_nid.szTip, (int)(sizeof(g_nid.szTip) / sizeof(g_nid.szTip[0])));
    g_haveNid = Shell_NotifyIconW(NIM_ADD, &g_nid);
}

static void tray_menu(HWND hwnd)
{
    HMENU menu = CreatePopupMenu();
    POINT pt;
    wchar_t text[64];
    UINT cmd;

    load_str(IDS_MENU_SETTINGS, text, 64);
    AppendMenuW(menu, MF_STRING, 1, text);
    SetMenuDefaultItem(menu, 1, FALSE);
    load_str(IDS_MENU_AUTOSTART, text, 64);
    AppendMenuW(menu, MF_STRING | (TrayUi_Autostart() ? MF_CHECKED : 0), 2, text);
    AppendMenuW(menu, MF_SEPARATOR, 0, 0);
    load_str(IDS_MENU_EXIT, text, 64);
    AppendMenuW(menu, MF_STRING, 3, text);

    GetCursorPos(&pt);
    SetForegroundWindow(hwnd);
    cmd = (UINT)TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
                               pt.x, pt.y, 0, hwnd, 0);
    DestroyMenu(menu);
    if (cmd == 1)
        TrayUi_Show();
    else if (cmd == 2)
        TrayUi_SetAutostart(!TrayUi_Autostart());
    else if (cmd == 3)
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
}

static LRESULT CALLBACK main_proc(HWND hwnd, UINT msg, WPARAM wParam, LPARAM lParam)
{
    switch (msg) {
    case WM_TRAY:
        if (LOWORD(lParam) == WM_LBUTTONUP)
            TrayUi_Show();
        else if (LOWORD(lParam) == WM_RBUTTONUP)
            tray_menu(hwnd);
        return 0;

    case WM_APP_SHOW:
        TrayUi_Show();
        return 0;

    case WM_APP_STATUS: {
        int i;
        for (i = 0; i < g_vmCount; i++) {
            /* A stopped session's last posts carry an old cookie. */
            if (g_vms[i].session && g_vms[i].cookie == (UINT_PTR)wParam) {
                g_vms[i].code = VS_CODE(lParam);
                g_vms[i].err = VS_ERR(lParam);
                list_sync_item(i);
                update_tooltip();
                break;
            }
        }
        return 0;
    }

    case WM_APP_VMUPDATE:
        refresh_vms();
        return 0;

    case WM_CLOSE:
        DestroyWindow(hwnd);
        return 0;

    case WM_DESTROY:
        stop_all_sessions();
        if (g_haveNid) {
            Shell_NotifyIconW(NIM_DELETE, &g_nid);
            g_haveNid = 0;
        }
        PostQuitMessage(0);
        return 0;

    default:
        if (msg == g_taskbarCreated && g_taskbarCreated) {
            /* Explorer restarted: the icon is gone. */
            tray_add();
            update_tooltip();
            return 0;
        }
        return DefWindowProcW(hwnd, msg, wParam, lParam);
    }
}

BOOL TrayUi_Init(HINSTANCE inst)
{
    WNDCLASSEXW wc;
    INITCOMMONCONTROLSEX icc;
    wchar_t title[64];

    g_inst = inst;

    icc.dwSize = sizeof(icc);
    icc.dwICC = ICC_LISTVIEW_CLASSES | ICC_BAR_CLASSES | ICC_STANDARD_CLASSES;
    InitCommonControlsEx(&icc);

    /* Sized for the DPI (scaled down from the larger image if need be). */
    if (FAILED(LoadIconMetric(inst, MAKEINTRESOURCEW(IDI_APP), LIM_LARGE, &g_iconBig)))
        g_iconBig = LoadIconW(0, IDI_APPLICATION);
    if (FAILED(LoadIconMetric(inst, MAKEINTRESOURCEW(IDI_APP), LIM_SMALL, &g_iconSmall)))
        g_iconSmall = LoadIconW(0, IDI_APPLICATION);

    ZeroMemory(&wc, sizeof(wc));
    wc.cbSize = sizeof(wc);
    wc.lpfnWndProc = main_proc;
    wc.hInstance = inst;
    wc.hIcon = g_iconBig;
    wc.hIconSm = g_iconSmall;
    wc.hCursor = LoadCursorW(0, IDC_ARROW);
    wc.lpszClassName = kWndClass;
    if (!RegisterClassExW(&wc))
        return FALSE;

    load_str(IDS_APP_TITLE, title, 64);
    g_mainWnd = CreateWindowExW(0, kWndClass, title, WS_OVERLAPPED,
                                0, 0, 0, 0, 0, 0, inst, 0);
    if (!g_mainWnd)
        return FALSE;

    g_taskbarCreated = RegisterWindowMessageW(L"TaskbarCreated");
    tray_add();

    /* Modeless settings dialog, created hidden. */
    CreateDialogParamW(inst, MAKEINTRESOURCEW(IDD_SETTINGS), g_mainWnd,
                       dialog_proc, 0);

    refresh_vms();
    HvHost_StartEvents(g_mainWnd, WM_APP_VMUPDATE);
    return TRUE;
}

void TrayUi_Shutdown(void)
{
    HvHost_StopEvents();
    stop_all_sessions();
    if (g_haveNid) {
        Shell_NotifyIconW(NIM_DELETE, &g_nid);
        g_haveNid = 0;
    }
    if (g_dlg) {
        DestroyWindow(g_dlg);
        g_dlg = 0;
        g_list = 0;
    }
    if (g_mainWnd) {
        DestroyWindow(g_mainWnd);
        g_mainWnd = 0;
    }
}

int TrayUi_Run(void)
{
    MSG msg;
    while (GetMessageW(&msg, 0, 0, 0) > 0) {
        if (g_dlg && IsDialogMessageW(g_dlg, &msg))
            continue;
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    return (int)msg.wParam;
}
