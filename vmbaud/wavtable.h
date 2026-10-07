/*
 * wavtable.h: wave filter pin/node/connection/property tables.
 *
 * Derived in part from Scream (https://github.com/duncanthrax/scream), which
 * is based on the MSVAD sample: Copyright (c) 1997-2000 Microsoft
 * Corporation.  All rights reserved.  Distributed under the MS-PL; see
 * LICENSE in this directory.
 */
#ifndef _VMBAUD_WAVTABLE_H_
#define _VMBAUD_WAVTABLE_H_

/* One PCM render sink plus an analog bridge pin to the topology. */
static KSDATARANGE_AUDIO PinDataRangesStream[] = {
    {
        {
            sizeof(KSDATARANGE_AUDIO),
            0, 0, 0,
            STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
            STATICGUIDOF(KSDATAFORMAT_SUBTYPE_PCM),
            STATICGUIDOF(KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)
        },
        2,      /* MaxChannels */
        16,     /* MinBitsPerSample */
        16,     /* MaxBitsPerSample */
        8000,   /* MinSampleRate */
        48000   /* MaxSampleRate */
    },
};

static PKSDATARANGE PinDataRangePointersStream[] = {
    PKSDATARANGE(&PinDataRangesStream[0])
};

static KSDATARANGE PinDataRangesBridge[] = {
    {
        sizeof(KSDATARANGE),
        0, 0, 0,
        STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
        STATICGUIDOF(KSDATAFORMAT_SUBTYPE_ANALOG),
        STATICGUIDOF(KSDATAFORMAT_SPECIFIER_NONE)
    }
};

static PKSDATARANGE PinDataRangePointersBridge[] = {
    &PinDataRangesBridge[0]
};

static PCPIN_DESCRIPTOR MiniportPins[] = {
    /* KSPIN_WAVE_RENDER_SINK */
    {
        1, 1, 0, NULL,
        {
            0, NULL, 0, NULL,
            SIZEOF_ARRAY(PinDataRangePointersStream),
            PinDataRangePointersStream,
            KSPIN_DATAFLOW_IN,
            KSPIN_COMMUNICATION_SINK,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    },
    /* KSPIN_WAVE_RENDER_SOURCE (bridge to topology) */
    {
        0, 0, 0, NULL,
        {
            0, NULL, 0, NULL,
            SIZEOF_ARRAY(PinDataRangePointersBridge),
            PinDataRangePointersBridge,
            KSPIN_DATAFLOW_OUT,
            KSPIN_COMMUNICATION_NONE,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    },
};

static PCNODE_DESCRIPTOR MiniportNodes[] = {
    { 0, NULL, &KSNODETYPE_DAC, NULL }
};

static PCCONNECTION_DESCRIPTOR MiniportConnections[] = {
    { PCFILTER_NODE,   KSPIN_WAVE_RENDER_SINK,   KSNODE_WAVE_DAC, 1 },
    { KSNODE_WAVE_DAC, 0,                        PCFILTER_NODE,   KSPIN_WAVE_RENDER_SOURCE },
};

static PCPROPERTY_ITEM PropertiesWaveFilter[] = {
    {
        &KSPROPSETID_General,
        KSPROPERTY_GENERAL_COMPONENTID,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_WaveFilter
    },
    {
        &KSPROPSETID_Pin,
        KSPROPERTY_PIN_PROPOSEDATAFORMAT,
        KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_WaveFilter
    }
};
DEFINE_PCAUTOMATION_TABLE_PROP(AutomationWaveFilter, PropertiesWaveFilter);

static PCFILTER_DESCRIPTOR MiniportFilterDescriptor = {
    0,
    &AutomationWaveFilter,
    sizeof(PCPIN_DESCRIPTOR),
    SIZEOF_ARRAY(MiniportPins),
    MiniportPins,
    sizeof(PCNODE_DESCRIPTOR),
    SIZEOF_ARRAY(MiniportNodes),
    MiniportNodes,
    SIZEOF_ARRAY(MiniportConnections),
    MiniportConnections,
    0,
    NULL
};

#endif /* _VMBAUD_WAVTABLE_H_ */
