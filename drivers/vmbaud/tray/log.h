/*
 * log.h: a small diagnostic log in %TEMP%, rewritten at every start, and/or
 * echoed to a handle (vmbaudcli: the console).  One line per event (offers,
 * connects, stream starts and ends, channel errors), never per packet.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#ifndef _VMBAUDTRAY_LOG_H_
#define _VMBAUDTRAY_LOG_H_

#include <windows.h>

#ifdef __cplusplus
extern "C" {
#endif

/* fileName: in %TEMP%, or 0 for none.  echo: also written there, or 0. */
void Log_Open(const wchar_t* fileName, HANDLE echo);
void Log_Close(void);
/* wsprintf format (no 64-bit or floating-point conversions); thread-safe. */
void Log_Printf(const wchar_t* fmt, ...);

#ifdef __cplusplus
}
#endif

#endif /* _VMBAUDTRAY_LOG_H_ */
