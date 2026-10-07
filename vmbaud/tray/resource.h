/*
 * resource.h: ids for the vmbaudtray resources.
 *
 * Distributed under the MS-PL; see LICENSE in the vmbaud directory.
 */
#ifndef _VMBAUDTRAY_RESOURCE_H_
#define _VMBAUDTRAY_RESOURCE_H_

#define IDI_APP         101

#define IDD_SETTINGS    200
#define IDC_LIST        201
#define IDC_VOLUME      202
#define IDC_MUTE        203
#define IDC_AUTOSTART   204
#define IDC_VOL_LABEL   205     /* the group box around the volume controls */
#define IDC_VOL_VALUE   206

/* STRINGTABLE ids (shared by the English and Simplified Chinese tables). */
#define IDS_APP_TITLE       1
#define IDS_MENU_SETTINGS   2
#define IDS_MENU_AUTOSTART  3
#define IDS_MENU_EXIT       4
#define IDS_COL_NAME        5
#define IDS_COL_STATE       6
#define IDS_COL_STATUS      7
#define IDS_GRP_SOUND       8       /* %d = count */
#define IDS_GRP_RUNNING     9
#define IDS_GRP_OFF         10
#define IDS_STATE_RUNNING   11
#define IDS_STATE_OFF       12
#define IDS_STATE_PAUSED    13
#define IDS_STATE_SAVED     14
#define IDS_STATE_STARTING  15
#define IDS_STATE_STOPPING  16
#define IDS_STATE_OTHER     17
#define IDS_ST_OFF          18      /* sound not enabled */
#define IDS_ST_WAITVM       19      /* enabled, VM not running */
#define IDS_ST_STARTING     20
#define IDS_ST_WAITIC       21
#define IDS_ST_OFFERING     22
#define IDS_ST_CONNECTED    23
#define IDS_ST_PLAYING      24      /* %u = sample rate */
#define IDS_ST_REOFFER      25      /* %u = Win32 error */
#define IDS_ST_OFFERFAIL    26      /* %u = Win32 error */
#define IDS_ST_NODEVICE     27
#define IDS_ST_ERROR        28      /* %u = Win32 error */
#define IDS_VOLUME_FOR      29      /* %s = VM name */
#define IDS_VOLUME_HINT     30
#define IDS_MUTE            31
#define IDS_WMI_FAIL        32
#define IDS_PIPE_FAIL       33
#define IDS_TIP_NONE        34
#define IDS_TIP_ONE         35
#define IDS_TIP_MANY        36      /* %d = count */
#define IDS_ST_BADFORMAT    37      /* %u = sample rate */

/* Private messages (not resources). */
#define WM_APP_VMUPDATE (WM_APP + 1)
#define WM_APP_STATUS   (WM_APP + 2)
#define WM_APP_SHOW     (WM_APP + 3)

#endif /* _VMBAUDTRAY_RESOURCE_H_ */
