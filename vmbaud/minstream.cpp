/*
 * minstream.cpp: WaveCyclic stream + IDmaChannel.
 *
 * Data path: IDmaChannel::CopyTo ships the client's PCM straight to the
 * VMBus PDO as an async write (CopyTo may run at DISPATCH, so it must not
 * wait).  The DMA buffer exists only because PortCls asks for one.  XP's
 * WaveCyclic port calls CopyTo already in KSSTATE_PAUSE, while the client
 * fills the buffer before it starts the stream.
 *
 * Position: a stream clock on the performance counter at the nominal rate,
 * trimmed from the host's queue depth (see VBAUD_TARGET_DEPTH_MS).  It must
 * advance smoothly: the port keeps only about 40 ms written ahead of
 * GetPosition, and a position that jumps in steps of tens of milliseconds
 * makes kmixer drain its client and fill the gaps with silence.
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
#include "common.h"
#include "minwave.h"
#include "minstream.h"

/*
 * The counts and the write list are touched by the CONSUMED reader thread at
 * PASSIVE_LEVEL and by the port at DISPATCH_LEVEL (GetPosition, CopyTo), so
 * the lock must raise IRQL: a DPC spinning on a lock its own CPU's thread
 * holds would never get it back.
 */
static void SpinLock(VBAUD_LOCK *lock)
{
    KIRQL old;
    KeAcquireSpinLock(&lock->Lock, &old);
    lock->OldIrql = old;
}

static void SpinUnlock(VBAUD_LOCK *lock)
{
    KeReleaseSpinLock(&lock->Lock, lock->OldIrql);
}


CMiniportWaveCyclicStream::CMiniportWaveCyclicStream(PUNKNOWN other) : CUnknown(other)
{
    m_Miniport = NULL;
    m_Format16Bit = TRUE;
    m_BlockAlign = 0;
    m_KsState = KSSTATE_STOP;
    m_Pin = 0;
    m_Dpc = NULL;
    m_Timer = NULL;
    m_DmaBuffer = NULL;
    m_DmaBufferSize = 0;
    m_Consumed = 0;
    m_Sent = 0;
    m_DevicePos = 0;
    m_PosFolded = 0;
    m_Position = 0;
    m_ClockLast = 0;
    m_ClockFrac = 0;
    m_ClockFreq = 0;
    m_TrimPpm = 0;
    m_DepthAvg = 0;
    m_SampleRate = 0;
    m_AvgBytesPerSec = 0;
    m_LowerDevice = NULL;
    m_ReadIrp = NULL;
    m_ReadBuf = NULL;
    m_Thread = NULL;
    m_Stopping = 0;
    m_Outstanding = 0;
    m_Writes = NULL;
    KeInitializeEvent(&m_IoDone, NotificationEvent, FALSE);
    KeInitializeEvent(&m_WritesDrained, NotificationEvent, TRUE);
    KeInitializeSpinLock(&m_CountLock.Lock);
    KeInitializeSpinLock(&m_ListLock.Lock);
}

CMiniportWaveCyclicStream::~CMiniportWaveCyclicStream()
{
    StopThread();

    if (m_Timer) {
        KeCancelTimer(m_Timer);
        ExFreePoolWithTag(m_Timer, VBAUD_POOLTAG);
        m_Timer = NULL;
    }
    if (m_Dpc) {
        ExFreePoolWithTag(m_Dpc, VBAUD_POOLTAG);
        m_Dpc = NULL;
    }
    FreeBuffer();

    if (m_ReadIrp) {
        IoFreeIrp(m_ReadIrp);
        m_ReadIrp = NULL;
    }
    if (m_ReadBuf) {
        ExFreePoolWithTag(m_ReadBuf, VBAUD_POOLTAG);
        m_ReadBuf = NULL;
    }

    if (m_Miniport) {
        m_Miniport->m_RenderAllocated = FALSE;
        m_Miniport = NULL;
    }
}

NTSTATUS CMiniportWaveCyclicStream::Init(
    IN PCMiniportWaveCyclic Miniport,
    IN PKSDATAFORMAT DataFormat)
{
    ASSERT(Miniport);
    ASSERT(DataFormat);

    PWAVEFORMATEX wfx = GetWaveFormatEx(DataFormat);
    if (!wfx)
        return STATUS_INVALID_PARAMETER;

    m_Miniport = Miniport;
    m_BlockAlign = wfx->nBlockAlign;
    m_Format16Bit = (wfx->wBitsPerSample == 16);
    m_SampleRate = wfx->nSamplesPerSec;
    m_AvgBytesPerSec = wfx->nAvgBytesPerSec;
    m_KsState = KSSTATE_STOP;
    m_Consumed = 0;
    m_Sent = 0;

    NTSTATUS status = AllocateBuffer(m_Miniport->m_MaxDmaBufferSize, NULL);
    if (!NT_SUCCESS(status))
        return status;

    m_Dpc = (PRKDPC)ExAllocatePoolWithTag(NonPagedPool, sizeof(KDPC), VBAUD_POOLTAG);
    m_Timer = (PKTIMER)ExAllocatePoolWithTag(NonPagedPool, sizeof(KTIMER), VBAUD_POOLTAG);
    if (!m_Dpc || !m_Timer)
        return STATUS_INSUFFICIENT_RESOURCES;

    KeInitializeDpc(m_Dpc, TimerNotify, m_Miniport);
    KeInitializeTimerEx(m_Timer, NotificationTimer);

    m_LowerDevice = m_Miniport->m_AdapterCommon->GetLowerDevice();
    m_ReadBuf = ExAllocatePoolWithTag(NonPagedPool, VBAUD_READ_SIZE, VBAUD_POOLTAG);
    if (!m_LowerDevice || !m_ReadBuf)
        return STATUS_INSUFFICIENT_RESOURCES;

    m_ReadIrp = IoAllocateIrp(m_LowerDevice->StackSize, FALSE);
    if (!m_ReadIrp)
        return STATUS_INSUFFICIENT_RESOURCES;

    return SetFormat(DataFormat);
}

//=============================================================================
// IMiniportWaveCyclicStream
//=============================================================================
STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclicStream::SetFormat(IN PKSDATAFORMAT Format)
{
    PAGED_CODE();

    ASSERT(Format);
    if (m_KsState == KSSTATE_RUN)
        return STATUS_INVALID_DEVICE_REQUEST;

    /* The port passes formats through here unchecked (DirectSound tries
     * rates down to 100 Hz); a refused one leaves the stream as it was. */
    if (!NT_SUCCESS(ValidatePcmFormat(Format))) {
        DbgPrint("vmbaud: SetFormat: format refused\n");
        return STATUS_INVALID_PARAMETER;
    }
    PWAVEFORMATEX wfx = GetWaveFormatEx(Format);
    BOOLEAN changed = m_SampleRate != wfx->nSamplesPerSec
                   || m_BlockAlign != wfx->nBlockAlign;

    KeWaitForSingleObject(&m_Miniport->m_SampleRateSync, Executive, KernelMode, FALSE, NULL);
    m_BlockAlign = wfx->nBlockAlign;
    m_Format16Bit = (wfx->wBitsPerSample == 16);
    m_SampleRate = wfx->nSamplesPerSec;
    m_AvgBytesPerSec = wfx->nAvgBytesPerSec;
    m_Miniport->m_SamplingFrequency = wfx->nSamplesPerSec;
    KeReleaseMutex(&m_Miniport->m_SampleRateSync, FALSE);

    /*
     * Past STOP the host already has this stream's FORMAT, and kmixer does
     * change the format there (DirectSound opens at 48 kHz, then plays
     * 44.1 kHz): unless the host hears of it, it plays the PCM at the old
     * rate.  A FORMAT starts a new stream for the host, so here too.
     */
    if (changed && m_KsState != KSSTATE_STOP) {
        DbgPrint("vmbaud: SetFormat %lu Hz in state %d\n", m_SampleRate, m_KsState);
        SpinLock(&m_CountLock);
        ResetClock();
        SpinUnlock(&m_CountLock);
        SendFormat();
    }
    return STATUS_SUCCESS;
}

STDMETHODIMP_(ULONG) CMiniportWaveCyclicStream::SetNotificationFreq(
    IN ULONG Interval, OUT PULONG FrameSize)
{
    DbgPrint("vmbaud: SetNotificationFreq interval=%lu\n", Interval);
    PAGED_CODE();

    ASSERT(FrameSize);

    if (Interval == 0)
        Interval = VBAUD_DEFAULT_INTERVAL_MS;

    ULONG prev = m_Miniport->m_NotificationInterval;
    m_Miniport->m_NotificationInterval = Interval;

    /* Bytes processed in one notification period. */
    *FrameSize = m_BlockAlign * m_SampleRate * Interval / 1000;
    return prev;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclicStream::SetState(IN KSSTATE NewState)
{
    PAGED_CODE();

    if (m_KsState == NewState)
        return STATUS_SUCCESS;
    DbgPrint("vmbaud: SetState %d -> %d\n", m_KsState, NewState);

    switch (NewState) {
    case KSSTATE_ACQUIRE:
        /*
         * STOP -> ACQUIRE starts a stream: reset the counts and tell the
         * host, which resets its own on FORMAT.  PAUSE -> ACQUIRE is on the
         * way down and changes nothing.
         */
        if (m_KsState == KSSTATE_STOP) {
            SpinLock(&m_CountLock);
            ResetClock();
            SpinUnlock(&m_CountLock);
            SendFormat();
        }
        break;

    case KSSTATE_PAUSE:
        KeCancelTimer(m_Timer);
        /* Leaving RUN: bring the clock up to now, then it stands still. */
        SpinLock(&m_CountLock);
        if (m_KsState == KSSTATE_RUN)
            AdvanceClock();
        m_KsState = NewState;
        SpinUnlock(&m_CountLock);
        return STATUS_SUCCESS;

    case KSSTATE_RUN:
        SpinLock(&m_CountLock);
        m_ClockLast = KeQueryPerformanceCounter(NULL).QuadPart;
        m_KsState = NewState;
        SpinUnlock(&m_CountLock);
        {
            LARGE_INTEGER due;
            due.QuadPart = -10000LL * m_Miniport->m_NotificationInterval;
            KeSetTimerEx(m_Timer, due, m_Miniport->m_NotificationInterval, m_Dpc);
        }
        StartThread();
        return STATUS_SUCCESS;

    case KSSTATE_STOP:
        KeCancelTimer(m_Timer);
        StopThread();
        SpinLock(&m_CountLock);
        ResetClock();
        SpinUnlock(&m_CountLock);
        break;

    default:
        break;
    }

    m_KsState = NewState;
    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclicStream::GetPosition(OUT PULONG Position)
{
    ASSERT(Position);

    /*
     * The cursor wraps at the size the port uses (SetBufferSize), which is
     * not a power of two in general, so fold the clock's progress in with
     * 32-bit arithmetic instead of reducing the cumulative count.
     */
    SpinLock(&m_CountLock);
    if (m_KsState == KSSTATE_RUN)
        AdvanceClock();
    ULONG buf = m_DmaBufferSize;
    if (buf) {
        ULONG delta = (ULONG)(m_DevicePos - m_PosFolded) % buf;
        m_PosFolded = m_DevicePos;
        m_Position += delta;
        if (m_Position >= buf)
            m_Position -= buf;
    }
    *Position = m_Position;
    SpinUnlock(&m_CountLock);
    return STATUS_SUCCESS;
}

/*
 * Moves the stream clock to now: the nominal rate on the performance
 * counter, corrected by m_TrimPpm.  The remainder below one frame carries
 * over, so the clock neither gains nor loses over time.  It stops at what
 * has been sent, as the host cannot play data it does not have.  Called
 * with m_CountLock held.
 */
void CMiniportWaveCyclicStream::AdvanceClock(void)
{
    LARGE_INTEGER freq;
    LONGLONG now = KeQueryPerformanceCounter(&freq).QuadPart;
    LONGLONG ticks = now - m_ClockLast;

    m_ClockLast = now;
    m_ClockFreq = freq.QuadPart;
    if (ticks <= 0 || m_ClockFreq <= 0 || m_SampleRate == 0)
        return;
    if (ticks > m_ClockFreq)
        ticks = m_ClockFreq;    /* at most a second per step: no overflow */

    /* frames = ticks * rate * (1 + trim) / freq, in units of 1e-6 frame */
    ULONGLONG num = m_ClockFrac + (ULONGLONG)ticks * m_SampleRate
                    * (ULONGLONG)(1000000 + m_TrimPpm);
    ULONGLONG den = (ULONGLONG)m_ClockFreq * 1000000;
    ULONGLONG frames = num / den;
    m_ClockFrac = num - frames * den;

    LONG64 pos = m_DevicePos + (LONG64)frames * m_BlockAlign;
    if (pos > m_Sent) {
        pos = m_Sent;
        m_ClockFrac = 0;
    }
    m_DevicePos = pos;
}

/* A new stream or a stop: everything back to zero.  m_CountLock held. */
void CMiniportWaveCyclicStream::ResetClock(void)
{
    m_Consumed = 0;
    m_Sent = 0;
    m_DevicePos = 0;
    m_PosFolded = 0;
    m_Position = 0;
    m_ClockFrac = 0;
    m_TrimPpm = 0;
    m_DepthAvg = (LONG)(m_AvgBytesPerSec / 1000 * VBAUD_TARGET_DEPTH_MS * 16);
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclicStream::NormalizePhysicalPosition(
    IN OUT PLONGLONG PhysicalPosition)
{
    ASSERT(PhysicalPosition);
    if (m_BlockAlign == 0 || m_SampleRate == 0) {
        *PhysicalPosition = 0;
        return STATUS_SUCCESS;
    }
    /* bytes -> 100 ns units */
    *PhysicalPosition = (_100NS_UNITS_PER_SECOND / m_BlockAlign * *PhysicalPosition)
                        / m_SampleRate;
    return STATUS_SUCCESS;
}

STDMETHODIMP_(void) CMiniportWaveCyclicStream::Silence(
    IN PVOID Buffer, IN ULONG ByteCount)
{
    RtlFillMemory(Buffer, ByteCount, m_Format16Bit ? 0 : 0x80);
}

//=============================================================================
// IDmaChannel
//=============================================================================
STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclicStream::AllocateBuffer(
    IN ULONG BufferSize,
    IN PPHYSICAL_ADDRESS PhysicalAddressConstraint OPTIONAL)
{
    UNREFERENCED_PARAMETER(PhysicalAddressConstraint);
    PAGED_CODE();

    if (BufferSize == 0 || BufferSize > VBAUD_DMA_BUFFER_SIZE)
        BufferSize = VBAUD_DMA_BUFFER_SIZE;

    m_DmaBuffer = ExAllocatePoolWithTag(NonPagedPool, BufferSize, VBAUD_POOLTAG);
    if (!m_DmaBuffer)
        return STATUS_INSUFFICIENT_RESOURCES;
    m_AllocatedSize = BufferSize;
    m_DmaBufferSize = BufferSize;
    DbgPrint("vmbaud: AllocateBuffer %lu\n", BufferSize);
    return STATUS_SUCCESS;
}

STDMETHODIMP_(void) CMiniportWaveCyclicStream::FreeBuffer(void)
{
    PAGED_CODE();

    if (m_DmaBuffer) {
        ExFreePoolWithTag(m_DmaBuffer, VBAUD_POOLTAG);
        m_DmaBuffer = NULL;
    }
    m_DmaBufferSize = 0;
}

STDMETHODIMP_(ULONG) CMiniportWaveCyclicStream::TransferCount(void)
{
    return m_DmaBufferSize;
}

STDMETHODIMP_(ULONG) CMiniportWaveCyclicStream::MaximumBufferSize(void)
{
    return m_Miniport ? m_Miniport->m_MaxDmaBufferSize : VBAUD_DMA_BUFFER_SIZE;
}

STDMETHODIMP_(ULONG) CMiniportWaveCyclicStream::AllocatedBufferSize(void)
{
    return m_AllocatedSize;
}

STDMETHODIMP_(ULONG) CMiniportWaveCyclicStream::BufferSize(void)
{
    return m_DmaBufferSize;
}

STDMETHODIMP_(void) CMiniportWaveCyclicStream::SetBufferSize(IN ULONG BufferSize)
{
    DbgPrint("vmbaud: SetBufferSize %lu (allocated %lu)\n", BufferSize, m_AllocatedSize);
    if (BufferSize == 0 || BufferSize > m_AllocatedSize)
        return;
    SpinLock(&m_CountLock);
    m_DmaBufferSize = BufferSize;
    m_Position %= BufferSize;
    SpinUnlock(&m_CountLock);
}

STDMETHODIMP_(PVOID) CMiniportWaveCyclicStream::SystemAddress(void)
{
    return m_DmaBuffer;
}

STDMETHODIMP_(PHYSICAL_ADDRESS) CMiniportWaveCyclicStream::PhysicalAddress(void)
{
    /* MSVAD hack: no real DMA, so return the VA in a PHYSICAL_ADDRESS. */
    PHYSICAL_ADDRESS pa;
    pa.QuadPart = (LONGLONG)m_DmaBuffer;
    return pa;
}

STDMETHODIMP_(PADAPTER_OBJECT) CMiniportWaveCyclicStream::GetAdapterObject(void)
{
    return NULL;
}

STDMETHODIMP_(void) CMiniportWaveCyclicStream::CopyTo(
    IN PVOID Destination, IN PVOID Source, IN ULONG ByteCount)
{
    UNREFERENCED_PARAMETER(Destination);

    /*
     * The client's PCM arrives here; ship it to the host.  CopyTo may run at
     * DISPATCH, so the write is fire-and-forget.
     */
    if (ByteCount == 0 || ByteCount > VBAUD_MAX_MESSAGE - VBAUD_HEADER_SIZE)
        return;
    /*
     * No KSSTATE check: WaveCyclic clients fill the DMA buffer before the
     * stream reaches KSSTATE_RUN (observed on XP: every CopyTo happened in
     * PAUSE), so gating on RUN dropped every sample.
     */
    if (m_Stopping)
        return;

    ULONG total = VBAUD_HEADER_SIZE + ByteCount;
    PUCHAR msg = (PUCHAR)ExAllocatePoolWithTag(NonPagedPool, total, VBAUD_POOLTAG);
    if (!msg)
        return;

    PVBAUD_HEADER hdr = (PVBAUD_HEADER)msg;
    hdr->Type = VBAUD_MSG_PCM_OUT;
    hdr->Size = ByteCount;
    RtlCopyMemory(msg + VBAUD_HEADER_SIZE, Source, ByteCount);

    /* Count before sending: the host's CONSUMED for these bytes may be
     * parsed on another CPU before SendAsync returns. */
    SpinLock(&m_CountLock);
    m_Sent += ByteCount;
    SpinUnlock(&m_CountLock);
    NTSTATUS status = SendAsync(msg, total, FALSE);
    if (!NT_SUCCESS(status)) {
        /*
         * STATUS_DEVICE_BUSY: the host has stopped taking PCM (nothing reads
         * the pipe).  Play into the void like a card with nothing plugged in:
         * the bytes still count as sent, so the stream clock keeps running
         * and the client does not hang.
         */
        if (status != STATUS_DEVICE_BUSY) {
            SpinLock(&m_CountLock);
            m_Sent -= ByteCount;
            SpinUnlock(&m_CountLock);
        }
        ExFreePoolWithTag(msg, VBAUD_POOLTAG);
    }
}

STDMETHODIMP_(void) CMiniportWaveCyclicStream::CopyFrom(
    IN PVOID Destination, IN PVOID Source, IN ULONG ByteCount)
{
    UNREFERENCED_PARAMETER(Destination);
    UNREFERENCED_PARAMETER(Source);
    UNREFERENCED_PARAMETER(ByteCount);
}

//=============================================================================
// QueryInterface
//=============================================================================
STDMETHODIMP_(NTSTATUS) CMiniportWaveCyclicStream::NonDelegatingQueryInterface(
    IN REFIID Interface, OUT PVOID *Object)
{
    ASSERT(Object);

    if (IsEqualGUIDAligned(Interface, IID_IUnknown))
        *Object = PUNKNOWN(PMINIPORTWAVECYCLICSTREAM(this));
    else if (IsEqualGUIDAligned(Interface, IID_IMiniportWaveCyclicStream))
        *Object = PMINIPORTWAVECYCLICSTREAM(this);
    else if (IsEqualGUIDAligned(Interface, IID_IDmaChannel))
        *Object = PDMACHANNEL(this);   /* MI-adjusted this */
    else
        *Object = NULL;

    if (*Object) {
        PUNKNOWN(*Object)->AddRef();
        return STATUS_SUCCESS;
    }
    return STATUS_INVALID_PARAMETER;
}

//=============================================================================
// VMBus pipe
//=============================================================================
NTSTATUS NTAPI CMiniportWaveCyclicStream::IoComplete(
    PDEVICE_OBJECT Fdo, PIRP Irp, PVOID Context)
{
    UNREFERENCED_PARAMETER(Fdo);
    KeSetEvent((PKEVENT)Context, IO_NO_INCREMENT, FALSE);
    return STATUS_MORE_PROCESSING_REQUIRED;
}

/*
 * Async write.  Takes ownership of Buffer on success (freed in the
 * completion).  On failure Buffer is still owned by the caller.
 */
NTSTATUS CMiniportWaveCyclicStream::SendAsync(PVOID Buffer, ULONG Length, BOOLEAN Track)
{
    UNREFERENCED_PARAMETER(Track);

    if (!m_LowerDevice || m_Stopping)
        return STATUS_INVALID_DEVICE_STATE;

    PIRP irp = IoAllocateIrp(m_LowerDevice->StackSize, FALSE);
    if (!irp)
        return STATUS_INSUFFICIENT_RESOURCES;

    PMDL mdl = IoAllocateMdl(Buffer, Length, FALSE, FALSE, NULL);
    if (!mdl) {
        IoFreeIrp(irp);
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    MmBuildMdlForNonPagedPool(mdl);

    VBAUD_WRITE_CTX *ctx = (VBAUD_WRITE_CTX *)ExAllocatePoolWithTag(
        NonPagedPool, sizeof(*ctx), VBAUD_POOLTAG);
    if (!ctx) {
        IoFreeMdl(mdl);
        IoFreeIrp(irp);
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    ctx->Stream = this;
    ctx->Buffer = Buffer;
    ctx->Irp = irp;
    ctx->Refs = 1;
    ctx->Cancelled = FALSE;

    irp->MdlAddress = mdl;
    PIO_STACK_LOCATION sp = IoGetNextIrpStackLocation(irp);
    sp->MajorFunction = IRP_MJ_WRITE;
    sp->Parameters.Write.Length = Length;
    IoSetCompletionRoutine(irp, WriteComplete, ctx, TRUE, TRUE, TRUE);

    /* Publish the IRP so StopThread can cancel it. */
    SpinLock(&m_ListLock);
    if (m_Stopping || m_Outstanding >= VBAUD_MAX_OUTSTANDING) {
        NTSTATUS status = m_Stopping ? STATUS_CANCELLED : STATUS_DEVICE_BUSY;
        SpinUnlock(&m_ListLock);
        ExFreePoolWithTag(ctx, VBAUD_POOLTAG);
        IoFreeMdl(mdl);
        IoFreeIrp(irp);
        return status;
    }
    ctx->Link = m_Writes;
    m_Writes = ctx;
    m_Outstanding++;
    KeClearEvent(&m_WritesDrained);
    SpinUnlock(&m_ListLock);

    IoCallDriver(m_LowerDevice, irp);
    return STATUS_SUCCESS;
}

NTSTATUS NTAPI CMiniportWaveCyclicStream::WriteComplete(
    PDEVICE_OBJECT Fdo, PIRP Irp, PVOID Context)
{
    UNREFERENCED_PARAMETER(Fdo);

    VBAUD_WRITE_CTX *ctx = (VBAUD_WRITE_CTX *)Context;
    CMiniportWaveCyclicStream *self = ctx->Stream;

    /* Unlink from the in-flight list. */
    SpinLock(&self->m_ListLock);
    VBAUD_WRITE_CTX **link = &self->m_Writes;
    while (*link) {
        if (*link == ctx) {
            *link = ctx->Link;
            break;
        }
        link = &(*link)->Link;
    }
    self->m_Outstanding--;
    if (self->m_Outstanding == 0)
        KeSetEvent(&self->m_WritesDrained, IO_NO_INCREMENT, FALSE);
    SpinUnlock(&self->m_ListLock);

    UNREFERENCED_PARAMETER(Irp);
    ReleaseWrite(ctx);
    return STATUS_MORE_PROCESSING_REQUIRED;
}

/* Frees a write when its last reference goes: the IRP's or a canceller's. */
void CMiniportWaveCyclicStream::ReleaseWrite(VBAUD_WRITE_CTX *Ctx)
{
    if (InterlockedDecrement(&Ctx->Refs) != 0)
        return;
    ExFreePoolWithTag(Ctx->Buffer, VBAUD_POOLTAG);
    IoFreeMdl(Ctx->Irp->MdlAddress);
    IoFreeIrp(Ctx->Irp);
    ExFreePoolWithTag(Ctx, VBAUD_POOLTAG);
}

/*
 * Cancels the in-flight writes.  vmbus.sys completes a cancelled pipe write
 * inside IoCancelIrp, and the completion routine takes m_ListLock, so the
 * writes are picked under the lock, each with a reference that keeps it
 * allocated, and cancelled after the lock is released.
 */
void CMiniportWaveCyclicStream::CancelWrites(void)
{
    for (;;) {
        VBAUD_WRITE_CTX *batch[16];
        ULONG n = 0;

        SpinLock(&m_ListLock);
        for (VBAUD_WRITE_CTX *ctx = m_Writes; ctx && n < 16; ctx = ctx->Link) {
            if (ctx->Cancelled)
                continue;
            ctx->Cancelled = TRUE;
            InterlockedIncrement(&ctx->Refs);
            batch[n++] = ctx;
        }
        SpinUnlock(&m_ListLock);
        if (n == 0)
            return;
        for (ULONG i = 0; i < n; i++) {
            IoCancelIrp(batch[i]->Irp);
            ReleaseWrite(batch[i]);
        }
    }
}

NTSTATUS CMiniportWaveCyclicStream::PipeRead(PVOID Buffer, ULONG Length)
{
    if (!m_LowerDevice || !m_ReadIrp || m_Stopping)
        return STATUS_INVALID_DEVICE_STATE;

    PMDL mdl = IoAllocateMdl(Buffer, Length, FALSE, FALSE, NULL);
    if (!mdl)
        return STATUS_INSUFFICIENT_RESOURCES;
    MmBuildMdlForNonPagedPool(mdl);

    IoReuseIrp(m_ReadIrp, STATUS_SUCCESS);
    m_ReadIrp->MdlAddress = mdl;
    PIO_STACK_LOCATION sp = IoGetNextIrpStackLocation(m_ReadIrp);
    sp->MajorFunction = IRP_MJ_READ;
    sp->Parameters.Read.Length = Length;
    IoSetCompletionRoutine(m_ReadIrp, IoComplete, &m_IoDone, TRUE, TRUE, TRUE);
    KeClearEvent(&m_IoDone);

    if (m_Stopping) {
        m_ReadIrp->MdlAddress = NULL;
        IoFreeMdl(mdl);
        return STATUS_CANCELLED;
    }

    IoCallDriver(m_LowerDevice, m_ReadIrp);
    KeWaitForSingleObject(&m_IoDone, Executive, KernelMode, FALSE, NULL);

    NTSTATUS status = m_ReadIrp->IoStatus.Status;
    ULONG done = (ULONG)m_ReadIrp->IoStatus.Information;
    m_ReadIrp->MdlAddress = NULL;
    IoFreeMdl(mdl);

    if (NT_SUCCESS(status))
        ParseConsumed(Buffer, done);
    return status;
}

void CMiniportWaveCyclicStream::ParseConsumed(PVOID Buffer, ULONG Length)
{
    if (Length < VBAUD_HEADER_SIZE + sizeof(VBAUD_CONSUMED))
        return;

    PVBAUD_HEADER hdr = (PVBAUD_HEADER)Buffer;
    if (hdr->Type != VBAUD_MSG_CONSUMED || hdr->Size < sizeof(VBAUD_CONSUMED))
        return;
    PVBAUD_CONSUMED c = (PVBAUD_CONSUMED)((PUCHAR)Buffer + VBAUD_HEADER_SIZE);

    SpinLock(&m_CountLock);
    /*
     * A count for this stream can neither go back nor exceed what the stream
     * has sent; anything else is a leftover of the previous stream still
     * queued in the pipe.
     */
    if ((LONG64)c->Played < m_Consumed || (LONG64)c->Played > m_Sent) {
        SpinUnlock(&m_CountLock);
        return;
    }
    m_Consumed = (LONG64)c->Played;

    /*
     * Rate trim, proportional to how far the host's queue (smoothed over
     * about 16 reports) is from the target: a full queue means this clock
     * runs fast against the host's sound card, so slow it down.
     */
    LONG queued = c->Queued > 0x7FFFFF ? 0x7FFFFF : (LONG)c->Queued;  /* x16 fits a LONG */
    m_DepthAvg += queued - m_DepthAvg / 16;
    LONG target = (LONG)(m_AvgBytesPerSec / 1000 * VBAUD_TARGET_DEPTH_MS);
    if (target > 0) {
        LONG err = m_DepthAvg / 16 - target;
        LONGLONG trim = -(LONGLONG)err * VBAUD_MAX_TRIM_PPM / target;
        if (trim > VBAUD_MAX_TRIM_PPM)
            trim = VBAUD_MAX_TRIM_PPM;
        if (trim < -VBAUD_MAX_TRIM_PPM)
            trim = -VBAUD_MAX_TRIM_PPM;
        if (m_KsState == KSSTATE_RUN)
            AdvanceClock();     /* the old rate up to now, the new one after */
        m_TrimPpm = (LONG)trim;
    }
    SpinUnlock(&m_CountLock);

    static LONG s_Count;
    LONG n = InterlockedIncrement(&s_Count);
    if ((n % 100) == 0)
        DbgPrint("vmbaud: host played %I64u queued %lu, avg %ld, trim %ld ppm\n",
                 c->Played, c->Queued, m_DepthAvg / 16, m_TrimPpm);
}

NTSTATUS CMiniportWaveCyclicStream::SendFormat(void)
{
    if (!m_LowerDevice)
        return STATUS_INVALID_DEVICE_STATE;

    ULONG total = VBAUD_HEADER_SIZE + sizeof(VBAUD_FORMAT);
    PUCHAR msg = (PUCHAR)ExAllocatePoolWithTag(NonPagedPool, total, VBAUD_POOLTAG);
    if (!msg)
        return STATUS_INSUFFICIENT_RESOURCES;

    PVBAUD_HEADER hdr = (PVBAUD_HEADER)msg;
    hdr->Type = VBAUD_MSG_FORMAT;
    hdr->Size = sizeof(VBAUD_FORMAT);
    PVBAUD_FORMAT fmt = (PVBAUD_FORMAT)(msg + VBAUD_HEADER_SIZE);
    fmt->Rate = m_SampleRate;
    fmt->Channels = m_BlockAlign ? (m_BlockAlign / (m_Format16Bit ? 2 : 1)) : 2;
    fmt->Bits = m_Format16Bit ? 16 : 8;

    NTSTATUS status = SendAsync(msg, total, TRUE);
    if (!NT_SUCCESS(status)) {
        ExFreePoolWithTag(msg, VBAUD_POOLTAG);
        return status;
    }

    return STATUS_SUCCESS;
}

//=============================================================================
// CONSUMED receiver thread (vmbecho pattern)
//=============================================================================
VOID NTAPI CMiniportWaveCyclicStream::ThreadEntry(PVOID Context)
{
    ((CMiniportWaveCyclicStream *)Context)->ThreadMain();
    PsTerminateSystemThread(STATUS_SUCCESS);
}

void CMiniportWaveCyclicStream::ThreadMain(void)
{
    while (!m_Stopping) {
        NTSTATUS status = PipeRead(m_ReadBuf, VBAUD_READ_SIZE);
        if (!NT_SUCCESS(status))
            break;      /* host closed the pipe or the device went away */
    }
}

void CMiniportWaveCyclicStream::StartThread(void)
{
    if (m_Thread)
        return;

    m_Stopping = 0;
    HANDLE handle;
    NTSTATUS status = PsCreateSystemThread(
        &handle, THREAD_ALL_ACCESS, NULL, NULL, NULL, ThreadEntry, this);
    if (!NT_SUCCESS(status))
        return;

    status = ObReferenceObjectByHandle(
        handle, THREAD_ALL_ACCESS, NULL, KernelMode, &m_Thread, NULL);
    ZwClose(handle);
    if (!NT_SUCCESS(status))
        m_Thread = NULL;
}

void CMiniportWaveCyclicStream::StopThread(void)
{
    LARGE_INTEGER step;

    /* No new writes after this: SendAsync checks it under m_ListLock. */
    InterlockedExchange(&m_Stopping, 1);
    step.QuadPart = -100 * 10000;   /* 100 ms */

    if (m_Thread) {
        do {
            if (m_ReadIrp)
                IoCancelIrp(m_ReadIrp);
            CancelWrites();
        } while (KeWaitForSingleObject(m_Thread, Executive, KernelMode, FALSE, &step)
                 == STATUS_TIMEOUT);
        ObDereferenceObject(m_Thread);
        m_Thread = NULL;
    }

    /* Drain the writes so teardown does not free the stream under them. */
    while (KeWaitForSingleObject(&m_WritesDrained, Executive, KernelMode, FALSE, &step)
           == STATUS_TIMEOUT)
        CancelWrites();
}
