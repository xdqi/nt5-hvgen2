/*
 * pipechannel.cpp: the host side of a VMBus pipe offered with vmbuspiper.dll.
 *
 * The offer layout and the overlapped ConnectPipe requirement come from
 * vmbaud-host.ps1 (a synchronous ConnectPipe makes CloseHandle block).
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#include "pipechannel.h"

/* Must match vmbaud.h / vmbaud-host.ps1. */
static const GUID kInterfaceType = {
    0x8b57f4e3, 0x2a3c, 0x4f6e, { 0x9c, 0x8d, 0x1e, 0x5a, 0x70, 0xb9, 0xc4, 0xd2 }
};
/* Fixed instance GUID: keeps the guest's device node across offers. */
static const GUID kInterfaceInstance = {
    0x2a7f3e10, 0x9c4d, 0x4b8a, { 0xa6, 0xe5, 0x7d, 0x1c, 0x0f, 0x3b, 0x8e, 0x62 }
};

typedef HANDLE (WINAPI *PFN_OfferChannel)(const void* offer, DWORD openMode, DWORD pipeMode);
typedef BOOL   (WINAPI *PFN_ConnectPipe)(HANDLE pipe, OVERLAPPED* overlapped);

static HMODULE g_vmbuspiper;
static PFN_OfferChannel g_OfferChannel;
static PFN_ConnectPipe  g_ConnectPipe;

int PipeChannel_Load(void)
{
    if (g_vmbuspiper)
        return 1;
    g_vmbuspiper = LoadLibraryW(L"vmbuspiper.dll");
    if (!g_vmbuspiper)
        return 0;
    g_OfferChannel = (PFN_OfferChannel)GetProcAddress(g_vmbuspiper, "VmbusPipeServerOfferChannel");
    g_ConnectPipe  = (PFN_ConnectPipe)GetProcAddress(g_vmbuspiper, "VmbusPipeServerConnectPipe");
    if (!g_OfferChannel || !g_ConnectPipe) {
        FreeLibrary(g_vmbuspiper);
        g_vmbuspiper = 0;
        g_OfferChannel = 0;
        g_ConnectPipe = 0;
        return 0;
    }
    return 1;
}

void PipeChannel_Unload(void)
{
    if (g_vmbuspiper) {
        FreeLibrary(g_vmbuspiper);
        g_vmbuspiper = 0;
        g_OfferChannel = 0;
        g_ConnectPipe = 0;
    }
}

int PipeChannel_Available(void)
{
    return g_vmbuspiper != 0;
}

void PipeChannel_Init(PIPE_CHANNEL* ch, void* readBuf, DWORD readCap)
{
    ZeroMemory(ch, sizeof(*ch));
    ch->Handle = INVALID_HANDLE_VALUE;
    ch->ReadBuf = readBuf;
    ch->ReadCap = readCap;
    ch->ReadEvent = CreateEventW(0, TRUE, FALSE, 0);
    ch->WriteEvent = CreateEventW(0, TRUE, FALSE, 0);
    ch->ConnectEvent = CreateEventW(0, TRUE, FALSE, 0);
}

void PipeChannel_Kill(PIPE_CHANNEL* ch)
{
    DWORD n;

    if (ch->Handle != INVALID_HANDLE_VALUE && ch->Handle != 0) {
        /* Cancel and wait for the pending connect and read: they complete
         * into ch's OVERLAPPEDs and read buffer, which the next offer reuses. */
        CancelIoEx(ch->Handle, 0);
        if (ch->ConnectPending)
            GetOverlappedResult(ch->Handle, &ch->ConnectOv, &n, TRUE);
        if (ch->ReadPending)
            GetOverlappedResult(ch->Handle, &ch->ReadOv, &n, TRUE);
        CloseHandle(ch->Handle);
        ch->Handle = INVALID_HANDLE_VALUE;
    }
    ch->ConnectPending = FALSE;
    ch->ReadPending = FALSE;
}

void PipeChannel_Free(PIPE_CHANNEL* ch)
{
    PipeChannel_Kill(ch);
    if (ch->ReadEvent) { CloseHandle(ch->ReadEvent); ch->ReadEvent = 0; }
    if (ch->WriteEvent) { CloseHandle(ch->WriteEvent); ch->WriteEvent = 0; }
    if (ch->ConnectEvent) { CloseHandle(ch->ConnectEvent); ch->ConnectEvent = 0; }
}

DWORD PipeChannel_Offer(PIPE_CHANNEL* ch, const GUID* vmId, DWORD pipeMode)
{
    PIPE_OFFER offer;

    if (!g_OfferChannel)
        return ERROR_DLL_INIT_FAILED;

    ZeroMemory(&offer, sizeof(offer));
    if (vmId)
        offer.VmId = *vmId;
    offer.InterfaceType = kInterfaceType;
    offer.InterfaceInstance = kInterfaceInstance;

    ch->Handle = g_OfferChannel(&offer, FILE_FLAG_OVERLAPPED, pipeMode);
    if (ch->Handle == 0 || ch->Handle == INVALID_HANDLE_VALUE) {
        ch->Handle = INVALID_HANDLE_VALUE;
        ch->LastError = GetLastError();
        return ch->LastError ? ch->LastError : (DWORD)ERROR_GEN_FAILURE;
    }
    return 0;
}

BOOL PipeChannel_StartConnect(PIPE_CHANNEL* ch)
{
    if (ch->Handle == INVALID_HANDLE_VALUE)
        return FALSE;
    ZeroMemory(&ch->ConnectOv, sizeof(ch->ConnectOv));
    ch->ConnectOv.hEvent = ch->ConnectEvent;
    ResetEvent(ch->ConnectEvent);
    /* Returns FALSE with ERROR_IO_PENDING when the guest has not opened yet. */
    if (g_ConnectPipe(ch->Handle, &ch->ConnectOv)) {
        SetEvent(ch->ConnectEvent);
        return TRUE;
    }
    if (GetLastError() == ERROR_IO_PENDING) {
        ch->ConnectPending = TRUE;
        return TRUE;
    }
    ch->LastError = GetLastError();
    return FALSE;
}

int PipeChannel_WaitConnect(PIPE_CHANNEL* ch, DWORD timeoutMs)
{
    DWORD n;

    if (ch->Handle == INVALID_HANDLE_VALUE)
        return 2;
    if (WaitForSingleObject(ch->ConnectEvent, timeoutMs) != WAIT_OBJECT_0)
        return 1;
    ch->ConnectPending = FALSE;
    if (!GetOverlappedResult(ch->Handle, &ch->ConnectOv, &n, FALSE)) {
        ch->LastError = GetLastError();
        return 2;
    }
    return 0;
}

BOOL PipeChannel_StartRead(PIPE_CHANNEL* ch)
{
    DWORD n;

    if (ch->Handle == INVALID_HANDLE_VALUE || ch->ReadPending)
        return FALSE;
    ZeroMemory(&ch->ReadOv, sizeof(ch->ReadOv));
    ch->ReadOv.hEvent = ch->ReadEvent;
    ResetEvent(ch->ReadEvent);
    if (ReadFile(ch->Handle, ch->ReadBuf, ch->ReadCap, &n, &ch->ReadOv)) {
        ch->ReadPending = TRUE;
        return TRUE;
    }
    if (GetLastError() == ERROR_IO_PENDING) {
        ch->ReadPending = TRUE;
        return TRUE;
    }
    ch->LastError = GetLastError();
    ch->ReadPending = FALSE;
    return FALSE;
}

int PipeChannel_WaitRead(PIPE_CHANNEL* ch, DWORD* got, DWORD timeoutMs)
{
    DWORD n;

    if (!ch->ReadPending)
        return 1;
    if (WaitForSingleObject(ch->ReadEvent, timeoutMs) != WAIT_OBJECT_0)
        return 1;
    ch->ReadPending = FALSE;
    if (!GetOverlappedResult(ch->Handle, &ch->ReadOv, &n, FALSE)) {
        ch->LastError = GetLastError();
        return 2;
    }
    *got = n;
    return 0;
}

DWORD PipeChannel_Write(PIPE_CHANNEL* ch, const void* data, DWORD len, DWORD timeoutMs)
{
    OVERLAPPED ov;
    DWORD n = 0;

    if (ch->Handle == INVALID_HANDLE_VALUE)
        return ERROR_INVALID_HANDLE;
    ZeroMemory(&ov, sizeof(ov));
    ov.hEvent = ch->WriteEvent;
    ResetEvent(ch->WriteEvent);
    if (WriteFile(ch->Handle, data, len, &n, &ov)) {
        return 0;
    }
    if (GetLastError() != ERROR_IO_PENDING) {
        DWORD err = GetLastError();
        ch->LastError = err;
        return err ? err : (DWORD)ERROR_GEN_FAILURE;
    }
    if (WaitForSingleObject(ch->WriteEvent, timeoutMs) != WAIT_OBJECT_0) {
        /* ov is on this stack: wait until the cancelled write is done. */
        CancelIoEx(ch->Handle, &ov);
        GetOverlappedResult(ch->Handle, &ov, &n, TRUE);
        ch->LastError = WAIT_TIMEOUT;
        return WAIT_TIMEOUT;
    }
    if (!GetOverlappedResult(ch->Handle, &ov, &n, FALSE)) {
        DWORD err = GetLastError();
        ch->LastError = err;
        return err ? err : (DWORD)ERROR_GEN_FAILURE;
    }
    return 0;
}

BOOL PipeChannel_IsOpen(const PIPE_CHANNEL* ch)
{
    return ch->Handle != INVALID_HANDLE_VALUE && ch->Handle != 0;
}
