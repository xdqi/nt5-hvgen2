/*
 * log.cpp: UTF-8, one timestamped line per call, to a file in %TEMP% and/or
 * an echo handle.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#include <windows.h>
#include <stdarg.h>

#include "log.h"

static CRITICAL_SECTION g_logLock;
static HANDLE g_logFile = INVALID_HANDLE_VALUE;
static HANDLE g_logEcho;
static int g_logOpen;

void Log_Open(const wchar_t* fileName, HANDLE echo)
{
    wchar_t path[MAX_PATH];
    DWORD n;

    InitializeCriticalSection(&g_logLock);
    g_logOpen = 1;
    if (echo != INVALID_HANDLE_VALUE)
        g_logEcho = echo;
    if (!fileName)
        return;
    n = GetTempPathW(MAX_PATH, path);
    if (n == 0 || n + (DWORD)lstrlenW(fileName) >= MAX_PATH)
        return;
    lstrcatW(path, fileName);
    /* Shared read so it can be followed while the tray runs. */
    g_logFile = CreateFileW(path, GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_DELETE,
                            0, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, 0);
}

void Log_Close(void)
{
    if (!g_logOpen)
        return;
    if (g_logFile != INVALID_HANDLE_VALUE) {
        CloseHandle(g_logFile);
        g_logFile = INVALID_HANDLE_VALUE;
    }
    g_logEcho = 0;
    g_logOpen = 0;
    DeleteCriticalSection(&g_logLock);
}

void Log_Printf(const wchar_t* fmt, ...)
{
    /* wvsprintfW writes up to 1024 characters after the timestamp. */
    wchar_t line[32 + 1024 + 2];
    char utf8[3 * (32 + 1024 + 2)];
    SYSTEMTIME t;
    va_list ap;
    int n;
    DWORD written;

    if (!g_logOpen || (g_logFile == INVALID_HANDLE_VALUE && !g_logEcho))
        return;
    GetLocalTime(&t);
    n = wsprintfW(line, L"%02u:%02u:%02u.%03u ", t.wHour, t.wMinute, t.wSecond,
                  t.wMilliseconds);
    va_start(ap, fmt);
    wvsprintfW(line + n, fmt, ap);
    va_end(ap);
    n = lstrlenW(line);
    line[n++] = L'\r';
    line[n++] = L'\n';
    n = WideCharToMultiByte(CP_UTF8, 0, line, n, utf8, (int)sizeof(utf8), 0, 0);
    if (n <= 0)
        return;
    EnterCriticalSection(&g_logLock);
    if (g_logFile != INVALID_HANDLE_VALUE)
        WriteFile(g_logFile, utf8, (DWORD)n, &written, 0);
    if (g_logEcho)
        WriteFile(g_logEcho, utf8, (DWORD)n, &written, 0);
    LeaveCriticalSection(&g_logLock);
}
