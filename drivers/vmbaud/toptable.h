/*
 * toptable.h: topology tables — one volume node on the render path.
 *
 * Derived in part from Scream (https://github.com/duncanthrax/scream), which
 * is based on the MSVAD sample: Copyright (c) 1997-2000 Microsoft
 * Corporation.  All rights reserved.  Distributed under the MS-PL; see
 * LICENSE in this directory.
 */
#ifndef _VMBAUD_TOPTABLE_H_
#define _VMBAUD_TOPTABLE_H_

static KSDATARANGE TopoPinDataRangesBridge[] = {
    {
        sizeof(KSDATARANGE),
        0, 0, 0,
        STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
        STATICGUIDOF(KSDATAFORMAT_SUBTYPE_ANALOG),
        STATICGUIDOF(KSDATAFORMAT_SPECIFIER_NONE)
    }
};

static PKSDATARANGE TopoPinDataRangePointers[] = {
    &TopoPinDataRangesBridge[0]
};

static PCPIN_DESCRIPTOR MiniportPins[] = {
    /* KSPIN_TOPO_WAVEOUT_SOURCE (bridge from wave) */
    {
        0, 0, 0, NULL,
        {
            0, NULL, 0, NULL,
            SIZEOF_ARRAY(TopoPinDataRangePointers),
            TopoPinDataRangePointers,
            KSPIN_DATAFLOW_IN,
            KSPIN_COMMUNICATION_NONE,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    },
    /* KSPIN_TOPO_LINEOUT_DEST */
    {
        0, 0, 0, NULL,
        {
            0, NULL, 0, NULL,
            SIZEOF_ARRAY(TopoPinDataRangePointers),
            TopoPinDataRangePointers,
            KSPIN_DATAFLOW_OUT,
            KSPIN_COMMUNICATION_NONE,
            &KSNODETYPE_SPEAKER,
            NULL,
            0
        }
    },
};

static PCPROPERTY_ITEM PropertiesVolume[] = {
    {
        &KSPROPSETID_Audio,
        KSPROPERTY_AUDIO_VOLUMELEVEL,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_Topology
    },
    {
        &KSPROPSETID_Audio,
        KSPROPERTY_AUDIO_CPU_RESOURCES,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_Topology
    }
};
DEFINE_PCAUTOMATION_TABLE_PROP(AutomationVolume, PropertiesVolume);

static PCNODE_DESCRIPTOR TopologyNodes[] = {
    {
        0,
        &AutomationVolume,
        &KSNODETYPE_VOLUME,
        &KSAUDFNAME_MASTER_VOLUME
    }
};

static PCCONNECTION_DESCRIPTOR MiniportConnections[] = {
    { PCFILTER_NODE,     KSPIN_TOPO_WAVEOUT_SOURCE, KSNODE_TOPO_VOLUME, 1 },
    { KSNODE_TOPO_VOLUME, 0,                        PCFILTER_NODE,      KSPIN_TOPO_LINEOUT_DEST },
};

static PCFILTER_DESCRIPTOR MiniportFilterDescriptor = {
    0,
    NULL,
    sizeof(PCPIN_DESCRIPTOR),
    SIZEOF_ARRAY(MiniportPins),
    MiniportPins,
    sizeof(PCNODE_DESCRIPTOR),
    SIZEOF_ARRAY(TopologyNodes),
    TopologyNodes,
    SIZEOF_ARRAY(MiniportConnections),
    MiniportConnections,
    0,
    NULL
};

#endif /* _VMBAUD_TOPTABLE_H_ */
