/*
 * vmbecho.sys: test driver for a VMBus pipe that a program on the Hyper-V
 * host offers with vmbuspiper.dll!VmbusPipeServerOfferChannel (see
 * vmbecho-host.ps1).  It is the transport test for paravirtual devices
 * whose back end is a host program instead of a Hyper-V component.
 *
 * The host offers the channel to a running VM; the guest's vmbus.sys
 * reports a child device VMBUS\{interface type}, and this driver is the
 * function driver for it.  When the host closes its handle, the offer is
 * rescinded and Plug and Play removes the device.
 *
 * The offer is in named pipe mode, and the Integration Services' vmbus.sys
 * (6.3.9600, the one for Windows XP/2003) implements such channels itself:
 * when the child device starts, it opens the channel and from then on
 * serves IRP_MJ_READ and IRP_MJ_WRITE sent to the child device (the PDO)
 * from and to the pipe, with direct I/O (the data buffer is described by
 * the IRP's MDL).  vmbus.sys adds and removes the pipe's packet headers, so
 * a write here is one message to the host and a read returns the data the
 * host wrote.  (An offer with the ENUMERATE_DEVICE_INTERFACE flag would be
 * opened per file object instead, on IRP_MJ_CREATE.)
 *
 * The driver starts a system thread once the device has started.  The
 * thread sends a hello message and then answers every read with a message
 * that gives the length and the first bytes in hex, so that the host sees
 * exactly what arrived in the guest.  The thread ends when a read fails,
 * that is when the host closes the pipe, or when the device stops.
 */
#include <ntddk.h>

#define VE_TAG          'ohcE'
#define VE_READ_SIZE    4096
#define VE_WRITE_SIZE   512
#define VE_DUMP_BYTES   64

typedef struct _VE_DEVICE {
    PDEVICE_OBJECT Fdo;
    PDEVICE_OBJECT LowerDevice;
    PVOID Thread;               /* referenced thread object, NULL if not running */
    PIRP Irp[2];                /* read and write IRP, owned by the thread */
    KEVENT IoDone;
    volatile LONG Stopping;
} VE_DEVICE, *PVE_DEVICE;

enum { VE_READ, VE_WRITE };

/* A message being built in a fixed buffer; output past the end is dropped. */
typedef struct _VE_TEXT {
    char *Buf;
    ULONG Len;
    ULONG Max;
} VE_TEXT;

static void VeAddChar(VE_TEXT *t, char c)
{
    if (t->Len < t->Max)
        t->Buf[t->Len++] = c;
}

static void VeAddString(VE_TEXT *t, const char *s)
{
    while (*s)
        VeAddChar(t, *s++);
}

static void VeAddHex(VE_TEXT *t, ULONG v, int digits)
{
    static const char hex[] = "0123456789abcdef";
    while (digits-- > 0)
        VeAddChar(t, hex[(v >> (digits * 4)) & 0xF]);
}

static void VeAddDec(VE_TEXT *t, ULONG v)
{
    char tmp[10];
    int n = 0;
    do {
        tmp[n++] = (char)('0' + v % 10);
        v /= 10;
    } while (v);
    while (n > 0)
        VeAddChar(t, tmp[--n]);
}

static NTSTATUS NTAPI VeIoComplete(PDEVICE_OBJECT fdo, PIRP irp, PVOID context)
{
    KeSetEvent(context, IO_NO_INCREMENT, FALSE);
    return STATUS_MORE_PROCESSING_REQUIRED;
}

/*
 * Reads from or writes to the pipe through the PDO and waits for the
 * result.  The IRP belongs to this driver and stays allocated until the
 * device is removed, so VeStopThread can cancel it at any time.  Buffer
 * must be nonpaged.  PASSIVE_LEVEL, the device's thread only.
 */
static NTSTATUS VeIo(PVE_DEVICE dev, int which, PVOID buffer, ULONG length,
                     PULONG transferred)
{
    PIRP irp = dev->Irp[which];
    PIO_STACK_LOCATION sp;
    PMDL mdl;
    NTSTATUS status;

    *transferred = 0;
    mdl = IoAllocateMdl(buffer, length, FALSE, FALSE, NULL);
    if (!mdl)
        return STATUS_INSUFFICIENT_RESOURCES;
    MmBuildMdlForNonPagedPool(mdl);

    IoReuseIrp(irp, STATUS_SUCCESS);
    irp->MdlAddress = mdl;
    sp = IoGetNextIrpStackLocation(irp);
    if (which == VE_READ) {
        sp->MajorFunction = IRP_MJ_READ;
        sp->Parameters.Read.Length = length;
    } else {
        sp->MajorFunction = IRP_MJ_WRITE;
        sp->Parameters.Write.Length = length;
    }
    IoSetCompletionRoutine(irp, VeIoComplete, &dev->IoDone, TRUE, TRUE, TRUE);
    KeClearEvent(&dev->IoDone);

    if (dev->Stopping) {
        status = STATUS_CANCELLED;
    } else {
        IoCallDriver(dev->LowerDevice, irp);
        KeWaitForSingleObject(&dev->IoDone, Executive, KernelMode, FALSE, NULL);
        status = irp->IoStatus.Status;
        *transferred = (ULONG)irp->IoStatus.Information;
    }
    irp->MdlAddress = NULL;
    IoFreeMdl(mdl);
    return status;
}

static NTSTATUS VeWrite(PVE_DEVICE dev, PCHAR buffer, ULONG length)
{
    ULONG written;
    NTSTATUS status = VeIo(dev, VE_WRITE, buffer, length, &written);

    if (!NT_SUCCESS(status))
        DbgPrint("vmbecho: write of %lu bytes failed %08lx\n", length, status);
    return status;
}

static VOID NTAPI VeThread(PVOID context)
{
    PVE_DEVICE dev = context;
    PUCHAR in = ExAllocatePoolWithTag(NonPagedPool, VE_READ_SIZE, VE_TAG);
    PCHAR out = ExAllocatePoolWithTag(NonPagedPool, VE_WRITE_SIZE, VE_TAG);
    ULONG count = 0, n, i;
    NTSTATUS status;
    VE_TEXT t;

    if (!in || !out)
        goto done;

    t = (VE_TEXT){ out, 0, VE_WRITE_SIZE };
    VeAddString(&t, "vmbecho: hello from the guest\n");
    if (!NT_SUCCESS(VeWrite(dev, out, t.Len)))
        goto done;

    for (;;) {
        status = VeIo(dev, VE_READ, in, VE_READ_SIZE, &n);
        if (!NT_SUCCESS(status)) {
            DbgPrint("vmbecho: read ended with %08lx after %lu reads\n", status, count);
            break;
        }
        count++;
        DbgPrint("vmbecho: read %lu: %lu bytes\n", count, n);
        t = (VE_TEXT){ out, 0, VE_WRITE_SIZE };
        VeAddString(&t, "vmbecho: read ");
        VeAddDec(&t, count);
        VeAddString(&t, ", ");
        VeAddDec(&t, n);
        VeAddString(&t, " bytes:");
        for (i = 0; i < n && i < VE_DUMP_BYTES; i++) {
            VeAddChar(&t, ' ');
            VeAddHex(&t, in[i], 2);
        }
        VeAddChar(&t, '\n');
        if (!NT_SUCCESS(VeWrite(dev, out, t.Len)))
            break;
    }

done:
    if (in)
        ExFreePoolWithTag(in, VE_TAG);
    if (out)
        ExFreePoolWithTag(out, VE_TAG);
    PsTerminateSystemThread(STATUS_SUCCESS);
}

static NTSTATUS VeStartThread(PVE_DEVICE dev)
{
    HANDLE handle;
    NTSTATUS status;

    dev->Stopping = 0;
    status = PsCreateSystemThread(&handle, THREAD_ALL_ACCESS, NULL, NULL, NULL, VeThread, dev);
    if (!NT_SUCCESS(status))
        return status;
    status = ObReferenceObjectByHandle(handle, THREAD_ALL_ACCESS, NULL, KernelMode,
                                       &dev->Thread, NULL);
    ZwClose(handle);
    if (!NT_SUCCESS(status)) {
        /* Cannot wait for it: let it run until the pipe closes. */
        dev->Thread = NULL;
    }
    return STATUS_SUCCESS;
}

/*
 * Makes the thread end: cancels its I/O until it is gone.  The thread checks
 * Stopping before it sends an IRP, so a cancel that misses the IRP (sent
 * between two of them) is repeated for the next one.
 */
static VOID VeStopThread(PVE_DEVICE dev)
{
    LARGE_INTEGER step;

    if (!dev->Thread)
        return;
    InterlockedExchange(&dev->Stopping, 1);
    step.QuadPart = -100 * 10000;   /* 100 ms */
    do {
        IoCancelIrp(dev->Irp[VE_READ]);
        IoCancelIrp(dev->Irp[VE_WRITE]);
    } while (KeWaitForSingleObject(dev->Thread, Executive, KernelMode, FALSE, &step)
             == STATUS_TIMEOUT);
    ObDereferenceObject(dev->Thread);
    dev->Thread = NULL;
}

static NTSTATUS VePassDown(PVE_DEVICE dev, PIRP irp)
{
    IoSkipCurrentIrpStackLocation(irp);
    return IoCallDriver(dev->LowerDevice, irp);
}

static NTSTATUS NTAPI VeDispatchPnp(PDEVICE_OBJECT fdo, PIRP irp)
{
    PVE_DEVICE dev = fdo->DeviceExtension;
    PIO_STACK_LOCATION sp = IoGetCurrentIrpStackLocation(irp);
    NTSTATUS status;

    switch (sp->MinorFunction) {
    case IRP_MN_START_DEVICE:
        /* vmbus.sys opens the pipe when the PDO starts. */
        status = IoForwardIrpSynchronously(dev->LowerDevice, irp)
            ? irp->IoStatus.Status : STATUS_UNSUCCESSFUL;
        if (NT_SUCCESS(status))
            status = VeStartThread(dev);
        DbgPrint("vmbecho: start %08lx\n", status);
        irp->IoStatus.Status = status;
        IoCompleteRequest(irp, IO_NO_INCREMENT);
        return status;

    case IRP_MN_STOP_DEVICE:
    case IRP_MN_SURPRISE_REMOVAL:
        VeStopThread(dev);
        irp->IoStatus.Status = STATUS_SUCCESS;
        return VePassDown(dev, irp);

    case IRP_MN_QUERY_STOP_DEVICE:
    case IRP_MN_QUERY_REMOVE_DEVICE:
    case IRP_MN_CANCEL_STOP_DEVICE:
    case IRP_MN_CANCEL_REMOVE_DEVICE:
        irp->IoStatus.Status = STATUS_SUCCESS;
        return VePassDown(dev, irp);

    case IRP_MN_REMOVE_DEVICE:
        VeStopThread(dev);
        irp->IoStatus.Status = STATUS_SUCCESS;
        status = VePassDown(dev, irp);
        IoDetachDevice(dev->LowerDevice);
        IoFreeIrp(dev->Irp[VE_READ]);
        IoFreeIrp(dev->Irp[VE_WRITE]);
        IoDeleteDevice(fdo);
        return status;

    default:
        return VePassDown(dev, irp);
    }
}

static NTSTATUS NTAPI VeDispatchPower(PDEVICE_OBJECT fdo, PIRP irp)
{
    PVE_DEVICE dev = fdo->DeviceExtension;

    PoStartNextPowerIrp(irp);
    IoSkipCurrentIrpStackLocation(irp);
    return PoCallDriver(dev->LowerDevice, irp);
}

static NTSTATUS NTAPI VeDispatchPassDown(PDEVICE_OBJECT fdo, PIRP irp)
{
    return VePassDown(fdo->DeviceExtension, irp);
}

static NTSTATUS NTAPI VeAddDevice(PDRIVER_OBJECT driver, PDEVICE_OBJECT pdo)
{
    PDEVICE_OBJECT fdo;
    PVE_DEVICE dev;
    NTSTATUS status;

    status = IoCreateDevice(driver, sizeof(VE_DEVICE), NULL, FILE_DEVICE_UNKNOWN,
                            FILE_DEVICE_SECURE_OPEN, FALSE, &fdo);
    if (!NT_SUCCESS(status))
        return status;
    dev = fdo->DeviceExtension;
    RtlZeroMemory(dev, sizeof(*dev));
    dev->Fdo = fdo;
    KeInitializeEvent(&dev->IoDone, NotificationEvent, FALSE);
    dev->LowerDevice = IoAttachDeviceToDeviceStack(fdo, pdo);
    if (!dev->LowerDevice) {
        IoDeleteDevice(fdo);
        return STATUS_NO_SUCH_DEVICE;
    }
    dev->Irp[VE_READ] = IoAllocateIrp(dev->LowerDevice->StackSize, FALSE);
    dev->Irp[VE_WRITE] = IoAllocateIrp(dev->LowerDevice->StackSize, FALSE);
    if (!dev->Irp[VE_READ] || !dev->Irp[VE_WRITE]) {
        if (dev->Irp[VE_READ])
            IoFreeIrp(dev->Irp[VE_READ]);
        if (dev->Irp[VE_WRITE])
            IoFreeIrp(dev->Irp[VE_WRITE]);
        IoDetachDevice(dev->LowerDevice);
        IoDeleteDevice(fdo);
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    fdo->Flags |= DO_POWER_PAGABLE;
    fdo->Flags &= ~DO_DEVICE_INITIALIZING;
    return STATUS_SUCCESS;
}

static VOID NTAPI VeUnload(PDRIVER_OBJECT driver)
{
}

NTSTATUS NTAPI DriverEntry(PDRIVER_OBJECT driver, PUNICODE_STRING registryPath)
{
    driver->DriverExtension->AddDevice = VeAddDevice;
    driver->MajorFunction[IRP_MJ_PNP] = VeDispatchPnp;
    driver->MajorFunction[IRP_MJ_POWER] = VeDispatchPower;
    driver->MajorFunction[IRP_MJ_SYSTEM_CONTROL] = VeDispatchPassDown;
    driver->DriverUnload = VeUnload;
    return STATUS_SUCCESS;
}
