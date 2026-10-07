/*
 * common.cpp: CAdapterCommon, the missing CUnknown method bodies, and the
 * plain operator new that stdunk.h does not provide.
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
#include "common.h"

/* Not declared in this mingw tree; present in XP ntoskrnl. */
extern "C" PDEVICE_OBJECT NTAPI IoGetDeviceAttachmentBaseRef(PDEVICE_OBJECT DeviceObject);

/*
 * Compiler-rt / libgcc helpers are not on this link line.  These cover the
 * 64-bit divide that NormalizePhysicalPosition needs, plus the pure-virtual
 * stub that the COM vtables reference.
 */
extern "C" void __cxa_pure_virtual(void)
{
    KeBugCheckEx(0xDEAD, 0, 0, 0, 0);
}

extern "C" unsigned long long __udivdi3(unsigned long long n, unsigned long long d)
{
    unsigned long long q = 0, r = 0;
    if (d == 0)
        return 0;
    for (int i = 63; i >= 0; i--) {
        r = (r << 1) | ((n >> i) & 1);
        if (r >= d) {
            r -= d;
            q |= 1ULL << i;
        }
    }
    return q;
}

extern "C" unsigned long long __umoddi3(unsigned long long n, unsigned long long d)
{
    if (d == 0)
        return 0;
    return n - __udivdi3(n, d) * d;
}

extern "C" long long __divdi3(long long n, long long d)
{
    int neg = 0;
    unsigned long long a = n, b = d;
    if (n < 0) { a = -a; neg ^= 1; }
    if (d < 0) { b = -b; neg ^= 1; }
    long long q = (long long)__udivdi3(a, b);
    return neg ? -q : q;
}

/* stdunk.h supplies placement new and operator delete; plain new is ours. */
void * __cdecl operator new(size_t size)
{
    return ExAllocatePoolWithTag(NonPagedPool, size ? size : 1, VBAUD_POOLTAG);
}

/*
 * stdunk.lib is not on this toolchain, so CUnknown's out-of-line methods
 * live here.  A NULL outer means "be your own IUnknown" (the MSVAD pattern).
 */
CUnknown::CUnknown(PUNKNOWN outer)
{
    m_ref_count = 0;
    m_outer_unknown = outer ? outer : PUNKNOWN(PNONDELEGATINGUNKNOWN(this));
}

CUnknown::~CUnknown()
{
}

STDMETHODIMP_(ULONG) CUnknown::NonDelegatingAddRef()
{
    return (ULONG)InterlockedIncrement(&m_ref_count);
}

STDMETHODIMP_(ULONG) CUnknown::NonDelegatingRelease()
{
    LONG r = InterlockedDecrement(&m_ref_count);
    if (r == 0) {
        delete this;
        return 0;
    }
    return (ULONG)r;
}

STDMETHODIMP_(NTSTATUS) CUnknown::NonDelegatingQueryInterface(REFIID, PVOID *Object)
{
    if (Object)
        *Object = NULL;
    return STATUS_INVALID_PARAMETER;
}

//=============================================================================
// CAdapterCommon
//=============================================================================
class CAdapterCommon final : public IAdapterCommon, public IAdapterPowerManagement, public CUnknown
{
private:
    PDEVICE_OBJECT      m_DeviceObject;
    PDEVICE_OBJECT      m_LowerDevice;      /* referenced VMBus PDO */
    PUNKNOWN            m_WavePortUnknown;  /* for WavePortDriverDest */
    PSERVICEGROUP       m_ServiceGroup;
    LONG                m_Volume[2];        /* left/right, 16.16 dB */
    BOOL                m_Mute;

    ~CAdapterCommon();

public:
    DECLARE_STD_UNKNOWN();
    CAdapterCommon(PUNKNOWN outer);

    /* IAdapterCommon — no IMP_ macro for a private interface. */
    STDMETHODIMP_(NTSTATUS) Init(IN PDEVICE_OBJECT DeviceObject);
    STDMETHODIMP_(PDEVICE_OBJECT) GetDeviceObject(void);
    STDMETHODIMP_(PDEVICE_OBJECT) GetLowerDevice(void);
    STDMETHODIMP_(VOID) SetWaveServiceGroup(IN PSERVICEGROUP ServiceGroup);
    STDMETHODIMP_(PUNKNOWN *) WavePortDriverDest(void);
    STDMETHODIMP_(LONG) MixerVolumeRead(IN ULONG Index, IN LONG Channel);
    STDMETHODIMP_(VOID) MixerVolumeWrite(IN ULONG Index, IN LONG Channel, IN LONG Value);
    STDMETHODIMP_(BOOL) MixerMuteRead(IN ULONG Index);
    STDMETHODIMP_(VOID) MixerMuteWrite(IN ULONG Index, IN BOOL Value);
    STDMETHODIMP_(VOID) MixerReset(void);

    IMP_IAdapterPowerManagement;

    friend NTSTATUS NewAdapterCommon(
        OUT PUNKNOWN *Unknown, IN REFCLSID, IN PUNKNOWN, IN POOL_TYPE);
};

STDMETHODIMP_(NTSTATUS) CAdapterCommon::Init(IN PDEVICE_OBJECT DeviceObject)
{
    PAGED_CODE();
    m_DeviceObject = DeviceObject;
    /* Bottom of the stack is the VMBus child PDO that serves the pipe. */
    m_LowerDevice = IoGetDeviceAttachmentBaseRef(DeviceObject);
    MixerReset();
    return m_LowerDevice ? STATUS_SUCCESS : STATUS_NO_SUCH_DEVICE;
}

STDMETHODIMP_(PDEVICE_OBJECT) CAdapterCommon::GetDeviceObject()
{
    return m_DeviceObject;
}

STDMETHODIMP_(PDEVICE_OBJECT) CAdapterCommon::GetLowerDevice()
{
    return m_LowerDevice;
}

STDMETHODIMP_(VOID) CAdapterCommon::SetWaveServiceGroup(IN PSERVICEGROUP ServiceGroup)
{
    if (m_ServiceGroup)
        m_ServiceGroup->Release();
    m_ServiceGroup = ServiceGroup;
    if (m_ServiceGroup)
        m_ServiceGroup->AddRef();
}

STDMETHODIMP_(PUNKNOWN *) CAdapterCommon::WavePortDriverDest()
{
    return &m_WavePortUnknown;
}

STDMETHODIMP_(LONG) CAdapterCommon::MixerVolumeRead(IN ULONG Index, IN LONG Channel)
{
    if (Index != KSNODE_TOPO_VOLUME)
        return 0;
    if (Channel == 0 || Channel == 1)
        return m_Volume[Channel];
    return m_Volume[0] < m_Volume[1] ? m_Volume[0] : m_Volume[1];
}

STDMETHODIMP_(VOID) CAdapterCommon::MixerVolumeWrite(IN ULONG Index, IN LONG Channel, IN LONG Value)
{
    if (Index != KSNODE_TOPO_VOLUME)
        return;
    if (Channel == 0 || Channel == 1)
        m_Volume[Channel] = Value;
    else {
        m_Volume[0] = Value;
        m_Volume[1] = Value;
    }
}

STDMETHODIMP_(BOOL) CAdapterCommon::MixerMuteRead(IN ULONG Index)
{
    UNREFERENCED_PARAMETER(Index);
    return m_Mute;
}

STDMETHODIMP_(VOID) CAdapterCommon::MixerMuteWrite(IN ULONG Index, IN BOOL Value)
{
    UNREFERENCED_PARAMETER(Index);
    m_Mute = Value;
}

STDMETHODIMP_(VOID) CAdapterCommon::MixerReset()
{
    m_Volume[0] = 0;
    m_Volume[1] = 0;
    m_Mute = FALSE;
}

/* Accept every power state; nothing to gate on a virtual pipe. */
STDMETHODIMP_(VOID) CAdapterCommon::PowerChangeState(IN POWER_STATE NewState)
{
    UNREFERENCED_PARAMETER(NewState);
}

STDMETHODIMP_(NTSTATUS) CAdapterCommon::QueryPowerChangeState(IN POWER_STATE NewStateQuery)
{
    UNREFERENCED_PARAMETER(NewStateQuery);
    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CAdapterCommon::QueryDeviceCapabilities(
    IN PDEVICE_CAPABILITIES PowerDeviceCaps)
{
    UNREFERENCED_PARAMETER(PowerDeviceCaps);
    return STATUS_SUCCESS;
}

CAdapterCommon::CAdapterCommon(PUNKNOWN outer) : CUnknown(outer)
{
    m_DeviceObject = NULL;
    m_LowerDevice = NULL;
    m_WavePortUnknown = NULL;
    m_ServiceGroup = NULL;
    MixerReset();
}

CAdapterCommon::~CAdapterCommon()
{
    if (m_ServiceGroup)
        m_ServiceGroup->Release();
    if (m_WavePortUnknown)
        m_WavePortUnknown->Release();
    if (m_LowerDevice)
        ObDereferenceObject(m_LowerDevice);
}

STDMETHODIMP_(NTSTATUS) CAdapterCommon::NonDelegatingQueryInterface(
    IN REFIID Interface, OUT PVOID *Object)
{
    ASSERT(Object);

    if (IsEqualGUIDAligned(Interface, IID_IUnknown))
        *Object = PUNKNOWN(PADAPTERCOMMON(this));
    else if (IsEqualGUIDAligned(Interface, IID_IAdapterCommon))
        *Object = PADAPTERCOMMON(this);
    else if (IsEqualGUIDAligned(Interface, IID_IAdapterPowerManagement))
        *Object = PADAPTERPOWERMANAGEMENT(this);
    else
        *Object = NULL;

    if (*Object) {
        PUNKNOWN(*Object)->AddRef();
        return STATUS_SUCCESS;
    }
    return STATUS_INVALID_PARAMETER;
}

NTSTATUS NewAdapterCommon(
    OUT PUNKNOWN *Unknown, IN REFCLSID, IN PUNKNOWN UnknownOuter, IN POOL_TYPE PoolType)
{
    ASSERT(Unknown);
    /* Two IUnknown bases make a bare PUNKNOWN cast ambiguous; go via
     * IAdapterCommon. */
    STD_CREATE_BODY_WITH_TAG_(
        CAdapterCommon, Unknown, UnknownOuter, PoolType, VBAUD_POOLTAG,
        PADAPTERCOMMON);
}
