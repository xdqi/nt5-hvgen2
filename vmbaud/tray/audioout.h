/*
 * audioout.h: one WASAPI shared-mode render stream per VM.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#ifndef _VMBAUDTRAY_AUDIOOUT_H_
#define _VMBAUDTRAY_AUDIOOUT_H_

#include <windows.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct _AUDIO_OUT AUDIO_OUT;

/* Process-wide: CoCreate the device enumerator and register the default-device
 * notification.  Call once after CoInitialize. */
BOOL AudioOut_GlobalInit(void);
void AudioOut_GlobalShutdown(void);
/* Bumped whenever the default render device changes; streams reopen on it. */
LONG AudioOut_DeviceGeneration(void);

/* sessionGuid is used as the WASAPI session id (stable per VM).
 * displayName is what the Windows volume mixer shows. */
AUDIO_OUT* AudioOut_Create(const GUID* sessionGuid, const wchar_t* displayName);
void AudioOut_Destroy(AUDIO_OUT* ao);

/* (Re)open the default render device at the guest's format.  False leaves the
 * stream closed; PlayedBytes then falls back to a wall-clock estimate. */
BOOL AudioOut_Open(AUDIO_OUT* ao, unsigned rate, unsigned channels, unsigned bits);
void AudioOut_Close(AUDIO_OUT* ao);
BOOL AudioOut_IsOpen(const AUDIO_OUT* ao);

/* Append guest PCM.  Returns the number of bytes dropped (ring full). */
unsigned AudioOut_Enqueue(AUDIO_OUT* ao, const void* src, unsigned n);

/* The WASAPI event fired: pull from the ring into the device buffer. */
void AudioOut_Fill(AUDIO_OUT* ao);

/* 10 ms tick: start after prebuffer / stall, stop on underrun, reopen on a
 * device change. */
void AudioOut_Tick(AUDIO_OUT* ao);

/* Guest PCM bytes of this stream the host has played. */
unsigned long long AudioOut_PlayedBytes(AUDIO_OUT* ao);

void AudioOut_SetVolume(AUDIO_OUT* ao, int volume0to100, int mute);
/* New stream, same format: drop whatever is still queued. */
void AudioOut_ResetStream(AUDIO_OUT* ao);

unsigned AudioOut_Underruns(const AUDIO_OUT* ao);
unsigned AudioOut_Dropped(const AUDIO_OUT* ao);
/* Fills that found the device buffer empty while playing (audible gaps). */
unsigned AudioOut_Starved(const AUDIO_OUT* ao);
HANDLE   AudioOut_Event(const AUDIO_OUT* ao);

#ifdef __cplusplus
}
#endif

#endif /* _VMBAUDTRAY_AUDIOOUT_H_ */
