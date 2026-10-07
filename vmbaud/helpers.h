/*
 * helpers.h: small PortCls property / format helpers (MSVAD-derived).
 *
 * Derived in part from Scream (https://github.com/duncanthrax/scream), which
 * is based on the MSVAD sample: Copyright (c) 1997-2000 Microsoft
 * Corporation.  All rights reserved.  Distributed under the MS-PL; see
 * LICENSE in this directory.
 */
#ifndef _VMBAUD_HELPERS_H_
#define _VMBAUD_HELPERS_H_

PWAVEFORMATEX GetWaveFormatEx(IN PKSDATAFORMAT DataFormat);

/* STATUS_SUCCESS for the formats the render pin plays, else STATUS_NO_MATCH. */
NTSTATUS ValidatePcmFormat(IN PKSDATAFORMAT DataFormat);

NTSTATUS NTAPI PropertyHandler_BasicSupport(
    IN PPCPROPERTY_REQUEST PropertyRequest,
    IN ULONG Flags,
    IN DWORD PropTypeSetId);

NTSTATUS ValidatePropertyParams(
    IN PPCPROPERTY_REQUEST PropertyRequest,
    IN ULONG cbValueSize,
    IN ULONG cbInstanceSize);

#endif /* _VMBAUD_HELPERS_H_ */
