/*
 * bootwait.sys: makes Windows XP wait for a boot disk that is enumerated
 * asynchronously, here the Hyper-V Generation 2 SCSI disk on VMBus.
 *
 * XP checks for the boot partition (IopMarkBootPartition, bug check 0x7B if
 * it is missing) right after the boot drivers have run.  The boot thread
 * runs that phase at priority 31 and does not block for long.  vmbus.sys
 * reports its child devices (the SCSI controller among them) from a work
 * item that first binds itself to processor 0, where the boot thread runs.
 * So the report waits until the boot thread blocks, and on a Gen2 VM that
 * does not happen before the boot partition check: the SCSI controller has
 * no device node yet, storvsc has no device, and there is no boot disk.
 * (Observed in the kernel debugger; on Gen1 nobody notices because XP boots
 * from the emulated IDE disk.)
 *
 * This driver is a boot driver that does nothing in DriverEntry except
 * register a boot driver reinitialization routine.  The I/O manager calls
 * that routine after all boot drivers are initialized, once Plug and Play
 * processes device changes on worker threads, and before it creates the ARC
 * names and looks for the boot partition.  The routine sleeps in short
 * steps until the boot partition exists or a timeout passes.  While it
 * sleeps, vmbus.sys reports its children and Plug and Play starts the
 * storage stack.  If the boot partition is already there (any machine whose
 * boot disk is enumerated synchronously), it returns at once.
 *
 * The boot partition is the one the loader booted from: the ARC path in
 * Control\SystemBootDevice, e.g. multi(0)disk(0)rdisk(0)partition(1).  Like
 * the kernel's ARC name code, it identifies the disk by its MBR signature,
 * taken from the BIOS disk's Identifier (written by NTDETECT under
 * HARDWARE\DESCRIPTION) or from a signature() ARC path, and falls back to
 * \Device\Harddisk<rdisk> if neither is available.
 *
 * Optionally, DriverEntry also repairs the Hyper-V SCSI controller's device
 * node (RepairStorvsc).  On an XP moved over from a Gen1 VM, the first Gen2
 * boot binds the new controller to storvsc through the CriticalDeviceDatabase,
 * and then user-mode Plug and Play finishes the installation with the best
 * driver it finds: the Integration Services INF, whose Windows XP section is
 * a NULL driver.  That deletes the device's Service value and the next boot
 * stops with 0x7B.  The repair writes Service=storvsc back into every
 * VMBUS\{ba6163d9-...} device node that has none, and leaves the rest of
 * the installation alone.  It runs before vmbus.sys reports its children
 * (see above), so Plug and Play reads the repaired value.
 *
 * Registry (Services\bootwait\Parameters):
 *   TimeoutSeconds  REG_DWORD  how long to wait at most (default 30, max 600)
 *   RepairStorvsc   REG_DWORD  nonzero: repair the SCSI controller's Service (default 0)
 */
#include <ntddk.h>
#include <ntdddisk.h>

#define BW_POLL_MS              100
#define BW_DEFAULT_TIMEOUT_S    30
#define BW_MAX_TIMEOUT_S        600
#define BW_MAX_ADAPTERS         16      /* MultifunctionAdapter\0..15 */
#define BW_LAYOUT_SIZE          4096    /* room for 4096/32 partition entries */
#define BW_TAG                  'tWwB'

/* First hardware ID of the Hyper-V SCSI controller (VMBus device class). */
#define BW_STORVSC_HWID         L"VMBUS\\{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f}"

typedef struct _BW_BOOT_DISK {
    ULONG   Rdisk;          /* rdisk(n) of the ARC boot path */
    ULONG   Partition;      /* partition(n) */
    ULONG   Signature;      /* MBR signature of the boot disk */
    BOOLEAN HaveSignature;
} BW_BOOT_DISK;

static ULONG BwTimeoutSeconds = BW_DEFAULT_TIMEOUT_S;
static ULONG BwRepairStorvsc;

/* ---- strings ---------------------------------------------------------- */

static WCHAR BwLower(WCHAR c)
{
    return (c >= L'A' && c <= L'Z') ? (WCHAR)(c - L'A' + L'a') : c;
}

/* Finds Key (lower case ASCII) in S[0..Len) ignoring case; returns the index
 * just after it, or -1. */
static LONG BwFind(const WCHAR *S, ULONG Len, const char *Key)
{
    ULONG klen = 0, i, j;

    while (Key[klen])
        klen++;
    for (i = 0; i + klen <= Len; i++) {
        for (j = 0; j < klen && BwLower(S[i + j]) == (WCHAR)Key[j]; j++)
            ;
        if (j == klen)
            return (LONG)(i + klen);
    }
    return -1;
}

/* Parses a number at S[Pos..Len) in base 10 or 16; FALSE if there is no digit. */
static BOOLEAN BwParse(const WCHAR *S, ULONG Len, LONG Pos, ULONG Base, ULONG *Value)
{
    ULONG v = 0, i, d;
    BOOLEAN any = FALSE;

    if (Pos < 0)
        return FALSE;
    for (i = (ULONG)Pos; i < Len; i++) {
        WCHAR c = BwLower(S[i]);
        if (c >= L'0' && c <= L'9')
            d = c - L'0';
        else if (Base == 16 && c >= L'a' && c <= L'f')
            d = c - L'a' + 10;
        else
            break;
        v = v * Base + d;
        any = TRUE;
    }
    *Value = v;
    return any;
}

static VOID BwAppendChar(UNICODE_STRING *Str, WCHAR C)
{
    if (Str->Length + sizeof(WCHAR) <= Str->MaximumLength) {
        Str->Buffer[Str->Length / sizeof(WCHAR)] = C;
        Str->Length += sizeof(WCHAR);
    }
}

/* Appends Text and then, if Number >= 0, Number in decimal. */
static VOID BwAppend(UNICODE_STRING *Str, const WCHAR *Text, LONG Number)
{
    WCHAR digits[10];
    ULONG n = 0;

    while (*Text)
        BwAppendChar(Str, *Text++);
    if (Number < 0)
        return;
    do {
        digits[n++] = (WCHAR)(L'0' + Number % 10);
        Number /= 10;
    } while (Number);
    while (n)
        BwAppendChar(Str, digits[--n]);
}

/* ---- registry --------------------------------------------------------- */

static NTSTATUS BwOpenKey(UNICODE_STRING *Path, HANDLE *Key)
{
    OBJECT_ATTRIBUTES oa;

    InitializeObjectAttributes(&oa, Path, OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE, NULL, NULL);
    return ZwOpenKey(Key, KEY_READ, &oa);
}

/* Reads a REG_SZ or REG_DWORD value into Info (Size bytes). */
static NTSTATUS BwQueryValue(UNICODE_STRING *KeyPath, const WCHAR *Name, ULONG Type,
                             KEY_VALUE_PARTIAL_INFORMATION *Info, ULONG Size)
{
    UNICODE_STRING name;
    HANDLE key;
    ULONG len;
    NTSTATUS status;

    status = BwOpenKey(KeyPath, &key);
    if (!NT_SUCCESS(status))
        return status;
    RtlInitUnicodeString(&name, Name);
    status = ZwQueryValueKey(key, &name, KeyValuePartialInformation, Info, Size, &len);
    ZwClose(key);
    if (NT_SUCCESS(status) && Info->Type != Type)
        status = STATUS_OBJECT_TYPE_MISMATCH;
    return status;
}

/* rdisk(), partition() and, if present, signature() of the ARC boot path. */
static VOID BwReadBootPath(BW_BOOT_DISK *Disk)
{
    UNICODE_STRING path = RTL_CONSTANT_STRING(L"\\Registry\\Machine\\System\\CurrentControlSet\\Control");
    union {
        KEY_VALUE_PARTIAL_INFORMATION info;
        UCHAR raw[sizeof(KEY_VALUE_PARTIAL_INFORMATION) + 256 * sizeof(WCHAR)];
    } buf;
    const WCHAR *s;
    ULONG len;

    Disk->Rdisk = 0;
    Disk->Partition = 1;
    Disk->HaveSignature = FALSE;
    RtlZeroMemory(&buf, sizeof(buf));
    if (!NT_SUCCESS(BwQueryValue(&path, L"SystemBootDevice", REG_SZ, &buf.info, sizeof(buf) - sizeof(WCHAR)))) {
        DbgPrint("bootwait: no SystemBootDevice, assuming rdisk(0)partition(1)\n");
        return;
    }
    s = (const WCHAR *)buf.info.Data;
    len = buf.info.DataLength / sizeof(WCHAR);
    DbgPrint("bootwait: boot device %ws\n", s);
    BwParse(s, len, BwFind(s, len, "rdisk("), 10, &Disk->Rdisk);
    BwParse(s, len, BwFind(s, len, "partition("), 10, &Disk->Partition);
    if (BwParse(s, len, BwFind(s, len, "signature("), 16, &Disk->Signature))
        Disk->HaveSignature = TRUE;
}

/* The MBR signature of BIOS disk Rdisk, from the Identifier NTDETECT writes
 * ("checksum-signature-A") under ...\MultifunctionAdapter\n\DiskController\0\DiskPeripheral\Rdisk. */
static BOOLEAN BwBiosDiskSignature(ULONG Rdisk, ULONG *Signature)
{
    union {
        KEY_VALUE_PARTIAL_INFORMATION info;
        UCHAR raw[sizeof(KEY_VALUE_PARTIAL_INFORMATION) + 64 * sizeof(WCHAR)];
    } buf;
    WCHAR pathBuf[160];
    UNICODE_STRING path;
    const WCHAR *s;
    ULONG adapter, len;

    for (adapter = 0; adapter < BW_MAX_ADAPTERS; adapter++) {
        path.Buffer = pathBuf;
        path.Length = 0;
        path.MaximumLength = sizeof(pathBuf);
        BwAppend(&path, L"\\Registry\\Machine\\Hardware\\Description\\System\\MultifunctionAdapter\\", (LONG)adapter);
        BwAppend(&path, L"\\DiskController\\0\\DiskPeripheral\\", (LONG)Rdisk);
        RtlZeroMemory(&buf, sizeof(buf));
        if (!NT_SUCCESS(BwQueryValue(&path, L"Identifier", REG_SZ, &buf.info, sizeof(buf) - sizeof(WCHAR))))
            continue;
        s = (const WCHAR *)buf.info.Data;
        len = buf.info.DataLength / sizeof(WCHAR);
        DbgPrint("bootwait: BIOS disk %lu identifier %ws\n", Rdisk, s);
        return BwParse(s, len, BwFind(s, len, "-"), 16, Signature);
    }
    return FALSE;
}

static VOID BwReadParameters(UNICODE_STRING *RegistryPath)
{
    union {
        KEY_VALUE_PARTIAL_INFORMATION info;
        UCHAR raw[sizeof(KEY_VALUE_PARTIAL_INFORMATION) + sizeof(ULONG)];
    } buf;
    WCHAR pathBuf[160];
    UNICODE_STRING path;
    ULONG v;

    path.Buffer = pathBuf;
    path.Length = 0;
    path.MaximumLength = sizeof(pathBuf);
    if (RegistryPath->Length + sizeof(L"\\Parameters") > sizeof(pathBuf))
        return;
    RtlCopyUnicodeString(&path, RegistryPath);
    BwAppend(&path, L"\\Parameters", -1);
    if (NT_SUCCESS(BwQueryValue(&path, L"TimeoutSeconds", REG_DWORD, &buf.info, sizeof(buf)))) {
        RtlCopyMemory(&v, buf.info.Data, sizeof(v));
        BwTimeoutSeconds = v > BW_MAX_TIMEOUT_S ? BW_MAX_TIMEOUT_S : v;
    }
    if (NT_SUCCESS(BwQueryValue(&path, L"RepairStorvsc", REG_DWORD, &buf.info, sizeof(buf))))
        RtlCopyMemory(&BwRepairStorvsc, buf.info.Data, sizeof(BwRepairStorvsc));
}

/* ---- SCSI controller repair ------------------------------------------- */

static NTSTATUS BwOpenSubKey(HANDLE Parent, const KEY_BASIC_INFORMATION *Sub, ACCESS_MASK Access, HANDLE *Key)
{
    UNICODE_STRING name;
    OBJECT_ATTRIBUTES oa;

    name.Buffer = (PWCH)Sub->Name;
    name.Length = name.MaximumLength = (USHORT)Sub->NameLength;
    InitializeObjectAttributes(&oa, &name, OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE, Parent, NULL);
    return ZwOpenKey(Key, Access, &oa);
}

/* Is Id, ignoring case, the first string of the REG_MULTI_SZ S[0..Len)? */
static BOOLEAN BwFirstStringIs(const WCHAR *S, ULONG Len, const WCHAR *Id)
{
    ULONG i;

    for (i = 0; Id[i]; i++)
        if (i >= Len || BwLower(S[i]) != BwLower(Id[i]))
            return FALSE;
    return i == Len || S[i] == 0;
}

/* Writes Service=storvsc into the device node Key (Enum\VMBUS\Dev\Inst) if it
 * is a Hyper-V SCSI controller without a service. */
static VOID BwRepairInstance(HANDLE Key, const KEY_BASIC_INFORMATION *Dev, const KEY_BASIC_INFORMATION *Inst)
{
    static const WCHAR storvsc[] = L"storvsc";
    union {
        KEY_VALUE_PARTIAL_INFORMATION info;
        UCHAR raw[sizeof(KEY_VALUE_PARTIAL_INFORMATION) + 512 * sizeof(WCHAR)];
    } buf;
    UNICODE_STRING name, dev, inst;
    ULONG len;
    NTSTATUS status;

    RtlInitUnicodeString(&name, L"HardwareID");
    status = ZwQueryValueKey(Key, &name, KeyValuePartialInformation, &buf, sizeof(buf), &len);
    if (!NT_SUCCESS(status) || buf.info.Type != REG_MULTI_SZ ||
        !BwFirstStringIs((const WCHAR *)buf.info.Data, buf.info.DataLength / sizeof(WCHAR), BW_STORVSC_HWID))
        return;
    RtlInitUnicodeString(&name, L"Service");
    status = ZwQueryValueKey(Key, &name, KeyValuePartialInformation, &buf, sizeof(buf), &len);
    if (NT_SUCCESS(status) && buf.info.Type == REG_SZ && buf.info.DataLength >= sizeof(WCHAR) &&
        *(const WCHAR *)buf.info.Data)
        return;

    dev.Buffer = (PWCH)Dev->Name;
    dev.Length = dev.MaximumLength = (USHORT)Dev->NameLength;
    inst.Buffer = (PWCH)Inst->Name;
    inst.Length = inst.MaximumLength = (USHORT)Inst->NameLength;
    status = ZwSetValueKey(Key, &name, 0, REG_SZ, (PVOID)storvsc, sizeof(storvsc));
    DbgPrint("bootwait: Service=storvsc restored on VMBUS\\%wZ\\%wZ (status %08lx)\n", &dev, &inst, status);
}

/* Walks Enum\VMBUS\<device>\<instance> and repairs the SCSI controllers. */
static VOID BwRepairStorvscNodes(VOID)
{
    UNICODE_STRING path = RTL_CONSTANT_STRING(L"\\Registry\\Machine\\System\\CurrentControlSet\\Enum\\VMBUS");
    union {
        KEY_BASIC_INFORMATION info;
        UCHAR raw[sizeof(KEY_BASIC_INFORMATION) + 256 * sizeof(WCHAR)];
    } dev, inst;
    HANDLE bus, devKey, instKey;
    ULONG i, j, len;
    NTSTATUS status;

    status = BwOpenKey(&path, &bus);
    if (!NT_SUCCESS(status)) {
        DbgPrint("bootwait: no Enum\\VMBUS (status %08lx), nothing to repair\n", status);
        return;
    }
    for (i = 0; NT_SUCCESS(ZwEnumerateKey(bus, i, KeyBasicInformation, &dev, sizeof(dev), &len)); i++) {
        if (!NT_SUCCESS(BwOpenSubKey(bus, &dev.info, KEY_READ, &devKey)))
            continue;
        for (j = 0; NT_SUCCESS(ZwEnumerateKey(devKey, j, KeyBasicInformation, &inst, sizeof(inst), &len)); j++) {
            if (!NT_SUCCESS(BwOpenSubKey(devKey, &inst.info, KEY_READ | KEY_SET_VALUE, &instKey)))
                continue;
            BwRepairInstance(instKey, &dev.info, &inst.info);
            ZwClose(instKey);
        }
        ZwClose(devKey);
    }
    ZwClose(bus);
}

/* ---- disks ------------------------------------------------------------ */

/* Opens \Device\Harddisk<Disk>\Partition<Partition> without mounting it
 * (FILE_READ_ATTRIBUTES only makes this a direct device open). */
static NTSTATUS BwOpenPartition(ULONG Disk, ULONG Partition, PFILE_OBJECT *File, PDEVICE_OBJECT *Device)
{
    WCHAR nameBuf[64];
    UNICODE_STRING name;

    name.Buffer = nameBuf;
    name.Length = 0;
    name.MaximumLength = sizeof(nameBuf);
    BwAppend(&name, L"\\Device\\Harddisk", (LONG)Disk);
    BwAppend(&name, L"\\Partition", (LONG)Partition);
    return IoGetDeviceObjectPointer(&name, FILE_READ_ATTRIBUTES, File, Device);
}

/* The MBR signature of \Device\Harddisk<Disk>, as IOCTL_DISK_GET_DRIVE_LAYOUT reports it. */
static BOOLEAN BwDiskSignature(ULONG Disk, ULONG *Signature)
{
    PFILE_OBJECT file;
    PDEVICE_OBJECT device;
    DRIVE_LAYOUT_INFORMATION *layout;
    IO_STATUS_BLOCK iosb;
    KEVENT event;
    PIRP irp;
    NTSTATUS status;
    BOOLEAN ok = FALSE;

    if (!NT_SUCCESS(BwOpenPartition(Disk, 0, &file, &device)))
        return FALSE;
    layout = ExAllocatePoolWithTag(NonPagedPool, BW_LAYOUT_SIZE, BW_TAG);
    if (layout) {
        KeInitializeEvent(&event, NotificationEvent, FALSE);
        irp = IoBuildDeviceIoControlRequest(IOCTL_DISK_GET_DRIVE_LAYOUT, device, NULL, 0,
                                            layout, BW_LAYOUT_SIZE, FALSE, &event, &iosb);
        if (irp) {
            status = IoCallDriver(device, irp);
            if (status == STATUS_PENDING) {
                KeWaitForSingleObject(&event, Executive, KernelMode, FALSE, NULL);
                status = iosb.Status;
            }
            if (NT_SUCCESS(status)) {
                *Signature = layout->Signature;
                ok = TRUE;
            }
        }
        ExFreePoolWithTag(layout, BW_TAG);
    }
    ObDereferenceObject(file);
    return ok;
}

static BOOLEAN BwPartitionExists(ULONG Disk, ULONG Partition)
{
    PFILE_OBJECT file;
    PDEVICE_OBJECT device;

    if (!NT_SUCCESS(BwOpenPartition(Disk, Partition, &file, &device)))
        return FALSE;
    ObDereferenceObject(file);
    return TRUE;
}

/* Is the boot partition there?  Sets *Found to its disk number if so. */
static BOOLEAN BwBootPartitionReady(const BW_BOOT_DISK *Boot, ULONG DiskCount, ULONG *Found)
{
    ULONG disk, sig;

    for (disk = 0; disk < DiskCount; disk++) {
        if (Boot->HaveSignature) {
            if (!BwDiskSignature(disk, &sig) || sig != Boot->Signature)
                continue;
        } else if (disk != Boot->Rdisk) {
            continue;
        }
        if (BwPartitionExists(disk, Boot->Partition)) {
            *Found = disk;
            return TRUE;
        }
    }
    return FALSE;
}

/* ---- entry points ----------------------------------------------------- */

static VOID NTAPI BwReinitialize(PDRIVER_OBJECT DriverObject, PVOID Context, ULONG Count)
{
    BW_BOOT_DISK boot;
    LARGE_INTEGER interval;
    ULONGLONG start;
    ULONG elapsedMs, disks, lastDisks = (ULONG)-1, found;

    BwReadBootPath(&boot);
    if (!boot.HaveSignature)
        boot.HaveSignature = BwBiosDiskSignature(boot.Rdisk, &boot.Signature);
    if (boot.HaveSignature)
        DbgPrint("bootwait: waiting up to %lu s for partition %lu of the disk with signature %08lx\n",
                 BwTimeoutSeconds, boot.Partition, boot.Signature);
    else
        DbgPrint("bootwait: waiting up to %lu s for \\Device\\Harddisk%lu\\Partition%lu\n",
                 BwTimeoutSeconds, boot.Rdisk, boot.Partition);

    interval.QuadPart = -(LONGLONG)BW_POLL_MS * 10000;
    start = KeQueryInterruptTime();
    for (;;) {
        /* 100 ns units -> ms without a 64-bit division (no compiler runtime here). */
        elapsedMs = (ULONG)((KeQueryInterruptTime() - start) >> 4) / 625;
        disks = IoGetConfigurationInformation()->DiskCount;
        if (disks != lastDisks) {
            DbgPrint("bootwait: %lu disk(s) at %lu ms\n", disks, elapsedMs);
            lastDisks = disks;
        }
        if (BwBootPartitionReady(&boot, disks, &found)) {
            DbgPrint("bootwait: boot partition is \\Device\\Harddisk%lu\\Partition%lu (after %lu ms)\n",
                     found, boot.Partition, elapsedMs);
            return;
        }
        if (elapsedMs >= BwTimeoutSeconds * 1000) {
            DbgPrint("bootwait: no boot partition after %lu s, giving up\n", BwTimeoutSeconds);
            return;
        }
        KeDelayExecutionThread(KernelMode, FALSE, &interval);
    }
}

NTSTATUS NTAPI DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath)
{
    BwReadParameters(RegistryPath);
    DbgPrint("bootwait: loaded, timeout %lu s%s\n", BwTimeoutSeconds,
             BwRepairStorvsc ? ", repairing the SCSI controller's device node" : "");
    if (BwRepairStorvsc)
        BwRepairStorvscNodes();
    IoRegisterBootDriverReinitialization(DriverObject, BwReinitialize, NULL);
    return STATUS_SUCCESS;
}
