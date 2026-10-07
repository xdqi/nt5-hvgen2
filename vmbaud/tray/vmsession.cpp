/*
 * vmsession.cpp: the per-VM worker.
 *
 * The protocol and the jitter-buffer policy are ported from vmbaud-host.ps1:
 *   FORMAT / PCM_OUT / CONSUMED over a vmbuspiper pipe;
 *   prebuffer 60 ms, start anyway after a 30 ms stall, CONSUMED every 10 ms,
 *   stop CONSUMED once the stream is idle (queue empty and 100 ms of no PCM),
 *   estimate played from the wall clock when there is no sound device.
 * The winmm chunking is gone: AudioOut's WASAPI event pulls from the ring.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#include <windows.h>
#include <avrt.h>
#include <wbemidl.h>

#include "vmsession.h"
#include "pipechannel.h"
#include "audioout.h"
#include "log.h"
#include "../vmbaud.h"

/* Wire protocol constants come from vmbaud.h (VBAUD_MSG_*, VBAUD_HEADER_SIZE). */

static const unsigned kReadBufSize       = 1024 * 1024;
static const unsigned kConsumedIntervalMs = 10;
static const unsigned kConsumedEveryNMsgs = 16;
static const unsigned kIdleMs             = 100;
static const unsigned kReofferDelayMs     = 1500;
static const unsigned kOfferRetryMs       = 3000;
static const unsigned kIcPollMs           = 2000;
static const unsigned kIcGraceMs          = 5000;
static const unsigned kPlayingHoldMs      = 500;  /* "Playing" after the last PCM */
static const unsigned kWriteTimeoutMs     = 200;
/* After the channel broke, a new offer the guest has not opened within this
 * long is withdrawn and made again (doubling up to the maximum): a re-offer
 * made while the guest is still removing the old device can go unnoticed.
 * No deadline for the first offer (the VM may still be booting, or the guest
 * may have no driver; withdrawing would repeat "Found New Hardware") nor
 * after the VM changed state (a guest reboot takes most of a minute). */
static const unsigned kReconnectFirstMs   = 10000;
static const unsigned kReconnectMaxMs     = 60000;

static int g_offerGate = kOfferWhenRunning;
static wchar_t g_capture[MAX_PATH];
static volatile LONG g_captureCount;

void VmSession_SetCapture(const wchar_t* prefix)
{
    lstrcpynW(g_capture, prefix ? prefix : L"", MAX_PATH - 16);
}

void VmSession_SetOfferGate(int gate)
{
    g_offerGate = gate;
}

int VmSession_GetOfferGate(void)
{
    return g_offerGate;
}

struct _VM_SESSION {
    GUID     vmId;
    wchar_t  name[64];
    HWND     hwnd;
    UINT     statusMsg;
    UINT_PTR cookie;
    HANDLE   thread;
    HANDLE   stopEvent;
    struct _WORKER* worker;     /* the thread's state, freed by VmSession_Stop */
    /* Written by the UI, read by the worker; 32-bit so they tear-free. */
    volatile LONG volume;
    volatile LONG mute;
    volatile LONG alive;
    volatile LONG stateChanged; /* VmSession_NotifyStateChange, for the worker */
    LPARAM   lastStatus;        /* worker thread only */
};

/* Worker thread only.  Posts when the status differs from the last one. */
static void post_status(VM_SESSION* s, int code, DWORD err)
{
    LPARAM lp = VS_STATUS(code, err);

    if (lp == s->lastStatus || !s->hwnd || !s->statusMsg)
        return;
    if (PostMessageW(s->hwnd, s->statusMsg, (WPARAM)s->cookie, lp))
        s->lastStatus = lp;
}

/* ------------------------------------------------------------------ */
/* Protocol                                                           */
/* ------------------------------------------------------------------ */

typedef struct _SESSION_STATE {
    PIPE_CHANNEL pipe;
    AUDIO_OUT*   audio;
    BYTE*        readBuf;

    BOOL   haveFormat;
    BOOL   formatOk;        /* within what the host plays */
    unsigned rate, channels, bits;
    unsigned long long pcmIn;         /* guest PCM bytes this stream */
    unsigned long long consumedSent;  /* last played we reported */
    unsigned long long streamStartTick;
    unsigned long long lastPcmTick;

    unsigned msgCount, pcmCount, formatCount, dropCount, underrunCount;
    unsigned pcmSinceConsumed;
    BOOL   guestDeaf;       /* a CONSUMED write timed out: the guest's reader
                             * has stopped; send nothing until PCM comes */
    BOOL   streaming;       /* PCM since the last "stream idle" log line */
    const wchar_t* name;    /* for the log */
    /* Per stream, for the log: CONSUMED reports and the queue they carried
     * once playback ran (ms). */
    unsigned reports, queueMin, queueMax;
    HANDLE wav;             /* --wav capture of this stream */
    DWORD  wavBytes;
} SESSION_STATE;

/* RIFF/WAVE header for PCM; sizes patched by wav_close. */
static void wav_open(SESSION_STATE* st)
{
    wchar_t path[MAX_PATH];
    unsigned char h[44];
    unsigned block = st->channels * (st->bits / 8);
    DWORD n;

    if (!g_capture[0])
        return;
    wsprintfW(path, L"%s-%u.wav", g_capture, (unsigned)InterlockedIncrement(&g_captureCount));
    st->wav = CreateFileW(path, GENERIC_WRITE, FILE_SHARE_READ, 0, CREATE_ALWAYS,
                          FILE_ATTRIBUTE_NORMAL, 0);
    if (st->wav == INVALID_HANDLE_VALUE) {
        st->wav = 0;
        return;
    }
    ZeroMemory(h, sizeof(h));
    CopyMemory(h, "RIFF", 4);
    CopyMemory(h + 8, "WAVEfmt ", 8);
    *(DWORD*)(h + 16) = 16;
    *(WORD*)(h + 20) = 1;                   /* PCM */
    *(WORD*)(h + 22) = (WORD)st->channels;
    *(DWORD*)(h + 24) = st->rate;
    *(DWORD*)(h + 28) = st->rate * block;
    *(WORD*)(h + 32) = (WORD)block;
    *(WORD*)(h + 34) = (WORD)st->bits;
    CopyMemory(h + 36, "data", 4);
    WriteFile(st->wav, h, sizeof(h), &n, 0);
    st->wavBytes = 0;
    Log_Printf(L"[%s] capture -> %s", st->name, path);
}

static void wav_close(SESSION_STATE* st)
{
    DWORD n, v;

    if (!st->wav)
        return;
    v = 36 + st->wavBytes;
    SetFilePointer(st->wav, 4, 0, FILE_BEGIN);
    WriteFile(st->wav, &v, 4, &n, 0);
    v = st->wavBytes;
    SetFilePointer(st->wav, 40, 0, FILE_BEGIN);
    WriteFile(st->wav, &v, 4, &n, 0);
    CloseHandle(st->wav);
    st->wav = 0;
}

static DWORD send_consumed(SESSION_STATE* st, unsigned long long played,
                           unsigned long long queued)
{
    unsigned char msg[VBAUD_HEADER_SIZE + 12];
    DWORD err;

    ZeroMemory(msg, sizeof(msg));
    /* Little-endian {u32 type, u32 size} + {u64 played, u32 queued}. */
    *(unsigned int*)(msg + 0) = VBAUD_MSG_CONSUMED;
    *(unsigned int*)(msg + 4) = 12;
    *(unsigned long long*)(msg + 8) = played;
    {
        unsigned int q = (queued > 0xFFFFFFFFull) ? 0xFFFFFFFFu : (unsigned int)queued;
        *(unsigned int*)(msg + 16) = q;
    }
    err = PipeChannel_Write(&st->pipe, msg, sizeof(msg), kWriteTimeoutMs);
    return err;
}

static void handle_message(SESSION_STATE* st, unsigned mtype,
                           const unsigned char* pay, unsigned msize)
{
    st->msgCount++;

    if (mtype == VBAUD_MSG_FORMAT) {
        unsigned rate, ch, bits;
        int changed;

        if (msize < 12)
            return;
        rate = *(const unsigned int*)(pay + 0);
        ch   = *(const unsigned int*)(pay + 4);
        bits = *(const unsigned int*)(pay + 8);
        st->formatCount++;
        /* Reopen also when the device was missing at the last FORMAT. */
        changed = !st->haveFormat || rate != st->rate
               || ch != st->channels || bits != st->bits
               || !AudioOut_IsOpen(st->audio);
        if (changed) {
            AudioOut_Close(st->audio);
            st->haveFormat = TRUE;
            st->rate = rate;
            st->channels = ch;
            st->bits = bits;
            /* Out of range or no device: the stream stays closed and
             * PlayedBytes estimates the clock; the status shows NoDevice. */
            st->formatOk = ch >= 1 && ch <= 8 && bits >= 8 && bits <= 32
                && (bits % 8) == 0 && rate >= 4000 && rate <= 192000;
            if (st->formatOk)
                AudioOut_Open(st->audio, rate, ch, bits);
            Log_Printf(L"[%s] FORMAT %u Hz %u ch %u bit, output %s", st->name,
                       rate, ch, bits, AudioOut_IsOpen(st->audio) ? L"open" : L"NOT open");
        } else {
            /* Same format, new stream: drop anything still queued. */
            AudioOut_ResetStream(st->audio);
            Log_Printf(L"[%s] FORMAT unchanged, new stream", st->name);
        }
        st->pcmIn = 0;
        st->consumedSent = 0;
        st->streamStartTick = GetTickCount64();
        st->lastPcmTick = st->streamStartTick;
        st->pcmSinceConsumed = 0;
        st->guestDeaf = FALSE;
        st->reports = 0;
        st->queueMin = 0xFFFFFFFFu;
        st->queueMax = 0;
        wav_close(st);
        wav_open(st);
        send_consumed(st, 0, 0);
    } else if (mtype == VBAUD_MSG_PCM_OUT) {
        if (!st->haveFormat) {
            st->dropCount++;
            return;
        }
        if (msize == 0)
            return;
        st->pcmCount++;
        st->guestDeaf = FALSE;
        st->streaming = TRUE;
        st->pcmIn += msize;
        st->lastPcmTick = GetTickCount64();
        st->pcmSinceConsumed++;
        AudioOut_Enqueue(st->audio, pay, msize);
        if (st->wav) {
            DWORD n;
            WriteFile(st->wav, pay, msize, &n, 0);
            st->wavBytes += n;
        }
    }
}

static void on_read_done(SESSION_STATE* st, DWORD n)
{
    unsigned off = 0;

    while (off + VBAUD_HEADER_SIZE <= n) {
        unsigned mtype = *(const unsigned int*)(st->readBuf + off);
        unsigned msize = *(const unsigned int*)(st->readBuf + off + 4);
        /* Unsigned compare: msize is guest-controlled. */
        if ((unsigned long long)off + VBAUD_HEADER_SIZE + (unsigned long long)msize > (unsigned long long)n)
            break;
        handle_message(st, mtype, st->readBuf + off + VBAUD_HEADER_SIZE, msize);
        off += VBAUD_HEADER_SIZE + msize;
    }
}

/* ------------------------------------------------------------------ */
/* Worker                                                             */
/* ------------------------------------------------------------------ */

typedef struct _WORKER {
    VM_SESSION* s;
    SESSION_STATE st;
    int appliedVolume, appliedMute;     /* what AudioOut has; -1 = not yet */
    int phase;                  /* 0 = not offered, 1 = offered, 2 = connected */
    ULONGLONG gateStart;
    ULONGLONG nextOffer;
    ULONGLONG offerDeadline;    /* withdraw an unopened offer then; 0 = never */
    DWORD reconnectMs;          /* 0 until the guest has connected once */
    BOOL vmRestarted;           /* state change seen: no deadline until connected */
    ULONGLONG nextConsumed;
} WORKER;

static void worker_offer(WORKER* w, ULONGLONG now)
{
    DWORD err = PipeChannel_Offer(&w->st.pipe, &w->s->vmId, 0);

    if (err == 0 && !PipeChannel_StartConnect(&w->st.pipe)) {
        err = w->st.pipe.LastError ? w->st.pipe.LastError : ERROR_GEN_FAILURE;
        PipeChannel_Kill(&w->st.pipe);
    }
    if (err != 0) {
        /* Error 1 while the VM is not running yet. */
        if (w->s->lastStatus != VS_STATUS(kVsOfferFailed, err))
            Log_Printf(L"[%s] offer failed, error %u; retrying", w->s->name, (unsigned)err);
        post_status(w->s, kVsOfferFailed, err);
        w->nextOffer = now + kOfferRetryMs;
        return;
    }
    w->phase = 1;
    w->offerDeadline = (w->reconnectMs && !w->vmRestarted) ? now + w->reconnectMs : 0;
    if (w->offerDeadline)
        Log_Printf(L"[%s] offered; withdrawn if not opened within %u ms",
                   w->s->name, (unsigned)w->reconnectMs);
    else
        Log_Printf(L"[%s] offered", w->s->name);
    post_status(w->s, kVsOffering, 0);
}

/* The channel broke: rescind the offer, forget the stream (a reconnecting
 * guest starts with FORMAT again), offer again shortly. */
static void drop_channel(WORKER* w, ULONGLONG now, const wchar_t* why, DWORD err)
{
    SESSION_STATE* st = &w->st;

    if (err == 0)
        err = ERROR_GEN_FAILURE;
    Log_Printf(L"[%s] channel dropped: %s, error %u (stream: %u bytes in, %u reported played)",
               w->s->name, why, (unsigned)err, (unsigned)st->pcmIn, (unsigned)st->consumedSent);
    PipeChannel_Kill(&st->pipe);
    AudioOut_Close(st->audio);
    st->haveFormat = FALSE;
    st->pcmIn = 0;
    st->consumedSent = 0;
    st->pcmSinceConsumed = 0;
    st->guestDeaf = FALSE;
    st->streaming = FALSE;
    wav_close(st);
    w->phase = 0;
    w->gateStart = now;
    w->nextOffer = now + kReofferDelayMs;
    post_status(w->s, kVsReoffer, err);
}

static int gate_allows(VM_SESSION* s, ULONGLONG startTick)
{
    HV_VM ic;

    if (g_offerGate == kOfferWhenRunning)
        return 1;
    ZeroMemory(&ic, sizeof(ic));
    if (!HvHost_ReadIntegrationServices(&s->vmId, &ic))
        Log_Printf(L"[%s] reading the integration services failed", s->name);
    if (ic.IcHeartbeatOk || ic.IcKvpOk) {
        Log_Printf(L"[%s] integration services up (heartbeat %d, KVP %d)",
                   s->name, ic.IcHeartbeatOk, ic.IcKvpOk);
        return 1;
    }
    if (!ic.IcSeen && GetTickCount64() - startTick >= kIcGraceMs)
        return 1; /* no integration services at all: offer anyway */
    return 0;
}

static void apply_volume(WORKER* w)
{
    VM_SESSION* s = w->s;

    /* The UI writes volume/mute into the session; apply them here so the
     * WASAPI object is only touched from this thread, and only when they
     * change: the Windows volume mixer may have set the session's volume
     * since. */
    if ((int)s->volume != w->appliedVolume || (int)s->mute != w->appliedMute) {
        w->appliedVolume = (int)s->volume;
        w->appliedMute = (int)s->mute;
        AudioOut_SetVolume(w->st.audio, w->appliedVolume, w->appliedMute);
    }
}

/* Connected: CONSUMED every 10 ms (or every 16 PCM messages) until the
 * stream is idle. */
static void report_consumed(WORKER* w, ULONGLONG now)
{
    SESSION_STATE* st = &w->st;
    unsigned long long played, queued;
    int idle;
    DWORD err;

    if (!st->haveFormat ||
        (now < w->nextConsumed && st->pcmSinceConsumed < kConsumedEveryNMsgs))
        return;
    w->nextConsumed = now + kConsumedIntervalMs;
    st->pcmSinceConsumed = 0;

    played = AudioOut_PlayedBytes(st->audio);
    if (played > st->pcmIn)
        played = st->pcmIn;
    if (played < st->consumedSent)
        played = st->consumedSent;
    queued = (st->pcmIn > played) ? (st->pcmIn - played) : 0;
    idle = (played >= st->pcmIn) && (now - st->lastPcmTick > kIdleMs);
    if (idle) {
        if (st->streaming) {
            unsigned bpms = st->rate * st->channels * (st->bits / 8) / 1000;
            Log_Printf(L"[%s] stream idle: %u bytes in, %u reports, queue %u..%u ms; "
                       L"so far %u underruns, %u starved fills, %u bytes dropped",
                       w->s->name, (unsigned)st->pcmIn, st->reports,
                       st->queueMin == 0xFFFFFFFFu ? 0 : st->queueMin / (bpms ? bpms : 1),
                       st->queueMax / (bpms ? bpms : 1), AudioOut_Underruns(st->audio),
                       AudioOut_Starved(st->audio), AudioOut_Dropped(st->audio));
            st->streaming = FALSE;
            wav_close(st);
        }
        return;
    }
    if (st->guestDeaf)
        return;
    err = send_consumed(st, played, queued);
    if (err == WAIT_TIMEOUT) {
        /* The pipe is full: the guest's reader stopped with its stream.  Not
         * a broken channel; resume with its PCM. */
        Log_Printf(L"[%s] CONSUMED write timed out (%u bytes in, %u played); pausing reports",
                   w->s->name, (unsigned)st->pcmIn, (unsigned)played);
        st->guestDeaf = TRUE;
    } else if (err != 0) {
        drop_channel(w, now, L"CONSUMED write failed", err);
    } else {
        st->consumedSent = played;
        st->reports++;
        /* While PCM flows, past the prebuffer: the tail's drain is not
         * interesting. */
        if (played > 0 && now - st->lastPcmTick < 30) {
            if ((unsigned)queued < st->queueMin)
                st->queueMin = (unsigned)queued;
            if ((unsigned)queued > st->queueMax)
                st->queueMax = (unsigned)queued;
        }
    }
}

static DWORD WINAPI worker_main(LPVOID param)
{
    WORKER* w = (WORKER*)param;
    VM_SESSION* s = w->s;
    SESSION_STATE* st = &w->st;
    HANDLE waits[4];
    HANDLE mmcss;

    CoInitializeEx(0, COINIT_MULTITHREADED);
    post_status(s, kVsStarting, 0);
    {
        /* MMCSS: the thread feeds a 50 ms device buffer every 10 ms. */
        DWORD task = 0;
        mmcss = AvSetMmThreadCharacteristicsW(L"Pro Audio", &task);
        Log_Printf(L"[%s] session start%s", s->name, mmcss ? L"" : L" (no MMCSS)");
    }

    st->name = s->name;
    st->readBuf = (BYTE*)HeapAlloc(GetProcessHeap(), 0, kReadBufSize);
    PipeChannel_Init(&st->pipe, st->readBuf, kReadBufSize);
    if (st->readBuf)
        st->audio = AudioOut_Create(&s->vmId, s->name);
    if (!st->readBuf || !st->audio) {
        post_status(s, kVsError, ERROR_OUTOFMEMORY);
        goto done;
    }
    apply_volume(w);

    w->gateStart = GetTickCount64();
    w->phase = 0;
    for (;;) {
        DWORD wr;
        ULONGLONG now;
        int nwaits;

        now = GetTickCount64();

        if (InterlockedExchange(&s->stateChanged, 0)) {
            /* The guest is (re)booting: wait for it however long it takes. */
            if (w->offerDeadline)
                Log_Printf(L"[%s] VM state changed; waiting for the guest without a deadline", s->name);
            w->vmRestarted = TRUE;
            w->offerDeadline = 0;
        }

        if (w->phase == 0) {
            if (now >= w->nextOffer) {
                if (!gate_allows(s, w->gateStart)) {
                    post_status(s, kVsWaitIc, 0);
                    w->nextOffer = now + kIcPollMs;
                } else {
                    worker_offer(w, now);
                }
            }
            if (w->phase == 0) {
                if (WaitForSingleObject(s->stopEvent, 50) == WAIT_OBJECT_0)
                    break;
                continue;
            }
        }

        if (w->phase == 2 && !st->pipe.ReadPending && !PipeChannel_StartRead(&st->pipe)) {
            drop_channel(w, now, L"read start failed", st->pipe.LastError);
            continue;
        }

        nwaits = 0;
        waits[nwaits++] = s->stopEvent;
        if (w->phase == 1)
            waits[nwaits++] = st->pipe.ConnectEvent;
        if (w->phase == 2 && st->pipe.ReadPending)
            waits[nwaits++] = st->pipe.ReadEvent;
        if (w->phase == 2 && AudioOut_Event(st->audio))
            waits[nwaits++] = AudioOut_Event(st->audio);

        /* Wake at least every 10 ms for CONSUMED. */
        wr = WaitForMultipleObjects((DWORD)nwaits, waits, FALSE, kConsumedIntervalMs);
        if (wr == WAIT_OBJECT_0)
            break;
        now = GetTickCount64();

        if (w->phase == 1) {
            int rc = PipeChannel_WaitConnect(&st->pipe, 0);
            if (rc == 0) {
                Log_Printf(L"[%s] guest connected", s->name);
                w->phase = 2;
                w->reconnectMs = kReconnectFirstMs;
                w->vmRestarted = FALSE;
                w->nextConsumed = now;
                post_status(s, kVsConnected, 0);
            } else if (rc == 2) {
                drop_channel(w, now, L"connect failed", st->pipe.LastError);
            } else if (w->offerDeadline && now >= w->offerDeadline) {
                Log_Printf(L"[%s] guest did not open the channel within %u ms; offering again",
                           s->name, (unsigned)w->reconnectMs);
                PipeChannel_Kill(&st->pipe);
                w->reconnectMs = (w->reconnectMs * 2 < kReconnectMaxMs)
                               ? w->reconnectMs * 2 : kReconnectMaxMs;
                w->phase = 0;
                w->nextOffer = now + kReofferDelayMs;
            }
            continue;
        }

        /* phase == 2 */
        if (st->pipe.ReadPending) {
            DWORD got = 0;
            int rc = PipeChannel_WaitRead(&st->pipe, &got, 0);
            if (rc == 0) {
                on_read_done(st, got);
                st->pipe.ReadPending = FALSE;
            } else if (rc == 2) {
                drop_channel(w, now, L"read failed", st->pipe.LastError);
                continue;
            }
        }

        /* On every wake, not only when the buffer event fired: the event is
         * auto-reset, so the wait above has already consumed it, and new PCM
         * should reach the device at once.  Fill looks at the padding. */
        AudioOut_Fill(st->audio);
        AudioOut_Tick(st->audio);
        apply_volume(w);

        if (st->haveFormat && !st->formatOk)
            post_status(s, kVsBadFormat, st->rate);
        else if (st->haveFormat && now - st->lastPcmTick <= kPlayingHoldMs)
            post_status(s, AudioOut_IsOpen(st->audio) ? kVsPlaying : kVsNoDevice,
                        AudioOut_IsOpen(st->audio) ? st->rate : 0);
        else
            post_status(s, kVsConnected, 0);

        report_consumed(w, now);
    }

    /* Final CONSUMED so the guest sees the tail of the stream. */
    if (st->haveFormat && !st->guestDeaf && PipeChannel_IsOpen(&st->pipe)) {
        unsigned long long played = AudioOut_PlayedBytes(st->audio);
        if (played > st->pcmIn)
            played = st->pcmIn;
        if (played < st->consumedSent)
            played = st->consumedSent;
        send_consumed(st, played, (st->pcmIn > played) ? (st->pcmIn - played) : 0);
    }

done:
    wav_close(st);
    if (st->audio) {
        AudioOut_Destroy(st->audio);
        st->audio = 0;
    }
    PipeChannel_Kill(&st->pipe);
    PipeChannel_Free(&st->pipe);
    if (st->readBuf) {
        /* A cancelled read may still complete into the buffer; the memory is
         * only released after CancelIoEx + CloseHandle above. */
        HeapFree(GetProcessHeap(), 0, st->readBuf);
        st->readBuf = 0;
    }
    Log_Printf(L"[%s] session end", s->name);
    HvHost_ThreadDone();
    if (mmcss)
        AvRevertMmThreadCharacteristics(mmcss);
    InterlockedExchange(&s->alive, 0);
    CoUninitialize();
    return 0;
}

VM_SESSION* VmSession_Start(const HV_VM* vm, HWND hwnd, UINT statusMsg,
                            UINT_PTR cookie)
{
    VM_SESSION* s;

    if (!vm)
        return 0;
    s = (VM_SESSION*)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, sizeof(*s));
    if (!s)
        return 0;
    s->vmId = vm->Id;
    lstrcpynW(s->name, vm->Name, 64);
    s->hwnd = hwnd;
    s->statusMsg = statusMsg;
    s->cookie = cookie;
    s->volume = vm->Volume;
    s->mute = vm->Mute;
    s->stopEvent = CreateEventW(0, TRUE, FALSE, 0);
    s->alive = 1;
    if (!s->stopEvent) {
        HeapFree(GetProcessHeap(), 0, s);
        return 0;
    }
    s->worker = (WORKER*)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, sizeof(WORKER));
    if (!s->worker) {
        CloseHandle(s->stopEvent);
        HeapFree(GetProcessHeap(), 0, s);
        return 0;
    }
    s->worker->s = s;
    s->worker->appliedVolume = -1;
    s->worker->appliedMute = -1;
    s->thread = CreateThread(0, 0, worker_main, s->worker, 0, 0);
    if (!s->thread) {
        HeapFree(GetProcessHeap(), 0, s->worker);
        CloseHandle(s->stopEvent);
        HeapFree(GetProcessHeap(), 0, s);
        return 0;
    }
    return s;
}

void VmSession_Stop(VM_SESSION* s)
{
    if (!s)
        return;
    SetEvent(s->stopEvent);
    /* The worker notices the event within one wait (at most a CONSUMED write
     * timeout); it uses s until it returns, so wait for it. */
    WaitForSingleObject(s->thread, INFINITE);
    CloseHandle(s->thread);
    CloseHandle(s->stopEvent);
    HeapFree(GetProcessHeap(), 0, s->worker);
    HeapFree(GetProcessHeap(), 0, s);
}

void VmSession_SetVolume(VM_SESSION* s, int volume, int mute)
{
    if (!s)
        return;
    InterlockedExchange(&s->volume, volume);
    InterlockedExchange(&s->mute, mute ? 1 : 0);
    /* The worker applies it within one wait. */
}

void VmSession_NotifyStateChange(VM_SESSION* s)
{
    if (s)
        InterlockedExchange(&s->stateChanged, 1);
}

const GUID* VmSession_Id(const VM_SESSION* s)
{
    return s ? &s->vmId : 0;
}
