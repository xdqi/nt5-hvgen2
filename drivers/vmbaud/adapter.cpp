/*
 * adapter.cpp: DriverEntry / AddDevice / StartDevice / InstallSubdevice.
 *
 * PortCls owns PnP: PcInitializeAdapterDriver registers AddDevice, and
 * PcAddAdapterDevice takes the StartDevice callback plus MAX_MINIPORTS.
 *
 * Derived in part from Scream (https://github.com/duncanthrax/scream), which
 * is based on the MSVAD sample: Copyright (c) 1997-2000 Microsoft
 * Corporation.  All rights reserved.  Distributed under the MS-PL; see
 * LICENSE in this directory.
 */
#define INITGUID

#include <ntddk.h>
#include "../common/ddk_compat.h"
#include <portcls.h>
#include <punknown.h>
#include <stdunk.h>
#include "vmbaud.h"
#include "common.h"
#include "minwave.h"
#include "mintopo.h"

#define MAX_MINIPORTS 2

static DRIVER_ADD_DEVICE AddDevice;
static NTSTATUS NTAPI StartDevice(IN PDEVICE_OBJECT DeviceObject, IN PIRP Irp, IN PRESOURCELIST ResourceList);

#pragma code_seg("INIT")
extern "C" NTSTATUS NTAPI DriverEntry(
    IN PDRIVER_OBJECT DriverObject,
    IN PUNICODE_STRING RegistryPathName)
{
    DbgPrint("vmbaud: DriverEntry\n");
    return PcInitializeAdapterDriver(DriverObject, RegistryPathName, AddDevice);
}
#pragma code_seg()

#pragma code_seg("PAGE")
static NTSTATUS NTAPI AddDevice(
    IN PDRIVER_OBJECT DriverObject,
    IN PDEVICE_OBJECT PhysicalDeviceObject)
{
    PAGED_CODE();
    DbgPrint("vmbaud: AddDevice\n");

    /* DO_DEVICE_INITIALIZING is cleared inside PcAddAdapterDevice. */
    return PcAddAdapterDevice(
        DriverObject, PhysicalDeviceObject, PCPFNSTARTDEVICE(StartDevice),
        MAX_MINIPORTS, 0);
}

static NTSTATUS InstallSubdevice(
    IN PDEVICE_OBJECT DeviceObject,
    IN PIRP Irp,
    IN PWSTR Name,
    IN REFGUID PortClassId,
    IN REFGUID MiniportClassId,
    IN PFNCREATEINSTANCE MiniportCreate,
    IN PUNKNOWN UnknownAdapter,
    IN REFGUID PortInterfaceId,
    OUT PUNKNOWN *OutPortUnknown)
{
    PAGED_CODE();

    ASSERT(DeviceObject);
    ASSERT(Irp);
    ASSERT(Name);

    PPORT port = NULL;
    PUNKNOWN miniport = NULL;

    NTSTATUS status = PcNewPort(&port, PortClassId);
    if (NT_SUCCESS(status))
        status = MiniportCreate(&miniport, MiniportClassId, NULL, NonPagedPool);

    if (NT_SUCCESS(status)) {
        status = port->Init(DeviceObject, Irp, miniport, UnknownAdapter, NULL);
        if (NT_SUCCESS(status))
            status = PcRegisterSubdevice(DeviceObject, Name, port);
        miniport->Release();
    }

    if (NT_SUCCESS(status) && OutPortUnknown)
        status = port->QueryInterface(IID_IUnknown, (PVOID *)OutPortUnknown);

    if (port)
        port->Release();
    return status;
}

static NTSTATUS NTAPI StartDevice(
    IN PDEVICE_OBJECT DeviceObject,
    IN PIRP Irp,
    IN PRESOURCELIST ResourceList)
{
    UNREFERENCED_PARAMETER(ResourceList);
    PAGED_CODE();

    ASSERT(DeviceObject);
    ASSERT(Irp);

    PUNKNOWN unknownTopology = NULL;
    PUNKNOWN unknownWave = NULL;
    PADAPTERCOMMON adapterCommon = NULL;
    PUNKNOWN unknownCommon = NULL;

    NTSTATUS status = NewAdapterCommon(
        &unknownCommon, IID_IAdapterCommon, NULL, NonPagedPool);
    if (NT_SUCCESS(status)) {
        status = unknownCommon->QueryInterface(IID_IAdapterCommon, (PVOID *)&adapterCommon);
        if (NT_SUCCESS(status)) {
            status = adapterCommon->Init(DeviceObject);
            if (NT_SUCCESS(status))
                status = PcRegisterAdapterPowerManagement(
                    PUNKNOWN(adapterCommon), DeviceObject);
        }
    }

    if (NT_SUCCESS(status)) {
        status = InstallSubdevice(
            DeviceObject, Irp, (PWSTR)L"Topology",
            CLSID_PortTopology, CLSID_PortTopology,
            CreateMiniportTopology, adapterCommon,
            IID_IPortTopology, &unknownTopology);
    }

    if (NT_SUCCESS(status)) {
        status = InstallSubdevice(
            DeviceObject, Irp, (PWSTR)L"Wave",
            CLSID_PortWaveCyclic, CLSID_PortWaveCyclic,
            CreateMiniportWaveCyclic, adapterCommon,
            IID_IPortWaveCyclic, &unknownWave);
    }

    if (unknownWave && unknownTopology) {
        /* Wave bridge pin <-> topology wave-in pin. */
        status = PcRegisterPhysicalConnection(
            DeviceObject,
            unknownWave, KSPIN_WAVE_RENDER_SOURCE,
            unknownTopology, KSPIN_TOPO_WAVEOUT_SOURCE);
    }

    if (adapterCommon)
        adapterCommon->Release();
    if (unknownCommon)
        unknownCommon->Release();
    if (unknownTopology)
        unknownTopology->Release();
    if (unknownWave)
        unknownWave->Release();
    DbgPrint("vmbaud: StartDevice %08lx\n", status);
    return status;
}
#pragma code_seg()
