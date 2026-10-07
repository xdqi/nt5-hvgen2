/*
 * vmsession.h: one worker thread per VM with sound on: offer lifecycle,
 * wire protocol, jitter buffer, CONSUMED reports.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#ifndef _VMBAUDTRAY_VMSESSION_H_
#define _VMBAUDTRAY_VMSESSION_H_

#include <windows.h>
#include "hvhost.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct _VM_SESSION VM_SESSION;

/* Offer timing.  Default: offer as soon as the VM is Running.  The other
 * gate waits until Heartbeat or KVP reports OK, with a fallback to "when
 * Running" for guests that have no integration services (Windows 9x).
 * Flipped by --wait-ic. */
enum {
    kOfferWhenRunning = 0,
    kOfferWhenIntegrationServiceOk = 1
};
void VmSession_SetOfferGate(int gate);
int  VmSession_GetOfferGate(void);

/* vmbaudcli --wav: every stream the guest sends is also written to
 * <prefix>-<n>.wav, as received.  0 = off. */
void VmSession_SetCapture(const wchar_t* prefix);

/* Worker status, posted to the UI as statusMsg with wParam = the cookie
 * given to VmSession_Start and lParam = VS_STATUS(code, Win32 error).
 * Posted only when it changes. */
enum {
    kVsStarting = 1,
    kVsWaitIc,          /* --wait-ic: no Heartbeat/KVP yet */
    kVsOffering,        /* offered, the guest has not opened it */
    kVsOfferFailed,     /* err; retried */
    kVsConnected,       /* open, no PCM flowing */
    kVsPlaying,         /* PCM flowing; err = its sample rate */
    kVsNoDevice,        /* PCM flowing, no host output device */
    kVsReoffer,         /* channel broke; offering again shortly */
    kVsError,           /* err; the session gave up */
    kVsBadFormat        /* err = the rate of a FORMAT the host cannot play */
};
#define VS_STATUS(code, err)  ((LPARAM)(((ULONG_PTR)(DWORD)(err) << 8) | (code)))
#define VS_CODE(lp)           ((int)((ULONG_PTR)(lp) & 0xFF))
#define VS_ERR(lp)            ((DWORD)((ULONG_PTR)(lp) >> 8))

VM_SESSION* VmSession_Start(const HV_VM* vm, HWND hwnd, UINT statusMsg,
                            UINT_PTR cookie);
void VmSession_Stop(VM_SESSION* s);

void VmSession_SetVolume(VM_SESSION* s, int volume, int mute);
/* The VM changed state (a reset shows as Running -> Stopping -> Running):
 * the guest is booting, so the next offer waits for it without a deadline. */
void VmSession_NotifyStateChange(VM_SESSION* s);
const GUID* VmSession_Id(const VM_SESSION* s);

#ifdef __cplusplus
}
#endif

#endif /* _VMBAUDTRAY_VMSESSION_H_ */
