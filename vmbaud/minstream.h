/*
 * minstream.h: WaveCyclic stream + IDmaChannel (same object, MSVAD pattern).
 *
 * Derived in part from Scream (https://github.com/duncanthrax/scream), which
 * is based on the MSVAD sample: Copyright (c) 1997-2000 Microsoft
 * Corporation.  All rights reserved.  Distributed under the MS-PL; see
 * LICENSE in this directory.
 */
#ifndef _VMBAUD_MINSTREAM_H_
#define _VMBAUD_MINSTREAM_H_

class CMiniportWaveCyclic;

/* A spin lock that remembers the IRQL its holder came from. */
typedef struct _VBAUD_LOCK {
    KSPIN_LOCK  Lock;
    KIRQL       OldIrql;
} VBAUD_LOCK;

/* In-flight async write; StopThread cancels through this list. */
typedef struct _VBAUD_WRITE_CTX {
    struct _VBAUD_WRITE_CTX *Link;
    PIRP                    Irp;
    PVOID                   Buffer;
    class CMiniportWaveCyclicStream *Stream;
} VBAUD_WRITE_CTX;

class CMiniportWaveCyclicStream final
    : public IMiniportWaveCyclicStream, public IDmaChannel, public CUnknown
{
private:
    PCMiniportWaveCyclic    m_Miniport;
    BOOLEAN                 m_Format16Bit;
    USHORT                  m_BlockAlign;
    KSSTATE                 m_KsState;
    ULONG                   m_Pin;

    PRKDPC                  m_Dpc;
    PKTIMER                 m_Timer;

    PVOID                   m_DmaBuffer;
    ULONG                   m_AllocatedSize;    /* what AllocateBuffer got */
    ULONG                   m_DmaBufferSize;    /* what the port uses (SetBufferSize) */

    VBAUD_LOCK              m_CountLock;
    /* Clock model: see VBAUD_TARGET_DEPTH_MS in vmbaud.h. */
    LONG64                  m_Consumed;     /* host-reported PCM bytes played */
    LONG64                  m_Sent;         /* PCM bytes shipped to the host */
    LONG64                  m_DevicePos;    /* PCM bytes the stream clock has played */
    LONG64                  m_PosFolded;    /* m_DevicePos already folded into m_Position */
    ULONG                   m_Position;     /* play cursor in the port's buffer */
    LONGLONG                m_ClockLast;    /* performance counter at the last advance */
    ULONGLONG               m_ClockFrac;    /* sub-frame remainder of the clock */
    LONGLONG                m_ClockFreq;
    LONG                    m_TrimPpm;      /* rate correction from the host's queue depth */
    LONG                    m_DepthAvg;     /* smoothed host queue depth, bytes * 16 */
    ULONG                   m_SampleRate;
    ULONG                   m_AvgBytesPerSec;

    /* Pipe: a thread receives CONSUMED; CopyTo ships PCM async. */
    PDEVICE_OBJECT          m_LowerDevice;
    PIRP                    m_ReadIrp;
    PVOID                   m_ReadBuf;
    KEVENT                  m_IoDone;
    PVOID                   m_Thread;
    volatile LONG           m_Stopping;

    VBAUD_LOCK              m_ListLock;
    VBAUD_WRITE_CTX         *m_Writes;
    LONG                    m_Outstanding;
    KEVENT                  m_WritesDrained;

    ~CMiniportWaveCyclicStream();

    NTSTATUS SendAsync(PVOID Buffer, ULONG Length, BOOLEAN Track);
    NTSTATUS PipeRead(PVOID Buffer, ULONG Length);
    NTSTATUS SendFormat(void);
    void     ParseConsumed(PVOID Buffer, ULONG Length);
    void     AdvanceClock(void);
    void     ResetClock(void);
    static NTSTATUS NTAPI IoComplete(PDEVICE_OBJECT Fdo, PIRP Irp, PVOID Context);
    static NTSTATUS NTAPI WriteComplete(PDEVICE_OBJECT Fdo, PIRP Irp, PVOID Context);
    static VOID NTAPI ThreadEntry(PVOID Context);
    void     ThreadMain(void);
    void     StartThread(void);
    void     StopThread(void);

public:
    DECLARE_STD_UNKNOWN();
    CMiniportWaveCyclicStream(PUNKNOWN other);

    IMP_IMiniportWaveCyclicStream;
    IMP_IDmaChannel;

    NTSTATUS Init(IN PCMiniportWaveCyclic Miniport, IN PKSDATAFORMAT DataFormat);

    friend class CMiniportWaveCyclic;
};
typedef CMiniportWaveCyclicStream *PCMiniportWaveCyclicStream;

#endif /* _VMBAUD_MINSTREAM_H_ */
