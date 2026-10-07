# vmbecho-host.ps1 - host side of the vmbecho test (run elevated on the Hyper-V host).
#
# Offers a VMBus pipe channel to a running VM with vmbuspiper.dll!VmbusPipeServerOfferChannel
# (an undocumented Windows API, the one the Hyper-V stack uses for its own pipe channels),
# waits for the guest's vmbecho.sys to open it, prints what the guest sends and sends
# "ping <n>" every -IntervalMs.  Closing the handle at the end rescinds the offer, and the
# guest removes the device.
#
#   vmbecho-host.ps1 -VMName <name> [-Seconds 30] [-IntervalMs 2000] [-PipeMode 0]
#                    [-ConnectSeconds 30] [-Instance <guid>]
#
# The offer (0xAC bytes, layout from vmbuspiper.dll): VM GUID at 0x00, interrupt latency
# (ms) at 0x10, interface type at 0x14, interface instance at 0x24, interface revision at
# 0x34, MMIO megabytes at 0x38, flags at 0x3A, 112 user-defined bytes at 0x3C.  The guest
# sees the device as VMBUS\{interface type}; keeping the instance GUID fixed keeps the
# guest's device node, so the driver is installed only once.
param([Parameter(Mandatory)][string]$VMName,
      [int]$Seconds = 30, [int]$IntervalMs = 2000, [uint32]$PipeMode = 0,
      [int]$ConnectSeconds = 30,
      [string]$Type = '39868fad-8ee5-403c-9d09-2ac377fe9889',
      [string]$Instance = '238a707b-c317-4337-84ec-6926e3ff47b4')
$ErrorActionPreference = 'Stop'

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this elevated: VmbusPipeServerOfferChannel fails with ERROR_ACCESS_DENIED otherwise.'
}

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class VmbEcho {
    const uint FILE_FLAG_OVERLAPPED = 0x40000000;
    const int ERROR_IO_PENDING = 997;
    const int ERROR_IO_INCOMPLETE = 996;

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

    static void Log(string s) {
        Console.WriteLine(DateTime.Now.ToString("HH:mm:ss.fff") + " " + s);
        Console.Out.Flush();
    }

    static string Show(IntPtr buf, int n) {
        var sb = new StringBuilder();
        for (int i = 0; i < n; i++) {
            byte b = Marshal.ReadByte(buf, i);
            if (b >= 0x20 && b < 0x7f) sb.Append((char)b);
            else if (b == (byte)'\n') sb.Append("\\n");
            else sb.AppendFormat("\\x{0:x2}", b);
        }
        return sb.ToString();
    }

    public static int Run(Guid vm, Guid type, Guid instance, uint pipeMode,
                          int connectSeconds, int seconds, int intervalMs) {
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
        int rc = 0;
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

            const int BufSize = 65536;
            IntPtr buf = Marshal.AllocHGlobal(BufSize);
            var read = new Op();
            var write = new Op();
            read.Reset();
            if (!read.Check(ReadFile(h, buf, BufSize, IntPtr.Zero, read.Ov))) {
                Log("read failed, error " + Marshal.GetLastWin32Error());
                return 1;
            }
            DateTime end = DateTime.Now.AddSeconds(seconds);
            DateTime nextPing = DateTime.Now.AddMilliseconds(intervalMs);
            int ping = 0;
            while (DateTime.Now < end) {
                int wait = (int)Math.Max(0, Math.Min((nextPing - DateTime.Now).TotalMilliseconds,
                                                     (end - DateTime.Now).TotalMilliseconds));
                if (read.Done.WaitOne(wait)) {
                    if (!GetOverlappedResult(h, read.Ov, out n, false)) {
                        Log("read failed, error " + Marshal.GetLastWin32Error());
                        rc = 1;
                        break;
                    }
                    Log("guest -> host " + n + " bytes: " + Show(buf, (int)n));
                    read.Reset();
                    if (!read.Check(ReadFile(h, buf, BufSize, IntPtr.Zero, read.Ov))) {
                        Log("read failed, error " + Marshal.GetLastWin32Error());
                        rc = 1;
                        break;
                    }
                }
                if (DateTime.Now >= nextPing) {
                    byte[] msg = Encoding.ASCII.GetBytes("ping " + (++ping) + "\n");
                    write.Reset();
                    if (!write.Check(WriteFile(h, msg, (uint)msg.Length, IntPtr.Zero, write.Ov))
                        || !write.Done.WaitOne(5000)
                        || !GetOverlappedResult(h, write.Ov, out n, false)) {
                        Log("write failed, error " + Marshal.GetLastWin32Error());
                        rc = 1;
                        break;
                    }
                    Log("host -> guest " + n + " bytes: ping " + ping);
                    nextPing = nextPing.AddMilliseconds(intervalMs);
                }
            }
            CancelIoEx(h, IntPtr.Zero);
            // The read buffer and OVERLAPPED blocks stay allocated: a cancelled read may
            // still complete into them while the handle closes, and the process ends soon.
            return rc;
        } finally {
            CloseHandle(h);
            Log("closed, offer rescinded");
        }
    }
}
'@

$vm = Get-VM -Name $VMName
exit [VmbEcho]::Run($vm.Id, [guid]$Type, [guid]$Instance, $PipeMode, $ConnectSeconds, $Seconds, $IntervalMs)
