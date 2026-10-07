/*
 * minwave.h: WaveCyclic miniport (render only).
 */
#ifndef _VMBAUD_MINWAVE_H_
#define _VMBAUD_MINWAVE_H_

class CMiniportWaveCyclicStream;

class CMiniportWaveCyclic final : public IMiniportWaveCyclic, public CUnknown
{
private:
    BOOL            m_RenderAllocated;
    ~CMiniportWaveCyclic();

protected:
    PADAPTERCOMMON      m_AdapterCommon;
    PPORTWAVECYCLIC     m_Port;
    PPCFILTER_DESCRIPTOR m_FilterDescriptor;
    ULONG               m_NotificationInterval; /* ms */
    ULONG               m_SamplingFrequency;
    PSERVICEGROUP       m_ServiceGroup;
    KMUTEX              m_SampleRateSync;
    ULONG               m_MaxDmaBufferSize;

public:
    DECLARE_STD_UNKNOWN();
    CMiniportWaveCyclic(PUNKNOWN other);

    IMP_IMiniportWaveCyclic;

    NTSTATUS NTAPI PropertyHandlerComponentId(IN PPCPROPERTY_REQUEST PropertyRequest);
    NTSTATUS NTAPI PropertyHandlerProposedFormat(IN PPCPROPERTY_REQUEST PropertyRequest);
    NTSTATUS NTAPI PropertyHandlerCpuResources(IN PPCPROPERTY_REQUEST PropertyRequest);

    friend class CMiniportWaveCyclicStream;
    friend void NTAPI TimerNotify(IN PKDPC Dpc, IN PVOID DeferredContext, IN PVOID SA1, IN PVOID SA2);
    friend NTSTATUS NTAPI PropertyHandler_WaveFilter(IN PPCPROPERTY_REQUEST PropertyRequest);
};
typedef CMiniportWaveCyclic *PCMiniportWaveCyclic;

NTSTATUS NTAPI CreateMiniportWaveCyclic(
    OUT PUNKNOWN *Unknown, IN REFCLSID, IN PUNKNOWN UnknownOuter, IN POOL_TYPE PoolType);

void NTAPI TimerNotify(IN PKDPC Dpc, IN PVOID DeferredContext, IN PVOID SA1, IN PVOID SA2);

#endif /* _VMBAUD_MINWAVE_H_ */
