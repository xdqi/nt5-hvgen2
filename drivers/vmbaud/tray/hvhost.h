/*
 * hvhost.h: Hyper-V WMI (root\virtualization\v2): list VMs, host-only KVP
 * settings, state-change events.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#ifndef _VMBAUDTRAY_HVHOST_H_
#define _VMBAUDTRAY_HVHOST_H_

#include <windows.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct _HV_VM {
    GUID     Id;
    wchar_t  Name[64];
    int      EnabledState;      /* 2 = Running */
    int      SoundEnabled;      /* 0/1, default 0 */
    int      Volume;            /* 0..100, default 100 */
    int      Mute;              /* 0/1, default 0 */
    int      IcHeartbeatOk;     /* Heartbeat IC reports OK */
    int      IcKvpOk;           /* KVP IC reports OK */
    int      IcSeen;            /* at least one IC component exists */
} HV_VM;

/* CoCreate the locator and open root\virtualization\v2. */
BOOL HvHost_Open(void);
void HvHost_Close(void);
BOOL HvHost_IsOpen(void);

/* Lists the VMs (skips the host) and fills their host-only KVP settings.
 * Returns the count written to out, or -1 if the query failed. */
int  HvHost_ListVms(HV_VM* out, int max);

/* Re-reads one VM's KVP settings into out (enabled/volume/mute). */
BOOL HvHost_ReadSettings(const GUID* vmId, HV_VM* out);

/* Writes / removes one host-only KVP item (Source = 4), waiting for the
 * job.  TRUE once a read back shows the change. */
BOOL HvHost_WriteSetting(const GUID* vmId, const wchar_t* key, const wchar_t* value);
BOOL HvHost_RemoveSetting(const GUID* vmId, const wchar_t* key);

/* Heartbeat / KVP integration service status.  Callable from any thread;
 * a thread other than the one that opened HvHost calls HvHost_ThreadDone
 * before it ends. */
BOOL HvHost_ReadIntegrationServices(const GUID* vmId, HV_VM* out);
void HvHost_ThreadDone(void);

/* Posts msg to hwnd on each Msvm_ComputerSystem state change. */
BOOL HvHost_StartEvents(HWND hwnd, UINT msg);
void HvHost_StopEvents(void);

#ifdef __cplusplus
}
#endif

#endif /* _VMBAUDTRAY_HVHOST_H_ */
