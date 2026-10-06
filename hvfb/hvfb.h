/*
 * hvfb - linear frame buffer display miniport for Windows XP on machines
 * without VGA hardware (Hyper-V Generation 2 booted through CSMWrap).
 *
 * Written from the public VideoPort documentation (Windows DDK/WDK) and the
 * VESA BIOS Extension Core Functions Standard 3.0.
 */
#ifndef HVFB_H
#define HVFB_H

/* The mingw-w64 DDK headers need exactly this order for a miniport. */
#include <ntdef.h>
#include <dderror.h>
#include <devioctl.h>
#include <miniport.h>
#include <ntddvdeo.h>
#include <video.h>

#define HVFB_MAX_MODES      64
#define HVFB_NO_VBE_MODE    0xFFFF          /* mode cannot be set, firmware already did */
#define HVFB_NO_MODE        ((ULONG)-1)

/* One display mode as offered to the display driver / text-mode setup. */
typedef struct _HVFB_MODE {
    USHORT VbeMode;                 /* VBE mode number or HVFB_NO_VBE_MODE */
    UCHAR  Bpp;                     /* storage bits per pixel: 16, 24 or 32 */
    UCHAR  Depth;                   /* significant colour bits: 15, 16, 24 */
    ULONG  Width;
    ULONG  Height;
    ULONG  Stride;                  /* bytes per scan line in linear mode */
    ULONG  VramLength;              /* bytes usable at FrameBuffer (>= Stride * Height) */
    PHYSICAL_ADDRESS FrameBuffer;
    UCHAR  RedSize, RedPos;
    UCHAR  GreenSize, GreenPos;
    UCHAR  BlueSize, BluePos;
    /*
     * Centred inside a fixed, larger scan-out (see HvfbCentreModes): the
     * picture starts at pixel (OffsetX, OffsetY) of the frame buffer that
     * the coreboot table describes, and the border around it is visible.
     */
    BOOLEAN Centred;
    ULONG  OffsetX, OffsetY;
} HVFB_MODE, *PHVFB_MODE;

/* Scratch space for VBE discovery; kept out of the (small) kernel stack. */
typedef struct _HVFB_SCRATCH {
    UCHAR  Block[512];              /* VbeInfoBlock / ModeInfoBlock copy */
    USHORT VbeModes[256];           /* copy of the VBE mode list */
} HVFB_SCRATCH;

typedef struct _HVFB_EXTENSION {
    /* INT 10h through videoprt (VideoPortQueryServices, XP and later). */
    VIDEO_PORT_INT10_INTERFACE Int10;
    BOOLEAN HaveInt10;
    BOOLEAN Initialized;            /* HwInitialize ran discovery */
    BOOLEAN UsingVbe;               /* mode list came from VBE */
    BOOLEAN HaveCbFb;               /* coreboot table frame buffer found */

    USHORT VbeVersion;
    ULONG  VbeMemory;               /* VbeInfoBlock.TotalMemory in bytes */
    HVFB_MODE CbFb;                 /* frame buffer from the coreboot table */

    ULONG  NumModes;
    ULONG  CurrentMode;             /* index into Modes or HVFB_NO_MODE */
    HVFB_MODE Modes[HVFB_MAX_MODES];

    /* Kernel mapping of a frame buffer, used to clear it on mode sets. */
    PVOID  FbVirt;
    PHYSICAL_ADDRESS FbVirtPhys;
    ULONG  FbVirtLength;

    /* Kernel mapping of the colour text buffer at 0xB8000. */
    PVOID  TextVirt;

    HVFB_SCRATCH Scratch;
} HVFB_EXTENSION, *PHVFB_EXTENSION;

/*
 * Logging.  VideoPortDebugPrint(Error, ...) is shown by a free-build XP
 * kernel debugger without changing the DbgPrint filter masks, so the few
 * one-time discovery messages use it; per-request traces use Trace.
 */
#define HvfbLog(...)    VideoPortDebugPrint(Error, "hvfb: " __VA_ARGS__)
#define HvfbTrace(...)  VideoPortDebugPrint(Trace, "hvfb: " __VA_ARGS__)

/* modes.c */
VOID HvfbReadCorebootTable(PHVFB_EXTENSION Ext);
BOOLEAN HvfbQueryVbeModes(PHVFB_EXTENSION Ext);
VOID HvfbFinishModeList(PHVFB_EXTENSION Ext);
VP_STATUS HvfbCallBios(PHVFB_EXTENSION Ext, PINT10_BIOS_ARGUMENTS Args);

/* Physical address and length of the picture of a mode (its first pixel on). */
static inline PHYSICAL_ADDRESS
HvfbPictureAddress(const HVFB_MODE *Mode)
{
    PHYSICAL_ADDRESS pa = Mode->FrameBuffer;

    if (Mode->Centred)
        pa.QuadPart += (LONGLONG)Mode->OffsetY * Mode->Stride + Mode->OffsetX * (Mode->Bpp / 8);
    return pa;
}

/*
 * A centred mode offers no off-screen memory: everything below and beside
 * the picture is the visible border.
 */
static inline ULONG
HvfbPictureLength(const HVFB_MODE *Mode)
{
    return Mode->Centred ? Mode->Stride * Mode->Height : Mode->VramLength;
}

#endif /* HVFB_H */
