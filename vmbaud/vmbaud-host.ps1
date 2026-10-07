# vmbaud-host.ps1 - host side of the virtual sound card (run elevated on the Hyper-V host).
#
# Offers a VMBus pipe channel to a running VM with vmbuspiper.dll!VmbusPipeServerOfferChannel
# (same transport as vmbecho-host.ps1), then plays the PCM the guest streams out and
# answers with periodic VBAUD_MSG_CONSUMED notices so the guest's playback position
# can advance.  Closing the handle at the end rescinds the offer, and the guest
# removes the device.
#
#   vmbaud-host.ps1 -VMName <name> [-Seconds 0] [-Volume 100] [-LogFile out.wav]
#                    [-ConnectSeconds 30] [-DeviceType <guid>] [-Instance <guid>]
#                    [-PipeMode 0]
#
# Protocol (one pipe write = one message; 8-byte little-endian header):
#   struct vmbaud_header { uint32_t type; uint32_t size; }  /* size = payload bytes */
#   type 1 VBAUD_MSG_FORMAT    guest -> host   u32 rate, u32 channels, u32 bits
#   type 2 VBAUD_MSG_PCM_OUT   guest -> host   raw interleaved PCM, size bytes
#   type 3 VBAUD_MSG_CONSUMED  host -> guest   u64 played, u32 queued  (this stream)
# The guest's play position runs on its own clock at the nominal rate; every
# 10 ms we send CONSUMED with the bytes played and the bytes still queued here,
# and the guest trims its rate (by at most 0.5%) to keep our queue near 60 ms.
# So our sound card sets the long-term pace while the guest's position stays
# smooth.  Each FORMAT starts a new stream and resets the counts to 0.
#
# The offer (0xAC bytes, layout from vmbuspiper.dll): VM GUID at 0x00, interrupt latency
# (ms) at 0x10, interface type at 0x14, interface instance at 0x24, interface revision at
# 0x34, MMIO megabytes at 0x38, flags at 0x3A, 112 user-defined bytes at 0x3C.  The guest
# sees the device as VMBUS\{interface type}; keeping the instance GUID fixed keeps the
# guest's device node, so the driver is installed only once.
#
# Assumptions that cannot be verified here (no guest VM in this environment):
#   - The guest writes one protocol message per pipe write and a host ReadFile
#     returns one such message (the parser still walks coalesced messages).
#   - FORMAT arrives once per playback stream, before any PCM_OUT; each FORMAT
#     is a new stream and resets CONSUMED to 0.  A later FORMAT with a different
#     rate/channels/bits reopens waveOut.  PCM_OUT before any FORMAT is dropped.
#   - PCM is interleaved little-endian packed samples (nBlockAlign = channels *
#     bits/8), i.e. stock WAVE_FORMAT_PCM.
#   - The guest tolerates CONSUMED lagging real playback by tens of milliseconds
#     and stops writing when received-but-unCONSUMED bytes get large.  Too-eager
#     CONSUMED makes the guest refill early (underrun risk); never sending it
#     stalls the guest's writer.
#   - Guest disconnect surfaces as a failed ReadFile/GetOverlappedResult with
#     ERROR_BROKEN_PIPE (109) or ERROR_PIPE_NOT_CONNECTED (233).
#   - One PCM_OUT is at most a few hundred KB; messages larger than the 1 MB
#     read buffer are dropped with a log line rather than reassembled.
#
# Distributed under the MS-PL; see LICENSE in this directory.
param([Parameter(Mandatory)][string]$VMName,
      [string]$DeviceType = '8b57f4e3-2a3c-4f6e-9c8d-1e5a70b9c4d2',
      [string]$Instance = '2a7f3e10-9c4d-4b8a-a6e5-7d1c0f3b8e62',
      [int]$Seconds = 0,
      [ValidateRange(0, 100)][int]$Volume = 100,
      [string]$LogFile = '',
      [int]$ConnectSeconds = 30,
      [uint32]$PipeMode = 0)
$ErrorActionPreference = 'Stop'

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this elevated: VmbusPipeServerOfferChannel fails with ERROR_ACCESS_DENIED otherwise.'
}

if (-not ('VmbAud' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class VmbAud {
    const uint FILE_FLAG_OVERLAPPED = 0x40000000;
    const int ERROR_IO_PENDING = 997;
    const int ERROR_BROKEN_PIPE = 109;
    const int ERROR_PIPE_NOT_CONNECTED = 233;

    const uint WAVE_MAPPER = 0xFFFFFFFF;
    const uint CALLBACK_NULL = 0;
    const uint WAVE_FORMAT_PCM = 1;
    const int WHDR_DONE = 0x01;
    const uint TIME_MS = 1;
    const uint TIME_SAMPLES = 2;
    const uint TIME_BYTES = 4;

    const uint VBAUD_MSG_FORMAT = 1;
    const uint VBAUD_MSG_PCM_OUT = 2;
    const uint VBAUD_MSG_CONSUMED = 3;

    const int NumSlots = 16;
    const int SlotCapacity = 65536;
    // The guest sends small PCM messages (a few ms each).  They are joined
    // into ChunkMs buffers for waveOut, and playback starts only once
    // PrebufferMs is queued (again after every underrun), unless the guest
    // stops sending for StallMs: then it is waiting for CONSUMED, and we play
    // what we have.
    const int ChunkMs = 20;
    const int PrebufferMs = 60;
    const int StallMs = 30;
    const int LoopMaxWaitMs = 10;
    const int ReadBufSize = 1048576;
    const int ConsumedIntervalMs = 10;
    const int IdleMs = 100;
    const int StatusIntervalMs = 5000;
    const int ConsumedEveryNMsgs = 16;
    const int SlotWaitMs = 2000;
    const int MmTimeSize = 12;          // sizeof(MMTIME)

    [DllImport("vmbuspiper.dll", SetLastError = true)]
    static extern IntPtr VmbusPipeServerOfferChannel(byte[] offer, uint openMode, uint pipeMode);
    [DllImport("vmbuspiper.dll", SetLastError = true)]
    static extern bool VmbusPipeServerConnectPipe(IntPtr pipe, IntPtr overlapped);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ReadFile(IntPtr h, IntPtr buf, uint len, IntPtr read, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool WriteFile(IntPtr h, byte[] buf, uint len, IntPtr written, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetOverlappedResult(IntPtr h, IntPtr ov, out uint n, bool wait);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CancelIoEx(IntPtr h, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")]
    static extern void RtlMoveMemory(IntPtr dest, IntPtr src, IntPtr length);

    [DllImport("winmm.dll")]
    static extern int waveOutGetNumDevs();
    [DllImport("winmm.dll", CharSet = CharSet.Ansi)]
    static extern int waveOutGetDevCaps(uint uDeviceID, out WAVEOUTCAPS pwoc, uint cbwoc);
    [DllImport("winmm.dll")]
    static extern int waveOutOpen(out IntPtr phwo, uint uDeviceID, IntPtr pwfx,
                                  IntPtr dwCallback, IntPtr dwInstance, uint fdwOpen);
    [DllImport("winmm.dll")]
    static extern int waveOutPrepareHeader(IntPtr hwo, IntPtr pwh, uint cbwh);
    [DllImport("winmm.dll")]
    static extern int waveOutWrite(IntPtr hwo, IntPtr pwh, uint cbwh);
    [DllImport("winmm.dll")]
    static extern int waveOutUnprepareHeader(IntPtr hwo, IntPtr pwh, uint cbwh);
    [DllImport("winmm.dll")]
    static extern int waveOutClose(IntPtr hwo);
    [DllImport("winmm.dll")]
    static extern int waveOutReset(IntPtr hwo);
    [DllImport("winmm.dll")]
    static extern int waveOutPause(IntPtr hwo);
    [DllImport("winmm.dll")]
    static extern int waveOutRestart(IntPtr hwo);
    [DllImport("winmm.dll")]
    static extern int waveOutSetVolume(IntPtr hwo, uint dwVolume);
    [DllImport("winmm.dll")]
    static extern int waveOutGetPosition(IntPtr hwo, IntPtr pmmt, uint cbmmt);

    // WAVEHDR: lpData, dwBufferLength, dwBytesRecorded, dwUser, dwFlags, dwLoops,
    // lpNext, reserved.  IntPtr for the pointer fields keeps the layout correct
    // on both x86 (32 bytes) and x64 (48 bytes).
    [StructLayout(LayoutKind.Sequential)]
    public struct WAVEHDR {
        public IntPtr lpData;
        public uint dwBufferLength;
        public uint dwBytesRecorded;
        public IntPtr dwUser;
        public uint dwFlags;
        public uint dwLoops;
        public IntPtr lpNext;
        public IntPtr reserved;
    }

    // WAVEFORMATEX is 18 bytes with cbSize; Pack=2 avoids trailing padding that
    // would make waveOutOpen read a garbage cbSize.
    [StructLayout(LayoutKind.Sequential, Pack = 2)]
    public struct WAVEFORMATEX {
        public ushort wFormatTag;
        public ushort nChannels;
        public uint nSamplesPerSec;
        public uint nAvgBytesPerSec;
        public ushort nBlockAlign;
        public ushort wBitsPerSample;
        public ushort cbSize;
    }

    // MMTIME is 12 bytes: UINT wType + an 8-byte union.  We only need TIME_BYTES
    // (u.cb) and fall back to TIME_SAMPLES / TIME_MS if the driver refuses it.
    [StructLayout(LayoutKind.Sequential)]
    public struct MMTIME {
        public uint wType;
        public uint cb;
        public uint pad;
    }

    [StructLayout(LayoutKind.Sequential, Pack = 2, CharSet = CharSet.Ansi)]
    public struct WAVEOUTCAPS {
        public ushort wMid;
        public ushort wPid;
        public uint vDriverVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
        public string szPname;
        public uint dwFormats;
        public ushort wChannels;
        public uint dwSupport;
    }

    sealed class Op {
        public readonly IntPtr Ov = Marshal.AllocHGlobal(32);
        public readonly ManualResetEvent Done = new ManualResetEvent(false);
        public void Reset() {
            for (int i = 0; i < 32; i++) Marshal.WriteByte(Ov, i, 0);
            Done.Reset();
            Marshal.WriteIntPtr(Ov, 24, Done.SafeWaitHandle.DangerousGetHandle());
        }
        // Started (or finished at once, which also sets the event): true.
        public bool Check(bool ok) {
            return ok || Marshal.GetLastWin32Error() == ERROR_IO_PENDING;
        }
    }

    // One waveOut buffer: a prepared-or-free WAVEHDR plus the PCM it points at.
    sealed class Slot {
        public IntPtr Hdr;
        public IntPtr Data;
        public bool InFlight;
    }

    static void Log(string s) {
        Console.WriteLine(DateTime.Now.ToString("HH:mm:ss.fff") + " " + s);
        Console.Out.Flush();
    }

    static void PutU32(byte[] b, int off, uint v) {
        b[off] = (byte)v;
        b[off + 1] = (byte)(v >> 8);
        b[off + 2] = (byte)(v >> 16);
        b[off + 3] = (byte)(v >> 24);
    }

    static void PutU64(byte[] b, int off, ulong v) {
        PutU32(b, off, (uint)v);
        PutU32(b, off + 4, (uint)(v >> 32));
    }

    static uint GetU32At(IntPtr p, int off) {
        return (uint)Marshal.ReadInt32(p, off);
    }

    static IntPtr AddPtr(IntPtr p, int off) {
        return new IntPtr(p.ToInt64() + off);
    }

    // Minimal WAV writer: 44-byte PCM header, sizes back-filled on close.
    sealed class WavLog {
        FileStream Fs;
        BinaryWriter W;

        public bool Open(string path, uint rate, uint channels, uint bits) {
            try {
                Fs = new FileStream(path, FileMode.Create, FileAccess.Write);
                W = new BinaryWriter(Fs);
            } catch (Exception e) {
                Log("wav open failed: " + e.Message);
                Fs = null;
                return false;
            }
            uint byteRate = rate * channels * (bits / 8);
            ushort blockAlign = (ushort)(channels * (bits / 8));
            W.Write((uint)0x46464952);          // 'RIFF'
            W.Write((uint)0);                   // riff size, patched later
            W.Write((uint)0x45564157);          // 'WAVE'
            W.Write((uint)0x20746D66);          // 'fmt '
            W.Write((uint)16);
            W.Write((ushort)1);                 // WAVE_FORMAT_PCM
            W.Write((ushort)channels);
            W.Write(rate);
            W.Write(byteRate);
            W.Write(blockAlign);
            W.Write((ushort)bits);
            W.Write((uint)0x61746164);          // 'data'
            W.Write((uint)0);                   // data size, patched later
            return true;
        }

        public void Append(IntPtr src, int n) {
            if (Fs == null || n <= 0) return;
            byte[] tmp = new byte[n];
            Marshal.Copy(src, tmp, 0, n);
            W.Write(tmp);
        }

        public void Close() {
            if (Fs == null) return;
            long end = Fs.Length;
            Fs.Seek(4, SeekOrigin.Begin);
            W.Write((uint)(end - 8));
            Fs.Seek(40, SeekOrigin.Begin);
            W.Write((uint)(end - 44));
            W.Close();
            Fs.Close();
            Fs = null;
        }
    }

    // Ring of WAVEHDRs plus the PCM each one owns.  The buffers stay allocated
    // for the lifetime of the stream: waveOutWrite does not copy the data.
    sealed class Wave {
        public bool Open;
        IntPtr Dev;
        Slot[] Slots;
        int HdrSize;
        uint Rate, Channels, Bits, AvgBytes;
        ulong PosBase;
        uint LastRaw;
        bool LastRawValid;
        int InFlight;
        // Staging buffer that joins small PCM messages, and the prebuffer state.
        IntPtr Stage;
        int StageLen;
        DateTime LastIn;
        bool Primed;
        int PausedQueued;
        int ChunkBytes, PrebufferBytes;

        public bool IsOpen { get { return Open; } }

        void AllocSlots() {
            Slots = new Slot[NumSlots];
            HdrSize = Marshal.SizeOf(typeof(WAVEHDR));
            for (int i = 0; i < NumSlots; i++) {
                Slot s = new Slot();
                s.Hdr = Marshal.AllocHGlobal(HdrSize);
                s.Data = Marshal.AllocHGlobal(SlotCapacity);
                s.InFlight = false;
                ZeroHdr(s);
                Slots[i] = s;
            }
        }

        void ZeroHdr(Slot s) {
            for (int i = 0; i < HdrSize; i++) Marshal.WriteByte(s.Hdr, i, 0);
        }

        public bool OpenDevice(uint rate, uint channels, uint bits, int volume) {
            if (Slots == null) AllocSlots();
            Rate = rate;
            Channels = channels;
            Bits = bits;
            AvgBytes = rate * channels * (bits / 8);

            WAVEFORMATEX fmt = new WAVEFORMATEX();
            fmt.wFormatTag = (ushort)WAVE_FORMAT_PCM;
            fmt.nChannels = (ushort)channels;
            fmt.nSamplesPerSec = rate;
            fmt.nAvgBytesPerSec = AvgBytes;
            fmt.nBlockAlign = (ushort)(channels * (bits / 8));
            fmt.wBitsPerSample = (ushort)bits;
            fmt.cbSize = 0;
            IntPtr pfmt = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(WAVEFORMATEX)));
            IntPtr dev;
            int rc;
            try {
                Marshal.StructureToPtr(fmt, pfmt, false);
                rc = waveOutOpen(out dev, WAVE_MAPPER, pfmt, IntPtr.Zero, IntPtr.Zero,
                                 CALLBACK_NULL);
            } finally {
                Marshal.FreeHGlobal(pfmt);
            }
            if (rc != 0) {
                Log("waveOutOpen failed, mmresult " + rc);
                return false;
            }
            Dev = dev;
            Open = true;
            PosBase = 0;
            LastRaw = 0;
            LastRawValid = false;
            InFlight = 0;
            for (int i = 0; i < NumSlots; i++) {
                Slots[i].InFlight = false;
                ZeroHdr(Slots[i]);
            }
            SetVolume(volume);
            uint block = channels * (bits / 8);
            ChunkBytes = (int)(AvgBytes * ChunkMs / 1000 / block * block);
            PrebufferBytes = (int)(AvgBytes * PrebufferMs / 1000 / block * block);
            if (Stage == IntPtr.Zero) Stage = Marshal.AllocHGlobal(SlotCapacity);
            StageLen = 0;
            LastIn = DateTime.Now;
            StartPaused();
            Log("waveOut opened, " + rate + " Hz " + channels + " ch " + bits + " bit, "
                + ChunkMs + " ms chunks, " + PrebufferMs + " ms prebuffer");
            return true;
        }

        // Holds playback until PrebufferBytes are queued (or the guest stalls).
        void StartPaused() {
            waveOutPause(Dev);
            Primed = false;
            PausedQueued = 0;
        }

        void StartPlaying() {
            waveOutRestart(Dev);
            Primed = true;
        }

        // Queues PCM: joins it into the staging buffer and hands full chunks to
        // waveOut.  Returns the number of bytes dropped (queue full).
        public int Enqueue(IntPtr src, int n) {
            if (!Open) return 0;
            int dropped = 0, off = 0;
            LastIn = DateTime.Now;
            while (off < n) {
                int take = Math.Min(n - off, SlotCapacity - StageLen);
                RtlMoveMemory(AddPtr(Stage, StageLen), AddPtr(src, off), (IntPtr)take);
                StageLen += take;
                off += take;
                if (StageLen >= ChunkBytes || StageLen == SlotCapacity)
                    dropped += SubmitStage();
            }
            return dropped;
        }

        int SubmitStage() {
            if (StageLen == 0) return 0;
            int len = StageLen;
            StageLen = 0;
            Slot slot = Acquire(SlotWaitMs);
            if (slot == null) {
                Log("waveOut queue full, dropping " + len + " bytes");
                return len;
            }
            if (!Submit(slot, Stage, len)) return len;
            if (!Primed) {
                PausedQueued += len;
                if (PausedQueued >= PrebufferBytes) StartPlaying();
            }
            return 0;
        }

        // Called from the main loop: flushes a partial chunk and starts
        // playback when the guest has stopped sending for StallMs.
        public int Tick() {
            if (!Open) return 0;
            if ((DateTime.Now - LastIn).TotalMilliseconds < StallMs) return 0;
            int dropped = SubmitStage();
            if (!Primed && InFlight > 0) StartPlaying();
            return dropped;
        }

        public void SetVolume(int volume) {
            if (!Open) return;
            if (volume < 0) volume = 0;
            if (volume > 100) volume = 100;
            uint ch = (uint)(volume * 0xFFFF / 100);
            waveOutSetVolume(Dev, ch | (ch << 16));
        }

        // Reclaims headers the driver is done with.  Returns 1 if the queue
        // drained to empty while data was in flight (a playback gap / underrun).
        public int Poll() {
            if (!Open) return 0;
            bool wasBusy = InFlight > 0;
            for (int i = 0; i < NumSlots; i++) {
                if (!Slots[i].InFlight) continue;
                WAVEHDR h = (WAVEHDR)Marshal.PtrToStructure(Slots[i].Hdr, typeof(WAVEHDR));
                if ((h.dwFlags & WHDR_DONE) == 0) continue;
                waveOutUnprepareHeader(Dev, Slots[i].Hdr, (uint)HdrSize);
                Slots[i].InFlight = false;
                InFlight--;
            }
            if (Primed && wasBusy && InFlight == 0) {
                StartPaused();
                return 1;
            }
            return 0;
        }

        // Finds a free slot, waiting for playback to drain if the ring is full.
        // Returns null on timeout (or when waveOut is not open) so the caller
        // can drop rather than hang.
        public Slot Acquire(int timeoutMs) {
            if (!Open || Slots == null) return null;
            DateTime end = DateTime.Now.AddMilliseconds(timeoutMs);
            for (;;) {
                Poll();
                for (int i = 0; i < NumSlots; i++) {
                    if (!Slots[i].InFlight) {
                        Slots[i].InFlight = true;
                        InFlight++;
                        return Slots[i];
                    }
                }
                if (DateTime.Now >= end) return null;
                Thread.Sleep(5);
            }
        }

        // Fills one slot from src and gives it to the driver.
        public bool Submit(Slot s, IntPtr src, int n) {
            if (n > SlotCapacity) n = SlotCapacity;
            if (n <= 0) {
                s.InFlight = false;
                InFlight--;
                return true;
            }
            RtlMoveMemory(s.Data, src, (IntPtr)n);
            ZeroHdr(s);
            WAVEHDR h = new WAVEHDR();
            h.lpData = s.Data;
            h.dwBufferLength = (uint)n;
            h.dwFlags = 0;
            Marshal.StructureToPtr(h, s.Hdr, false);
            int rc = waveOutPrepareHeader(Dev, s.Hdr, (uint)HdrSize);
            if (rc != 0) {
                Log("waveOutPrepareHeader failed, mmresult " + rc);
                s.InFlight = false;
                InFlight--;
                return false;
            }
            rc = waveOutWrite(Dev, s.Hdr, (uint)HdrSize);
            if (rc != 0) {
                Log("waveOutWrite failed, mmresult " + rc);
                waveOutUnprepareHeader(Dev, s.Hdr, (uint)HdrSize);
                s.InFlight = false;
                InFlight--;
                return false;
            }
            return true;
        }

        // Playback position in bytes for the current stream, unwrapped to 64-bit.
        // PCM streams get a byte offset from the driver; other unit answers are
        // converted with the stream's own rate so CONSUMED stays meaningful.
        public ulong PlayedBytes() {
            if (!Open) return 0;
            IntPtr p = Marshal.AllocHGlobal(MmTimeSize);
            uint raw = 0;
            try {
                MMTIME mt = new MMTIME();
                mt.wType = TIME_BYTES;
                Marshal.StructureToPtr(mt, p, false);
                if (waveOutGetPosition(Dev, p, MmTimeSize) != 0) return 0;
                mt = (MMTIME)Marshal.PtrToStructure(p, typeof(MMTIME));
                if (mt.wType == TIME_SAMPLES) raw = mt.cb * (uint)(Channels * (Bits / 8));
                else if (mt.wType == TIME_MS) raw = (uint)((ulong)mt.cb * AvgBytes / 1000);
                else raw = mt.cb;
            } finally {
                Marshal.FreeHGlobal(p);
            }
            if (!LastRawValid) {
                LastRaw = raw;
                LastRawValid = true;
                return raw;
            }
            if (raw < LastRaw) PosBase += 0x100000000UL;
            LastRaw = raw;
            return PosBase + raw;
        }

        // Drops anything still queued without closing the device: used when the
        // guest starts a new stream with the same format.
        public void ResetPlayback() {
            if (!Open) return;
            waveOutReset(Dev);
            Poll();
            for (int i = 0; i < NumSlots; i++) {
                if (Slots[i].InFlight) {
                    waveOutUnprepareHeader(Dev, Slots[i].Hdr, (uint)HdrSize);
                    Slots[i].InFlight = false;
                }
            }
            InFlight = 0;
            PosBase = 0;
            LastRaw = 0;
            LastRawValid = false;
            StageLen = 0;
            StartPaused();
        }

        public void CloseDevice() {
            if (!Open) return;
            waveOutReset(Dev);
            Poll();
            for (int i = 0; i < NumSlots; i++) {
                if (Slots[i].InFlight) {
                    waveOutUnprepareHeader(Dev, Slots[i].Hdr, (uint)HdrSize);
                    Slots[i].InFlight = false;
                }
            }
            InFlight = 0;
            StageLen = 0;
            waveOutClose(Dev);
            Dev = IntPtr.Zero;
            Open = false;
            Log("waveOut closed");
        }

        public void Free() {
            CloseDevice();
            if (Slots == null) return;
            for (int i = 0; i < NumSlots; i++) {
                if (Slots[i] != null) {
                    if (Slots[i].Hdr != IntPtr.Zero) Marshal.FreeHGlobal(Slots[i].Hdr);
                    if (Slots[i].Data != IntPtr.Zero) Marshal.FreeHGlobal(Slots[i].Data);
                }
            }
            Slots = null;
            if (Stage != IntPtr.Zero) {
                Marshal.FreeHGlobal(Stage);
                Stage = IntPtr.Zero;
            }
        }
    }

    static volatile bool Stop;

    // Wall-clock stand-in for waveOutGetPosition when there is no device.
    // Reports the PCM that should have played by now, minus a small pipeline
    // delay so the guest does not think the first chunk vanished instantly.
    static ulong EstimatePlayed(ulong submitted, DateTime streamStart,
                                uint rate, uint channels, uint bits) {
        if (submitted == 0 || streamStart == DateTime.MinValue) return 0;
        ulong avg = (ulong)rate * channels * (bits / 8);
        if (avg == 0) return 0;
        double elapsedMs = (DateTime.Now - streamStart).TotalMilliseconds - 50.0;
        if (elapsedMs < 0) return 0;
        ulong played = (ulong)(elapsedMs * (double)avg / 1000.0);
        return played > submitted ? submitted : played;
    }

    // CONSUMED: u64 bytes played, u32 bytes received but not yet played.  The
    // guest runs its own clock and uses the queue depth to trim its rate.
    static bool SendConsumed(IntPtr h, Op write, ulong played, ulong queued) {
        byte[] msg = new byte[20];
        PutU32(msg, 0, VBAUD_MSG_CONSUMED);
        PutU32(msg, 4, 12);
        PutU64(msg, 8, played);
        PutU32(msg, 16, queued > 0xFFFFFFFFUL ? 0xFFFFFFFFU : (uint)queued);
        write.Reset();
        if (!write.Check(WriteFile(h, msg, (uint)msg.Length, IntPtr.Zero, write.Ov))) {
            Log("CONSUMED write failed, error " + Marshal.GetLastWin32Error());
            return false;
        }
        if (!write.Done.WaitOne(5000)) {
            Log("CONSUMED write timed out");
            return false;
        }
        uint n;
        if (!GetOverlappedResult(h, write.Ov, out n, false)) {
            Log("CONSUMED write failed, error " + Marshal.GetLastWin32Error());
            return false;
        }
        return true;
    }

    public static int Run(Guid vm, Guid type, Guid instance, uint pipeMode,
                          int connectSeconds, int seconds, int volume, string logFile) {
        byte[] offer = new byte[0xAC];
        Array.Copy(vm.ToByteArray(), 0, offer, 0x00, 16);
        Array.Copy(type.ToByteArray(), 0, offer, 0x14, 16);
        Array.Copy(instance.ToByteArray(), 0, offer, 0x24, 16);
        IntPtr h = VmbusPipeServerOfferChannel(offer, FILE_FLAG_OVERLAPPED, pipeMode);
        if (h == IntPtr.Zero || h == new IntPtr(-1)) {
            Log("offer failed, error " + Marshal.GetLastWin32Error());
            return 1;
        }
        Log("offered {" + type + "} instance {" + instance + "}, pipe mode " + pipeMode);

        Stop = false;
        ConsoleCancelEventHandler onCancel = delegate(object s, ConsoleCancelEventArgs e) {
            e.Cancel = true;
            Stop = true;
            Log("stop requested");
        };
        Console.CancelKeyPress += onCancel;

        int rc = 0;
        IntPtr buf = IntPtr.Zero;
        Wave wave = new Wave();
        WavLog wav = new WavLog();
        bool wavOpen = false;
        bool haveFormat = false;
        uint fmtRate = 0, fmtChannels = 0, fmtBits = 0;
        int msgCount = 0, pcmCount = 0, formatCount = 0, dropCount = 0, underrunCount = 0;
        ulong pcmIn = 0, consumedSent = 0;
        DateTime started = DateTime.Now;
        DateTime streamStart = DateTime.MinValue;

        try {
            var connect = new Op();
            connect.Reset();
            if (!connect.Check(VmbusPipeServerConnectPipe(h, connect.Ov))) {
                Log("connect failed, error " + Marshal.GetLastWin32Error());
                return 1;
            }
            uint n;
            if (!connect.Done.WaitOne(connectSeconds * 1000)) {
                Log("the guest did not open the channel within " + connectSeconds + " s");
                return 2;
            }
            if (!GetOverlappedResult(h, connect.Ov, out n, false)) {
                Log("connect failed, error " + Marshal.GetLastWin32Error());
                return 1;
            }
            Log("guest opened the channel");

            int ndev = waveOutGetNumDevs();
            Log("waveOut devices: " + ndev);
            for (int i = 0; i < ndev; i++) {
                WAVEOUTCAPS caps;
                if (waveOutGetDevCaps((uint)i, out caps, (uint)Marshal.SizeOf(typeof(WAVEOUTCAPS))) == 0)
                    Log("  [" + i + "] " + caps.szPname + " (" + caps.wChannels + " ch)");
            }
            if (ndev == 0)
                Log("no waveOut device: logging only, CONSUMED will be estimated from the clock");

            buf = Marshal.AllocHGlobal(ReadBufSize);
            var read = new Op();
            var write = new Op();
            read.Reset();
            if (!read.Check(ReadFile(h, buf, ReadBufSize, IntPtr.Zero, read.Ov))) {
                Log("read failed, error " + Marshal.GetLastWin32Error());
                return 1;
            }

            DateTime end = seconds > 0 ? DateTime.Now.AddSeconds(seconds) : DateTime.MaxValue;
            DateTime nextConsumed = DateTime.Now.AddMilliseconds(ConsumedIntervalMs);
            DateTime nextStatus = DateTime.Now.AddMilliseconds(StatusIntervalMs);
            bool readOutstanding = true;
            int pcmSinceConsumed = 0;
            DateTime lastPcm = DateTime.MinValue;

            while (!Stop && DateTime.Now < end) {
                int wait = (int)Math.Max(0, Math.Min((nextConsumed - DateTime.Now).TotalMilliseconds,
                                                     (end - DateTime.Now).TotalMilliseconds));
                if (wait > LoopMaxWaitMs) wait = LoopMaxWaitMs;
                if (wave.Tick() > 0) dropCount++;

                if (readOutstanding && read.Done.WaitOne(wait)) {
                    if (!GetOverlappedResult(h, read.Ov, out n, false)) {
                        int err = Marshal.GetLastWin32Error();
                        if (err == ERROR_BROKEN_PIPE || err == ERROR_PIPE_NOT_CONNECTED
                            || err == 6 || err == 995) {
                            Log("guest closed the channel (error " + err + ")");
                        } else {
                            Log("read failed, error " + err);
                            rc = 1;
                        }
                        break;
                    }
                    readOutstanding = false;

                    // Walk the buffer as a sequence of (type, size, payload).
                    int off = 0;
                    while (off + 8 <= (int)n) {
                        uint mtype = GetU32At(buf, off);
                        uint msize = GetU32At(buf, off + 4);
                        // Unsigned compare: msize is guest-controlled.
                        if ((ulong)off + 8UL + (ulong)msize > (ulong)n) {
                            Log("truncated message: type " + mtype + " size " + msize
                                + " at offset " + off + " of " + n);
                            break;
                        }
                        int pay = off + 8;
                        msgCount++;

                        if (mtype == VBAUD_MSG_FORMAT) {
                            if (msize < 12) {
                                Log("FORMAT with short payload " + msize);
                            } else {
                                uint rate = GetU32At(buf, pay);
                                uint ch = GetU32At(buf, pay + 4);
                                uint bits = GetU32At(buf, pay + 8);
                                formatCount++;
                                Log("FORMAT " + rate + " Hz " + ch + " ch " + bits + " bit"
                                    + " (stream #" + formatCount + ")");
                                bool changed = !haveFormat || rate != fmtRate
                                    || ch != fmtChannels || bits != fmtBits;
                                if (changed) {
                                    wave.CloseDevice();
                                    if (ch < 1 || ch > 8 || bits < 8 || bits > 32
                                        || (bits % 8) != 0 || rate < 4000 || rate > 192000) {
                                        Log("format out of range, keeping file-only mode");
                                        haveFormat = true;
                                        fmtRate = rate;
                                        fmtChannels = ch;
                                        fmtBits = bits;
                                    } else {
                                        haveFormat = true;
                                        fmtRate = rate;
                                        fmtChannels = ch;
                                        fmtBits = bits;
                                        if (!wave.OpenDevice(rate, ch, bits, volume))
                                            Log("waveOut unavailable, continuing with estimated CONSUMED");
                                    }
                                    if (wavOpen) {
                                        wav.Close();
                                        wavOpen = false;
                                    }
                                    if (!string.IsNullOrEmpty(logFile)) {
                                        wavOpen = wav.Open(logFile, fmtRate, fmtChannels, fmtBits);
                                        if (wavOpen)
                                            Log("wav log " + logFile + " (" + fmtRate + " Hz "
                                                + fmtChannels + " ch " + fmtBits + " bit)");
                                    }
                                } else {
                                    // Same format, new stream: drop anything still
                                    // queued so the two streams do not interleave.
                                    wave.ResetPlayback();
                                }
                                // New stream: the guest's position starts at 0 again.
                                streamStart = DateTime.Now;
                                pcmIn = 0;
                                consumedSent = 0;
                                pcmSinceConsumed = 0;
                                SendConsumed(h, write, 0, 0);
                                nextConsumed = DateTime.Now.AddMilliseconds(ConsumedIntervalMs);
                            }
                        } else if (mtype == VBAUD_MSG_PCM_OUT) {
                            if (!haveFormat) {
                                dropCount++;
                                if (dropCount == 1)
                                    Log("PCM_OUT before any FORMAT, dropping");
                            } else if ((int)msize > 0) {
                                pcmCount++;
                                pcmIn += msize;
                                lastPcm = DateTime.Now;
                                pcmSinceConsumed++;
                                if (wavOpen) wav.Append(AddPtr(buf, pay), (int)msize);

                                // Queue to waveOut only when the device is open;
                                // otherwise the WAV log is the only sink and the
                                // clock estimate keeps the guest flowing.
                                if (wave.IsOpen && wave.Enqueue(AddPtr(buf, pay), (int)msize) > 0)
                                    dropCount++;
                                if (pcmCount == 1 || (pcmCount % 100) == 0)
                                    Log("PCM #" + pcmCount + " " + msize + " bytes ("
                                        + pcmIn + " total)");
                            }
                        } else {
                            Log("unknown message type " + mtype + " size " + msize);
                        }
                        off += 8 + (int)msize;
                    }

                    read.Reset();
                    if (!read.Check(ReadFile(h, buf, ReadBufSize, IntPtr.Zero, read.Ov))) {
                        Log("read failed, error " + Marshal.GetLastWin32Error());
                        rc = 1;
                        break;
                    }
                    readOutstanding = true;
                }

                if (haveFormat && (DateTime.Now >= nextConsumed
                                   || pcmSinceConsumed >= ConsumedEveryNMsgs)) {
                    underrunCount += wave.Poll();
                    ulong played = wave.IsOpen
                        ? wave.PlayedBytes()
                        : EstimatePlayed(pcmIn, streamStart, fmtRate, fmtChannels, fmtBits);
                    // Never claim more than we accepted, and never rewind.
                    if (played > pcmIn) played = pcmIn;
                    if (played < consumedSent) played = consumedSent;
                    // Sent every interval while a stream plays: the queue depth
                    // steers the guest's clock.  Once everything is played and
                    // nothing new comes, the stream has stopped and nobody in
                    // the guest reads: writes would only fill the pipe.
                    bool idle = played >= pcmIn
                        && (DateTime.Now - lastPcm).TotalMilliseconds > IdleMs;
                    if (!idle && SendConsumed(h, write, played, pcmIn - played))
                        consumedSent = played;
                    pcmSinceConsumed = 0;
                    nextConsumed = DateTime.Now.AddMilliseconds(ConsumedIntervalMs);
                }

                if (DateTime.Now >= nextStatus) {
                    double t = (DateTime.Now - started).TotalSeconds;
                    Log("status msgs=" + msgCount + " pcm=" + pcmIn + " B played="
                        + consumedSent + " B drops=" + dropCount + " underruns="
                        + underrunCount + " t=" + t.ToString("0.0") + " s");
                    nextStatus = DateTime.Now.AddMilliseconds(StatusIntervalMs);
                }
            }

            // Final CONSUMED so the guest sees the tail of the stream.
            if (haveFormat) {
                ulong played = wave.IsOpen
                    ? wave.PlayedBytes()
                    : EstimatePlayed(pcmIn, streamStart, fmtRate, fmtChannels, fmtBits);
                if (played > pcmIn) played = pcmIn;
                if (played < consumedSent) played = consumedSent;
                SendConsumed(h, write, played, pcmIn - played);
            }
            return rc;
        } finally {
            CancelIoEx(h, IntPtr.Zero);
            // The read buffer and OVERLAPPED blocks stay allocated: a cancelled
            // read may still complete into them while the handle closes, and the
            // process ends soon.
            wave.Free();
            wav.Close();
            CloseHandle(h);
            double t = (DateTime.Now - started).TotalSeconds;
            Log("closed, offer rescinded; msgs=" + msgCount + " pcm=" + pcmIn
                + " B drops=" + dropCount + " underruns=" + underrunCount + " t="
                + t.ToString("0.0") + " s");
            Console.CancelKeyPress -= onCancel;
        }
    }
}
'@
}

$vm = Get-VM -Name $VMName
exit [VmbAud]::Run($vm.Id, [guid]$DeviceType, [guid]$Instance, $PipeMode,
                   $ConnectSeconds, $Seconds, $Volume, $LogFile)
