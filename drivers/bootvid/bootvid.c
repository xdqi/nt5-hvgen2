/*
 * bootvid.dll for Windows XP on machines without VGA hardware (Hyper-V
 * Generation 2 booted through CSMWrap).
 *
 * The kernel draws its boot screen, the /sos text and bug check screens
 * through bootvid.dll (ntoskrnl's Inbv* functions call the Vid* exports).
 * XP's own bootvid programs a VGA in planar mode 12h directly, so on a
 * machine without VGA nothing appears, not even a blue screen.
 *
 * This bootvid keeps the kernel's model, a 640x480 screen with a 16-entry
 * palette, as an array of colour indices in memory and copies whatever
 * changes to the linear frame buffer that the firmware left displayed.  It
 * finds that frame buffer in the coreboot table CSMWrap writes to low
 * memory.  The 640x480 screen is scaled by the largest integer factor that
 * fits and centred; the area around it shows the colour of the last fill
 * of the whole screen (black, or blue on a bug check).
 *
 * Behaviour follows the original, as observed in XP SP3's bootvid.dll
 * (5.1.2600.0; SP3 still ships it) and in the kernel's use of it:
 *
 * - All exports are __stdcall.  The default palette uses the Windows VGA
 *   colour order (index 4 is the dark blue of the bug check screen, 15 is
 *   white).  VidBitBlt loads the bitmap's colour table into the palette
 *   before drawing, and a palette change recolours everything already on
 *   the screen, as a VGA DAC does.  The kernel relies on this: it draws its
 *   boot screen with a black palette and then fades the real one in, one
 *   step per progress bar tick, by drawing a 1x1 bitmap below the screen.
 * - Text uses an 8x13 font on 14-pixel lines, drawn transparently in the
 *   text colour.  When VidDisplayString starts a new line it saves the
 *   background of that line; a carriage return not followed by a line feed
 *   makes the next character restore it first (overwriting the line), and
 *   scrolling restores it into the line that becomes free.  As on the VGA,
 *   the saved line lives right below the visible screen, where scrolling
 *   reads from.
 *
 * It never calls the HAL's display reset (HalResetDisplay, which the
 * original uses through HalPrivateDispatchTable).  The HAL implements it as
 * a real-mode INT 10h mode 12h call, which is meaningless for a frame
 * buffer and would only make the BIOS change what it scans out.
 */
#include <ntddk.h>
#include "bootvid.h"
#include "../common/cbtable.h"

#define BV_TRANSPARENT          16      /* background colour: leave pixels alone */
#define BV_SAVE_TOP             BV_SCREEN_HEIGHT    /* saved line background */
#define BV_SHADOW_HEIGHT        (BV_SCREEN_HEIGHT + BV_LINE_HEIGHT)
#define BV_BI_RLE4              2

/* VidDisplayStringXY draws in these colours, like the original. */
#define BV_XY_TEXT_COLOR        12
#define BV_XY_BACK_COLOR        14

#pragma pack(push, 1)
typedef struct _BV_BITMAPINFOHEADER {
    ULONG  biSize;
    LONG   biWidth;
    LONG   biHeight;
    USHORT biPlanes;
    USHORT biBitCount;
    ULONG  biCompression;
    ULONG  biSizeImage;
    LONG   biXPelsPerMeter;
    LONG   biYPelsPerMeter;
    ULONG  biClrUsed;
    ULONG  biClrImportant;
} BV_BITMAPINFOHEADER;
#pragma pack(pop)

/*
 * The frame buffer and where the 640x480 screen sits in it.  Not static,
 * so that the optimiser keeps it whole and the debugger finds bootvid!Fb.
 */
typedef struct _BV_FRAMEBUFFER {
    PUCHAR Base;                /* kernel mapping of the whole frame buffer */
    ULONG  MapLength;
    ULONG  Width;
    ULONG  Height;
    ULONG  Pitch;
    ULONG  BytesPerPixel;       /* 2, 3 or 4 */
    UCHAR  RedPos, RedSize;
    UCHAR  GreenPos, GreenSize;
    UCHAR  BluePos, BlueSize;
    ULONG  Scale;               /* screen pixel -> Scale x Scale frame buffer pixels */
    ULONG  OriginX;             /* frame buffer position of screen pixel (0, 0) */
    ULONG  OriginY;
} BV_FRAMEBUFFER;

BV_FRAMEBUFFER Fb;

/*
 * Colour index of every screen pixel, plus BV_LINE_HEIGHT rows below the
 * screen that hold the saved line background.  Never read back from the
 * (uncached) frame buffer.
 */
static UCHAR Screen[BV_SHADOW_HEIGHT][BV_SCREEN_WIDTH];

/* Palette, kept as 6-bit VGA DAC values, and the matching pixel values. */
static UCHAR Dac[BV_COLORS][3];
static ULONG Pixel[BV_COLORS];
static UCHAR Border;            /* colour index of the area around the screen */

/* Text state.  Initial values as in the original. */
static ULONG ScrollLeft = 0;
static ULONG ScrollTop = 0;
static ULONG ScrollRight = BV_SCREEN_WIDTH - 1;
static ULONG ScrollBottom = BV_SCREEN_HEIGHT - 1;
static ULONG TextColor = 15;
static ULONG CurrentX;
static ULONG CurrentY;
static BOOLEAN RestoreLine;     /* a lone CR was seen: restore the line first */

/* Pending frame buffer update, done by BvFlush at the end of each export. */
static ULONG DirtyLeft = 1, DirtyTop, DirtyRight = 0, DirtyBottom;  /* empty if Left > Right */
static ULONG ChangedColors;     /* bit i: palette entry i changed */
static BOOLEAN BorderDirty;

/* Windows' 16-colour VGA palette (6-bit DAC values), the original's default. */
static const UCHAR DefaultDac[BV_COLORS][3] = {
    { 0x00, 0x00, 0x00 },       /*  0 black */
    { 0x20, 0x00, 0x00 },       /*  1 dark red */
    { 0x00, 0x20, 0x00 },       /*  2 dark green */
    { 0x20, 0x20, 0x00 },       /*  3 dark yellow */
    { 0x00, 0x00, 0x20 },       /*  4 dark blue (bug check background) */
    { 0x20, 0x00, 0x20 },       /*  5 dark magenta */
    { 0x00, 0x20, 0x20 },       /*  6 dark cyan */
    { 0x20, 0x20, 0x20 },       /*  7 dark grey */
    { 0x30, 0x30, 0x30 },       /*  8 light grey */
    { 0x3F, 0x00, 0x00 },       /*  9 red */
    { 0x00, 0x3F, 0x00 },       /* 10 green */
    { 0x3F, 0x3F, 0x00 },       /* 11 yellow */
    { 0x00, 0x00, 0x3F },       /* 12 blue */
    { 0x3F, 0x00, 0x3F },       /* 13 magenta */
    { 0x00, 0x3F, 0x3F },       /* 14 cyan */
    { 0x3F, 0x3F, 0x3F },       /* 15 white */
};

NTSTATUS NTAPI DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath);

/* ---------------------------------------------------------------------- */
/* Frame buffer output                                                    */
/* ---------------------------------------------------------------------- */

static ULONG
BvChannel(ULONG Value6, UCHAR Pos, UCHAR Size)
{
    ULONG v8 = (Value6 << 2) | (Value6 >> 4);      /* as a VGA DAC widens 6 bits */

    if (Size == 0)
        return 0;
    return (Size >= 8 ? v8 << (Size - 8) : v8 >> (8 - Size)) << Pos;
}

static VOID
BvUpdatePixel(ULONG Index)
{
    Pixel[Index] = BvChannel(Dac[Index][0], Fb.RedPos, Fb.RedSize) |
                   BvChannel(Dac[Index][1], Fb.GreenPos, Fb.GreenSize) |
                   BvChannel(Dac[Index][2], Fb.BluePos, Fb.BlueSize);
}

static VOID
BvSetDac(ULONG Index, UCHAR Red, UCHAR Green, UCHAR Blue)
{
    if (Dac[Index][0] == Red && Dac[Index][1] == Green && Dac[Index][2] == Blue)
        return;
    Dac[Index][0] = Red;
    Dac[Index][1] = Green;
    Dac[Index][2] = Blue;
    BvUpdatePixel(Index);
    ChangedColors |= 1u << Index;
    if (Index == Border)
        BorderDirty = TRUE;
}

static VOID
BvLoadDefaultPalette(VOID)
{
    ULONG i;

    for (i = 0; i < BV_COLORS; i++)
        BvSetDac(i, DefaultDac[i][0], DefaultDac[i][1], DefaultDac[i][2]);
}

static inline VOID
BvStore(PUCHAR Dst, ULONG Value)
{
    switch (Fb.BytesPerPixel) {
    case 4:
        *(volatile ULONG *)Dst = Value;
        break;
    case 3:
        ((volatile UCHAR *)Dst)[0] = (UCHAR)Value;
        ((volatile UCHAR *)Dst)[1] = (UCHAR)(Value >> 8);
        ((volatile UCHAR *)Dst)[2] = (UCHAR)(Value >> 16);
        break;
    default:
        *(volatile USHORT *)Dst = (USHORT)Value;
        break;
    }
}

/* Fill a rectangle of the frame buffer (frame buffer pixels). */
static VOID
BvFillFb(ULONG X, ULONG Y, ULONG Width, ULONG Height, ULONG Value)
{
    ULONG x, y;

    for (y = Y; y < Y + Height; y++) {
        PUCHAR dst = Fb.Base + y * Fb.Pitch + X * Fb.BytesPerPixel;
        for (x = 0; x < Width; x++, dst += Fb.BytesPerPixel)
            BvStore(dst, Value);
    }
}

static VOID
BvDrawBorder(VOID)
{
    ULONG w = BV_SCREEN_WIDTH * Fb.Scale, h = BV_SCREEN_HEIGHT * Fb.Scale;
    ULONG v = Pixel[Border];

    BvFillFb(0, 0, Fb.Width, Fb.OriginY, v);
    BvFillFb(0, Fb.OriginY + h, Fb.Width, Fb.Height - Fb.OriginY - h, v);
    BvFillFb(0, Fb.OriginY, Fb.OriginX, h, v);
    BvFillFb(Fb.OriginX + w, Fb.OriginY, Fb.Width - Fb.OriginX - w, h, v);
}

/*
 * Copy screen pixels in [Left..Right] x [Top..Bottom] whose colour index is
 * in Mask to the frame buffer.
 */
static VOID
BvDrawRect(ULONG Left, ULONG Top, ULONG Right, ULONG Bottom, ULONG Mask)
{
    ULONG s = Fb.Scale, bpp = Fb.BytesPerPixel;
    ULONG x, y, sx, sy;

    for (y = Top; y <= Bottom; y++) {
        const UCHAR *src = Screen[y];
        PUCHAR row = Fb.Base + (Fb.OriginY + y * s) * Fb.Pitch + Fb.OriginX * bpp;

        for (sy = 0; sy < s; sy++, row += Fb.Pitch) {
            if (s == 1 && bpp == 4 && Mask == 0xFFFF) {
                volatile ULONG *dst = (volatile ULONG *)row;
                for (x = Left; x <= Right; x++)
                    dst[x] = Pixel[src[x]];
                continue;
            }
            for (x = Left; x <= Right; x++) {
                UCHAR c = src[x];
                PUCHAR dst = row + x * s * bpp;

                if (!(Mask & (1u << c)))
                    continue;
                for (sx = 0; sx < s; sx++, dst += bpp)
                    BvStore(dst, Pixel[c]);
            }
        }
    }
}

/* Mark a screen rectangle (inclusive, may exceed the screen) for BvFlush. */
static VOID
BvDirty(ULONG Left, ULONG Top, ULONG Right, ULONG Bottom)
{
    if (Right >= BV_SCREEN_WIDTH)
        Right = BV_SCREEN_WIDTH - 1;
    if (Bottom >= BV_SCREEN_HEIGHT)
        Bottom = BV_SCREEN_HEIGHT - 1;
    if (Left > Right || Top > Bottom)
        return;
    if (DirtyLeft > DirtyRight) {
        DirtyLeft = Left;
        DirtyTop = Top;
        DirtyRight = Right;
        DirtyBottom = Bottom;
        return;
    }
    if (Left < DirtyLeft)
        DirtyLeft = Left;
    if (Top < DirtyTop)
        DirtyTop = Top;
    if (Right > DirtyRight)
        DirtyRight = Right;
    if (Bottom > DirtyBottom)
        DirtyBottom = Bottom;
}

static VOID
BvFlush(VOID)
{
    /* A full redraw already shows the new colours. */
    if (DirtyLeft == 0 && DirtyTop == 0 && DirtyRight == BV_SCREEN_WIDTH - 1 &&
        DirtyBottom == BV_SCREEN_HEIGHT - 1)
        ChangedColors = 0;
    if (ChangedColors) {
        BvDrawRect(0, 0, BV_SCREEN_WIDTH - 1, BV_SCREEN_HEIGHT - 1, ChangedColors);
        ChangedColors = 0;
    }
    if (DirtyLeft <= DirtyRight) {
        BvDrawRect(DirtyLeft, DirtyTop, DirtyRight, DirtyBottom, 0xFFFF);
        DirtyLeft = 1;
        DirtyRight = 0;
    }
    if (BorderDirty) {
        BvDrawBorder();
        BorderDirty = FALSE;
    }
}

/* ---------------------------------------------------------------------- */
/* Drawing on the screen array                                            */
/* ---------------------------------------------------------------------- */

static inline VOID
BvSetPixel(ULONG X, ULONG Y, ULONG Color)
{
    if (X < BV_SCREEN_WIDTH && Y < BV_SCREEN_HEIGHT)
        Screen[Y][X] = (UCHAR)(Color & 0xF);
}

static VOID
BvFill(ULONG Left, ULONG Top, ULONG Right, ULONG Bottom, ULONG Color)
{
    ULONG y;

    if (Right >= BV_SCREEN_WIDTH)
        Right = BV_SCREEN_WIDTH - 1;
    if (Bottom >= BV_SCREEN_HEIGHT)
        Bottom = BV_SCREEN_HEIGHT - 1;
    if (Left > Right || Top > Bottom)
        return;
    for (y = Top; y <= Bottom; y++)
        RtlFillMemory(&Screen[y][Left], Right - Left + 1, (UCHAR)(Color & 0xF));
    BvDirty(Left, Top, Right, Bottom);
}

static VOID
BvDrawChar(UCHAR Char, ULONG Left, ULONG Top, ULONG Color, ULONG BackColor)
{
    const UCHAR *glyph = BvFont[Char];
    ULONG row, col;

    for (row = 0; row < BV_CHAR_HEIGHT; row++) {
        for (col = 0; col < BV_CHAR_WIDTH; col++) {
            if (glyph[row] & (0x80 >> col))
                BvSetPixel(Left + col, Top + row, Color);
            else if (BackColor < BV_TRANSPARENT)
                BvSetPixel(Left + col, Top + row, BackColor);
        }
    }
    BvDirty(Left, Top, Left + BV_CHAR_WIDTH - 1, Top + BV_CHAR_HEIGHT - 1);
}

/*
 * Move the scroll region up by Lines rows.  The rows that come in from
 * below the region are read from the screen array beyond it, which near
 * the bottom of the screen is the saved line background.  Columns are
 * moved in 8-pixel units, like the original's byte-wise planar copy.
 */
static VOID
BvScroll(ULONG Lines)
{
    ULONG left = ScrollLeft & ~7u, right = ScrollRight | 7u;
    ULONG bottom = ScrollBottom, y;

    if (right >= BV_SCREEN_WIDTH)
        right = BV_SCREEN_WIDTH - 1;
    if (bottom >= BV_SCREEN_HEIGHT)
        bottom = BV_SCREEN_HEIGHT - 1;
    if (left > right || ScrollTop > bottom)
        return;
    for (y = ScrollTop; y <= bottom; y++) {
        if (y + Lines < BV_SHADOW_HEIGHT)
            RtlMoveMemory(&Screen[y][left], &Screen[y + Lines][left], right - left + 1);
        else
            RtlZeroMemory(&Screen[y][left], right - left + 1);
    }
    BvDirty(left, ScrollTop, right, bottom);
}

/* Save (Restore = FALSE) or restore the background of the text line at Top. */
static VOID
BvLineBackground(ULONG Top, BOOLEAN Restore)
{
    ULONG i;

    for (i = 0; i < BV_LINE_HEIGHT && Top + i < BV_SCREEN_HEIGHT; i++) {
        if (Restore)
            RtlCopyMemory(Screen[Top + i], Screen[BV_SAVE_TOP + i], BV_SCREEN_WIDTH);
        else
            RtlCopyMemory(Screen[BV_SAVE_TOP + i], Screen[Top + i], BV_SCREEN_WIDTH);
    }
    if (Restore)
        BvDirty(0, Top, BV_SCREEN_WIDTH - 1, Top + BV_LINE_HEIGHT - 1);
}

/*
 * Draw Width x Height pixels from a packed bitmap (4 bpp: high nibble
 * first; 1 bpp: set bits in colour 8, as the original does).  Delta is the
 * signed distance between source rows.
 */
static VOID
BvBlt(const UCHAR *Bits, ULONG Left, ULONG Top, ULONG Width, ULONG Height, ULONG Bpp,
      LONG Delta)
{
    ULONG x, y;

    for (y = 0; y < Height && Top + y < BV_SCREEN_HEIGHT; y++) {
        const UCHAR *src = Bits + (LONG)y * Delta;
        PUCHAR dst = Screen[Top + y];

        for (x = 0; x < Width && Left + x < BV_SCREEN_WIDTH; x++) {
            if (Bpp == 4)
                dst[Left + x] = (x & 1) ? (src[x >> 1] & 0xF) : (src[x >> 1] >> 4);
            else
                dst[Left + x] = (src[x >> 3] & (0x80 >> (x & 7))) ? 8 : 0;
        }
    }
    if (Width && Height)
        BvDirty(Left, Top, Left + Width - 1, Top + Height - 1);
}

/* Draw a BI_RLE4 bitmap (bottom-up), clipped to Width columns. */
static VOID
BvRle4(const UCHAR *Bits, ULONG Left, ULONG Top, ULONG Width, ULONG Height)
{
    const UCHAR *p = Bits;
    ULONG right = Left + Width;             /* first column not drawn */
    ULONG x = Left, y = Top + Height - 1;
    ULONG lines = 0, i;

    while (lines <= Height) {
        UCHAR n = *p++;

        if (n != 0) {
            /* Encoded run: n pixels alternating between two colours. */
            UCHAR c = *p++;
            for (i = 0; i < n && x < right; i++, x++)
                BvSetPixel(x, y, (i & 1) ? (c & 0xF) : (c >> 4));
            continue;
        }
        n = *p++;
        if (n == 0) {                       /* end of line */
            x = Left;
            y--;
            lines++;
        } else if (n == 1) {                /* end of bitmap */
            break;
        } else if (n == 2) {                /* delta */
            x += p[0];
            y -= p[1];
            lines += p[1];
            p += 2;
        } else {                            /* absolute run, padded to 16 bits */
            for (i = 0; i < n; i++, x++) {
                UCHAR c = p[i >> 1];
                if (x < right)
                    BvSetPixel(x, y, (i & 1) ? (c & 0xF) : (c >> 4));
            }
            if (x > right)
                x = right;
            p += (n + 1) / 2;
            if ((p - Bits) & 1)
                p++;
        }
    }
    BvDirty(Left, Top, Left + Width - 1, Top + Height - 1);
}

/* ---------------------------------------------------------------------- */
/* Frame buffer discovery                                                 */
/* ---------------------------------------------------------------------- */

static PVOID
BvMap(PVOID Context, ULONGLONG Physical, ULONG Length)
{
    PHYSICAL_ADDRESS pa;

    pa.QuadPart = (LONGLONG)Physical;
    return MmMapIoSpace(pa, Length, MmNonCached);
}

static VOID
BvUnmap(PVOID Context, PVOID Virtual, ULONG Length)
{
    MmUnmapIoSpace(Virtual, Length);
}

static BOOLEAN
BvFindFramebuffer(VOID)
{
    CB_FB_INFO info;
    CB_STATUS status;
    PHYSICAL_ADDRESS pa;
    ULONG bpp, bits;

    status = CbFindFramebuffer(BvMap, BvUnmap, NULL, &info);
    if (status != CbFound) {
        DbgPrint("bootvid: no frame buffer in the coreboot table (status %d)\n", status);
        return FALSE;
    }

    bpp = CbFramebufferBpp(&info);
    switch (bpp) {
    case 32: Fb.BytesPerPixel = 4; break;
    case 24: Fb.BytesPerPixel = 3; break;
    case 16:
    case 15: Fb.BytesPerPixel = 2; break;
    default:
        DbgPrint("bootvid: unsupported frame buffer depth %u\n", bpp);
        return FALSE;
    }
    Fb.RedPos = info.RedMaskPos;     Fb.RedSize = info.RedMaskSize;
    Fb.GreenPos = info.GreenMaskPos; Fb.GreenSize = info.GreenMaskSize;
    Fb.BluePos = info.BlueMaskPos;   Fb.BlueSize = info.BlueMaskSize;
    if (Fb.RedSize == 0 && Fb.GreenSize == 0 && Fb.BlueSize == 0) {
        if (bpp == 15) {
            Fb.RedPos = 10; Fb.RedSize = 5; Fb.GreenPos = 5; Fb.GreenSize = 5; Fb.BlueSize = 5;
        } else if (bpp == 16) {
            Fb.RedPos = 11; Fb.RedSize = 5; Fb.GreenPos = 5; Fb.GreenSize = 6; Fb.BlueSize = 5;
        } else {
            Fb.RedPos = 16; Fb.RedSize = 8; Fb.GreenPos = 8; Fb.GreenSize = 8; Fb.BlueSize = 8;
        }
        Fb.BluePos = 0;
    }
    bits = Fb.BytesPerPixel * 8;
    if (Fb.RedPos + Fb.RedSize > bits || Fb.GreenPos + Fb.GreenSize > bits ||
        Fb.BluePos + Fb.BlueSize > bits) {
        DbgPrint("bootvid: bad frame buffer colour masks\n");
        return FALSE;
    }

    Fb.Width = info.XResolution;
    Fb.Height = info.YResolution;
    Fb.Pitch = info.BytesPerLine;
    if (Fb.Width < BV_SCREEN_WIDTH || Fb.Height < BV_SCREEN_HEIGHT ||
        Fb.Width > 0x4000 || Fb.Height > 0x4000 ||
        Fb.Pitch < Fb.Width * Fb.BytesPerPixel || info.PhysicalAddress == 0) {
        DbgPrint("bootvid: unusable frame buffer %ux%u pitch %u\n", Fb.Width, Fb.Height, Fb.Pitch);
        return FALSE;
    }

    Fb.Scale = Fb.Width / BV_SCREEN_WIDTH;
    if (Fb.Height / BV_SCREEN_HEIGHT < Fb.Scale)
        Fb.Scale = Fb.Height / BV_SCREEN_HEIGHT;
    Fb.OriginX = (Fb.Width - BV_SCREEN_WIDTH * Fb.Scale) / 2;
    Fb.OriginY = (Fb.Height - BV_SCREEN_HEIGHT * Fb.Scale) / 2;

    Fb.MapLength = Fb.Pitch * Fb.Height;
    pa.QuadPart = (LONGLONG)info.PhysicalAddress;
    Fb.Base = MmMapIoSpace(pa, Fb.MapLength, MmNonCached);
    if (Fb.Base == NULL) {
        DbgPrint("bootvid: cannot map the frame buffer at 0x%08lx\n", pa.LowPart);
        return FALSE;
    }
    DbgPrint("bootvid: frame buffer %ux%u, %u bpp, pitch %u at 0x%08lx; screen x%u at (%u,%u)\n",
             Fb.Width, Fb.Height, bpp, Fb.Pitch, pa.LowPart, Fb.Scale, Fb.OriginX, Fb.OriginY);
    return TRUE;
}

/* ---------------------------------------------------------------------- */
/* Exports                                                                */
/* ---------------------------------------------------------------------- */

/*
 * Called once from InbvDriverInitialize.  SetMode is FALSE when the loader
 * options contain BOOTLOGO or the kernel does not own the display; then the
 * screen is left as it is.  Returns FALSE (the kernel then never calls
 * bootvid again) if there is no usable frame buffer.
 */
BOOLEAN NTAPI
VidInitialize(BOOLEAN SetMode)
{
    ULONG i;

    if (Fb.Base == NULL && !BvFindFramebuffer())
        return FALSE;

    for (i = 0; i < BV_COLORS; i++) {
        Dac[i][0] = DefaultDac[i][0];
        Dac[i][1] = DefaultDac[i][1];
        Dac[i][2] = DefaultDac[i][2];
        BvUpdatePixel(i);
    }
    if (SetMode) {
        RtlZeroMemory(Screen, sizeof(Screen));
        Border = 0;
        BorderDirty = TRUE;
        BvDirty(0, 0, BV_SCREEN_WIDTH - 1, BV_SCREEN_HEIGHT - 1);
        BvFlush();
    }
    return TRUE;
}

/*
 * Called when the kernel takes the display back, e.g. for a bug check after
 * the display driver owned the screen: default palette, black screen,
 * cursor at the top left (the scroll region is kept).  Everything is
 * redrawn, since another driver may have drawn over the frame buffer.
 */
VOID NTAPI
VidResetDisplay(BOOLEAN HalReset)
{
    if (Fb.Base == NULL)
        return;
    CurrentX = 0;
    CurrentY = 0;
    RestoreLine = FALSE;
    BvLoadDefaultPalette();
    BvFill(0, 0, BV_SCREEN_WIDTH - 1, BV_SCREEN_HEIGHT - 1, 0);
    Border = 0;
    BorderDirty = TRUE;
    BvDirty(0, 0, BV_SCREEN_WIDTH - 1, BV_SCREEN_HEIGHT - 1);
    BvFlush();
}

/*
 * Called when a display driver takes the screen over.  The original
 * restores VGA state here; a frame buffer needs nothing.
 */
VOID NTAPI
VidCleanUp(VOID)
{
}

ULONG NTAPI
VidSetTextColor(ULONG Color)
{
    ULONG old = TextColor;

    TextColor = Color;
    return old;
}

VOID NTAPI
VidSetScrollRegion(ULONG Left, ULONG Top, ULONG Right, ULONG Bottom)
{
    ScrollLeft = Left;
    ScrollTop = Top;
    ScrollRight = Right;
    ScrollBottom = Bottom;
    CurrentX = Left;
    CurrentY = Top;
}

/*
 * Print at the cursor in the text colour, wrapping at the right edge of
 * the scroll region and scrolling it.  The two boundary tests differ
 * slightly (a line feed scrolls when the new line starts at the bottom
 * row, wrapping only when it starts below it); both are kept as in the
 * original so that the kernel's screens lay out identically.
 */
VOID NTAPI
VidDisplayString(PUCHAR String)
{
    if (Fb.Base == NULL)
        return;

    for (; *String; String++) {
        UCHAR c = *String;

        if (c == '\n') {
            CurrentY += BV_LINE_HEIGHT;
            if (CurrentY >= ScrollBottom) {
                BvScroll(BV_LINE_HEIGHT);
                CurrentY -= BV_LINE_HEIGHT;
                BvLineBackground(CurrentY, TRUE);
            }
            CurrentX = ScrollLeft;
            BvLineBackground(CurrentY, FALSE);
        } else if (c == '\r') {
            CurrentX = ScrollLeft;
            if (String[1] != '\n')
                RestoreLine = TRUE;
        } else {
            if (RestoreLine) {
                BvLineBackground(CurrentY, TRUE);
                RestoreLine = FALSE;
            }
            BvDrawChar(c, CurrentX, CurrentY, TextColor, BV_TRANSPARENT);
            CurrentX += BV_CHAR_WIDTH;
            if (CurrentX > ScrollRight) {
                CurrentY += BV_LINE_HEIGHT;
                if (CurrentY > ScrollBottom) {
                    BvScroll(BV_LINE_HEIGHT);
                    CurrentY -= BV_LINE_HEIGHT;
                    BvLineBackground(CurrentY, TRUE);
                }
                CurrentX = ScrollLeft;
            }
        }
    }
    BvFlush();
}

/* Not used by XP's kernel.  No wrapping; fixed colours as in the original. */
VOID NTAPI
VidDisplayStringXY(PUCHAR String, ULONG Left, ULONG Top, BOOLEAN Transparent)
{
    if (Fb.Base == NULL)
        return;
    for (; *String; String++, Left += BV_CHAR_WIDTH)
        BvDrawChar(*String, Left, Top, BV_XY_TEXT_COLOR,
                   Transparent ? BV_TRANSPARENT : BV_XY_BACK_COLOR);
    BvFlush();
}

VOID NTAPI
VidSolidColorFill(ULONG Left, ULONG Top, ULONG Right, ULONG Bottom, UCHAR Color)
{
    if (Fb.Base == NULL)
        return;
    BvFill(Left, Top, Right, Bottom, Color);
    /* A fill of the whole screen also colours the area around it. */
    if (Left == 0 && Top == 0 && Right >= BV_SCREEN_WIDTH - 1 &&
        Bottom >= BV_SCREEN_HEIGHT - 1 && Border != (Color & 0xF)) {
        Border = Color & 0xF;
        BorderDirty = TRUE;
    }
    BvFlush();
}

/*
 * Draw a bitmap: BITMAPINFOHEADER, a 16-entry colour table, then 4 bpp
 * pixels (BI_RGB, bottom-up or top-down, or BI_RLE4).  The colour table is
 * loaded into the palette first (biClrUsed entries, all 16 if zero), which
 * also recolours the rest of the screen.  Deeper bitmaps only load the
 * palette, as in the original.
 */
VOID NTAPI
VidBitBlt(PUCHAR Buffer, ULONG Left, ULONG Top)
{
    const BV_BITMAPINFOHEADER *bih = (const BV_BITMAPINFOHEADER *)Buffer;
    const UCHAR *palette = Buffer + bih->biSize;
    const UCHAR *bits = palette + BV_COLORS * 4;
    ULONG colors, bpp, i, width, height;
    LONG delta;

    if (Fb.Base == NULL)
        return;

    colors = bih->biClrUsed ? bih->biClrUsed : BV_COLORS;
    if (colors > BV_COLORS)
        colors = BV_COLORS;
    for (i = 0; i < colors; i++)    /* RGBQUAD: blue, green, red, reserved */
        BvSetDac(i, palette[4 * i + 2] >> 2, palette[4 * i + 1] >> 2, palette[4 * i] >> 2);

    bpp = (ULONG)bih->biBitCount * bih->biPlanes;
    width = (ULONG)bih->biWidth;
    if (bpp != 4 && bpp != 1)
        goto out;
    if (bih->biCompression == BV_BI_RLE4) {
        if (bih->biWidth > 0 && bih->biHeight > 0)
            BvRle4(bits, Left, Top, width, (ULONG)bih->biHeight);
        goto out;
    }
    if (bih->biWidth <= 0 || bih->biHeight == 0)
        goto out;
    delta = (LONG)((((width * bpp) + 31) >> 3) & ~3u);
    if (bih->biHeight < 0) {
        height = (ULONG)-bih->biHeight;             /* top-down */
    } else {
        height = (ULONG)bih->biHeight;              /* bottom-up */
        bits += (height - 1) * (ULONG)delta;
        delta = -delta;
    }
    BvBlt(bits, Left, Top, width, height, bpp, delta);
out:
    BvFlush();
}

/* Draw a 4 bpp buffer (high nibble = left pixel, Delta bytes per row). */
VOID NTAPI
VidBufferToScreenBlt(PUCHAR Buffer, ULONG Left, ULONG Top, ULONG Width, ULONG Height,
                     ULONG Delta)
{
    if (Fb.Base == NULL || Width == 0 || Height == 0)
        return;
    BvBlt(Buffer, Left, Top, Width, Height, 4, (LONG)Delta);
    BvFlush();
}

/* Read the screen into a 4 bpp buffer; the buffer is cleared first. */
VOID NTAPI
VidScreenToBufferBlt(PUCHAR Buffer, ULONG Left, ULONG Top, ULONG Width, ULONG Height,
                     ULONG Delta)
{
    ULONG x, y;

    RtlZeroMemory(Buffer, Height * Delta);
    for (y = 0; y < Height && Top + y < BV_SCREEN_HEIGHT; y++) {
        PUCHAR dst = Buffer + y * Delta;
        const UCHAR *src = Screen[Top + y];

        for (x = 0; x < Width && Left + x < BV_SCREEN_WIDTH; x++)
            dst[x >> 1] |= (x & 1) ? src[Left + x] : (UCHAR)(src[Left + x] << 4);
    }
}

/*
 * Image entry point.  bootvid.dll is loaded as an import of the kernel and
 * this is never called; it exists because every image needs one.
 */
NTSTATUS NTAPI
DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath)
{
    return STATUS_SUCCESS;
}
