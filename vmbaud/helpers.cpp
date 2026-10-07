/*
 * helpers.cpp: PortCls property / format helpers (MSVAD-derived).
 *
 * Derived in part from Scream (https://github.com/duncanthrax/scream), which
 * is based on the MSVAD sample: Copyright (c) 1997-2000 Microsoft
 * Corporation.  All rights reserved.  Distributed under the MS-PL; see
 * LICENSE in this directory.
 */
#include <ntddk.h>
#include "common/ddk_compat.h"
#include <portcls.h>
#include <punknown.h>
#include <stdunk.h>
#include "vmbaud.h"
#include "helpers.h"

PWAVEFORMATEX GetWaveFormatEx(IN PKSDATAFORMAT DataFormat)
{
    PAGED_CODE();

    if (!DataFormat)
        return NULL;
    if (!IsEqualGUIDAligned(DataFormat->MajorFormat, KSDATAFORMAT_TYPE_AUDIO))
        return NULL;
    if (IsEqualGUIDAligned(DataFormat->Specifier, KSDATAFORMAT_SPECIFIER_WAVEFORMATEX))
        return PWAVEFORMATEX(DataFormat + 1);
    if (IsEqualGUIDAligned(DataFormat->Specifier, KSDATAFORMAT_SPECIFIER_DSOUND)) {
        PKSDSOUND_BUFFERDESC desc = PKSDSOUND_BUFFERDESC(DataFormat + 1);
        return &desc->WaveFormatEx;
    }
    return NULL;
}

/*
 * PCM in a WAVEFORMATEX (or WAVEFORMATEXTENSIBLE with a PCM subformat),
 * 16 bits, 1 or 2 channels, 8 to 48 kHz, with consistent block and byte
 * rate.  Not DirectSound's hardware buffer formats (KSDATAFORMAT_SPECIFIER_
 * DSOUND): the card mixes nothing itself, and DirectSound probes those with
 * rates down to 100 Hz; refused, it mixes in software through kmixer.
 */
NTSTATUS ValidatePcmFormat(IN PKSDATAFORMAT DataFormat)
{
    PAGED_CODE();

    if (!DataFormat || DataFormat->FormatSize < sizeof(KSDATAFORMAT_WAVEFORMATEX))
        return STATUS_NO_MATCH;
    if (!IsEqualGUIDAligned(DataFormat->MajorFormat, KSDATAFORMAT_TYPE_AUDIO) ||
        !IsEqualGUIDAligned(DataFormat->SubFormat, KSDATAFORMAT_SUBTYPE_PCM) ||
        !IsEqualGUIDAligned(DataFormat->Specifier, KSDATAFORMAT_SPECIFIER_WAVEFORMATEX))
        return STATUS_NO_MATCH;

    PWAVEFORMATEX wfx = PWAVEFORMATEX(DataFormat + 1);
    if (wfx->wFormatTag == WAVE_FORMAT_EXTENSIBLE) {
        PWAVEFORMATEXTENSIBLE ext = PWAVEFORMATEXTENSIBLE(wfx);
        if (DataFormat->FormatSize < sizeof(KSDATAFORMAT) + sizeof(WAVEFORMATEXTENSIBLE) ||
            !IsEqualGUIDAligned(ext->SubFormat, KSDATAFORMAT_SUBTYPE_PCM) ||
            ext->Samples.wValidBitsPerSample != 16)
            return STATUS_NO_MATCH;
    } else if (wfx->wFormatTag != WAVE_FORMAT_PCM) {
        return STATUS_NO_MATCH;
    }
    if (wfx->nChannels < 1 || wfx->nChannels > 2 || wfx->wBitsPerSample != 16)
        return STATUS_NO_MATCH;
    if (wfx->nSamplesPerSec < 8000 || wfx->nSamplesPerSec > 48000)
        return STATUS_NO_MATCH;
    if (wfx->nBlockAlign != wfx->nChannels * 2 ||
        wfx->nAvgBytesPerSec != wfx->nSamplesPerSec * wfx->nBlockAlign)
        return STATUS_NO_MATCH;
    return STATUS_SUCCESS;
}

NTSTATUS NTAPI PropertyHandler_BasicSupport(
    IN PPCPROPERTY_REQUEST PropertyRequest,
    IN ULONG Flags,
    IN DWORD PropTypeSetId)
{
    PAGED_CODE();

    if (PropertyRequest->ValueSize >= sizeof(KSPROPERTY_DESCRIPTION)) {
        PKSPROPERTY_DESCRIPTION desc = PKSPROPERTY_DESCRIPTION(PropertyRequest->Value);
        desc->AccessFlags     = Flags;
        desc->DescriptionSize = sizeof(KSPROPERTY_DESCRIPTION);
        if (VT_ILLEGAL != PropTypeSetId) {
            desc->PropTypeSet.Set = KSPROPTYPESETID_General;
            desc->PropTypeSet.Id  = PropTypeSetId;
        } else {
            desc->PropTypeSet.Set = GUID_NULL;
            desc->PropTypeSet.Id  = 0;
        }
        desc->PropTypeSet.Flags  = 0;
        desc->MembersListCount   = 0;
        desc->Reserved           = 0;
        PropertyRequest->ValueSize = sizeof(KSPROPERTY_DESCRIPTION);
        return STATUS_SUCCESS;
    }
    if (PropertyRequest->ValueSize >= sizeof(ULONG)) {
        *(PULONG(PropertyRequest->Value)) = Flags;
        PropertyRequest->ValueSize = sizeof(ULONG);
        return STATUS_SUCCESS;
    }
    PropertyRequest->ValueSize = 0;
    return STATUS_BUFFER_TOO_SMALL;
}

NTSTATUS ValidatePropertyParams(
    IN PPCPROPERTY_REQUEST PropertyRequest,
    IN ULONG cbValueSize,
    IN ULONG cbInstanceSize)
{
    PAGED_CODE();

    NTSTATUS status = STATUS_UNSUCCESSFUL;

    if (PropertyRequest && cbValueSize) {
        if (0 == PropertyRequest->ValueSize) {
            PropertyRequest->ValueSize = cbValueSize;
            status = STATUS_BUFFER_OVERFLOW;
        } else if (PropertyRequest->ValueSize < cbValueSize) {
            status = STATUS_BUFFER_TOO_SMALL;
        } else if (PropertyRequest->InstanceSize < cbInstanceSize) {
            status = STATUS_BUFFER_TOO_SMALL;
        } else if (PropertyRequest->ValueSize == cbValueSize && PropertyRequest->Value) {
            status = STATUS_SUCCESS;
        }
    } else {
        status = STATUS_INVALID_PARAMETER;
    }

    if (PropertyRequest && status != STATUS_SUCCESS && status != STATUS_BUFFER_OVERFLOW)
        PropertyRequest->ValueSize = 0;
    return status;
}
