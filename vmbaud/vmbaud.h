/*
 * vmbaud.h: shared definitions for the VMBus-pipe WaveCyclic render driver.
 *
 * Distributed under the MS-PL; see LICENSE in this directory.
 */
#ifndef _VMBAUD_H_
#define _VMBAUD_H_

/* VMBus interface type of the audio channel; must match vmbaud-host.ps1. */
#define STATIC_VBAUD_INTERFACE_TYPE \
    0x8b57f4e3, 0x2a3c, 0x4f6e, 0x9c, 0x8d, 0x1e, 0x5a, 0x70, 0xb9, 0xc4, 0xd2

#define VBAUD_POOLTAG   'abmV'
#define VBAUD_VERSION   1
#define VBAUD_REVISION  0

/* DMA buffer: PortCls asks for one; CopyTo does not use it for the data path. */
#define VBAUD_DMA_BUFFER_SIZE   0x20000

/*
 * Stream clock: the play position runs on the guest's performance counter at
 * the nominal rate, trimmed by up to VBAUD_MAX_TRIM_PPM so that the host's
 * queue of not yet played PCM stays near VBAUD_TARGET_DEPTH_MS.  The host's
 * sound card thus sets the long-term pace while the position the port sees
 * advances smoothly, like a real device's.
 */
#define VBAUD_TARGET_DEPTH_MS   60
#define VBAUD_MAX_TRIM_PPM      5000

/* Notification period until SetNotificationFreq sets one. */
#define VBAUD_DEFAULT_INTERVAL_MS   20

/* Wire protocol: one pipe write per message, 8-byte LE header + payload. */
#define VBAUD_MSG_FORMAT    1   /* u32 rate, u32 channels, u32 bits */
#define VBAUD_MSG_PCM_OUT   2   /* raw interleaved PCM */
#define VBAUD_MSG_CONSUMED  3   /* u64 played, u32 queued, host -> guest */

#define VBAUD_HEADER_SIZE   8
#define VBAUD_MAX_MESSAGE   (1024 * 1024)
#define VBAUD_READ_SIZE     32

#ifndef KSPROPERTY_TYPE_ALL
#define KSPROPERTY_TYPE_ALL \
    (KSPROPERTY_TYPE_BASICSUPPORT | KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET)
#endif

#include <pshpack1.h>
typedef struct _VBAUD_HEADER {
    ULONG Type;
    ULONG Size;         /* payload bytes after the header */
} VBAUD_HEADER, *PVBAUD_HEADER;

typedef struct _VBAUD_FORMAT {
    ULONG Rate;
    ULONG Channels;
    ULONG Bits;
} VBAUD_FORMAT, *PVBAUD_FORMAT;

typedef struct _VBAUD_CONSUMED {
    ULONGLONG Played;   /* PCM bytes of this stream the host has played */
    ULONG     Queued;   /* PCM bytes received but not yet played */
} VBAUD_CONSUMED, *PVBAUD_CONSUMED;
#include <poppack.h>

/* Wave filter pins / nodes. */
enum {
    KSPIN_WAVE_RENDER_SINK = 0,
    KSPIN_WAVE_RENDER_SOURCE
};

enum {
    KSNODE_WAVE_DAC = 0
};

/* Topology pins / nodes. */
enum {
    KSPIN_TOPO_WAVEOUT_SOURCE = 0,
    KSPIN_TOPO_LINEOUT_DEST
};

enum {
    KSNODE_TOPO_VOLUME = 0
};

#endif /* _VMBAUD_H_ */
