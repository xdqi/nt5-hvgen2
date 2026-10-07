/*
 * vsstest - a minimal Windows XP VSS backup requester that mirrors the call
 * order Microsoft's Hyper-V IC (icsvc.dll VssClientBase) uses, printing every
 * HRESULT and every IVssAsync::QueryStatus, to find which step throws
 * DISP_E_EXCEPTION (0x80020009) on XP's vssvc.
 *
 * It declares IVssBackupComponents with XP's (2003-era) vtable layout, derived
 * from XP vssapi.dll, NOT the Vista+ layout in the mingw headers.  It loads
 * vssapi.dll and resolves the 2003 export
 *   ?CreateVssBackupComponents@@YGJPAPAVIVssBackupComponents@@@Z
 *
 * Build (mingw i686):
 *   i686-w64-mingw32-gcc -O2 -municode? no - ANSI. See Makefile target.
 * Usage (in the guest):
 *   vsstest.exe [provider-guid] > C:\vsstest.txt 2>&1
 *   provider-guid: omitted/"sys" = system provider (GUID_NULL);
 *                  "hv" = Hyper-V IC provider {74600e39-...};
 *                  or an explicit {guid}.
 *
 * MIT (c) nt5-hvgen2.
 */
#define COBJMACROS
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <objbase.h>
#include <stdio.h>

typedef WCHAR *VSS_PWSZ;
typedef GUID VSS_ID;
typedef enum { VSS_BT_UNDEFINED=0, VSS_BT_FULL=1, VSS_BT_INCREMENTAL=2,
               VSS_BT_DIFFERENTIAL=3, VSS_BT_LOG=4, VSS_BT_COPY=5, VSS_BT_OTHER=6 } VSS_BACKUP_TYPE;

/* IVssAsync (XP): QI,AddRef,Release,Cancel,Wait,QueryStatus */
typedef struct IVssAsyncXP IVssAsyncXP;
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IVssAsyncXP*, REFIID, void**);
    ULONG   (__stdcall *AddRef)(IVssAsyncXP*);
    ULONG   (__stdcall *Release)(IVssAsyncXP*);
    HRESULT (__stdcall *Cancel)(IVssAsyncXP*);
    HRESULT (__stdcall *Wait)(IVssAsyncXP*, DWORD);
    HRESULT (__stdcall *QueryStatus)(IVssAsyncXP*, HRESULT*, INT*);
} IVssAsyncXPVtbl;
struct IVssAsyncXP { IVssAsyncXPVtbl *lpVtbl; };

/* IVssExamineWriterMetadata (XP): QI,AddRef,Release,GetIdentity,... (GetIdentity at +0xc) */
typedef struct IVssEWMXP IVssEWMXP;
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IVssEWMXP*, REFIID, void**);
    ULONG   (__stdcall *AddRef)(IVssEWMXP*);
    ULONG   (__stdcall *Release)(IVssEWMXP*);
    HRESULT (__stdcall *GetIdentity)(IVssEWMXP*, VSS_ID*, VSS_ID*, BSTR*, int*, int*);
} IVssEWMXPVtbl;
struct IVssEWMXP { IVssEWMXPVtbl *lpVtbl; };

/* IVssBackupComponents - XP (2003) vtable order, from vssapi.dll. */
typedef struct IVssBCXP IVssBCXP;
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IVssBCXP*, REFIID, void**);        /*0*/
    ULONG   (__stdcall *AddRef)(IVssBCXP*);                                /*1*/
    ULONG   (__stdcall *Release)(IVssBCXP*);                               /*2*/
    HRESULT (__stdcall *GetWriterComponentsCount)(IVssBCXP*, UINT*);       /*3*/
    HRESULT (__stdcall *GetWriterComponents)(IVssBCXP*, UINT, void**);     /*4*/
    HRESULT (__stdcall *InitializeForBackup)(IVssBCXP*, BSTR);             /*5*/
    HRESULT (__stdcall *SetBackupState)(IVssBCXP*, BOOL, BOOL, VSS_BACKUP_TYPE, BOOL); /*6*/
    HRESULT (__stdcall *InitializeForRestore)(IVssBCXP*, BSTR);            /*7*/
    HRESULT (__stdcall *GatherWriterMetadata)(IVssBCXP*, IVssAsyncXP**);   /*8*/
    HRESULT (__stdcall *GetWriterMetadataCount)(IVssBCXP*, UINT*);         /*9*/
    HRESULT (__stdcall *GetWriterMetadata)(IVssBCXP*, UINT, VSS_ID*, void**); /*10*/
    HRESULT (__stdcall *FreeWriterMetadata)(IVssBCXP*);                    /*11*/
    HRESULT (__stdcall *AddComponent)(IVssBCXP*, VSS_ID, VSS_ID, int, LPCWSTR, LPCWSTR); /*12*/
    HRESULT (__stdcall *PrepareForBackup)(IVssBCXP*, IVssAsyncXP**);       /*13*/
    HRESULT (__stdcall *AbortBackup)(IVssBCXP*);                           /*14*/
    HRESULT (__stdcall *GatherWriterStatus)(IVssBCXP*, IVssAsyncXP**);     /*15*/
    HRESULT (__stdcall *GetWriterStatusCount)(IVssBCXP*, UINT*);           /*16*/
    HRESULT (__stdcall *FreeWriterStatus)(IVssBCXP*);                      /*17*/
    HRESULT (__stdcall *GetWriterStatus)(IVssBCXP*, UINT, VSS_ID*, VSS_ID*, BSTR*, int*, HRESULT*); /*18*/
    HRESULT (__stdcall *SetBackupSucceeded)(IVssBCXP*, VSS_ID, VSS_ID, LPCWSTR, BOOL); /*19*/
    HRESULT (__stdcall *SetBackupOptions)(IVssBCXP*, VSS_ID, VSS_ID, LPCWSTR, LPCWSTR); /*20*/
    HRESULT (__stdcall *SetSelectedForRestore)(IVssBCXP*, VSS_ID, VSS_ID, LPCWSTR, BOOL); /*21*/
    HRESULT (__stdcall *SetRestoreOptions)(IVssBCXP*, VSS_ID, VSS_ID, LPCWSTR, LPCWSTR); /*22*/
    HRESULT (__stdcall *SetAdditionalRestores)(IVssBCXP*, VSS_ID, VSS_ID, LPCWSTR, BOOL); /*23*/
    HRESULT (__stdcall *SetPreviousBackupStamp)(IVssBCXP*, VSS_ID, VSS_ID, LPCWSTR, LPCWSTR); /*24*/
    HRESULT (__stdcall *SaveAsXML)(IVssBCXP*, BSTR*);                      /*25*/
    HRESULT (__stdcall *BackupComplete)(IVssBCXP*, IVssAsyncXP**);         /*26*/
    HRESULT (__stdcall *AddAlternativeLocationMapping)(IVssBCXP*, VSS_ID, int, LPCWSTR, LPCWSTR, LPCWSTR, BOOL); /*27*/
    HRESULT (__stdcall *AddRestoreSubcomponent)(IVssBCXP*, VSS_ID, int, LPCWSTR, LPCWSTR, LPCWSTR, BOOL); /*28*/
    HRESULT (__stdcall *SetFileRestoreStatus)(IVssBCXP*, VSS_ID, int, LPCWSTR, LPCWSTR, int); /*29*/
    HRESULT (__stdcall *PreRestore)(IVssBCXP*, IVssAsyncXP**);             /*30*/
    HRESULT (__stdcall *PostRestore)(IVssBCXP*, IVssAsyncXP**);            /*31*/
    HRESULT (__stdcall *SetContext)(IVssBCXP*, LONG);                      /*32*/
    HRESULT (__stdcall *StartSnapshotSet)(IVssBCXP*, VSS_ID*);             /*33*/
    HRESULT (__stdcall *AddToSnapshotSet)(IVssBCXP*, VSS_PWSZ, VSS_ID, VSS_ID*); /*34*/
    HRESULT (__stdcall *DoSnapshotSet)(IVssBCXP*, IVssAsyncXP**);          /*35*/
    /* ... remaining methods not needed ... */
} IVssBCXPVtbl;
struct IVssBCXP { IVssBCXPVtbl *lpVtbl; };

typedef HRESULT (__stdcall *PFN_CreateVssBC)(IVssBCXP**);

static void logh(const char *step, HRESULT hr) {
    printf("%-34s HRESULT=0x%08lX\n", step, (unsigned long)hr);
    fflush(stdout);
}

#define VSS_S_ASYNC_PENDING   0x00042309L
#define VSS_S_ASYNC_FINISHED  0x0004230AL

/* Wait on an IVssAsync (INFINITE, reliable in MTA) and report its final
 * QueryStatus (like icsvc's WaitAndCheckForAsyncOperation). */
static HRESULT wait_async(const char *step, IVssAsyncXP *pa) {
    HRESULT hr, st = VSS_S_ASYNC_PENDING; INT reserved = 0;
    if (!pa) { printf("%-34s (null IVssAsync)\n", step); fflush(stdout); return E_FAIL; }
    hr = pa->lpVtbl->Wait(pa, INFINITE);
    hr = pa->lpVtbl->QueryStatus(pa, &st, &reserved);
    printf("%-34s Wait+QueryStatus call=0x%08lX status=0x%08lX\n",
           step, (unsigned long)hr, (unsigned long)st);
    fflush(stdout);
    pa->lpVtbl->Release(pa);
    return st;
}

int main(int argc, char **argv) {
    HRESULT hr;
    HMODULE h;
    PFN_CreateVssBC pCreate;
    IVssBCXP *bc = NULL;
    IVssAsyncXP *pa = NULL;
    VSS_ID provider;        /* GUID_NULL = system provider */
    VSS_ID ssid;
    const char *mode = (argc > 1) ? argv[1] : "sys";

    memset(&provider, 0, sizeof(provider));
    /* Hyper-V IC provider {74600e39-7dc5-4567-a03b-f091d6c7b092} */
    if (!strcmp(mode, "hv")) {
        GUID g = {0x74600e39,0x7dc5,0x4567,{0xa0,0x3b,0xf0,0x91,0xd6,0xc7,0xb0,0x92}};
        provider = g;
        printf("provider = Hyper-V IC {74600e39-...}\n");
    } else {
        printf("provider = system (GUID_NULL)\n");
    }
    fflush(stdout);

    /* icsvc uses STA, but a diagnostic standalone requester needs MTA so the
     * async waits complete without a message pump; which step errors is the
     * same either way. */
    hr = CoInitializeEx(NULL, COINIT_MULTITHREADED);
    logh("CoInitializeEx(MTA)", hr);
    hr = CoInitializeSecurity(NULL, -1, NULL, NULL,
            RPC_C_AUTHN_LEVEL_PKT_PRIVACY, RPC_C_IMP_LEVEL_IMPERSONATE,
            NULL, EOAC_NONE, NULL);
    logh("CoInitializeSecurity", hr);

    h = LoadLibraryA("vssapi.dll");
    if (!h) { logh("LoadLibrary vssapi.dll", HRESULT_FROM_WIN32(GetLastError())); return 1; }
    pCreate = (PFN_CreateVssBC)GetProcAddress(h,
        "?CreateVssBackupComponents@@YGJPAPAVIVssBackupComponents@@@Z");
    if (!pCreate) { logh("GetProcAddress CreateVssBC", HRESULT_FROM_WIN32(GetLastError())); return 1; }

    hr = pCreate(&bc);
    logh("CreateVssBackupComponents", hr);
    if (FAILED(hr) || !bc) return 2;

    hr = bc->lpVtbl->InitializeForBackup(bc, NULL);
    logh("InitializeForBackup", hr);

    hr = bc->lpVtbl->SetContext(bc, 0 /*VSS_CTX_BACKUP*/);
    logh("SetContext(VSS_CTX_BACKUP)", hr);   /* expect E_NOTIMPL on XP */

    /* icsvc's VssClientBase::Initialize calls SetBackupState(1,1,VSS_BT_FULL,0):
     * bSelectComponents=TRUE, bBackupBootableSystemState=TRUE. The TRUE for
     * select-components is what drives the writer-metadata retrieval that throws
     * on XP.  Pass "nosel" to compare with bSelectComponents=FALSE. */
    {
        BOOL bSel = (argc > 2 && !strcmp(argv[2], "nosel")) ? FALSE : TRUE;
        hr = bc->lpVtbl->SetBackupState(bc, bSel, TRUE, VSS_BT_FULL, FALSE);
        printf("SetBackupState(sel=%d,boot=1,FULL)  HRESULT=0x%08lX\n", bSel, (unsigned long)hr);
        fflush(stdout);
    }

    pa = NULL;
    hr = bc->lpVtbl->GatherWriterMetadata(bc, &pa);
    logh("GatherWriterMetadata call", hr);
    if (SUCCEEDED(hr)) wait_async("GatherWriterMetadata async", pa);

    /* Reproduce icsvc's VssClientBase::InitializeWriterMetadata: GetWriterMetadataCount
     * then a loop of GetWriterMetadata(i,&id,&pMeta) + GetIdentity (the step that
     * throws 0x80020009 in the real IC).  Print every HRESULT and the writer id/name. */
    {
        UINT wn = 0, i;
        hr = bc->lpVtbl->GetWriterMetadataCount(bc, &wn);
        printf("GetWriterMetadataCount             HRESULT=0x%08lX count=%u\n", (unsigned long)hr, wn);
        fflush(stdout);
        for (i = 0; SUCCEEDED(hr) && i < wn; i++) {
            VSS_ID wid; IVssEWMXP *pMeta = NULL;
            memset(&wid, 0, sizeof(wid));
            HRESULT hm = bc->lpVtbl->GetWriterMetadata(bc, i, &wid, (void**)&pMeta);
            printf("  GetWriterMetadata[%u] HRESULT=0x%08lX instId={%08lX-%04X-%04X-...}\n",
                   i, (unsigned long)hm, wid.Data1, wid.Data2, wid.Data3);
            fflush(stdout);
            if (SUCCEEDED(hm) && pMeta) {
                VSS_ID iid, wrid; BSTR wname = NULL; int usage = 0, src = 0;
                HRESULT hi = pMeta->lpVtbl->GetIdentity(pMeta, &iid, &wrid, &wname, &usage, &src);
                printf("     GetIdentity HRESULT=0x%08lX name=%ls\n",
                       (unsigned long)hi, wname ? wname : L"");
                fflush(stdout);
                pMeta->lpVtbl->Release(pMeta);
            }
        }
    }

    memset(&ssid, 0, sizeof(ssid));
    hr = bc->lpVtbl->StartSnapshotSet(bc, &ssid);
    logh("StartSnapshotSet", hr);

    /* add every fixed (DRIVE_FIXED) volume */
    {
        WCHAR vol[MAX_PATH]; HANDLE fh = FindFirstVolumeW(vol, MAX_PATH);
        if (fh != INVALID_HANDLE_VALUE) {
            do {
                if (GetDriveTypeW(vol) == DRIVE_FIXED) {
                    VSS_ID sid;
                    hr = bc->lpVtbl->AddToSnapshotSet(bc, vol, provider, &sid);
                    printf("AddToSnapshotSet %-25ls HRESULT=0x%08lX\n", vol, (unsigned long)hr);
                    fflush(stdout);
                }
            } while (FindNextVolumeW(fh, vol, MAX_PATH));
            FindVolumeClose(fh);
        }
    }

    pa = NULL;
    hr = bc->lpVtbl->PrepareForBackup(bc, &pa);
    logh("PrepareForBackup call", hr);
    if (SUCCEEDED(hr)) wait_async("PrepareForBackup async", pa);

    pa = NULL;
    hr = bc->lpVtbl->DoSnapshotSet(bc, &pa);
    logh("DoSnapshotSet call", hr);
    if (SUCCEEDED(hr)) wait_async("DoSnapshotSet async", pa);

    /* writer status last (diagnostic; may be slow). */
    pa = NULL;
    hr = bc->lpVtbl->GatherWriterStatus(bc, &pa);
    logh("GatherWriterStatus call", hr);
    if (SUCCEEDED(hr)) {
        UINT n = 0, i;
        wait_async("GatherWriterStatus async", pa);
        hr = bc->lpVtbl->GetWriterStatusCount(bc, &n);
        printf("GetWriterStatusCount               HRESULT=0x%08lX count=%u\n", (unsigned long)hr, n);
        for (i = 0; i < n; i++) {
            VSS_ID inst, wr; BSTR name = NULL; int state = 0; HRESULT whr = 0;
            hr = bc->lpVtbl->GetWriterStatus(bc, i, &inst, &wr, &name, &state, &whr);
            printf("  writer[%u] call=0x%08lX state=%d hr=0x%08lX name=%ls\n",
                   i, (unsigned long)hr, state, (unsigned long)whr, name ? name : L"");
        }
        fflush(stdout);
    }

    bc->lpVtbl->AbortBackup(bc);
    bc->lpVtbl->Release(bc);
    CoUninitialize();
    printf("DONE\n");
    return 0;
}
