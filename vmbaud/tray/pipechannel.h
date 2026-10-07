/*
 * pipechannel.h: vmbuspiper.dll offer / connect / read / write / close.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#ifndef _VMBAUDTRAY_PIPECHANNEL_H_
#define _VMBAUDTRAY_PIPECHANNEL_H_

#include <windows.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Offer blob handed to VmbusPipeServerOfferChannel; 0xAC bytes. */
#pragma pack(push, 1)
typedef struct _PIPE_OFFER {
    GUID     VmId;          /* 0x00 */
    DWORD    LatencyMs;     /* 0x10, always 0 */
    GUID     InterfaceType; /* 0x14 */
    GUID     InterfaceInstance; /* 0x24 */
    DWORD    Revision;      /* 0x34, always 0 */
    WORD     MmioMb;        /* 0x38, always 0 */
    WORD     Flags;         /* 0x3A, always 0 */
    BYTE     User[112];     /* 0x3C */
} PIPE_OFFER;
#pragma pack(pop)

/* LoadLibraryW("vmbuspiper.dll") + resolve the two exports. */
int  PipeChannel_Load(void);
void PipeChannel_Unload(void);
int  PipeChannel_Available(void);

typedef struct _PIPE_CHANNEL {
    HANDLE    Handle;
    HANDLE    ReadEvent;
    HANDLE    WriteEvent;
    HANDLE    ConnectEvent;
    OVERLAPPED ReadOv;
    OVERLAPPED WriteOv;
    OVERLAPPED ConnectOv;
    void*     ReadBuf;
    DWORD     ReadCap;
    BOOL      ReadPending;
    BOOL      ConnectPending;
    DWORD     LastError;
} PIPE_CHANNEL;

/* Fills the events and the buffer pointers; does not offer. */
void PipeChannel_Init(PIPE_CHANNEL* ch, void* readBuf, DWORD readCap);

/* CancelIoEx + CloseHandle.  Closing rescinds the offer. */
void PipeChannel_Kill(PIPE_CHANNEL* ch);
/* Kill, and close the channel's events (at the end of the worker). */
void PipeChannel_Free(PIPE_CHANNEL* ch);

/* Returns 0 on success, else the Win32 error (1 = the VM is off). */
DWORD PipeChannel_Offer(PIPE_CHANNEL* ch, const GUID* vmId, DWORD pipeMode);

/* Start/await the overlapped VmbusPipeServerConnectPipe. */
BOOL PipeChannel_StartConnect(PIPE_CHANNEL* ch);
/* 0 = connected, 1 = still pending, 2 = failed (LastError set). */
int  PipeChannel_WaitConnect(PIPE_CHANNEL* ch, DWORD timeoutMs);

/* One outstanding read of up to ReadCap bytes. */
BOOL PipeChannel_StartRead(PIPE_CHANNEL* ch);
/* 0 = completed (*got bytes), 1 = still pending, 2 = failed (LastError). */
int  PipeChannel_WaitRead(PIPE_CHANNEL* ch, DWORD* got, DWORD timeoutMs);

/* Synchronous write with a timeout.  0 = ok, else Win32 error. */
DWORD PipeChannel_Write(PIPE_CHANNEL* ch, const void* data, DWORD len, DWORD timeoutMs);

BOOL PipeChannel_IsOpen(const PIPE_CHANNEL* ch);

#ifdef __cplusplus
}
#endif

#endif /* _VMBAUDTRAY_PIPECHANNEL_H_ */
