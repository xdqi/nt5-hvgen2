/*
 * helpers.h: small PortCls property / format helpers (MSVAD-derived).
 */
#ifndef _VMBAUD_HELPERS_H_
#define _VMBAUD_HELPERS_H_

PWAVEFORMATEX GetWaveFormatEx(IN PKSDATAFORMAT DataFormat);

NTSTATUS NTAPI PropertyHandler_BasicSupport(
    IN PPCPROPERTY_REQUEST PropertyRequest,
    IN ULONG Flags,
    IN DWORD PropTypeSetId);

NTSTATUS ValidatePropertyParams(
    IN PPCPROPERTY_REQUEST PropertyRequest,
    IN ULONG cbValueSize,
    IN ULONG cbInstanceSize);

#endif /* _VMBAUD_HELPERS_H_ */
