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
 * The same repair is available for other VMBus devices through a table in
 * Parameters\Devices, for Integration Services whose Windows XP section of
 * the INF is a NULL driver but that work with a driver or service that is
 * installed by other means (Dynamic Memory, Backup).  Each subkey names one
 * device class by its first hardware ID and may give the service to restore
 * and a FriendlyName.  Device Manager shows the FriendlyName instead of the
 * INF's "... (not supported)".  The name is only written on a later boot than
 * the one that creates the device node, since the node does not exist before.
 * A table entry with a service also keeps that service in the device class's
 * Critical Device Database entry, which the NULL driver installation empties
 * (see BwRepairCddb).
 *
 * Registry (Services\bootwait\Parameters):
 *   TimeoutSeconds  REG_DWORD  how long to wait at most (default 30, max 600)
 *   RepairStorvsc   REG_DWORD  nonzero: repair the SCSI controller's Service and
 *                              name it (default 0)
 *   Devices\<any>   REG_SZ     HardwareID    first hardware ID, VMBUS\{...}
 *                   REG_SZ     Service       service to write if the node has none (optional)
 *                   REG_SZ     FriendlyName  name to write (optional)
 *                   REG_SZ     ClassGUID     class to write if the node has none, with
 *                   REG_SZ     Class         its name (optional, both or neither)
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
#define BW_STORVSC_NAME         L"Microsoft Hyper-V SCSI Controller"

#define BW_MAX_FIXES            16
#define BW_FIX_ID_CHARS         80
#define BW_FIX_SERVICE_CHARS    40
#define BW_FIX_NAME_CHARS       100
#define BW_FIX_CLASS_CHARS      40

typedef struct _BW_BOOT_DISK {
    ULONG   Rdisk;          /* rdisk(n) of the ARC boot path */
    ULONG   Partition;      /* partition(n) */
    ULONG   Signature;      /* MBR signature of the boot disk */
    BOOLEAN HaveSignature;
} BW_BOOT_DISK;

/* One device class to repair: Service and Name are empty strings if unused. */
typedef struct _BW_FIX {
    WCHAR   HardwareId[BW_FIX_ID_CHARS];
    WCHAR   Service[BW_FIX_SERVICE_CHARS];
    WCHAR   Name[BW_FIX_NAME_CHARS];
    WCHAR   ClassGuid[BW_FIX_CLASS_CHARS];  /* "{4D36E97D-...}", empty: leave the class alone */
    WCHAR   Class[BW_FIX_CLASS_CHARS];      /* its name, "System" */
} BW_FIX;

static ULONG BwTimeoutSeconds = BW_DEFAULT_TIMEOUT_S;
static ULONG BwRepairStorvsc;
static BW_FIX BwFixes[BW_MAX_FIXES];
static ULONG BwFixCount;

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

/* ---- device node repair ----------------------------------------------- */

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

static ULONG BwStrLen(const WCHAR *S)
{
    ULONG n = 0;

    while (S[n])
        n++;
    return n;
}

static BOOLEAN BwEqualNoCase(const WCHAR *A, const WCHAR *B)
{
    for (; *A && BwLower(*A) == BwLower(*B); A++, B++)
        ;
    return *A == *B;
}

/* Copies S into Dst (Chars wide characters), always terminated. */
static VOID BwCopyString(WCHAR *Dst, ULONG Chars, const WCHAR *S)
{
    ULONG n = BwStrLen(S);

    if (n >= Chars)
        n = Chars - 1;
    RtlCopyMemory(Dst, S, n * sizeof(WCHAR));
    Dst[n] = 0;
}

/* Reads the REG_SZ value Name of Key into Dst (Chars wide characters, always
 * terminated); FALSE, with Dst empty, if there is none or it is empty. */
static BOOLEAN BwReadString(HANDLE Key, const WCHAR *Name, WCHAR *Dst, ULONG Chars)
{
    union {
        KEY_VALUE_PARTIAL_INFORMATION info;
        UCHAR raw[sizeof(KEY_VALUE_PARTIAL_INFORMATION) + 256 * sizeof(WCHAR)];
    } buf;
    UNICODE_STRING name;
    ULONG len, n;

    Dst[0] = 0;
    RtlInitUnicodeString(&name, Name);
    if (!NT_SUCCESS(ZwQueryValueKey(Key, &name, KeyValuePartialInformation, &buf, sizeof(buf), &len)) ||
        buf.info.Type != REG_SZ)
        return FALSE;
    n = buf.info.DataLength / sizeof(WCHAR);
    if (n >= Chars)
        n = Chars - 1;
    RtlCopyMemory(Dst, buf.info.Data, n * sizeof(WCHAR));
    Dst[n] = 0;
    return Dst[0] != 0;
}

/* Adds a device class to repair; a later entry with the same hardware ID replaces the earlier one. */
static VOID BwAddFix(const WCHAR *HardwareId, const WCHAR *Service, const WCHAR *Name,
                     const WCHAR *ClassGuid, const WCHAR *Class)
{
    ULONG i;
    BW_FIX *fix;

    for (i = 0; i < BwFixCount; i++)
        if (BwEqualNoCase(BwFixes[i].HardwareId, HardwareId))
            break;
    if (i == BwFixCount) {
        if (i == BW_MAX_FIXES)
            return;
        BwFixCount++;
    }
    fix = &BwFixes[i];
    BwCopyString(fix->HardwareId, BW_FIX_ID_CHARS, HardwareId);
    BwCopyString(fix->Service, BW_FIX_SERVICE_CHARS, Service);
    BwCopyString(fix->Name, BW_FIX_NAME_CHARS, Name);
    BwCopyString(fix->ClassGuid, BW_FIX_CLASS_CHARS, ClassGuid);
    BwCopyString(fix->Class, BW_FIX_CLASS_CHARS, Class);
}

/* Reads the device table, Services\bootwait\Parameters\Devices\<any>. */
static VOID BwReadFixes(UNICODE_STRING *RegistryPath)
{
    WCHAR pathBuf[180];
    UNICODE_STRING path;
    union {
        KEY_BASIC_INFORMATION info;
        UCHAR raw[sizeof(KEY_BASIC_INFORMATION) + 256 * sizeof(WCHAR)];
    } sub;
    WCHAR id[BW_FIX_ID_CHARS], service[BW_FIX_SERVICE_CHARS], name[BW_FIX_NAME_CHARS];
    WCHAR classGuid[BW_FIX_CLASS_CHARS], class[BW_FIX_CLASS_CHARS];
    HANDLE devices, key;
    ULONG i, len;

    path.Buffer = pathBuf;
    path.Length = 0;
    path.MaximumLength = sizeof(pathBuf);
    if (RegistryPath->Length + sizeof(L"\\Parameters\\Devices") > sizeof(pathBuf))
        return;
    RtlCopyUnicodeString(&path, RegistryPath);
    BwAppend(&path, L"\\Parameters\\Devices", -1);
    if (!NT_SUCCESS(BwOpenKey(&path, &devices)))
        return;
    for (i = 0; NT_SUCCESS(ZwEnumerateKey(devices, i, KeyBasicInformation, &sub, sizeof(sub), &len)); i++) {
        if (!NT_SUCCESS(BwOpenSubKey(devices, &sub.info, KEY_READ, &key)))
            continue;
        if (BwReadString(key, L"HardwareID", id, BW_FIX_ID_CHARS)) {
            BwReadString(key, L"Service", service, BW_FIX_SERVICE_CHARS);
            BwReadString(key, L"FriendlyName", name, BW_FIX_NAME_CHARS);
            BwReadString(key, L"ClassGUID", classGuid, BW_FIX_CLASS_CHARS);
            BwReadString(key, L"Class", class, BW_FIX_CLASS_CHARS);
            BwAddFix(id, service, name, classGuid, class);
        }
        ZwClose(key);
    }
    ZwClose(devices);
}

/* Applies the table entry for the device node Key (Enum\VMBUS\Dev\Inst), if there is one:
 * writes the entry's Service if the node has none, and its FriendlyName. */
static VOID BwRepairInstance(HANDLE Key, const KEY_BASIC_INFORMATION *Dev, const KEY_BASIC_INFORMATION *Inst)
{
    union {
        KEY_VALUE_PARTIAL_INFORMATION info;
        UCHAR raw[sizeof(KEY_VALUE_PARTIAL_INFORMATION) + 512 * sizeof(WCHAR)];
    } buf;
    UNICODE_STRING name, dev, inst;
    const BW_FIX *fix = NULL;
    ULONG len, size, i;
    NTSTATUS status;

    RtlInitUnicodeString(&name, L"HardwareID");
    status = ZwQueryValueKey(Key, &name, KeyValuePartialInformation, &buf, sizeof(buf), &len);
    if (!NT_SUCCESS(status) || buf.info.Type != REG_MULTI_SZ)
        return;
    for (i = 0; i < BwFixCount && !fix; i++)
        if (BwFirstStringIs((const WCHAR *)buf.info.Data, buf.info.DataLength / sizeof(WCHAR), BwFixes[i].HardwareId))
            fix = &BwFixes[i];
    if (!fix)
        return;

    dev.Buffer = (PWCH)Dev->Name;
    dev.Length = dev.MaximumLength = (USHORT)Dev->NameLength;
    inst.Buffer = (PWCH)Inst->Name;
    inst.Length = inst.MaximumLength = (USHORT)Inst->NameLength;

    if (fix->Service[0]) {
        RtlInitUnicodeString(&name, L"Service");
        status = ZwQueryValueKey(Key, &name, KeyValuePartialInformation, &buf, sizeof(buf), &len);
        if (!NT_SUCCESS(status) || buf.info.Type != REG_SZ || buf.info.DataLength < sizeof(WCHAR) ||
            !*(const WCHAR *)buf.info.Data) {
            status = ZwSetValueKey(Key, &name, 0, REG_SZ, (PVOID)fix->Service,
                                   (BwStrLen(fix->Service) + 1) * sizeof(WCHAR));
            DbgPrint("bootwait: Service=%ws restored on VMBUS\\%wZ\\%wZ (status %08lx)\n", fix->Service, &dev, &inst, status);
        }
    }
    if (fix->ClassGuid[0] && fix->Class[0]) {
        RtlInitUnicodeString(&name, L"ClassGUID");
        status = ZwQueryValueKey(Key, &name, KeyValuePartialInformation, &buf, sizeof(buf), &len);
        if (!NT_SUCCESS(status) || buf.info.Type != REG_SZ || buf.info.DataLength < sizeof(WCHAR) ||
            !*(const WCHAR *)buf.info.Data) {
            status = ZwSetValueKey(Key, &name, 0, REG_SZ, (PVOID)fix->ClassGuid,
                                   (BwStrLen(fix->ClassGuid) + 1) * sizeof(WCHAR));
            RtlInitUnicodeString(&name, L"Class");
            if (NT_SUCCESS(status))
                status = ZwSetValueKey(Key, &name, 0, REG_SZ, (PVOID)fix->Class,
                                       (BwStrLen(fix->Class) + 1) * sizeof(WCHAR));
            DbgPrint("bootwait: class %ws set on VMBUS\\%wZ\\%wZ (status %08lx)\n", fix->Class, &dev, &inst, status);
        }
    }
    if (fix->Name[0]) {
        RtlInitUnicodeString(&name, L"FriendlyName");
        size = (BwStrLen(fix->Name) + 1) * sizeof(WCHAR);
        status = ZwQueryValueKey(Key, &name, KeyValuePartialInformation, &buf, sizeof(buf), &len);
        if (!NT_SUCCESS(status) || buf.info.Type != REG_SZ || buf.info.DataLength != size ||
            RtlCompareMemory(buf.info.Data, fix->Name, size) != size) {
            status = ZwSetValueKey(Key, &name, 0, REG_SZ, (PVOID)fix->Name, size);
            DbgPrint("bootwait: FriendlyName \"%ws\" set on VMBUS\\%wZ\\%wZ (status %08lx)\n", fix->Name, &dev, &inst, status);
        }
    }
}

/* Writes the entry's Service into the device class's Critical Device Database key,
 * CurrentControlSet\Control\CriticalDeviceDatabase\<hardware ID with \ replaced by #>, if that
 * has none.  Plug and Play binds a device node it has not seen before to this service.  The NULL
 * driver installation removes the value, so without this a disk that has booted once stops with
 * 0x7B in any VM whose controller is a new device node (another VM, an imported copy). */
static VOID BwRepairCddb(const BW_FIX *Fix)
{
    static const WCHAR prefix[] = L"\\Registry\\Machine\\System\\CurrentControlSet\\Control\\CriticalDeviceDatabase\\";
    WCHAR pathBuf[sizeof(prefix) / sizeof(WCHAR) + BW_FIX_ID_CHARS];
    union {
        KEY_VALUE_PARTIAL_INFORMATION info;
        UCHAR raw[sizeof(KEY_VALUE_PARTIAL_INFORMATION) + 128 * sizeof(WCHAR)];
    } buf;
    UNICODE_STRING path, name;
    OBJECT_ATTRIBUTES oa;
    HANDLE key;
    ULONG i, len, disp;
    NTSTATUS status;

    path.Buffer = pathBuf;
    path.Length = 0;
    path.MaximumLength = sizeof(pathBuf);
    BwAppend(&path, prefix, -1);
    for (i = 0; Fix->HardwareId[i]; i++)
        BwAppendChar(&path, Fix->HardwareId[i] == L'\\' ? L'#' : Fix->HardwareId[i]);
    InitializeObjectAttributes(&oa, &path, OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE, NULL, NULL);
    status = ZwCreateKey(&key, KEY_READ | KEY_SET_VALUE, &oa, 0, NULL, REG_OPTION_NON_VOLATILE, &disp);
    if (!NT_SUCCESS(status)) {
        DbgPrint("bootwait: cannot open %wZ (status %08lx)\n", &path, status);
        return;
    }
    RtlInitUnicodeString(&name, L"Service");
    status = ZwQueryValueKey(key, &name, KeyValuePartialInformation, &buf, sizeof(buf), &len);
    if (!NT_SUCCESS(status) || buf.info.Type != REG_SZ || buf.info.DataLength < sizeof(WCHAR) ||
        !*(const WCHAR *)buf.info.Data) {
        status = ZwSetValueKey(key, &name, 0, REG_SZ, (PVOID)Fix->Service, (BwStrLen(Fix->Service) + 1) * sizeof(WCHAR));
        DbgPrint("bootwait: Service=%ws restored in the Critical Device Database for %ws (status %08lx)\n",
                 Fix->Service, Fix->HardwareId, status);
    }
    ZwClose(key);
}

/* Walks Enum\VMBUS\<device>\<instance> and repairs the device nodes in the table. */
static VOID BwRepairNodes(VOID)
{
    UNICODE_STRING path = RTL_CONSTANT_STRING(L"\\Registry\\Machine\\System\\CurrentControlSet\\Enum\\VMBUS");
    union {
        KEY_BASIC_INFORMATION info;
        UCHAR raw[sizeof(KEY_BASIC_INFORMATION) + 256 * sizeof(WCHAR)];
    } dev, inst;
    HANDLE bus, devKey, instKey;
    ULONG i, j, len;
    NTSTATUS status;

    for (i = 0; i < BwFixCount; i++)
        if (BwFixes[i].Service[0])
            BwRepairCddb(&BwFixes[i]);
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
    if (BwRepairStorvsc)
        BwAddFix(BW_STORVSC_HWID, L"storvsc", BW_STORVSC_NAME, L"", L"");
    BwReadFixes(RegistryPath);
    DbgPrint("bootwait: loaded, timeout %lu s, %lu device class(es) to repair\n", BwTimeoutSeconds, BwFixCount);
    if (BwFixCount)
        BwRepairNodes();
    IoRegisterBootDriverReinitialization(DriverObject, BwReinitialize, NULL);
    return STATUS_SUCCESS;
}
