/*
 * Mode discovery: VESA BIOS Extensions through INT 10h, with the coreboot
 * table frame buffer record as a fallback.
 */
#include "hvfb.h"

/* ---------------------------------------------------------------------- */
/* VESA BIOS Extension 3.0 data structures                                */
/* ---------------------------------------------------------------------- */

#pragma pack(push, 1)
typedef struct _VBE_INFO_BLOCK {
    UCHAR  Signature[4];            /* "VESA" (caller may preset "VBE2") */
    USHORT Version;                 /* BCD, 0x0300 for VBE 3.0 */
    ULONG  OemStringPtr;            /* real-mode far pointers: seg << 16 | off */
    ULONG  Capabilities;
    ULONG  VideoModePtr;
    USHORT TotalMemory;             /* in 64 KiB blocks */
    USHORT OemSoftwareRev;
    ULONG  OemVendorNamePtr;
    ULONG  OemProductNamePtr;
    ULONG  OemProductRevPtr;
    UCHAR  Reserved[222];
    UCHAR  OemData[256];
} VBE_INFO_BLOCK;

typedef struct _VBE_MODE_INFO_BLOCK {
    USHORT ModeAttributes;
    UCHAR  WinAAttributes;
    UCHAR  WinBAttributes;
    USHORT WinGranularity;
    USHORT WinSize;
    USHORT WinASegment;
    USHORT WinBSegment;
    ULONG  WinFuncPtr;
    USHORT BytesPerScanLine;
    USHORT XResolution;
    USHORT YResolution;
    UCHAR  XCharSize;
    UCHAR  YCharSize;
    UCHAR  NumberOfPlanes;
    UCHAR  BitsPerPixel;
    UCHAR  NumberOfBanks;
    UCHAR  MemoryModel;
    UCHAR  BankSize;
    UCHAR  NumberOfImagePages;
    UCHAR  Reserved0;
    UCHAR  RedMaskSize;
    UCHAR  RedFieldPosition;
    UCHAR  GreenMaskSize;
    UCHAR  GreenFieldPosition;
    UCHAR  BlueMaskSize;
    UCHAR  BlueFieldPosition;
    UCHAR  RsvdMaskSize;
    UCHAR  RsvdFieldPosition;
    UCHAR  DirectColorModeInfo;
    ULONG  PhysBasePtr;             /* VBE 2.0+ */
    ULONG  Reserved1;
    USHORT Reserved2;
    USHORT LinBytesPerScanLine;     /* VBE 3.0+ */
    UCHAR  BnkNumberOfImagePages;
    UCHAR  LinNumberOfImagePages;
    UCHAR  LinRedMaskSize;
    UCHAR  LinRedFieldPosition;
    UCHAR  LinGreenMaskSize;
    UCHAR  LinGreenFieldPosition;
    UCHAR  LinBlueMaskSize;
    UCHAR  LinBlueFieldPosition;
    UCHAR  LinRsvdMaskSize;
    UCHAR  LinRsvdFieldPosition;
    ULONG  MaxPixelClock;
    UCHAR  Reserved3[190];
} VBE_MODE_INFO_BLOCK;
#pragma pack(pop)

_Static_assert(sizeof(VBE_INFO_BLOCK) == 512, "VbeInfoBlock is 512 bytes");
_Static_assert(sizeof(VBE_MODE_INFO_BLOCK) == 256, "ModeInfoBlock is 256 bytes");

#define VBE_SUCCESS                 0x004F
#define VBE_MODE_ATTR_SUPPORTED     0x0001
#define VBE_MODE_ATTR_GRAPHICS      0x0010
#define VBE_MODE_ATTR_LFB           0x0080
#define VBE_MEMORY_MODEL_PACKED     4
#define VBE_MEMORY_MODEL_DIRECT     6
#define VBE_MODE_LIST_END           0xFFFF

/* ---------------------------------------------------------------------- */
/* coreboot table (as written by CSMWrap and coreboot)                    */
/* ---------------------------------------------------------------------- */

#define CB_SIGNATURE            0x4F49424C      /* "LBIO" */
#define CB_TAG_FORWARD          0x0011
#define CB_TAG_FRAMEBUFFER      0x0012
#define CB_SCAN_LENGTH          0x1000          /* header lives in 0..4 KiB */
#define CB_MAX_TABLE            0x10000

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

/* ---------------------------------------------------------------------- */

VP_STATUS
HvfbCallBios(PHVFB_EXTENSION Ext, PINT10_BIOS_ARGUMENTS Args)
{
    if (!Ext->HaveInt10)
        return ERROR_INVALID_FUNCTION;
    return Ext->Int10.Int10CallBios(Ext->Int10.Context, Args);
}

/* Fill in default channel layouts when the BIOS reports no masks. */
static BOOLEAN
HvfbDefaultMasks(PHVFB_MODE Mode, ULONG Bpp)
{
    switch (Bpp) {
    case 15:
        Mode->RedSize = 5;   Mode->RedPos = 10;
        Mode->GreenSize = 5; Mode->GreenPos = 5;
        Mode->BlueSize = 5;  Mode->BluePos = 0;
        return TRUE;
    case 16:
        Mode->RedSize = 5;   Mode->RedPos = 11;
        Mode->GreenSize = 6; Mode->GreenPos = 5;
        Mode->BlueSize = 5;  Mode->BluePos = 0;
        return TRUE;
    case 24:
    case 32:
        Mode->RedSize = 8;   Mode->RedPos = 16;
        Mode->GreenSize = 8; Mode->GreenPos = 8;
        Mode->BlueSize = 8;  Mode->BluePos = 0;
        return TRUE;
    }
    return FALSE;
}

/*
 * Validate a candidate mode and fill in the derived fields.  Bpp is the
 * firmware's bits per pixel (15 is accepted and stored as 16).
 */
static BOOLEAN
HvfbCompleteMode(PHVFB_MODE Mode, ULONG Bpp)
{
    ULONG bytesPerPixel;
    ULONG depth;

    if (Bpp == 15)
        Bpp = 16;
    if (Bpp != 16 && Bpp != 24 && Bpp != 32)
        return FALSE;
    if (Mode->RedSize == 0 && Mode->GreenSize == 0 && Mode->BlueSize == 0)
        HvfbDefaultMasks(Mode, Bpp);

    depth = (ULONG)Mode->RedSize + Mode->GreenSize + Mode->BlueSize;
    if (depth == 0 || depth > Bpp)
        return FALSE;
    if (Mode->RedPos + Mode->RedSize > Bpp ||
        Mode->GreenPos + Mode->GreenSize > Bpp ||
        Mode->BluePos + Mode->BlueSize > Bpp)
        return FALSE;

    bytesPerPixel = Bpp / 8;
    if (Mode->Width < 320 || Mode->Height < 200 ||
        Mode->Width > 0x4000 || Mode->Height > 0x4000)
        return FALSE;
    if (Mode->Stride < Mode->Width * bytesPerPixel)
        return FALSE;
    if (Mode->FrameBuffer.QuadPart == 0 ||
        Mode->FrameBuffer.QuadPart + (ULONGLONG)Mode->Stride * Mode->Height > 0x100000000ULL)
        return FALSE;   /* 32-bit XP without PAE-aware mapping: stay below 4 GiB */

    if (Mode->VramLength < Mode->Stride * Mode->Height)
        Mode->VramLength = Mode->Stride * Mode->Height;
    Mode->Bpp = (UCHAR)Bpp;
    Mode->Depth = (UCHAR)depth;
    return TRUE;
}

/* ---------------------------------------------------------------------- */
/* coreboot table                                                         */
/* ---------------------------------------------------------------------- */

/* RFC 1071 checksum, as used by coreboot for its tables. */
static USHORT
HvfbIpChecksum(const UCHAR *Data, ULONG Length)
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

static PUCHAR
HvfbMap(PHVFB_EXTENSION Ext, ULONGLONG Physical, ULONG Length)
{
    PHYSICAL_ADDRESS pa;

    pa.QuadPart = (LONGLONG)Physical;
    return VideoPortGetDeviceBase(Ext, pa, Length, VIDEO_MEMORY_SPACE_MEMORY);
}

/* Returns the offset of a valid header within Base[0..Length), or -1. */
static LONG
HvfbFindCbHeader(const UCHAR *Base, ULONG Length)
{
    ULONG off;

    for (off = 0; off + sizeof(CB_HEADER) <= Length; off += 16) {
        const CB_HEADER *h = (const CB_HEADER *)(Base + off);

        if (h->Signature != CB_SIGNATURE || h->HeaderBytes != sizeof(CB_HEADER))
            continue;
        if (h->TableBytes == 0 || h->TableBytes > CB_MAX_TABLE)
            continue;
        if (HvfbIpChecksum((const UCHAR *)h, sizeof(CB_HEADER)) != 0)
            continue;
        return (LONG)off;
    }
    return -1;
}

/*
 * Walk a table whose header is at physical HeaderPa.  Returns TRUE and the
 * forward pointer if the table only forwards elsewhere.
 */
static BOOLEAN
HvfbParseCbTable(PHVFB_EXTENSION Ext, ULONGLONG HeaderPa, PULONGLONG Forward)
{
    PUCHAR map;
    PCB_HEADER h;
    PUCHAR rec, end;
    ULONG length, i;
    BOOLEAN found = FALSE;

    *Forward = 0;

    /* The header has already been validated; map header + table. */
    map = HvfbMap(Ext, HeaderPa, sizeof(CB_HEADER));
    if (map == NULL)
        return FALSE;
    length = sizeof(CB_HEADER) + ((PCB_HEADER)map)->TableBytes;
    VideoPortFreeDeviceBase(Ext, map);
    if (length > sizeof(CB_HEADER) + CB_MAX_TABLE)
        return FALSE;

    map = HvfbMap(Ext, HeaderPa, length);
    if (map == NULL)
        return FALSE;
    h = (PCB_HEADER)map;
    if (HvfbIpChecksum(map + sizeof(CB_HEADER), h->TableBytes) != (USHORT)h->TableChecksum) {
        HvfbLog("coreboot table checksum mismatch\n");
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
            PHVFB_MODE m = &Ext->CbFb;
            ULONG bpp;

            VideoPortZeroMemory(m, sizeof(*m));
            m->VbeMode = HVFB_NO_VBE_MODE;
            m->Width = fb->XResolution;
            m->Height = fb->YResolution;
            m->Stride = fb->BytesPerLine;
            m->FrameBuffer.QuadPart = (LONGLONG)fb->PhysicalAddress;
            m->RedSize = fb->RedMaskSize;     m->RedPos = fb->RedMaskPos;
            m->GreenSize = fb->GreenMaskSize; m->GreenPos = fb->GreenMaskPos;
            m->BlueSize = fb->BlueMaskSize;   m->BluePos = fb->BlueMaskPos;
            /* Same rule as SeaVGABIOS's cbvga: trust the masks first. */
            bpp = (ULONG)fb->RedMaskSize + fb->GreenMaskSize + fb->BlueMaskSize +
                  fb->ReservedMaskSize;
            if (bpp != 15 && bpp != 16 && bpp != 24 && bpp != 32)
                bpp = fb->BitsPerPixel;
            if (HvfbCompleteMode(m, bpp)) {
                Ext->HaveCbFb = TRUE;
                found = TRUE;
                HvfbLog("coreboot frame buffer %ux%u %u bpp stride %u at 0x%08x\n",
                        m->Width, m->Height, (ULONG)m->Bpp, m->Stride,
                        m->FrameBuffer.LowPart);
            } else {
                HvfbLog("coreboot frame buffer record not usable\n");
            }
        }
        rec += r->Size;
    }

out:
    VideoPortFreeDeviceBase(Ext, map);
    return found;
}

VOID
HvfbReadCorebootTable(PHVFB_EXTENSION Ext)
{
    PUCHAR low;
    LONG off;
    ULONGLONG headerPa, forward;
    int hops;

    low = HvfbMap(Ext, 0, CB_SCAN_LENGTH);
    if (low == NULL) {
        HvfbLog("cannot map low memory for the coreboot table\n");
        return;
    }
    off = HvfbFindCbHeader(low, CB_SCAN_LENGTH);
    VideoPortFreeDeviceBase(Ext, low);
    if (off < 0)
        return;

    headerPa = (ULONGLONG)off;
    for (hops = 0; hops < 2; hops++) {
        PUCHAR fwd;

        if (HvfbParseCbTable(Ext, headerPa, &forward) || forward == 0)
            return;
        if (forward >= 0x100000000ULL)
            return;
        /* A forwarded header sits at the given address (seabios scans 256 bytes). */
        fwd = HvfbMap(Ext, forward, 0x100);
        if (fwd == NULL)
            return;
        off = HvfbFindCbHeader(fwd, 0x100);
        VideoPortFreeDeviceBase(Ext, fwd);
        if (off < 0)
            return;
        headerPa = forward + (ULONG)off;
    }
}

/* ---------------------------------------------------------------------- */
/* VBE                                                                    */
/* ---------------------------------------------------------------------- */

static VOID
HvfbFarToSegOff(ULONG Far, ULONG ByteOffset, PUSHORT Seg, PUSHORT Off)
{
    ULONG linear = ((Far >> 16) << 4) + (Far & 0xFFFF) + ByteOffset;

    *Seg = (USHORT)(linear >> 4);
    *Off = (USHORT)(linear & 0xF);
}

static BOOLEAN
HvfbAddVbeMode(PHVFB_EXTENSION Ext, USHORT Number, const VBE_MODE_INFO_BLOCK *mi)
{
    HVFB_MODE m;
    BOOLEAN useLin;

    if ((mi->ModeAttributes & (VBE_MODE_ATTR_SUPPORTED | VBE_MODE_ATTR_GRAPHICS |
                               VBE_MODE_ATTR_LFB)) !=
        (VBE_MODE_ATTR_SUPPORTED | VBE_MODE_ATTR_GRAPHICS | VBE_MODE_ATTR_LFB))
        return FALSE;
    if (mi->NumberOfPlanes > 1)
        return FALSE;
    /* Some BIOSes label 16/32 bpp modes as packed pixel; accept both. */
    if (mi->MemoryModel != VBE_MEMORY_MODEL_DIRECT &&
        !(mi->MemoryModel == VBE_MEMORY_MODEL_PACKED && mi->BitsPerPixel >= 15))
        return FALSE;
    if (mi->PhysBasePtr == 0)
        return FALSE;

    VideoPortZeroMemory(&m, sizeof(m));
    m.VbeMode = Number;
    m.Width = mi->XResolution;
    m.Height = mi->YResolution;
    m.FrameBuffer.LowPart = mi->PhysBasePtr;
    m.VramLength = Ext->VbeMemory;

    useLin = Ext->VbeVersion >= 0x0300;
    m.Stride = (useLin && mi->LinBytesPerScanLine) ? mi->LinBytesPerScanLine
                                                   : mi->BytesPerScanLine;
    if (useLin && (mi->LinRedMaskSize | mi->LinGreenMaskSize | mi->LinBlueMaskSize)) {
        m.RedSize = mi->LinRedMaskSize;     m.RedPos = mi->LinRedFieldPosition;
        m.GreenSize = mi->LinGreenMaskSize; m.GreenPos = mi->LinGreenFieldPosition;
        m.BlueSize = mi->LinBlueMaskSize;   m.BluePos = mi->LinBlueFieldPosition;
    } else {
        m.RedSize = mi->RedMaskSize;        m.RedPos = mi->RedFieldPosition;
        m.GreenSize = mi->GreenMaskSize;    m.GreenPos = mi->GreenFieldPosition;
        m.BlueSize = mi->BlueMaskSize;      m.BluePos = mi->BlueFieldPosition;
    }

    if (!HvfbCompleteMode(&m, mi->BitsPerPixel))
        return FALSE;
    if (Ext->VbeMemory && (ULONGLONG)m.Stride * m.Height > Ext->VbeMemory)
        return FALSE;
    if (Ext->NumModes >= HVFB_MAX_MODES)
        return FALSE;

    Ext->Modes[Ext->NumModes++] = m;
    return TRUE;
}

BOOLEAN
HvfbQueryVbeModes(PHVFB_EXTENSION Ext)
{
    PVIDEO_PORT_INT10_INTERFACE i10 = &Ext->Int10;
    VBE_INFO_BLOCK *info = (VBE_INFO_BLOCK *)Ext->Scratch.Block;
    VBE_MODE_INFO_BLOCK *mi = (VBE_MODE_INFO_BLOCK *)Ext->Scratch.Block;
    INT10_BIOS_ARGUMENTS args;
    USHORT bufSeg, bufOff, seg, off;
    ULONG bufLen = sizeof(VBE_INFO_BLOCK);
    ULONG count, i;
    VP_STATUS status;

    if (!Ext->HaveInt10)
        return FALSE;

    status = i10->Int10AllocateBuffer(i10->Context, &bufSeg, &bufOff, &bufLen);
    if (status != NO_ERROR || bufLen < sizeof(VBE_INFO_BLOCK)) {
        HvfbLog("Int10AllocateBuffer failed (%u, %u bytes)\n", status, bufLen);
        if (status == NO_ERROR)
            i10->Int10FreeBuffer(i10->Context, bufSeg, bufOff);
        return FALSE;
    }

    /* 4F00: controller information; "VBE2" asks for the VBE 2.0+ layout. */
    VideoPortZeroMemory(info, sizeof(*info));
    info->Signature[0] = 'V'; info->Signature[1] = 'B';
    info->Signature[2] = 'E'; info->Signature[3] = '2';
    i10->Int10WriteMemory(i10->Context, bufSeg, bufOff, info, sizeof(*info));

    VideoPortZeroMemory(&args, sizeof(args));
    args.Eax = 0x4F00;
    args.SegEs = bufSeg;
    args.Edi = bufOff;
    status = i10->Int10CallBios(i10->Context, &args);
    if (status != NO_ERROR || (args.Eax & 0xFFFF) != VBE_SUCCESS) {
        HvfbLog("VBE 4F00 failed (status %u, ax %04x)\n", status, args.Eax & 0xFFFF);
        goto fail;
    }
    i10->Int10ReadMemory(i10->Context, bufSeg, bufOff, info, sizeof(*info));
    if (info->Signature[0] != 'V' || info->Signature[1] != 'E' ||
        info->Signature[2] != 'S' || info->Signature[3] != 'A' ||
        info->Version < 0x0200) {
        HvfbLog("no VBE 2.0+ controller (version %04x)\n", (ULONG)info->Version);
        goto fail;
    }
    Ext->VbeVersion = info->Version;
    Ext->VbeMemory = (ULONG)info->TotalMemory << 16;

    /*
     * Copy the mode list before the first 4F01: it often lives inside the
     * buffer that 4F01 will overwrite.
     */
    for (count = 0; count < 256; count++) {
        USHORT number;

        HvfbFarToSegOff(info->VideoModePtr, count * 2, &seg, &off);
        if (i10->Int10ReadMemory(i10->Context, seg, off, &number, sizeof(number)) != NO_ERROR)
            break;
        if (number == VBE_MODE_LIST_END)
            break;
        Ext->Scratch.VbeModes[count] = number;
    }
    HvfbLog("VBE %x.%x, %u KiB, %u modes listed\n", Ext->VbeVersion >> 8,
            Ext->VbeVersion & 0xFF, Ext->VbeMemory >> 10, count);

    /* 4F01: per-mode information. */
    for (i = 0; i < count; i++) {
        USHORT number = Ext->Scratch.VbeModes[i];

        VideoPortZeroMemory(mi, sizeof(*mi));
        i10->Int10WriteMemory(i10->Context, bufSeg, bufOff, mi, sizeof(*mi));
        VideoPortZeroMemory(&args, sizeof(args));
        args.Eax = 0x4F01;
        args.Ecx = number;
        args.SegEs = bufSeg;
        args.Edi = bufOff;
        status = i10->Int10CallBios(i10->Context, &args);
        if (status != NO_ERROR || (args.Eax & 0xFFFF) != VBE_SUCCESS)
            continue;
        i10->Int10ReadMemory(i10->Context, bufSeg, bufOff, mi, sizeof(*mi));
        if (HvfbAddVbeMode(Ext, number, mi))
            HvfbTrace("VBE mode %03x: %ux%u %u bpp stride %u lfb 0x%08x\n",
                      (ULONG)number, (ULONG)mi->XResolution, (ULONG)mi->YResolution,
                      (ULONG)mi->BitsPerPixel, Ext->Modes[Ext->NumModes - 1].Stride,
                      mi->PhysBasePtr);
    }

    i10->Int10FreeBuffer(i10->Context, bufSeg, bufOff);
    Ext->UsingVbe = Ext->NumModes != 0;
    return Ext->UsingVbe;

fail:
    i10->Int10FreeBuffer(i10->Context, bufSeg, bufOff);
    return FALSE;
}

/* ---------------------------------------------------------------------- */
/* Mode list post-processing                                              */
/* ---------------------------------------------------------------------- */

static VOID
HvfbRemoveMode(PHVFB_EXTENSION Ext, ULONG Index)
{
    ULONG i;

    for (i = Index; i + 1 < Ext->NumModes; i++)
        Ext->Modes[i] = Ext->Modes[i + 1];
    Ext->NumModes--;
}

/*
 * Text columns XP text-mode setup gets out of a mode: setupdd.sys uses an
 * 8x16 font and doubles the cell width from 160 columns up.
 */
static ULONG
HvfbSetupColumns(PHVFB_MODE Mode)
{
    ULONG cols = Mode->Width / 8;

    return cols >= 160 ? cols / 2 : cols;
}

/*
 * Pick the mode that goes first.  XP text-mode setup (setupdd.sys) has no
 * configured resolution on x86 and simply takes mode 0 if it is a graphics
 * mode of at least 640x480 with 8 or more bits per pixel.  Its frame
 * buffer back end handles 1, 2 and 4 bytes per pixel, so mode 0 must not
 * be 24 bpp, and its screen code assumes at most 80 text columns: clearing
 * a region that starts beyond column 79 overruns an 80-column buffer on its
 * stack (bug check 0x50 while formatting, seen at 1024x768).  So mode 0 is
 * the smallest mode of at least 640x480 that gives exactly 80 columns,
 * normally 640x480; deeper colour wins.
 */
static ULONG
HvfbPreferredMode(PHVFB_EXTENSION Ext)
{
    ULONG best = HVFB_NO_MODE;
    ULONG i;

    for (i = 0; i < Ext->NumModes; i++) {
        PHVFB_MODE m = &Ext->Modes[i], b;

        if (m->Bpp == 24 || m->Width < 640 || m->Height < 480 ||
            HvfbSetupColumns(m) != 80)
            continue;
        if (best == HVFB_NO_MODE) {
            best = i;
            continue;
        }
        b = &Ext->Modes[best];
        if (m->Bpp > b->Bpp ||
            (m->Bpp == b->Bpp && m->Width * m->Height < b->Width * b->Height))
            best = i;
    }
    if (best != HVFB_NO_MODE)
        return best;

    /* No 80-column mode: setup may fail, but the installed system works. */
    HvfbLog("no 80-column mode for text-mode setup\n");
    for (i = 0; i < Ext->NumModes; i++)
        if (Ext->Modes[i].Bpp != 24 && Ext->Modes[i].Width >= 640 &&
            Ext->Modes[i].Height >= 480)
            return i;
    return 0;
}

VOID
HvfbFinishModeList(PHVFB_EXTENSION Ext)
{
    ULONG i, j;
    HVFB_MODE first;

    /* Fall back to the frame buffer the firmware left behind. */
    if (Ext->NumModes == 0 && Ext->HaveCbFb) {
        Ext->Modes[0] = Ext->CbFb;
        Ext->NumModes = 1;
    }

    /*
     * Drop duplicates: same size and storage format.  A 15-bit mode is
     * dropped when a 16-bit mode of the same size exists, because both are
     * "16 bits per pixel" to GDI and the display applet.
     */
    for (i = 0; i < Ext->NumModes; i++) {
        for (j = 0; j < Ext->NumModes; j++) {
            PHVFB_MODE a = &Ext->Modes[i], b = &Ext->Modes[j];
            if (i == j || a->Width != b->Width || a->Height != b->Height ||
                a->Bpp != b->Bpp)
                continue;
            if (a->Depth < b->Depth || (a->Depth == b->Depth && i > j)) {
                HvfbRemoveMode(Ext, i);
                i--;
                break;
            }
        }
    }

    if (Ext->NumModes == 0)
        return;

    /* Move the preferred mode to index 0, keeping the others in order. */
    i = HvfbPreferredMode(Ext);
    first = Ext->Modes[i];
    for (; i > 0; i--)
        Ext->Modes[i] = Ext->Modes[i - 1];
    Ext->Modes[0] = first;

    for (i = 0; i < Ext->NumModes; i++) {
        PHVFB_MODE m = &Ext->Modes[i];
        HvfbLog("mode %u: %ux%u %u bpp (depth %u) stride %u fb 0x%08x vbe %03x\n",
                i, m->Width, m->Height, (ULONG)m->Bpp, (ULONG)m->Depth, m->Stride,
                m->FrameBuffer.LowPart, (ULONG)m->VbeMode);
    }
}
