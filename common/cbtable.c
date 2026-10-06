/*
 * Frame buffer lookup in the coreboot table (the "LBIO" table that coreboot
 * and CSMWrap write into low physical memory).  See cbtable.h.
 */
#include <ntdef.h>
#include "cbtable.h"

#define CB_SIGNATURE            0x4F49424C      /* "LBIO" */
#define CB_SCAN_LENGTH          0x1000          /* the header lives in 0..4 KiB */
#define CB_FORWARD_SCAN         0x100           /* SeaBIOS also scans 256 bytes at a forward */
#define CB_MAX_TABLE            0x10000
#define CB_MAX_FORWARDS         2

#pragma pack(push, 1)
typedef struct _CB_HEADER {
    ULONG Signature;
    ULONG HeaderBytes;
    ULONG HeaderChecksum;
    ULONG TableBytes;
    ULONG TableChecksum;
    ULONG TableEntries;
} CB_HEADER, *PCB_HEADER;

typedef struct _CB_RECORD {
    ULONG Tag;
    ULONG Size;
} CB_RECORD, *PCB_RECORD;

typedef struct _CB_FORWARD {
    ULONG Tag;
    ULONG Size;
    ULONGLONG Forward;
} CB_FORWARD, *PCB_FORWARD;

typedef struct _CB_FRAMEBUFFER {
    ULONG Tag;
    ULONG Size;
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
} CB_FRAMEBUFFER, *PCB_FRAMEBUFFER;
#pragma pack(pop)

/* RFC 1071 checksum, as used by coreboot for its tables. */
static USHORT
CbIpChecksum(const UCHAR *Data, ULONG Length)
{
    ULONG sum = 0;
    ULONG i;

    for (i = 0; i + 1 < Length; i += 2)
        sum += (ULONG)Data[i] | ((ULONG)Data[i + 1] << 8);
    if (Length & 1)
        sum += Data[Length - 1];
    while (sum >> 16)
        sum = (sum & 0xFFFF) + (sum >> 16);
    return (USHORT)~sum;
}

/* Returns the offset of a valid header within Base[0..Length), or -1. */
static LONG
CbFindHeader(const UCHAR *Base, ULONG Length)
{
    ULONG off;

    for (off = 0; off + sizeof(CB_HEADER) <= Length; off += 16) {
        const CB_HEADER *h = (const CB_HEADER *)(Base + off);

        if (h->Signature != CB_SIGNATURE || h->HeaderBytes != sizeof(CB_HEADER))
            continue;
        if (h->TableBytes == 0 || h->TableBytes > CB_MAX_TABLE)
            continue;
        if (CbIpChecksum((const UCHAR *)h, sizeof(CB_HEADER)) != 0)
            continue;
        return (LONG)off;
    }
    return -1;
}

/* Scan Length bytes at Physical for a valid header; *HeaderPa gets its address. */
static CB_STATUS
CbScan(PCB_MAP Map, PCB_UNMAP Unmap, PVOID Context, ULONGLONG Physical, ULONG Length,
       PULONGLONG HeaderPa)
{
    PUCHAR va;
    LONG off;

    va = Map(Context, Physical, Length);
    if (va == NULL)
        return CbMapFailed;
    off = CbFindHeader(va, Length);
    Unmap(Context, va, Length);
    if (off < 0)
        return CbNoTable;
    *HeaderPa = Physical + (ULONG)off;
    return CbFound;
}

/*
 * Walk the table whose (already validated) header is at physical HeaderPa.
 * Copies the frame buffer record to Info, or returns the forward pointer
 * in *Forward if the table has none.
 */
static CB_STATUS
CbParseTable(PCB_MAP Map, PCB_UNMAP Unmap, PVOID Context, ULONGLONG HeaderPa,
             PCB_FB_INFO Info, PULONGLONG Forward)
{
    PUCHAR map;
    PCB_HEADER h;
    PUCHAR rec, end;
    ULONG length, i;
    CB_STATUS status = CbNoFramebuffer;

    *Forward = 0;

    map = Map(Context, HeaderPa, sizeof(CB_HEADER));
    if (map == NULL)
        return CbMapFailed;
    length = sizeof(CB_HEADER) + ((PCB_HEADER)map)->TableBytes;
    Unmap(Context, map, sizeof(CB_HEADER));
    if (length > sizeof(CB_HEADER) + CB_MAX_TABLE)
        return CbNoTable;

    map = Map(Context, HeaderPa, length);
    if (map == NULL)
        return CbMapFailed;
    h = (PCB_HEADER)map;
    if (CbIpChecksum(map + sizeof(CB_HEADER), h->TableBytes) != (USHORT)h->TableChecksum) {
        status = CbBadChecksum;
        goto out;
    }

    rec = map + sizeof(CB_HEADER);
    end = rec + h->TableBytes;
    for (i = 0; i < h->TableEntries && rec + sizeof(CB_RECORD) <= end; i++) {
        PCB_RECORD r = (PCB_RECORD)rec;

        if (r->Size < sizeof(CB_RECORD) || r->Size > (ULONG)(end - rec))
            break;

        if (r->Tag == CB_TAG_FORWARD && r->Size >= sizeof(CB_FORWARD)) {
            *Forward = ((PCB_FORWARD)r)->Forward;
        } else if (r->Tag == CB_TAG_FRAMEBUFFER && r->Size >= sizeof(CB_FRAMEBUFFER)) {
            PCB_FRAMEBUFFER fb = (PCB_FRAMEBUFFER)r;

            Info->PhysicalAddress = fb->PhysicalAddress;
            Info->XResolution = fb->XResolution;
            Info->YResolution = fb->YResolution;
            Info->BytesPerLine = fb->BytesPerLine;
            Info->BitsPerPixel = fb->BitsPerPixel;
            Info->RedMaskPos = fb->RedMaskPos;
            Info->RedMaskSize = fb->RedMaskSize;
            Info->GreenMaskPos = fb->GreenMaskPos;
            Info->GreenMaskSize = fb->GreenMaskSize;
            Info->BlueMaskPos = fb->BlueMaskPos;
            Info->BlueMaskSize = fb->BlueMaskSize;
            Info->ReservedMaskPos = fb->ReservedMaskPos;
            Info->ReservedMaskSize = fb->ReservedMaskSize;
            status = CbFound;
            break;
        }
        rec += r->Size;
    }

out:
    Unmap(Context, map, length);
    return status;
}

CB_STATUS
CbFindFramebuffer(PCB_MAP Map, PCB_UNMAP Unmap, PVOID Context, PCB_FB_INFO Info)
{
    CB_STATUS status;
    ULONGLONG headerPa = 0, forward;
    int hops;

    status = CbScan(Map, Unmap, Context, 0, CB_SCAN_LENGTH, &headerPa);
    for (hops = 0; status == CbFound; hops++) {
        status = CbParseTable(Map, Unmap, Context, headerPa, Info, &forward);
        if (status != CbNoFramebuffer || forward == 0)
            break;
        /* A forwarded header sits at (or shortly after) the given address. */
        if (hops == CB_MAX_FORWARDS || forward >= 0x100000000ULL)
            break;
        status = CbScan(Map, Unmap, Context, forward, CB_FORWARD_SCAN, &headerPa);
    }
    return status;
}

ULONG
CbFramebufferBpp(const CB_FB_INFO *Info)
{
    ULONG bpp = (ULONG)Info->RedMaskSize + Info->GreenMaskSize + Info->BlueMaskSize +
                Info->ReservedMaskSize;

    if (bpp != 15 && bpp != 16 && bpp != 24 && bpp != 32)
        bpp = Info->BitsPerPixel;
    return bpp;
}
