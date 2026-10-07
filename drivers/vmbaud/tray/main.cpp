/*
 * main.cpp: entry point, single instance, --wait-ic.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#include <windows.h>
#include <shellapi.h>

#include "trayui.h"
#include "hvhost.h"
#include "pipechannel.h"
#include "audioout.h"
#include "vmsession.h"
#include "log.h"
#include "resource.h"

static const wchar_t kMutexName[] = L"Local\\vmbaudtray.single";
static const wchar_t kWndClass[]  = L"vmbaudtray.main";

/* ------------------------------------------------------------------ */
/* wWinMain                                                           */
/* ------------------------------------------------------------------ */

int WINAPI wWinMain(HINSTANCE inst, HINSTANCE, LPWSTR cmdLine, int)
{
    HANDLE mutex;
    int waitIc = 0;

    (void)cmdLine;
    {
        int argc = 0;
        LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &argc);
        if (argv) {
            int i;
            for (i = 1; i < argc; i++) {
                if (lstrcmpiW(argv[i], L"--wait-ic") == 0)
                    waitIc = 1;
            }
            LocalFree(argv);
        }
    }

    if (waitIc)
        VmSession_SetOfferGate(kOfferWhenIntegrationServiceOk);

    mutex = CreateMutexW(0, TRUE, kMutexName);
    if (mutex && GetLastError() == ERROR_ALREADY_EXISTS) {
        /* Another copy is running: bring its settings window up. */
        HWND other = FindWindowW(kWndClass, 0);
        if (other)
            PostMessageW(other, WM_APP_SHOW, 0, 0);
        CloseHandle(mutex);
        return 0;
    }

    CoInitializeEx(0, COINIT_APARTMENTTHREADED);
    Log_Open(L"vmbaudtray.log", 0);
    Log_Printf(L"vmbaudtray start%s", waitIc ? L" (--wait-ic)" : L"");

    if (!PipeChannel_Load() || !HvHost_Open()) {
        wchar_t text[256];
        LoadStringW(inst, PipeChannel_Available() ? IDS_WMI_FAIL : IDS_PIPE_FAIL,
                    text, 256);
        MessageBoxW(0, text, L"vmbaudtray", MB_OK | MB_ICONERROR);
        PipeChannel_Unload();
        if (mutex) {
            ReleaseMutex(mutex);
            CloseHandle(mutex);
        }
        Log_Close();
        CoUninitialize();
        return 1;
    }

    if (!AudioOut_GlobalInit()) {
        /* No enumerator is not fatal: the sessions will estimate the clock. */
    }

    if (!TrayUi_Init(inst)) {
        HvHost_Close();
        AudioOut_GlobalShutdown();
        PipeChannel_Unload();
        if (mutex) {
            ReleaseMutex(mutex);
            CloseHandle(mutex);
        }
        Log_Close();
        CoUninitialize();
        return 1;
    }

    {
        int rc = TrayUi_Run();
        TrayUi_Shutdown();
        HvHost_Close();
        AudioOut_GlobalShutdown();
        PipeChannel_Unload();
        Log_Printf(L"vmbaudtray exit");
        Log_Close();
        CoUninitialize();
        if (mutex) {
            ReleaseMutex(mutex);
            CloseHandle(mutex);
        }
        return rc;
    }
}
