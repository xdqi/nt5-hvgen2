/*
 * mintopo.h: minimal topology — one render bridge + one volume node.
 */
#ifndef _VMBAUD_MINTOPO_H_
#define _VMBAUD_MINTOPO_H_

/* portcls.h has no IMP_IMiniportTopology; IMiniport + Init. */
#define IMP_IMiniportTopology \
    IMP_IMiniport; \
    STDMETHODIMP_(NTSTATUS) Init( \
        IN PUNKNOWN UnknownAdapter, \
        IN PRESOURCELIST ResourceList, \
        IN PPORTTOPOLOGY Port)

class CMiniportTopology final : public IMiniportTopology, public CUnknown
{
private:
    PADAPTERCOMMON          m_AdapterCommon;
    PPCFILTER_DESCRIPTOR    m_FilterDescriptor;
    ~CMiniportTopology();

public:
    DECLARE_STD_UNKNOWN();
    CMiniportTopology(PUNKNOWN outer);

    IMP_IMiniportTopology;

    NTSTATUS NTAPI PropertyHandlerBasicSupportVolume(IN PPCPROPERTY_REQUEST PropertyRequest);
    NTSTATUS NTAPI PropertyHandlerCpuResources(IN PPCPROPERTY_REQUEST PropertyRequest);
    NTSTATUS NTAPI PropertyHandlerVolume(IN PPCPROPERTY_REQUEST PropertyRequest);

    friend NTSTATUS NTAPI PropertyHandler_Topology(IN PPCPROPERTY_REQUEST PropertyRequest);
};
typedef CMiniportTopology *PCMiniportTopology;

NTSTATUS NTAPI CreateMiniportTopology(
    OUT PUNKNOWN *Unknown, IN REFCLSID, IN PUNKNOWN UnknownOuter, IN POOL_TYPE PoolType);

extern NTSTATUS NTAPI PropertyHandler_Topology(IN PPCPROPERTY_REQUEST PropertyRequest);

#endif /* _VMBAUD_MINTOPO_H_ */
