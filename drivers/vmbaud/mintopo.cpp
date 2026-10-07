/*
 * mintopo.cpp: topology miniport — one volume node on the render path.
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
#include "mintopo.h"

#include "toptable.h"

NTSTATUS NTAPI CreateMiniportTopology(
    OUT PUNKNOWN *Unknown, IN REFCLSID, IN PUNKNOWN UnknownOuter, IN POOL_TYPE PoolType)
{
    PAGED_CODE();
    ASSERT(Unknown);
    STD_CREATE_BODY(CMiniportTopology, Unknown, UnknownOuter, PoolType);
}

CMiniportTopology::CMiniportTopology(PUNKNOWN outer) : CUnknown(outer)
{
    PAGED_CODE();
    m_AdapterCommon = NULL;
    m_FilterDescriptor = NULL;
}

CMiniportTopology::~CMiniportTopology()
{
    PAGED_CODE();
    if (m_AdapterCommon)
        m_AdapterCommon->Release();
}

STDMETHODIMP_(NTSTATUS) CMiniportTopology::DataRangeIntersection(
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

    PAGED_CODE();
    return STATUS_NOT_IMPLEMENTED;
}

STDMETHODIMP_(NTSTATUS) CMiniportTopology::GetDescription(
    OUT PPCFILTER_DESCRIPTOR *OutFilterDescriptor)
{
    PAGED_CODE();
    ASSERT(OutFilterDescriptor);
    *OutFilterDescriptor = m_FilterDescriptor;
    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportTopology::Init(
    IN PUNKNOWN UnknownAdapter,
    IN PRESOURCELIST ResourceList,
    IN PPORTTOPOLOGY Port)
{
    UNREFERENCED_PARAMETER(ResourceList);
    UNREFERENCED_PARAMETER(Port);

    PAGED_CODE();
    ASSERT(UnknownAdapter);
    ASSERT(Port);

    NTSTATUS status = UnknownAdapter->QueryInterface(
        IID_IAdapterCommon, (PVOID *)&m_AdapterCommon);
    if (NT_SUCCESS(status)) {
        m_AdapterCommon->MixerReset();
        m_FilterDescriptor = &MiniportFilterDescriptor;
    } else if (m_AdapterCommon) {
        m_AdapterCommon->Release();
        m_AdapterCommon = NULL;
    }
    return status;
}

STDMETHODIMP_(NTSTATUS) CMiniportTopology::NonDelegatingQueryInterface(
    IN REFIID Interface, OUT PVOID *Object)
{
    PAGED_CODE();
    ASSERT(Object);

    if (IsEqualGUIDAligned(Interface, IID_IUnknown))
        *Object = PUNKNOWN(PMINIPORTTOPOLOGY(this));
    else if (IsEqualGUIDAligned(Interface, IID_IMiniport))
        *Object = PMINIPORT(this);
    else if (IsEqualGUIDAligned(Interface, IID_IMiniportTopology))
        *Object = PMINIPORTTOPOLOGY(this);
    else
        *Object = NULL;

    if (*Object) {
        PUNKNOWN(*Object)->AddRef();
        return STATUS_SUCCESS;
    }
    return STATUS_INVALID_PARAMETER;
}

STDMETHODIMP_(NTSTATUS) CMiniportTopology::PropertyHandlerBasicSupportVolume(
    IN PPCPROPERTY_REQUEST PropertyRequest)
{
    PAGED_CODE();

    ULONG fullSize = sizeof(KSPROPERTY_DESCRIPTION)
                   + sizeof(KSPROPERTY_MEMBERSHEADER)
                   + sizeof(KSPROPERTY_STEPPING_LONG);

    if (PropertyRequest->ValueSize >= sizeof(KSPROPERTY_DESCRIPTION)) {
        PKSPROPERTY_DESCRIPTION desc = PKSPROPERTY_DESCRIPTION(PropertyRequest->Value);
        desc->AccessFlags     = KSPROPERTY_TYPE_ALL;
        desc->DescriptionSize = fullSize;
        desc->PropTypeSet.Set = KSPROPTYPESETID_General;
        desc->PropTypeSet.Id  = VT_I4;
        desc->PropTypeSet.Flags = 0;
        desc->MembersListCount  = 1;
        desc->Reserved          = 0;

        if (PropertyRequest->ValueSize >= fullSize) {
            PKSPROPERTY_MEMBERSHEADER members = PKSPROPERTY_MEMBERSHEADER(desc + 1);
            members->MembersFlags = KSPROPERTY_MEMBER_STEPPEDRANGES;
            members->MembersSize  = sizeof(KSPROPERTY_STEPPING_LONG);
            members->MembersCount = 1;
            members->Flags        = KSPROPERTY_MEMBER_FLAG_BASICSUPPORT_MULTICHANNEL;

            PKSPROPERTY_STEPPING_LONG range = PKSPROPERTY_STEPPING_LONG(members + 1);
            range->Bounds.SignedMaximum = 0x00000000;      /* 0 dB */
            range->Bounds.SignedMinimum = -96 * 0x10000;   /* -96 dB */
            range->SteppingDelta        = 0x08000;         /* 0.5 dB */
            range->Reserved             = 0;

            PropertyRequest->ValueSize = fullSize;
        } else {
            PropertyRequest->ValueSize = sizeof(KSPROPERTY_DESCRIPTION);
        }
        return STATUS_SUCCESS;
    }

    if (PropertyRequest->ValueSize >= sizeof(ULONG)) {
        *(PULONG(PropertyRequest->Value)) = KSPROPERTY_TYPE_ALL;
        PropertyRequest->ValueSize = sizeof(ULONG);
        return STATUS_SUCCESS;
    }

    PropertyRequest->ValueSize = 0;
    return STATUS_BUFFER_TOO_SMALL;
}

STDMETHODIMP_(NTSTATUS) CMiniportTopology::PropertyHandlerCpuResources(
    IN PPCPROPERTY_REQUEST PropertyRequest)
{
    PAGED_CODE();

    if (PropertyRequest->Verb & KSPROPERTY_TYPE_BASICSUPPORT)
        return PropertyHandler_BasicSupport(
            PropertyRequest, KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT, VT_I4);

    if (PropertyRequest->Verb & KSPROPERTY_TYPE_GET) {
        NTSTATUS status = ValidatePropertyParams(PropertyRequest, sizeof(ULONG), 0);
        if (NT_SUCCESS(status)) {
            *(PLONG(PropertyRequest->Value)) = KSAUDIO_CPU_RESOURCES_HOST_CPU;
            PropertyRequest->ValueSize = sizeof(LONG);
        }
        return status;
    }
    return STATUS_INVALID_DEVICE_REQUEST;
}

STDMETHODIMP_(NTSTATUS) CMiniportTopology::PropertyHandlerVolume(
    IN PPCPROPERTY_REQUEST PropertyRequest)
{
    PAGED_CODE();

    if (PropertyRequest->Verb & KSPROPERTY_TYPE_BASICSUPPORT)
        return PropertyHandlerBasicSupportVolume(PropertyRequest);

    NTSTATUS status = ValidatePropertyParams(PropertyRequest, sizeof(ULONG), sizeof(LONG));
    if (!NT_SUCCESS(status))
        return status;

    LONG channel = *(PLONG(PropertyRequest->Instance));
    PULONG value = PULONG(PropertyRequest->Value);

    if (PropertyRequest->Verb & KSPROPERTY_TYPE_GET) {
        *value = (ULONG)m_AdapterCommon->MixerVolumeRead(PropertyRequest->Node, channel);
        PropertyRequest->ValueSize = sizeof(ULONG);
    } else if (PropertyRequest->Verb & KSPROPERTY_TYPE_SET) {
        m_AdapterCommon->MixerVolumeWrite(PropertyRequest->Node, channel, (LONG)*value);
    }
    return STATUS_SUCCESS;
}

NTSTATUS NTAPI PropertyHandler_Topology(IN PPCPROPERTY_REQUEST PropertyRequest)
{
    PAGED_CODE();

    PCMiniportTopology topo = (PCMiniportTopology)PropertyRequest->MajorTarget;

    switch (PropertyRequest->PropertyItem->Id) {
    case KSPROPERTY_AUDIO_VOLUMELEVEL:
        return topo->PropertyHandlerVolume(PropertyRequest);
    case KSPROPERTY_AUDIO_CPU_RESOURCES:
        return topo->PropertyHandlerCpuResources(PropertyRequest);
    default:
        return STATUS_INVALID_DEVICE_REQUEST;
    }
}
