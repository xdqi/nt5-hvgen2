# VMBus pipes from a host program (vmbecho.sys)

A test driver for a VMBus channel offered by an ordinary host program, the transport
[vmbaud](../vmbaud/README.md) uses. Hyper-V has no interface for adding VMBus devices, but the
host's `vmbuspiper.dll` exports `VmbusPipeServerOfferChannel` (undocumented; Hyper-V uses it for its
own pipe channels). An elevated program calls it with a VM's ID and an interface type GUID of its
own: the running VM gets a channel offer, and the program a handle on which `ReadFile` and
`WriteFile` exchange messages with the guest. Closing the handle rescinds the offer.

## Host side

The offer is 0xAC bytes (layout read from `vmbuspiper.dll`):

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

`HANDLE VmbusPipeServerOfferChannel(offer, openMode, pipeMode)` returns -1 and sets the last error
on failure (`ERROR_ACCESS_DENIED` without a full administrator token; Hyper-V Administrators is not
enough). `VmbusPipeServerConnectPipe(handle, overlapped)` completes when the guest opens the
channel. Use `openMode` = `FILE_FLAG_OVERLAPPED`: a synchronous ConnectPipe waiting in another
thread makes `CloseHandle` block.

## Guest side

XP sees the offer at once as a new device, hardware IDs `VMBUS\{type}` and `VMBUS\{instance}`. An
INF matches the type; reusing the instance GUID keeps the device node, so the driver is installed
once.

The Integration Services' `vmbus.sys` (6.3.9600) serves named pipe mode channels itself, without
KMCL. When the device starts it opens the channel (5 ring buffer pages each way), then serves
`IRP_MJ_READ` and `IRP_MJ_WRITE` sent to the PDO with direct I/O (buffer in the IRP's MDL) and adds
and strips the pipe's packet headers: a guest write is one message to the host, a read returns what
the host wrote. With offer flag `0x1` it opens the channel on `IRP_MJ_CREATE` instead, one open at a
time, so a user-mode program could use it. Opening the channel with KMCL as well fails
(`VmbChannelEnable` returns `STATUS_UNSUCCESSFUL`): `vmbus.sys` already has it.

vmbecho.sys is the smallest function driver for this: on start a thread writes a hello message to
the PDO, then answers every read with its length and first bytes, until a read fails (the host
closed the pipe, so the device goes away and the read is cancelled) or the device stops.

## Testing

XP SP3 on Hyper-V Gen2 through CSMWrap, Integration Services 6.3.9600.16384, Windows 11 host:

```
.\vmbecho-host.ps1 -VMName <vm> -Seconds 10 -IntervalMs 100     # on the host, elevated
```

On the first offer, install `vmbecho.inf` and `vmbecho.sys` from a CD or folder in the Found New
Hardware wizard. All 100 messages arrived intact and were answered, about 12 ms per round trip (with
a kernel debugger attached). Closing the host handle removes the device; a new offer starts the
driver again. Not tried yet: `pipeMode` other than 0, offering to a VM that is off or restarts, and
large messages.

## Files

```
vmbecho.c         function driver for a host-offered VMBus pipe: echoes what the host writes
vmbecho.inf       installs it for VMBUS\{39868fad-8ee5-403c-9d09-2ac377fe9889}
vmbecho-host.ps1  offers the pipe from the host and talks to vmbecho (PS 5.1, elevated)
```
