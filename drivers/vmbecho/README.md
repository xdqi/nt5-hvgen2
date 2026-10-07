# VMBus pipes from a host program (vmbecho.sys)

Hyper-V has no interface for third parties to add a VMBus device to a VM,
but the host's `vmbuspiper.dll` exports `VmbusPipeServerOfferChannel`, which
the Hyper-V stack itself uses for its pipe channels. It is not documented.
An elevated program on the host can call it with a VM's ID and an interface
type GUID of its own; the running VM then gets a VMBus channel offer, and the
program gets a handle on which `ReadFile` and `WriteFile` exchange messages
with the guest. Closing the handle rescinds the offer.

The offer (0xAC bytes, layout read from the host's `vmbuspiper.dll`):

| Offset | Size | Field |
|---|---|---|
| 0x00 | 16 | VM ID (`(Get-VM).Id`) |
| 0x10 | 4 | interrupt latency in ms (0) |
| 0x14 | 16 | interface type GUID: the guest's hardware ID is `VMBUS\{type}` |
| 0x24 | 16 | interface instance GUID |
| 0x34 | 4 | interface revision (0) |
| 0x38 | 2 | MMIO megabytes (0) |
| 0x3A | 2 | flags (0; `0x1` = open per file object, see below) |
| 0x3C | 112 | user-defined bytes |

`HANDLE VmbusPipeServerOfferChannel(offer, openMode, pipeMode)` returns -1
and sets the last error on failure (`ERROR_ACCESS_DENIED` without a full
administrator token, a Hyper-V Administrators member is not enough).
`VmbusPipeServerConnectPipe(handle, overlapped)` completes when the guest
opens the channel. Use an overlapped handle (`openMode` =
`FILE_FLAG_OVERLAPPED`): a synchronous ConnectPipe still waiting in another
thread makes `CloseHandle` block.

On XP, the offer appears as a new device right away ("Found New Hardware").
Its hardware IDs are `VMBUS\{type}` and `VMBUS\{instance}`, so an INF
matches the type; keeping the instance GUID the same across offers keeps the
device node, and the driver is installed once.

The channel is in named pipe mode, and the Integration Services' `vmbus.sys`
(6.3.9600) handles such channels itself; its function driver does not use
the KMCL library. When the device starts, `vmbus.sys` opens the channel (5
ring buffer pages each way) and from then on serves `IRP_MJ_READ` and
`IRP_MJ_WRITE` sent to the device's PDO, with direct I/O (the buffer is the
IRP's MDL). It adds and strips the pipe's packet headers: a write in the
guest is one message to the host, and a read returns what the host wrote.
(With flag `0x1` in the offer, `vmbus.sys` opens the channel on
`IRP_MJ_CREATE` instead, one open at a time, so that a user-mode program
could use it.) Opening the channel with KMCL as well fails
(`VmbChannelEnable` returns `STATUS_UNSUCCESSFUL`): `vmbus.sys` already has it.

`vmbecho.sys` is the smallest function driver for such a device: on start it
runs a thread that writes a hello message to the PDO, then reads, and
answers every read with its length and first bytes. The thread ends when a
read fails (the host closed the pipe: the read is cancelled when the device
goes away) or when the device stops.

Tested on Hyper-V Gen2 through CSMWrap, XP SP3, Integration Services
6.3.9600.16384, Windows 11 host:

```
# on the host, elevated:
.\vmbecho-host.ps1 -VMName <vm> -Seconds 10 -IntervalMs 100
```

The first offer shows the Found New Hardware wizard; install from a CD or
folder with `vmbecho.inf` and `vmbecho.sys`. 100 messages at 100 ms
intervals each reached the guest intact and were answered, about 12 ms per
round trip (with a kernel debugger attached); after the host closes its
handle the device is removed, and a new offer starts the driver again. Not
tried yet: the `pipeMode` argument (0 here), offering to a VM that is off or
restarts, and large messages.
