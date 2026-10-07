/*
 * common.h: CAdapterCommon — mixer state, service group, VMBus PDO pointer.
 */
#ifndef _VMBAUD_COMMON_H_
#define _VMBAUD_COMMON_H_

DEFINE_GUID(IID_IAdapterCommon,
    0x7eda2950, 0xbf9f, 0x11d0, 0x87, 0x1f, 0x00, 0xa0, 0xc9, 0x11, 0xb5, 0x44);

DECLARE_INTERFACE_(IAdapterCommon, IUnknown)
{
    STDMETHOD_(NTSTATUS, Init)(THIS_ IN PDEVICE_OBJECT DeviceObject) PURE;
    STDMETHOD_(PDEVICE_OBJECT, GetDeviceObject)(THIS) PURE;
    STDMETHOD_(PDEVICE_OBJECT, GetLowerDevice)(THIS) PURE;
    STDMETHOD_(VOID, SetWaveServiceGroup)(THIS_ IN PSERVICEGROUP ServiceGroup) PURE;
    STDMETHOD_(PUNKNOWN *, WavePortDriverDest)(THIS) PURE;
    STDMETHOD_(LONG, MixerVolumeRead)(THIS_ IN ULONG Index, IN LONG Channel) PURE;
    STDMETHOD_(VOID, MixerVolumeWrite)(THIS_ IN ULONG Index, IN LONG Channel, IN LONG Value) PURE;
    STDMETHOD_(BOOL, MixerMuteRead)(THIS_ IN ULONG Index) PURE;
    STDMETHOD_(VOID, MixerMuteWrite)(THIS_ IN ULONG Index, IN BOOL Value) PURE;
    STDMETHOD_(VOID, MixerReset)(THIS) PURE;
};
typedef IAdapterCommon *PADAPTERCOMMON;

NTSTATUS NewAdapterCommon(
    OUT PUNKNOWN *Unknown,
    IN REFCLSID,
    IN PUNKNOWN UnknownOuter OPTIONAL,
    IN POOL_TYPE PoolType);

#endif /* _VMBAUD_COMMON_H_ */
