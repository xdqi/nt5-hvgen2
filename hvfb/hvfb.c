/*
 * hvfb - linear frame buffer display miniport for Windows XP on machines
 * without VGA hardware (Hyper-V Generation 2 booted through CSMWrap).
 *
 * A legacy (non-PnP) VideoPort miniport, like vga.sys: it works as the
 * display miniport of XP text-mode setup and, with framebuf.dll, as the
 * display driver of the installed system.
 *
 * Firmware contract: modes are set with VBE 4F02 (linear frame buffer bit
 * set), and the device returns to text mode 3 through INT 10h.  A BIOS that
 * scans a text buffer out to the frame buffer (CSMWrap on Hyper-V Gen2)
 * stops doing so as soon as a graphics mode is set this way.
 */
#include "hvfb.h"

#define TEXT_BUFFER_PHYS    0xB8000
#define TEXT_BUFFER_CELLS   (80 * 25)

static const WCHAR ChipType[] = L"Linear frame buffer (VBE)";
static const WCHAR ChipTypeCb[] = L"Linear frame buffer (firmware)";
static const WCHAR DacType[] = L"Direct colour";
static const WCHAR AdapterString[] = L"Hyper-V Gen2 / VBE frame buffer (hvfb)";
static const WCHAR BiosString[] = L"VESA BIOS Extensions";

static VOID
HvfbSetRegistryString(PHVFB_EXTENSION Ext, PWSTR Name, const WCHAR *Value, ULONG Size)
{
    VideoPortSetRegistryParameters(Ext, Name, (PVOID)Value, Size);
}

/* ---------------------------------------------------------------------- */
/* Frame buffer helpers                                                   */
/* ---------------------------------------------------------------------- */

/* Return a kernel mapping that covers Length bytes of Mode's frame buffer. */
static PVOID
HvfbKernelMapping(PHVFB_EXTENSION Ext, PHVFB_MODE Mode, ULONG Length)
{
    if (Ext->FbVirt != NULL &&
        (Ext->FbVirtPhys.QuadPart != Mode->FrameBuffer.QuadPart ||
         Ext->FbVirtLength < Length)) {
        VideoPortFreeDeviceBase(Ext, Ext->FbVirt);
        Ext->FbVirt = NULL;
    }
    if (Ext->FbVirt == NULL) {
        Ext->FbVirt = VideoPortGetDeviceBase(Ext, Mode->FrameBuffer, Length,
                                             VIDEO_MEMORY_SPACE_MEMORY);
        Ext->FbVirtPhys = Mode->FrameBuffer;
        Ext->FbVirtLength = Ext->FbVirt ? Length : 0;
        if (Ext->FbVirt == NULL)
            HvfbLog("cannot map frame buffer 0x%08x (%u bytes)\n",
                    Mode->FrameBuffer.LowPart, Length);
    }
    return Ext->FbVirt;
}

static VP_STATUS
HvfbSetMode(PHVFB_EXTENSION Ext, ULONG Index, BOOLEAN NoZero)
{
    PHVFB_MODE m = &Ext->Modes[Index];
    INT10_BIOS_ARGUMENTS args;
    VP_STATUS status;

    if (m->VbeMode != HVFB_NO_VBE_MODE) {
        /*
         * Bit 14: linear frame buffer.  Bit 15: keep memory; we clear it
         * ourselves below, because some BIOSes (SeaVGABIOS's coreboot
         * frame buffer back end) clear a high frame buffer with INT 15h
         * AH=87h, which switches to protected mode and cannot run in the
         * virtual-8086 monitor that VideoPort uses for INT 10h.
         */
        VideoPortZeroMemory(&args, sizeof(args));
        args.Eax = 0x4F02;
        args.Ebx = (ULONG)m->VbeMode | 0x4000 | 0x8000;
        status = HvfbCallBios(Ext, &args);
        if (status != NO_ERROR || (args.Eax & 0xFFFF) != 0x004F) {
            HvfbLog("VBE 4F02 %03x failed (status %u, ax %04x)\n",
                    (ULONG)m->VbeMode, status, args.Eax & 0xFFFF);
            return status != NO_ERROR ? status : ERROR_INVALID_PARAMETER;
        }
    }

    if (!NoZero) {
        ULONG length = m->Stride * m->Height;
        PVOID fb = HvfbKernelMapping(Ext, m, length);
        if (fb != NULL)
            VideoPortZeroDeviceMemory(fb, length);
    }

    Ext->CurrentMode = Index;
    HvfbTrace("mode %u set (%ux%u %u bpp)\n", Index, m->Width, m->Height, (ULONG)m->Bpp);
    return NO_ERROR;
}

static VP_STATUS
HvfbResetToText(PHVFB_EXTENSION Ext)
{
    INT10_BIOS_ARGUMENTS args;
    VP_STATUS status = NO_ERROR;
    ULONG i;

    Ext->CurrentMode = HVFB_NO_MODE;
    if (!Ext->HaveInt10)
        return NO_ERROR;

    /*
     * AH=00h AL=83h: mode 3, 80x25 colour text, without clearing (same
     * INT 15h AH=87h concern as in HvfbSetMode); the text buffer is
     * cleared here instead.
     */
    VideoPortZeroMemory(&args, sizeof(args));
    args.Eax = 0x0083;
    status = HvfbCallBios(Ext, &args);
    if (status != NO_ERROR)
        HvfbLog("INT 10h mode 3 failed (%u)\n", status);

    if (Ext->TextVirt != NULL) {
        volatile USHORT *cell = Ext->TextVirt;
        for (i = 0; i < TEXT_BUFFER_CELLS; i++)
            cell[i] = 0x0720;
    }
    return status;
}

static VOID
HvfbFillModeInfo(PHVFB_EXTENSION Ext, ULONG Index, PVIDEO_MODE_INFORMATION Info)
{
    PHVFB_MODE m = &Ext->Modes[Index];
    ULONG lines;

    VideoPortZeroMemory(Info, sizeof(*Info));
    Info->Length = sizeof(VIDEO_MODE_INFORMATION);
    Info->ModeIndex = Index;
    Info->VisScreenWidth = m->Width;
    Info->VisScreenHeight = m->Height;
    Info->ScreenStride = m->Stride;
    Info->NumberOfPlanes = 1;
    Info->BitsPerPlane = m->Bpp;
    Info->Frequency = 60;           /* unknown; a fixed value keeps DEVMODEs stable */
    Info->XMillimeter = 320;
    Info->YMillimeter = 240;
    /* setupdd.sys clamps colour intensities to these widths. */
    Info->NumberRedBits = m->RedSize;
    Info->NumberGreenBits = m->GreenSize;
    Info->NumberBlueBits = m->BlueSize;
    Info->RedMask = ((1UL << m->RedSize) - 1) << m->RedPos;
    Info->GreenMask = ((1UL << m->GreenSize) - 1) << m->GreenPos;
    Info->BlueMask = ((1UL << m->BlueSize) - 1) << m->BluePos;
    Info->AttributeFlags = VIDEO_MODE_COLOR | VIDEO_MODE_GRAPHICS | VIDEO_MODE_LINEAR;
    Info->VideoMemoryBitmapWidth = m->Stride / (m->Bpp / 8);
    lines = m->VramLength / m->Stride;
    if (lines > 0xFFFF)
        lines = 0xFFFF;
    if (lines < m->Height)
        lines = m->Height;
    Info->VideoMemoryBitmapHeight = lines;
    if (lines == m->Height)
        Info->AttributeFlags |= VIDEO_MODE_NO_OFF_SCREEN;
}

/* ---------------------------------------------------------------------- */
/* VideoPort callbacks                                                    */
/* ---------------------------------------------------------------------- */

static VP_STATUS NTAPI
HvfbFindAdapter(PVOID HwDeviceExtension, PVOID HwContext, PWSTR ArgumentString,
                PVIDEO_PORT_CONFIG_INFO ConfigInfo, PUCHAR Again)
{
    PHVFB_EXTENSION Ext = HwDeviceExtension;
    PHYSICAL_ADDRESS pa;
    VP_STATUS status;

    (void)HwContext;
    (void)ArgumentString;
    *Again = FALSE;

    if (ConfigInfo->Length < sizeof(VIDEO_PORT_CONFIG_INFO))
        return ERROR_INVALID_PARAMETER;

    Ext->CurrentMode = HVFB_NO_MODE;

    /* INT 10h with buffer support (needed for VBE 4F00/4F01). */
    Ext->Int10.Size = sizeof(VIDEO_PORT_INT10_INTERFACE);
    Ext->Int10.Version = VIDEO_PORT_INT10_INTERFACE_VERSION_1;
    /*
     * The interface comes back referenced; it is kept for the lifetime of
     * the driver, so InterfaceReference/InterfaceDereference are never
     * called.  (Careful if that changes: mingw-w64's miniport.h declares
     * PINTERFACE_REFERENCE without NTAPI, but videoprt implements it as
     * stdcall, the default calling convention of WDK-built x86 drivers.)
     */
    status = VideoPortQueryServices(Ext, VideoPortServicesInt10, (PINTERFACE)&Ext->Int10);
    if (status == NO_ERROR) {
        Ext->HaveInt10 = TRUE;
    } else {
        HvfbLog("no INT 10h interface (%u)\n", status);
    }

    /* Fallback frame buffer description left in low memory by CSMWrap. */
    HvfbReadCorebootTable(Ext);

    if (!Ext->HaveInt10 && !Ext->HaveCbFb)
        return ERROR_DEV_NOT_EXIST;

    /* Optional: lets RESET_DEVICE blank the text screen it switches to. */
    pa.QuadPart = TEXT_BUFFER_PHYS;
    Ext->TextVirt = VideoPortGetDeviceBase(Ext, pa, TEXT_BUFFER_CELLS * 2,
                                           VIDEO_MEMORY_SPACE_MEMORY);

    /*
     * XP's videoprt builds the V86 address space for INT 10h on the first
     * open of the device, and skips that entirely (every INT 10h then fails
     * with ERROR_INVALID_PARAMETER) unless the miniport names a VDM video
     * memory range.  The range is mapped live from physical memory; the
     * rest of the first megabyte (IVT/BDA, EBDA, ROMs) is a private copy
     * taken at that point.  Use the legacy VGA window, as vga.sys does; on
     * Hyper-V Gen2 it is plain RAM.
     */
    ConfigInfo->VdmPhysicalVideoMemoryAddress.QuadPart = 0xA0000;
    ConfigInfo->VdmPhysicalVideoMemoryLength = 0x20000;

    /* No VGA registers: nothing for full-screen DOS boxes to emulate. */
    ConfigInfo->NumEmulatorAccessEntries = 0;
    ConfigInfo->EmulatorAccessEntries = NULL;
    ConfigInfo->EmulatorAccessEntriesContext = 0;
    ConfigInfo->HardwareStateSize = 0;

    HvfbSetRegistryString(Ext, L"HardwareInformation.ChipType",
                          Ext->HaveInt10 ? ChipType : ChipTypeCb,
                          Ext->HaveInt10 ? sizeof(ChipType) : sizeof(ChipTypeCb));
    HvfbSetRegistryString(Ext, L"HardwareInformation.DacType", DacType, sizeof(DacType));
    HvfbSetRegistryString(Ext, L"HardwareInformation.AdapterString", AdapterString,
                          sizeof(AdapterString));
    HvfbSetRegistryString(Ext, L"HardwareInformation.BiosString", BiosString,
                          sizeof(BiosString));

    HvfbLog("adapter found (int10 %s, coreboot fb %s)\n",
            Ext->HaveInt10 ? "yes" : "no", Ext->HaveCbFb ? "yes" : "no");
    return NO_ERROR;
}

/*
 * Called on the first open of \Device\VideoN.  VideoPort only accepts INT 10h
 * calls after that open (it runs them in the opening process, CSRSS or
 * text-mode setup), so VBE discovery happens here and not in FindAdapter.
 */
static BOOLEAN NTAPI
HvfbInitialize(PVOID HwDeviceExtension)
{
    PHVFB_EXTENSION Ext = HwDeviceExtension;
    ULONG memory;

    if (!Ext->Initialized) {
        Ext->Initialized = TRUE;
        if (!HvfbQueryVbeModes(Ext) && Ext->HaveCbFb)
            HvfbLog("VBE unusable, using the coreboot frame buffer\n");
        HvfbFinishModeList(Ext);

        if (Ext->NumModes != 0) {
            memory = Ext->UsingVbe && Ext->VbeMemory ? Ext->VbeMemory
                                                     : Ext->Modes[0].VramLength;
            VideoPortSetRegistryParameters(Ext, L"HardwareInformation.MemorySize",
                                           &memory, sizeof(memory));
        }
    }

    if (Ext->NumModes == 0)
        HvfbLog("no usable display mode\n");
    return Ext->NumModes != 0;
}

static VP_STATUS
HvfbMapVideoMemory(PHVFB_EXTENSION Ext, PVIDEO_REQUEST_PACKET Rp)
{
    PVIDEO_MEMORY in = Rp->InputBuffer;
    PVIDEO_MEMORY_INFORMATION out = Rp->OutputBuffer;
    PHVFB_MODE m;
    ULONG inIoSpace = VIDEO_MEMORY_SPACE_MEMORY;
    ULONG length;
    PVOID va;
    VP_STATUS status;

    if (Rp->InputBufferLength < sizeof(VIDEO_MEMORY) ||
        Rp->OutputBufferLength < sizeof(VIDEO_MEMORY_INFORMATION))
        return ERROR_INSUFFICIENT_BUFFER;

    m = &Ext->Modes[Ext->CurrentMode != HVFB_NO_MODE ? Ext->CurrentMode : 0];
    length = m->VramLength;
    va = in->RequestedVirtualAddress;
    status = VideoPortMapMemory(Ext, m->FrameBuffer, &length, &inIoSpace, &va);
    if (status != NO_ERROR) {
        HvfbLog("VideoPortMapMemory(0x%08x, %u) failed (%u)\n",
                m->FrameBuffer.LowPart, m->VramLength, status);
        return status;
    }

    out->VideoRamBase = va;
    out->VideoRamLength = length;
    out->FrameBufferBase = va;
    out->FrameBufferLength = m->Stride * m->Height;
    Rp->StatusBlock->Information = sizeof(VIDEO_MEMORY_INFORMATION);
    return NO_ERROR;
}

static VP_STATUS
HvfbShareVideoMemory(PHVFB_EXTENSION Ext, PVIDEO_REQUEST_PACKET Rp)
{
    PVIDEO_SHARE_MEMORY in = Rp->InputBuffer;
    PVIDEO_SHARE_MEMORY_INFORMATION out = Rp->OutputBuffer;
    PHVFB_MODE m;
    ULONG inIoSpace = VIDEO_MEMORY_SPACE_USER_MODE;
    ULONG length;
    PVOID va;
    VP_STATUS status;

    if (Rp->InputBufferLength < sizeof(VIDEO_SHARE_MEMORY) ||
        Rp->OutputBufferLength < sizeof(VIDEO_SHARE_MEMORY_INFORMATION))
        return ERROR_INSUFFICIENT_BUFFER;

    m = &Ext->Modes[Ext->CurrentMode != HVFB_NO_MODE ? Ext->CurrentMode : 0];
    if (in->ViewOffset > m->VramLength || in->ViewSize > m->VramLength - in->ViewOffset)
        return ERROR_INVALID_PARAMETER;

    /* Map from the start of the frame buffer; the caller adds the offset. */
    length = in->ViewOffset + in->ViewSize;
    va = in->ProcessHandle;         /* USER_MODE: VirtualAddress carries the process */
    status = VideoPortMapMemory(Ext, m->FrameBuffer, &length, &inIoSpace, &va);
    if (status != NO_ERROR)
        return status;

    out->SharedViewOffset = in->ViewOffset;
    out->VirtualAddress = va;
    out->SharedViewSize = in->ViewSize;
    Rp->StatusBlock->Information = sizeof(VIDEO_SHARE_MEMORY_INFORMATION);
    return NO_ERROR;
}

static BOOLEAN NTAPI
HvfbStartIO(PVOID HwDeviceExtension, PVIDEO_REQUEST_PACKET Rp)
{
    PHVFB_EXTENSION Ext = HwDeviceExtension;
    VP_STATUS status;
    ULONG i;

    Rp->StatusBlock->Information = 0;

    switch (Rp->IoControlCode) {
    case IOCTL_VIDEO_QUERY_NUM_AVAIL_MODES: {
        PVIDEO_NUM_MODES out = Rp->OutputBuffer;
        if (Rp->OutputBufferLength < sizeof(VIDEO_NUM_MODES)) {
            status = ERROR_INSUFFICIENT_BUFFER;
            break;
        }
        out->NumModes = Ext->NumModes;
        out->ModeInformationLength = sizeof(VIDEO_MODE_INFORMATION);
        Rp->StatusBlock->Information = sizeof(VIDEO_NUM_MODES);
        status = NO_ERROR;
        break;
    }

    case IOCTL_VIDEO_QUERY_AVAIL_MODES: {
        PVIDEO_MODE_INFORMATION out = Rp->OutputBuffer;
        if (Rp->OutputBufferLength < Ext->NumModes * sizeof(VIDEO_MODE_INFORMATION)) {
            status = ERROR_INSUFFICIENT_BUFFER;
            break;
        }
        for (i = 0; i < Ext->NumModes; i++)
            HvfbFillModeInfo(Ext, i, &out[i]);
        Rp->StatusBlock->Information = Ext->NumModes * sizeof(VIDEO_MODE_INFORMATION);
        status = NO_ERROR;
        break;
    }

    case IOCTL_VIDEO_QUERY_CURRENT_MODE:
        if (Rp->OutputBufferLength < sizeof(VIDEO_MODE_INFORMATION)) {
            status = ERROR_INSUFFICIENT_BUFFER;
            break;
        }
        if (Ext->CurrentMode == HVFB_NO_MODE) {
            status = ERROR_INVALID_FUNCTION;
            break;
        }
        HvfbFillModeInfo(Ext, Ext->CurrentMode, Rp->OutputBuffer);
        Rp->StatusBlock->Information = sizeof(VIDEO_MODE_INFORMATION);
        status = NO_ERROR;
        break;

    case IOCTL_VIDEO_SET_CURRENT_MODE: {
        ULONG req;
        if (Rp->InputBufferLength < sizeof(VIDEO_MODE)) {
            status = ERROR_INSUFFICIENT_BUFFER;
            break;
        }
        req = ((PVIDEO_MODE)Rp->InputBuffer)->RequestedMode;
        i = req & ~(VIDEO_MODE_NO_ZERO_MEMORY | VIDEO_MODE_MAP_MEM_LINEAR);
        if (i >= Ext->NumModes) {
            status = ERROR_INVALID_PARAMETER;
            break;
        }
        status = HvfbSetMode(Ext, i, (req & VIDEO_MODE_NO_ZERO_MEMORY) != 0);
        break;
    }

    case IOCTL_VIDEO_RESET_DEVICE:
        status = HvfbResetToText(Ext);
        break;

    case IOCTL_VIDEO_MAP_VIDEO_MEMORY:
        status = HvfbMapVideoMemory(Ext, Rp);
        break;

    case IOCTL_VIDEO_UNMAP_VIDEO_MEMORY:
        if (Rp->InputBufferLength < sizeof(VIDEO_MEMORY)) {
            status = ERROR_INSUFFICIENT_BUFFER;
            break;
        }
        status = VideoPortUnmapMemory(Ext,
                    ((PVIDEO_MEMORY)Rp->InputBuffer)->RequestedVirtualAddress, NULL);
        break;

    case IOCTL_VIDEO_SHARE_VIDEO_MEMORY:
        status = HvfbShareVideoMemory(Ext, Rp);
        break;

    case IOCTL_VIDEO_UNSHARE_VIDEO_MEMORY: {
        PVIDEO_SHARE_MEMORY in = Rp->InputBuffer;
        if (Rp->InputBufferLength < sizeof(VIDEO_SHARE_MEMORY)) {
            status = ERROR_INSUFFICIENT_BUFFER;
            break;
        }
        status = VideoPortUnmapMemory(Ext, in->RequestedVirtualAddress, in->ProcessHandle);
        break;
    }

    case IOCTL_VIDEO_SET_COLOR_REGISTERS:
        /* Only direct colour modes are offered; there is no palette. */
        status = NO_ERROR;
        break;

    case IOCTL_VIDEO_QUERY_PUBLIC_ACCESS_RANGES:
        /* No registers to expose. */
        status = NO_ERROR;
        break;

    case IOCTL_VIDEO_QUERY_POINTER_CAPABILITIES: {
        PVIDEO_POINTER_CAPABILITIES out = Rp->OutputBuffer;
        if (Rp->OutputBufferLength < sizeof(VIDEO_POINTER_CAPABILITIES)) {
            status = ERROR_INSUFFICIENT_BUFFER;
            break;
        }
        /* No hardware cursor: GDI draws the pointer. */
        out->Flags = 0;
        out->MaxWidth = 0;
        out->MaxHeight = 0;
        out->HWPtrBitmapStart = (ULONG)-1;
        out->HWPtrBitmapEnd = (ULONG)-1;
        Rp->StatusBlock->Information = sizeof(VIDEO_POINTER_CAPABILITIES);
        status = NO_ERROR;
        break;
    }

    default:
        HvfbTrace("unsupported IOCTL %08x\n", Rp->IoControlCode);
        status = ERROR_INVALID_FUNCTION;
        break;
    }

    Rp->StatusBlock->Status = status;
    return TRUE;
}

/* ---------------------------------------------------------------------- */

ULONG NTAPI
DriverEntry(PVOID Context1, PVOID Context2)
{
    /*
     * Legacy (non-PnP) miniport: VideoPort calls HwFindAdapter once per bus
     * of the given type.  A Hyper-V Gen2 VM has no PCI bus but always has
     * the ISA bus of the PC HALs; the first type that works wins.
     */
    static const INTERFACE_TYPE buses[] = { Isa, Internal, PCIBus };
    VIDEO_HW_INITIALIZATION_DATA hw;
    ULONG status = (ULONG)ERROR_DEV_NOT_EXIST;
    ULONG i;

    for (i = 0; i < sizeof(buses) / sizeof(buses[0]); i++) {
        VideoPortZeroMemory(&hw, sizeof(hw));
        hw.HwInitDataSize = sizeof(hw);
        hw.AdapterInterfaceType = buses[i];
        hw.HwFindAdapter = HvfbFindAdapter;
        hw.HwInitialize = HvfbInitialize;
        hw.HwStartIO = HvfbStartIO;
        hw.HwDeviceExtensionSize = sizeof(HVFB_EXTENSION);

        status = VideoPortInitialize(Context1, Context2, &hw, NULL);
        if (status == NO_ERROR)
            break;
    }
    return status;
}
