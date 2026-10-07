/*
 * audioout.cpp: WASAPI shared-mode render stream, one per VM.
 *
 * Replaces vmbaud-host.ps1's winmm path.  Differences from winmm:
 *  - The guest's small PCM messages go into a ring; the WASAPI buffer event
 *    pulls whatever the device needs (no 20 ms chunk joining).
 *  - Playback starts once 60 ms sit in the ring, and again after an underrun;
 *    if the guest stops sending for 30 ms we start anyway.  winmm paused the
 *    device; here we IAudioClient::Stop() / Start().  An underrun is the
 *    device buffer running empty with nothing in the ring; a partial fill is
 *    not one (the guest sends 10 ms at a time, the engine asks for more).
 *  - played comes from IAudioClock::GetPosition (the design), converted to
 *    guest bytes and clamped to what the device has been given since the
 *    last stop.  At a stop the device buffer is empty, so everything given is
 *    played: it moves into playedBase, and the clock reading is the new base
 *    (the clock stands still while stopped).
 *  - With no render device, played is estimated from the wall clock.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#include <windows.h>
#include <initguid.h>
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <audiopolicy.h>

#include "audioout.h"
#include "log.h"

#ifndef AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
#define AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM 0x80000000
#endif
#ifndef AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
#define AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY 0x08000000
#endif

/* Tunables carried over from vmbaud-host.ps1. */
static const unsigned kPrebufferMs   = 60;
static const unsigned kStallMs       = 30;
static const unsigned kRingCap       = 1024 * 1024;

/* ------------------------------------------------------------------ */
/* Global device enumerator + default-device notification             */
/* ------------------------------------------------------------------ */

static IMMDeviceEnumerator* g_enum;
static LONG g_devGen;

class DeviceNotify : public IMMNotificationClient {
public:
    LONG refs;

    DeviceNotify() : refs(0) {}

    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void** ppv) {
        if (!ppv)
            return E_POINTER;
        if (riid == IID_IUnknown || riid == IID_IMMNotificationClient) {
            *ppv = static_cast<IMMNotificationClient*>(this);
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
        /* A single static instance; the count is never driven to zero. */
        return (ULONG)InterlockedDecrement(&refs);
    }
    HRESULT STDMETHODCALLTYPE OnDeviceStateChanged(LPCWSTR, DWORD) {
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE OnDeviceAdded(LPCWSTR) {
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE OnDeviceRemoved(LPCWSTR) {
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE OnDefaultDeviceChanged(EDataFlow flow, ERole, LPCWSTR) {
        if (flow == eRender || flow == eAll)
            InterlockedIncrement(&g_devGen);
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE OnPropertyValueChanged(LPCWSTR, const PROPERTYKEY) {
        return S_OK;
    }
};

static DeviceNotify g_notify;

BOOL AudioOut_GlobalInit(void)
{
    HRESULT hr;

    if (g_enum)
        return TRUE;
    hr = CoCreateInstance(CLSID_MMDeviceEnumerator, 0, CLSCTX_INPROC_SERVER,
                          IID_IMMDeviceEnumerator, (void**)&g_enum);
    if (FAILED(hr) || !g_enum) {
        g_enum = 0;
        return FALSE;
    }
    g_notify.AddRef();
    g_enum->RegisterEndpointNotificationCallback(&g_notify);
    return TRUE;
}

void AudioOut_GlobalShutdown(void)
{
    if (g_enum) {
        g_enum->UnregisterEndpointNotificationCallback(&g_notify);
        g_enum->Release();
        g_enum = 0;
    }
}

LONG AudioOut_DeviceGeneration(void)
{
    return g_devGen;
}

/* ------------------------------------------------------------------ */
/* One VM's stream                                                    */
/* ------------------------------------------------------------------ */

struct _AUDIO_OUT {
    CRITICAL_SECTION cs;
    GUID     sessionGuid;
    wchar_t  name[64];
    HANDLE   event;

    BYTE*    ring;
    unsigned ringCap, ringHead, ringLen;

    unsigned rate, channels, bits, blockAlign, avgBytes;
    BOOL     haveFormat;

    IAudioClient*        client;
    IAudioRenderClient*  render;
    IAudioClock*         clock;
    ISimpleAudioVolume*  vol;
    IAudioSessionControl* session;
    unsigned bufFrames;
    BOOL     started;     /* IAudioClient::Start issued */
    BOOL     primed;      /* past the prebuffer and running */

    /* Accounting.  played = playedBase + min(clock - clockBase, submitted).
     * At every stop (device buffer empty) submitted moves into playedBase and
     * clockBase becomes the clock reading; frames written before Start() thus
     * count, and the stopped time does not. */
    unsigned long long playedBase;
    unsigned long long clockBase;
    unsigned long long submitted;
    unsigned long long accepted;    /* guest bytes kept (not dropped) this stream */
    ULONGLONG streamStartTick;
    ULONGLONG lastPcmTick;

    int volume0to100, mute;
    unsigned underruns, dropped;
    unsigned starved;   /* fills that found the device buffer already empty */
    LONG     devGen;
};

static unsigned long long read_clock_bytes(AUDIO_OUT* ao)
{
    UINT64 pos = 0, freq = 0;
    unsigned long long frames;

    if (!ao->clock)
        return 0;
    if (FAILED(ao->clock->GetFrequency(&freq)) || freq == 0)
        return 0;
    if (FAILED(ao->clock->GetPosition(&pos, 0)))
        return 0;
    /* The stream is opened at the guest rate, so one clock frame is one
     * guest frame; scale anyway if the engine says otherwise. */
    if (freq == ao->rate)
        frames = pos;
    else
        frames = (unsigned long long)((double)pos * (double)ao->rate / (double)freq);
    return frames * ao->blockAlign;
}

static void release_stream(AUDIO_OUT* ao)
{
    if (ao->session) { ao->session->Release(); ao->session = 0; }
    if (ao->vol)     { ao->vol->Release();     ao->vol = 0; }
    if (ao->clock)   { ao->clock->Release();   ao->clock = 0; }
    if (ao->render)  { ao->render->Release();  ao->render = 0; }
    if (ao->client) {
        if (ao->started)
            ao->client->Stop();
        ao->client->Release();
        ao->client = 0;
    }
    ao->started = FALSE;
    ao->primed = FALSE;
    ao->bufFrames = 0;
}

static void apply_volume(AUDIO_OUT* ao)
{
    if (ao->vol) {
        float f = (float)ao->volume0to100 / 100.0f;
        if (f < 0.0f) f = 0.0f;
        if (f > 1.0f) f = 1.0f;
        ao->vol->SetMasterVolume(f, 0);
        ao->vol->SetMute(ao->mute ? TRUE : FALSE, 0);
    }
}

static BOOL open_device(AUDIO_OUT* ao)
{
    IMMDevice* dev = 0;
    WAVEFORMATEX fmt;
    HRESULT hr;
    /* 50 ms: the engine takes 10 ms per period, so this leaves 40 ms for a
     * late worker wake-up before it plays silence.  The latency does not
     * matter much: the guest's clock follows the host queue either way. */
    REFERENCE_TIME dur = 50 * 10000;

    release_stream(ao);
    ao->devGen = AudioOut_DeviceGeneration();

    if (!g_enum)
        return FALSE;
    hr = g_enum->GetDefaultAudioEndpoint(eRender, eMultimedia, &dev);
    if (FAILED(hr) || !dev)
        return FALSE;

    ZeroMemory(&fmt, sizeof(fmt));
    fmt.wFormatTag = WAVE_FORMAT_PCM;
    fmt.nChannels = (WORD)ao->channels;
    fmt.nSamplesPerSec = ao->rate;
    fmt.wBitsPerSample = (WORD)ao->bits;
    fmt.nBlockAlign = (WORD)ao->blockAlign;
    fmt.nAvgBytesPerSec = ao->avgBytes;
    fmt.cbSize = 0;

    hr = dev->Activate(IID_IAudioClient, CLSCTX_INPROC_SERVER, 0, (void**)&ao->client);
    if (FAILED(hr) || !ao->client) {
        dev->Release();
        return FALSE;
    }
    hr = ao->client->Initialize(AUDCLNT_SHAREMODE_SHARED,
                                AUDCLNT_STREAMFLAGS_EVENTCALLBACK |
                                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM |
                                AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                                dur, 0, &fmt, &ao->sessionGuid);
    if (FAILED(hr)) {
        /* Retry without the buffer duration hint (some builds reject 20 ms). */
        hr = ao->client->Initialize(AUDCLNT_SHAREMODE_SHARED,
                                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK |
                                    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM |
                                    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                                    0, 0, &fmt, &ao->sessionGuid);
    }
    if (FAILED(hr)) {
        dev->Release();
        release_stream(ao);
        return FALSE;
    }

    ao->client->SetEventHandle(ao->event);
    ao->client->GetBufferSize(&ao->bufFrames);
    ao->client->GetService(IID_IAudioRenderClient, (void**)&ao->render);
    ao->client->GetService(IID_IAudioClock, (void**)&ao->clock);
    ao->client->GetService(IID_ISimpleAudioVolume, (void**)&ao->vol);
    ao->client->GetService(IID_IAudioSessionControl, (void**)&ao->session);
    if (ao->session && ao->name[0])
        ao->session->SetDisplayName(ao->name, 0);
    apply_volume(ao);

    dev->Release();
    /* A new IAudioClient's clock starts at 0.  What the old device was given
     * counts as played (the guest must not see its position go back). */
    ao->playedBase += ao->submitted;
    ao->submitted = 0;
    ao->clockBase = 0;
    ao->started = FALSE;
    ao->primed = FALSE;
    return ao->render != 0;
}

AUDIO_OUT* AudioOut_Create(const GUID* sessionGuid, const wchar_t* displayName)
{
    AUDIO_OUT* ao;

    ao = (AUDIO_OUT*)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, sizeof(*ao));
    if (!ao)
        return 0;
    InitializeCriticalSection(&ao->cs);
    ao->ringCap = kRingCap;
    ao->ring = (BYTE*)HeapAlloc(GetProcessHeap(), 0, ao->ringCap);
    if (!ao->ring) {
        DeleteCriticalSection(&ao->cs);
        HeapFree(GetProcessHeap(), 0, ao);
        return 0;
    }
    ao->event = CreateEventW(0, FALSE, FALSE, 0);
    if (sessionGuid)
        ao->sessionGuid = *sessionGuid;
    if (displayName)
        lstrcpynW(ao->name, displayName, (int)(sizeof(ao->name) / sizeof(ao->name[0])));
    ao->volume0to100 = 100;
    return ao;
}

void AudioOut_Destroy(AUDIO_OUT* ao)
{
    if (!ao)
        return;
    EnterCriticalSection(&ao->cs);
    release_stream(ao);
    LeaveCriticalSection(&ao->cs);
    if (ao->event)
        CloseHandle(ao->event);
    if (ao->ring)
        HeapFree(GetProcessHeap(), 0, ao->ring);
    DeleteCriticalSection(&ao->cs);
    HeapFree(GetProcessHeap(), 0, ao);
}

BOOL AudioOut_Open(AUDIO_OUT* ao, unsigned rate, unsigned channels, unsigned bits)
{
    BOOL ok;

    if (!ao || !rate || !channels || !bits)
        return FALSE;
    EnterCriticalSection(&ao->cs);
    ao->rate = rate;
    ao->channels = channels;
    ao->bits = bits;
    ao->blockAlign = channels * (bits / 8);
    ao->avgBytes = rate * ao->blockAlign;
    ao->haveFormat = TRUE;
    ao->ringHead = 0;
    ao->ringLen = 0;
    ao->playedBase = 0;
    ao->clockBase = 0;
    ao->submitted = 0;
    ao->accepted = 0;
    ao->streamStartTick = GetTickCount64();
    ao->lastPcmTick = 0;
    ok = open_device(ao);
    ao->playedBase = 0;     /* a new stream: count from zero */
    LeaveCriticalSection(&ao->cs);
    return ok;
}

void AudioOut_Close(AUDIO_OUT* ao)
{
    if (!ao)
        return;
    EnterCriticalSection(&ao->cs);
    release_stream(ao);
    ao->haveFormat = FALSE;
    LeaveCriticalSection(&ao->cs);
}

BOOL AudioOut_IsOpen(const AUDIO_OUT* ao)
{
    return ao && ao->client && ao->render;
}

unsigned AudioOut_Enqueue(AUDIO_OUT* ao, const void* src, unsigned n)
{
    unsigned dropped = 0;

    if (!ao || !src || n == 0)
        return 0;
    EnterCriticalSection(&ao->cs);
    ao->lastPcmTick = GetTickCount64();
    if (ao->ringLen + n > ao->ringCap) {
        /* Keep the tail: drop from the head so the newest audio survives. */
        unsigned need = ao->ringLen + n - ao->ringCap;
        if (need > ao->ringLen)
            need = ao->ringLen;
        ao->ringHead = (ao->ringHead + need) % ao->ringCap;
        ao->ringLen -= need;
        dropped += need;
    }
    {
        unsigned first = ao->ringCap - ((ao->ringHead + ao->ringLen) % ao->ringCap);
        unsigned take = (n < first) ? n : first;
        CopyMemory(ao->ring + ((ao->ringHead + ao->ringLen) % ao->ringCap), src, take);
        if (take < n)
            CopyMemory(ao->ring, (const BYTE*)src + take, n - take);
        ao->ringLen += n;
    }
    ao->accepted += (n - dropped);
    ao->dropped += dropped;
    LeaveCriticalSection(&ao->cs);
    return dropped;
}

/* Moves what fits from the ring into the device buffer.  Holding cs. */
static void fill_locked(AUDIO_OUT* ao)
{
    UINT32 pad = 0, want;
    BYTE* data = 0;
    unsigned give;
    HRESULT hr;

    if (!ao->client || !ao->render)
        return;
    hr = ao->client->GetCurrentPadding(&pad);
    if (FAILED(hr)) {
        if (hr == AUDCLNT_E_DEVICE_INVALIDATED)
            open_device(ao);
        return;
    }
    want = (ao->bufFrames > pad) ? (ao->bufFrames - pad) : 0;
    if (want == 0)
        return;
    hr = ao->render->GetBuffer(want, &data);
    if (FAILED(hr) || !data) {
        if (hr == AUDCLNT_E_DEVICE_INVALIDATED)
            open_device(ao);
        return;
    }
    give = want * ao->blockAlign;
    if (give > ao->ringLen)
        give = ao->ringLen;
    if (give > 0) {
        unsigned first = ao->ringCap - ao->ringHead;
        if (first > give)
            first = give;
        CopyMemory(data, ao->ring + ao->ringHead, first);
        if (first < give)
            CopyMemory(data + first, ao->ring, give - first);
        ao->ringHead = (ao->ringHead + give) % ao->ringCap;
        ao->ringLen -= give;
    }
    /* Release only the frames we filled: the rest of the buffer stays empty
     * and the engine plays silence there. */
    ao->render->ReleaseBuffer(give / ao->blockAlign, 0);
    if (pad == 0 && ao->started && ao->submitted > 0) {
        /* The engine ran dry before this fill: a gap (or the stream's end). */
        ao->starved++;
        Log_Printf(L"[%s] device buffer was empty at %u ms of output, %u ms left in the ring",
                   ao->name, (unsigned)((ao->playedBase + ao->submitted) * 1000 / ao->avgBytes),
                   ao->ringLen * 1000 / ao->avgBytes);
    }
    ao->submitted += give;
    if (give == 0 && pad == 0 && ao->started) {
        /* The device has played everything and the ring is empty: an
         * underrun (or the end of the stream).  Stop, and prebuffer again
         * before the next start. */
        ao->underruns++;
        ao->client->Stop();
        ao->started = FALSE;
        ao->primed = FALSE;
        ao->playedBase += ao->submitted;
        ao->submitted = 0;
        ao->clockBase = read_clock_bytes(ao);
    }
}

void AudioOut_Fill(AUDIO_OUT* ao)
{
    if (!ao)
        return;
    EnterCriticalSection(&ao->cs);
    /* Until Start the PCM stays in the ring, so the prebuffer and stall
     * checks in AudioOut_Tick see all of it. */
    if (ao->started)
        fill_locked(ao);
    LeaveCriticalSection(&ao->cs);
}

void AudioOut_Tick(AUDIO_OUT* ao)
{
    if (!ao)
        return;
    EnterCriticalSection(&ao->cs);

    if (ao->haveFormat && ao->client &&
        ao->devGen != AudioOut_DeviceGeneration()) {
        /* Default device changed; the guest must not notice (open_device
         * keeps the played count). */
        open_device(ao);
    }

    if (ao->haveFormat && ao->client && ao->render && !ao->started) {
        unsigned need = (ao->avgBytes * kPrebufferMs) / 1000;
        ULONGLONG now = GetTickCount64();
        BOOL stalled = ao->lastPcmTick != 0 && (now - ao->lastPcmTick) >= kStallMs;
        if (ao->ringLen >= need || (stalled && ao->ringLen >= ao->blockAlign)) {
            /* Pre-roll the device buffer, then start. */
            fill_locked(ao);
            if (SUCCEEDED(ao->client->Start())) {
                ao->started = TRUE;
                ao->primed = TRUE;
            }
        }
    }

    LeaveCriticalSection(&ao->cs);
}

unsigned long long AudioOut_PlayedBytes(AUDIO_OUT* ao)
{
    unsigned long long played;

    if (!ao)
        return 0;
    EnterCriticalSection(&ao->cs);
    if (ao->client && ao->clock) {
        unsigned long long now = read_clock_bytes(ao);
        unsigned long long delta = (now > ao->clockBase) ? (now - ao->clockBase) : 0;
        if (delta > ao->submitted)
            delta = ao->submitted;
        played = ao->playedBase + delta;
    } else if (ao->haveFormat && ao->avgBytes) {
        /* No device: the wall clock keeps the guest's writer moving, the way
         * vmbaud-host.ps1's EstimatePlayed does (minus a small pipeline delay). */
        ULONGLONG now = GetTickCount64();
        ULONGLONG elapsed = 0;
        if (now > ao->streamStartTick + 50)
            elapsed = now - ao->streamStartTick - 50;
        played = (unsigned long long)((double)elapsed * (double)ao->avgBytes / 1000.0);
        if (played > ao->accepted)
            played = ao->accepted;
    } else {
        played = 0;
    }
    LeaveCriticalSection(&ao->cs);
    return played;
}

void AudioOut_SetVolume(AUDIO_OUT* ao, int volume0to100, int mute)
{
    if (!ao)
        return;
    if (volume0to100 < 0) volume0to100 = 0;
    if (volume0to100 > 100) volume0to100 = 100;
    EnterCriticalSection(&ao->cs);
    ao->volume0to100 = volume0to100;
    ao->mute = mute ? 1 : 0;
    apply_volume(ao);
    LeaveCriticalSection(&ao->cs);
}

void AudioOut_ResetStream(AUDIO_OUT* ao)
{
    if (!ao)
        return;
    EnterCriticalSection(&ao->cs);
    ao->ringHead = 0;
    ao->ringLen = 0;
    ao->playedBase = 0;
    ao->clockBase = 0;
    ao->submitted = 0;
    ao->accepted = 0;
    ao->lastPcmTick = 0;
    if (ao->client) {
        if (ao->started) {
            ao->client->Stop();
            ao->started = FALSE;
        }
        ao->client->Reset();
        ao->clockBase = 0;
    }
    ao->primed = FALSE;
    LeaveCriticalSection(&ao->cs);
}

unsigned AudioOut_Underruns(const AUDIO_OUT* ao)
{
    return ao ? ao->underruns : 0;
}

unsigned AudioOut_Dropped(const AUDIO_OUT* ao)
{
    return ao ? ao->dropped : 0;
}

unsigned AudioOut_Starved(const AUDIO_OUT* ao)
{
    return ao ? ao->starved : 0;
}

HANDLE AudioOut_Event(const AUDIO_OUT* ao)
{
    return ao ? ao->event : 0;
}
