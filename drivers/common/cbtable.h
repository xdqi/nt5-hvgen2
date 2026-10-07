/*
 * Frame buffer lookup in the coreboot table that CSMWrap (and coreboot)
 * leave in low physical memory.  Shared by hvfb.sys, which maps memory
 * through VideoPort, and bootvid.dll, which uses MmMapIoSpace: the caller
 * supplies the mapping functions.
 *
 * Only needs the basic types of <ntdef.h>, which both the miniport and the
 * WDM headers provide.
 */
#ifndef CBTABLE_H
#define CBTABLE_H

#define CB_TAG_FORWARD          0x0011
#define CB_TAG_FRAMEBUFFER      0x0012
#define CB_TAG_CSMWRAP_VIDEO    0x43534D57      /* "CSMW", CSMWrap video options */

/* Contents of a CB_TAG_FRAMEBUFFER record. */
typedef struct _CB_FB_INFO {
    ULONGLONG PhysicalAddress;
    ULONG XResolution;
    ULONG YResolution;
    ULONG BytesPerLine;
    UCHAR BitsPerPixel;
    UCHAR RedMaskPos;
    UCHAR RedMaskSize;
    UCHAR GreenMaskPos;
    UCHAR GreenMaskSize;
    UCHAR BlueMaskPos;
    UCHAR BlueMaskSize;
    UCHAR ReservedMaskPos;
    UCHAR ReservedMaskSize;
} CB_FB_INFO, *PCB_FB_INFO;

typedef enum _CB_STATUS {
    CbFound = 0,            /* frame buffer record copied out */
    CbNoTable,              /* no valid table header (or forward) found */
    CbBadChecksum,          /* a header was found but its table checksum is wrong */
    CbNoFramebuffer,        /* valid table without a frame buffer record */
    CbMapFailed,            /* the map callback failed */
} CB_STATUS;

/* Map Length bytes of physical memory for reading; NULL on failure. */
typedef PVOID (*PCB_MAP)(PVOID Context, ULONGLONG Physical, ULONG Length);
typedef VOID (*PCB_UNMAP)(PVOID Context, PVOID Virtual, ULONG Length);

/*
 * Find the table header in physical 0-4 KiB, follow up to two
 * CB_TAG_FORWARD records and copy the first CB_TAG_FRAMEBUFFER record.
 */
CB_STATUS CbFindFramebuffer(PCB_MAP Map, PCB_UNMAP Unmap, PVOID Context, PCB_FB_INFO Info);

/*
 * Bits per pixel of a frame buffer record.  Like SeaVGABIOS's cbvga, the
 * sum of the mask sizes wins when it is a known depth (15, 16, 24, 32);
 * otherwise the record's BitsPerPixel is used.
 */
ULONG CbFramebufferBpp(const CB_FB_INFO *Info);

#endif /* CBTABLE_H */
