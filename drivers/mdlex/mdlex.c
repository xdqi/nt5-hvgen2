/*
 * mdlex.sys: a kernel export driver that gives Windows XP two ntoskrnl
 * routines that Hyper-V's Dynamic Memory client driver needs.
 *
 * dmvsc.sys (Integration Services 6.3.9600, built for Windows Server 2003
 * SP1+) uses two routines that XP SP3's kernel cannot satisfy:
 *
 *   MmAllocatePagesForMdlEx  introduced in Server 2003 SP1, XP does not export
 *                            it, so without it dmvsc cannot even load.  dmvsc
 *                            calls it on its balloon-inflate path to take pages
 *                            away from the guest and hand the page runs to the
 *                            host.
 *   MmAddPhysicalMemory      XP exports it but cannot hot-add memory; see below.
 *
 * Everything else dmvsc needs (MmAllocatePagesForMdl, MmFreePagesFromMdl, KMDF
 * via vmbkmcl.sys, ...) already exists on XP.
 *
 * MmAllocatePagesForMdlEx is MmAllocatePagesForMdl plus a trailing CacheType
 * and Flags argument.  The first four arguments are identical and have the
 * same meaning, so this routine forwards them to XP's MmAllocatePagesForMdl
 * and then applies the two flags that matter:
 *
 *   MM_ALLOCATE_FULLY_REQUIRED  the Ex contract frees a short allocation and
 *                               returns NULL instead of a partial MDL; the
 *                               base routine always returns whatever it got,
 *                               so we check the byte count and free it.
 *   MM_DONT_ZERO_ALLOCATION     the Ex routine zeroes the pages unless this is
 *                               set; the base routine never zeroes, so when it
 *                               is clear we map the MDL and zero it ourselves.
 *
 * The CacheType is honoured only in that the pages are mapped with it while
 * zeroing; XP's MmAllocatePagesForMdl itself allocates ordinary cached RAM,
 * which is what Dynamic Memory asks for (MmCached).  The contiguity hints
 * (MM_ALLOCATE_PREFER_CONTIGUOUS / _REQUIRE_CONTIGUOUS_CHUNKS /
 * _FAST_LARGE_PAGES) that dmvsc passes are advisory and are ignored: the
 * pages are still removed from the guest working set, which is all the
 * balloon needs, only not necessarily in large contiguous runs.
 *
 * The driver owns no device and has no dispatch routines.  dmvsc.sys is
 * import-patched to bind these two imports to mdlex.sys (see
 * migrate/Patch-Dmvsc.ps1), so the kernel loads mdlex.sys as a dependency of
 * dmvsc.sys and snaps the imports to the exports below.  A module that is
 * loaded only as an import has its exports used and its DriverEntry is not
 * called; the DriverEntry below only exists so that the image is a valid
 * driver.
 */
#include <ntddk.h>

#ifndef STATUS_INVALID_PARAMETER_1
#define STATUS_INVALID_PARAMETER_1  ((NTSTATUS)0xC00000EFL)
#endif

/* Flags for MmAllocatePagesForMdlEx (wdm.h; guard in case the DDK omits one). */
#ifndef MM_DONT_ZERO_ALLOCATION
#define MM_DONT_ZERO_ALLOCATION     0x00000001
#endif
#ifndef MM_ALLOCATE_FULLY_REQUIRED
#define MM_ALLOCATE_FULLY_REQUIRED  0x00000004
#endif

/*
 * The DDK header declares MmAllocatePagesForMdlEx as a dllimport (ntoskrnl
 * exports it from Server 2003 SP1 on).  Here we *define* it, so clang warns
 * that the definition drops the dllimport attribute; silence that one warning.
 * The base MmAllocatePagesForMdl/MmFreePagesFromMdl and the MDL mapping
 * helpers we call are ordinary Win2K/XP routines from the same header.
 */
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Winconsistent-dllimport"

PMDL NTAPI
MmAllocatePagesForMdlEx(PHYSICAL_ADDRESS LowAddress,
                        PHYSICAL_ADDRESS HighAddress,
                        PHYSICAL_ADDRESS SkipBytes,
                        SIZE_T TotalBytes,
                        MEMORY_CACHING_TYPE CacheType,
                        ULONG Flags)
{
    PMDL mdl;

    mdl = MmAllocatePagesForMdl(LowAddress, HighAddress, SkipBytes, TotalBytes);
    DbgPrint("mdlex: MmAllocatePagesForMdlEx(total=%lu KB, flags=%#lx) -> %s %lu KB\n",
             (ULONG)(TotalBytes / 1024), Flags,
             mdl ? "mdl" : "NULL",
             mdl ? (ULONG)(MmGetMdlByteCount(mdl) / 1024) : 0);
    if (mdl == NULL)
        return NULL;

    /* Honour MM_ALLOCATE_FULLY_REQUIRED: no partial MDLs. */
    if ((Flags & MM_ALLOCATE_FULLY_REQUIRED) &&
        MmGetMdlByteCount(mdl) < TotalBytes) {
        MmFreePagesFromMdl(mdl);
        ExFreePool(mdl);
        return NULL;
    }

    /* The Ex routine zeroes the pages unless MM_DONT_ZERO_ALLOCATION is set. */
    if ((Flags & MM_DONT_ZERO_ALLOCATION) == 0) {
        PVOID va = MmMapLockedPagesSpecifyCache(mdl, KernelMode, CacheType,
                                                NULL, FALSE, LowPagePriority);
        if (va != NULL) {
            RtlZeroMemory(va, MmGetMdlByteCount(mdl));
            MmUnmapLockedPages(va, mdl);
        }
        /* If the map failed we still return the (unzeroed) MDL: Dynamic
         * Memory always passes MM_DONT_ZERO_ALLOCATION, so this never runs
         * for it, and a failed zeroing is better than a failed balloon. */
    }

    return mdl;
}

/*
 * Hyper-V does not issue balloon requests unless the guest advertises the
 * hot-add capability, even for a balloon-only guest (the Linux and macOS
 * balloon drivers advertise hot-add for exactly this reason and then refuse
 * the host's hot-add requests).  dmvsc decides whether to advertise hot-add
 * from a start-time probe: it "adds" one already-present page with
 * MmAddPhysicalMemory, and advertises hot-add only if that returns success.
 *
 * On XP the real MmAddPhysicalMemory does not return success for that probe,
 * so dmvsc advertises hot-add = 0, the host rejects the capabilities (STATUS
 * 0xC000A013) and the Dynamic Memory device fails to start.  This stub makes
 * the one-page probe succeed so dmvsc advertises hot-add and the host accepts
 * the capabilities and starts ballooning.  Any larger request is a real
 * hot-add (the host issues them only when Maximum > Startup); XP cannot add
 * physical memory, so we refuse it.  The status matters: dmvsc maps exactly
 * STATUS_INVALID_PARAMETER_1 to "zero pages added, success" and then answers
 * the host's request, as a balloon-only guest should.  Any other failure
 * (STATUS_NOT_SUPPORTED for one) is passed up unchanged: dmvsc sends no answer,
 * its message loop ends and the Dynamic Memory device fails.  The start-time
 * probe only treats STATUS_NOT_SUPPORTED as "no hot-add", so it is not
 * affected by this choice.
 *
 * dmvsc.sys is import-patched to bind its MmAddPhysicalMemory import here.
 */
NTSTATUS NTAPI
MmAddPhysicalMemory(PPHYSICAL_ADDRESS StartAddress, PLARGE_INTEGER NumberOfBytes)
{
    UNREFERENCED_PARAMETER(StartAddress);
    if (NumberOfBytes != NULL && NumberOfBytes->QuadPart <= PAGE_SIZE)
        return STATUS_SUCCESS;          /* the hot-add capability probe */
    return STATUS_INVALID_PARAMETER_1;  /* a real hot-add: XP cannot do it; dmvsc answers "0 pages added" */
}

#pragma clang diagnostic pop

NTSTATUS NTAPI DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath)
{
    UNREFERENCED_PARAMETER(DriverObject);
    UNREFERENCED_PARAMETER(RegistryPath);
    DbgPrint("mdlex: loaded, MmAllocatePagesForMdlEx available\n");
    return STATUS_SUCCESS;
}
