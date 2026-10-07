/*
 * trayui.h: notification icon, settings dialog, autostart task.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#ifndef _VMBAUDTRAY_TRAYUI_H_
#define _VMBAUDTRAY_TRAYUI_H_

#include <windows.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Creates the hidden window, the tray icon and the modeless settings dialog.
 * Returns FALSE on failure. */
BOOL TrayUi_Init(HINSTANCE inst);
void TrayUi_Shutdown(void);

/* Message loop; returns the exit code. */
int  TrayUi_Run(void);

/* Brings the settings window to the front (second start). */
void TrayUi_Show(void);

BOOL TrayUi_Autostart(void);
BOOL TrayUi_SetAutostart(BOOL on);

#ifdef __cplusplus
}
#endif

#endif /* _VMBAUDTRAY_TRAYUI_H_ */
