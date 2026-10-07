/*
 * minwave.cpp: WaveCyclic miniport — Init, NewStream, property handlers.
 *
 * Derived in part from Scream (https://github.com/duncanthrax/scream), which
 * is based on the MSVAD sample: Copyright (c) 1997-2000 Microsoft
 * Corporation.  All rights reserved.  Distributed under the MS-PL; see
 * LICENSE in this directory.
 */
#include <ntddk.h>
#include "../common/ddk_compat.h"
#include <portcls.h>
#include <punknown.h>
#include <stdunk.h>
#include "vmbaud.h"
#include "helpers.h"
#include "common.h"
#include "minwave.h"
#include "minstream.h"

NTSTATUS NTAPI PropertyHandler_WaveFilter(IN PPCPROPERTY_REQUEST PropertyRequest);

#include "wavtable.h"

NTSTATUS NTAPI CreateMiniportWaveCyclic(
    OUT PUNKNOWN *Unknown, IN REFCLSID, IN PUNKNOWN UnknownOuter, IN POOL_TYPE PoolType)
{
    ASSERT(Unknown);
    STD_CREATE_BODY(CMiniportWaveCyclic, Unknown, UnknownOuter, PoolType);
}

CMiniportWaveCyclic::CMiniportWaveCyclic(PUNKNOWN other) : CUnknown(other)
{
    m_RenderAllocated = FALSE;
    m_AdapterCommon = NULL;
    m_Port = NULL;
    m_FilterDescriptor = NULL;
    m_NotificationInterval = VBAUD_DEFAULT_INTERVAL_MS;
    m_SamplingFrequency = 0;
    m_ServiceGroup = NULL;
    m_MaxDmaBufferSize = VBAUD_DMA_BUFFER_SIZE;
}

CMiniportWaveCyclic::~CMiniportWaveCyclic()
{
    if (m_Port)
        m_Port->Release();
    if (m_ServiceGroup)
        m_ServiceGroup->Release();
    if (m_AdapterCommon)
        m_AdapterCommon->Release();
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclic::DataRangeIntersection(
    IN ULONG PinId,
    IN PKSDATARANGE ClientDataRange,
    IN PKSDATARANGE MyDataRange,
    IN ULONG OutputBufferLength,
    OUT PVOID ResultantFormat,
    OUT PULONG ResultantFormatLength)
{
    UNREFERENCED_PARAMETER(PinId);
    UNREFERENCED_PARAMETER(ClientDataRange);
    UNREFERENCED_PARAMETER(MyDataRange);
    UNREFERENCED_PARAMETER(OutputBufferLength);
    UNREFERENCED_PARAMETER(ResultantFormat);
    UNREFERENCED_PARAMETER(ResultantFormatLength);

    /* PCM-only pin: let PortCls do the default intersection. */
    return STATUS_NOT_IMPLEMENTED;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclic::GetDescription(
    OUT PPCFILTER_DESCRIPTOR *OutFilterDescriptor)
{
    ASSERT(OutFilterDescriptor);
    *OutFilterDescriptor = m_FilterDescriptor;
    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclic::Init(
    IN PUNKNOWN UnknownAdapter,
    IN PRESOURCELIST ResourceList,
    IN PPORTWAVECYCLIC Port)
{
    UNREFERENCED_PARAMETER(ResourceList);

    ASSERT(UnknownAdapter);
    ASSERT(Port);

    m_Port = Port;
    m_Port->AddRef();

    NTSTATUS status = UnknownAdapter->QueryInterface(IID_IAdapterCommon, (PVOID *)&m_AdapterCommon);
    if (NT_SUCCESS(status)) {
        KeInitializeMutex(&m_SampleRateSync, 1);
        status = PcNewServiceGroup(&m_ServiceGroup, NULL);
        if (NT_SUCCESS(status))
            m_AdapterCommon->SetWaveServiceGroup(m_ServiceGroup);
    }

    if (NT_SUCCESS(status)) {
        m_FilterDescriptor = &MiniportFilterDescriptor;
        m_RenderAllocated = FALSE;
    } else {
        if (m_AdapterCommon) {
            if (m_ServiceGroup) {
                m_AdapterCommon->SetWaveServiceGroup(NULL);
                m_ServiceGroup->Release();
                m_ServiceGroup = NULL;
            }
            m_AdapterCommon->Release();
            m_AdapterCommon = NULL;
        }
        m_Port->Release();
        m_Port = NULL;
    }
    return status;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclic::NewStream(
    OUT PMINIPORTWAVECYCLICSTREAM *OutStream,
    IN PUNKNOWN OuterUnknown,
    IN POOL_TYPE PoolType,
    IN ULONG Pin,
    IN BOOLEAN Capture,
    IN PKSDATAFORMAT DataFormat,
    OUT PDMACHANNEL *OutDmaChannel,
    OUT PSERVICEGROUP *OutServiceGroup)
{
    DbgPrint("vmbaud: NewStream pin=%lu capture=%d\n", Pin, Capture);
    UNREFERENCED_PARAMETER(PoolType);
    UNREFERENCED_PARAMETER(Pin);

    ASSERT(OutStream);
    ASSERT(DataFormat);
    ASSERT(OutDmaChannel);
    ASSERT(OutServiceGroup);

    if (Capture)
        return STATUS_INVALID_DEVICE_REQUEST;   /* render only for now */
    if (m_RenderAllocated)
        return STATUS_INSUFFICIENT_RESOURCES;
    if (!NT_SUCCESS(ValidatePcmFormat(DataFormat))) {
        DbgPrint("vmbaud: NewStream: format refused\n");
        return STATUS_INVALID_PARAMETER;
    }

    PCMiniportWaveCyclicStream stream = new (NonPagedPool, VBAUD_POOLTAG)
        CMiniportWaveCyclicStream(OuterUnknown);
    if (!stream)
        return STATUS_INSUFFICIENT_RESOURCES;
    stream->AddRef();

    NTSTATUS status = stream->Init(this, DataFormat);
    if (NT_SUCCESS(status)) {
        m_RenderAllocated = TRUE;

        /* The stream is also the DMA channel (MSVAD pattern). */
        *OutStream = PMINIPORTWAVECYCLICSTREAM(stream);
        (*OutStream)->AddRef();

        *OutDmaChannel = PDMACHANNEL(stream);
        (*OutDmaChannel)->AddRef();

        *OutServiceGroup = m_ServiceGroup;
        (*OutServiceGroup)->AddRef();
    }

    stream->Release();
    return status;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclic::NonDelegatingQueryInterface(
    IN REFIID Interface, OUT PVOID *Object)
{
    ASSERT(Object);

    if (IsEqualGUIDAligned(Interface, IID_IUnknown))
        *Object = PUNKNOWN(PMINIPORTWAVECYCLIC(this));
    else if (IsEqualGUIDAligned(Interface, IID_IMiniport))
        *Object = PMINIPORT(this);
    else if (IsEqualGUIDAligned(Interface, IID_IMiniportWaveCyclic))
        *Object = PMINIPORTWAVECYCLIC(this);
    else
        *Object = NULL;

    if (*Object) {
        PUNKNOWN(*Object)->AddRef();
        return STATUS_SUCCESS;
    }
    return STATUS_INVALID_PARAMETER;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclic::PropertyHandlerComponentId(
    IN PPCPROPERTY_REQUEST PropertyRequest)
{
    if (PropertyRequest->Verb & KSPROPERTY_TYPE_BASICSUPPORT)
        return PropertyHandler_BasicSupport(
            PropertyRequest, KSPROPERTY_TYPE_BASICSUPPORT | KSPROPERTY_TYPE_GET, VT_ILLEGAL);

    NTSTATUS status = ValidatePropertyParams(PropertyRequest, sizeof(KSCOMPONENTID), 0);
    if (!NT_SUCCESS(status))
        return status;

    if (PropertyRequest->Verb & KSPROPERTY_TYPE_GET) {
        PKSCOMPONENTID id = (PKSCOMPONENTID)PropertyRequest->Value;
        INIT_MMREG_MID(&id->Manufacturer, MM_MICROSOFT);
        id->Product   = GUID_NULL;
        id->Name      = GUID_NULL;
        id->Component = GUID_NULL;
        id->Version   = VBAUD_VERSION;
        id->Revision  = VBAUD_REVISION;
        PropertyRequest->ValueSize = sizeof(KSCOMPONENTID);
    }
    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclic::PropertyHandlerProposedFormat(
    IN PPCPROPERTY_REQUEST PropertyRequest)
{
    if (PropertyRequest->Verb & KSPROPERTY_TYPE_BASICSUPPORT)
        return PropertyHandler_BasicSupport(
            PropertyRequest, KSPROPERTY_TYPE_BASICSUPPORT | KSPROPERTY_TYPE_SET, VT_ILLEGAL);

    ULONG minSize = sizeof(KSDATAFORMAT_WAVEFORMATEX);
    if (PropertyRequest->ValueSize == 0) {
        PropertyRequest->ValueSize = minSize;
        return STATUS_BUFFER_OVERFLOW;
    }
    if (PropertyRequest->ValueSize < minSize)
        return STATUS_BUFFER_TOO_SMALL;

    if (!(PropertyRequest->Verb & KSPROPERTY_TYPE_SET))
        return STATUS_INVALID_PARAMETER;

    if (PKSDATAFORMAT(PropertyRequest->Value)->FormatSize > PropertyRequest->ValueSize)
        return STATUS_BUFFER_TOO_SMALL;
    return ValidatePcmFormat(PKSDATAFORMAT(PropertyRequest->Value));
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclic::PropertyHandlerCpuResources(
    IN PPCPROPERTY_REQUEST PropertyRequest)
{
    if (PropertyRequest->Verb & KSPROPERTY_TYPE_BASICSUPPORT)
        return PropertyHandler_BasicSupport(
            PropertyRequest, KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT, VT_I4);

    if (PropertyRequest->Verb & KSPROPERTY_TYPE_GET) {
        NTSTATUS status = ValidatePropertyParams(PropertyRequest, sizeof(LONG), 0);
        if (NT_SUCCESS(status)) {
            *(PLONG(PropertyRequest->Value)) = KSAUDIO_CPU_RESOURCES_HOST_CPU;
            PropertyRequest->ValueSize = sizeof(LONG);
        }
        return status;
    }
    return STATUS_INVALID_DEVICE_REQUEST;
}

void NTAPI TimerNotify(
    IN PKDPC Dpc, IN PVOID DeferredContext, IN PVOID SA1, IN PVOID SA2)
{
    UNREFERENCED_PARAMETER(Dpc);
    UNREFERENCED_PARAMETER(SA1);
    UNREFERENCED_PARAMETER(SA2);

    PCMiniportWaveCyclic miniport = (PCMiniportWaveCyclic)DeferredContext;
    if (miniport && miniport->m_Port)
        miniport->m_Port->Notify(miniport->m_ServiceGroup);
}

NTSTATUS NTAPI PropertyHandler_WaveFilter(IN PPCPROPERTY_REQUEST PropertyRequest)
{
    PCMiniportWaveCyclic wave = (PCMiniportWaveCyclic)PropertyRequest->MajorTarget;

    switch (PropertyRequest->PropertyItem->Id) {
    case KSPROPERTY_GENERAL_COMPONENTID:
        return wave->PropertyHandlerComponentId(PropertyRequest);
    case KSPROPERTY_PIN_PROPOSEDATAFORMAT:
        return wave->PropertyHandlerProposedFormat(PropertyRequest);
    default:
        return STATUS_INVALID_DEVICE_REQUEST;
    }
}
