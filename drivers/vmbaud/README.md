# Sound (vmbaud.sys)

A sound card for XP on Hyper-V, which otherwise gives guests sound only through an RDP enhanced
session (newer Windows only). XP plays into a PortCls WaveCyclic render device; the driver sends the
PCM over a VMBus pipe (see [vmbecho](../vmbecho/README.md)) to `vmbaudtray.exe`, which plays it on
the host's default output. Render only: PCM, 16 bits, 1 or 2 channels, 8 to 48 kHz. Unlike the rest
of the repository this directory is under the MS-PL ([LICENSE](LICENSE)): parts derive from Scream,
which is based on Microsoft's MSVAD sample.

## Install

The host program offers the device (interface type `{8b57f4e3-2a3c-4f6e-9c8d-1e5a70b9c4d2}`, a fixed
instance) to each running VM with sound on. XP shows it as "VMBus PCM Audio" (winmm: `VMbaud_Wave`)
while the program serves the VM; exiting the program removes it.

- `hvkit setup-cd --vmbaud` pre-installs it: `guest/predev`, run from the CD's `cmdlines.txt` near
  the end of GUI-mode setup, creates the device node and installs `vmbaud.inf` on it, so the first
  offer just starts the driver, with no wizard. (A device first seen after setup always gets the
  wizard: XP's non-interactive Plug and Play install refuses unsigned files whatever the policy.)
- Otherwise install `vmbaud.inf` and `vmbaud.sys` from a CD or folder in the Found New Hardware
  wizard. `hvkit inject --vmbaud` only puts them into DevicePath for the wizard.

## Host programs

`vmbaudtray.exe` runs elevated (offering a pipe needs a full administrator token) in the
notification area; closing its window only hides it, and Exit is in the icon's menu. The window
lists the VMs (sound on, running, off), switches sound per VM and sets the selected VM's volume and
mute; each VM gets its own volume mixer entry. "Start with Windows" adds a Task Scheduler task that
runs it at logon with highest privileges (no UAC prompt). Log: `%TEMP%\vmbaudtray.log`. Settings are
host-only KVP items of the VM (not seen by the guest, exported with the VM):

| item | values | absent |
|---|---|---|
| `vmbaud.enabled` | `0`, `1` | `0` |
| `vmbaud.volume` | `0` to `100` | `100` |
| `vmbaud.mute` | `0`, `1` | `0` |

The program offers the device as soon as such a VM runs, even in its firmware (XP picks the offer up
when vmbaud.sys starts), and again when the channel breaks, e.g. on a guest reboot. An offer not
opened within 10 s (doubling up to 60 s) is withdrawn and made again, except while the VM reboots.
`--wait-ic` holds each offer until the Heartbeat or KVP integration service reports OK.

`vmbaudcli.exe` is the console version; `run --mute` plays on the real device (clock included) with
the session muted, `--wav` saves each stream as received to `prefix-<n>.wav`:

```
vmbaudcli list                                     # VMs, state, settings
vmbaudcli set <vm> enabled|volume|mute <value>     # writes the KVP item
vmbaudcli unset <vm> enabled|volume|mute
vmbaudcli run <vm> [seconds] [--mute] [--wav prefix]
        # serves one VM in the foreground, logging to the console; elevated;
        # not while vmbaudtray serves the VM
```

The earlier PowerShell host (winmm, one VM) is kept for scripting:

```
.\vmbaud-host.ps1 -VMName <vm> [-Volume 0..100] [-LogFile capture.wav]   # elevated
```

## Wire protocol

Little endian; one pipe message per write: an 8-byte header (`u32 type`, `u32 size` of the payload),
then the payload.

| type | name | direction | payload |
|---|---|---|---|
| 1 | FORMAT | guest to host | `u32 rate, u32 channels, u32 bits`: a stream starts |
| 2 | PCM_OUT | guest to host | interleaved PCM |
| 3 | CONSUMED | host to guest | `u64 played, u32 queued`: bytes of this stream played and still queued on the host |

FORMAT is sent on KSSTATE_STOP -> ACQUIRE and starts both sides' byte counts. CONSUMED is sent every
10 ms while a stream plays.

## Clock

The play position the driver reports (`GetPosition`) is the stream's clock: it paces how fast XP's
audio stack delivers data. It must move smoothly, as XP's WaveCyclic port writes only about 40 ms
ahead of it and steps of tens of milliseconds make kmixer run dry and insert silence; and it must
follow the host's sound card, or the host's queue grows or empties. So it runs on the guest's
performance counter at the nominal byte rate, trimmed by up to 0.5% to hold the host's queue
(`queued`, smoothed over about 16 reports) near 60 ms. There is no resampling, and the clock stops
at what has been sent. (Following the host's played count lagged by the round trip; following its
received count moved in steps.)

The host starts playing once 60 ms are queued (again after running dry), or when the guest stops
sending for 30 ms. vmbaudtray uses shared-mode WASAPI with a 50 ms device buffer, refilled on every
wake-up of its MMCSS ("Pro Audio") thread; vmbaud-host.ps1 joins messages into 20 ms winmm buffers.

## Implementation notes

- The port passes the client's PCM to `IDmaChannel::CopyTo`, and the driver sends it as is (as MSVAD
  and Scream do); the DMA buffer exists only because the port asks for one. XP calls `CopyTo`
  already in KSSTATE_PAUSE, as the client fills the buffer before starting. The stream object is
  both the `IMiniportWaveCyclicStream` and the `IDmaChannel` from `NewStream`.
- A spin lock guards the counts and write list, shared by the CONSUMED reader (PASSIVE_LEVEL) and
  the port (DISPATCH_LEVEL). Stopping cancels pending writes outside the lock, because vmbus.sys
  completes a cancelled pipe write inside `IoCancelIrp` and the completion routine takes the lock.
- Only PCM in a WAVEFORMATEX(TENSIBLE) is accepted (`NewStream`, `SetFormat`, proposed-format
  property). DirectSound's hardware buffer formats (`KSDATAFORMAT_SPECIFIER_DSOUND`, down to 100 Hz)
  are refused, so DirectSound mixes through kmixer, which converts to a format the pin takes. kmixer
  changes the format after STOP -> ACQUIRE (DirectSound opens at 48 kHz, then plays 44.1 kHz), so
  `SetFormat` past STOP sends a new FORMAT.
- If the host stops reading, at most 64 writes (about 640 ms) stay pending; then PCM is dropped
  while the clock runs on, like a card with nothing plugged in, so guest programs do not hang.
- Toolchain: `drivers/common/ddk_compat.h` (after `ntddk.h`, before `portcls.h`) supplies what
  `ddk/portcls.h` lacks under C++. `-fno-exceptions` is required, as otherwise `STDMETHOD` is
  `noexcept` and `STDMETHODIMP_` is not. `operator new` and the 64-bit division helpers are in
  `common.cpp`. The portcls.sys import library is made from `drivers/common/portcls.def`; XP has
  `PcForwardIrpSynchronous` but no `PcTerminateAdapterDriver`.

## Testing

XP SP3 on Hyper-V Gen2 through CSMWrap, Integration Services 6.3.9600.16384, Windows 11 host.
`testplay.exe` (built by `make`) plays a sine through winmm and prints one line per buffer:

```
c:\testplay -s 20              (20 s, 4 buffers of 20 ms queued)
c:\testplay -s 5 -n 8 -m 50    (5 s, 8 buffers of 50 ms queued)
c:\testplay -T                 (the clock tick; XP's default is 15.6 ms)
```

`-t` sets a 1 ms tick while playing. `-p` polls for finished buffers with `Sleep(2)` (a whole tick)
instead of waiting on an event like a player; with 4 buffers of 20 ms kmixer then inserts 10 ms of
silence about every 0.25 s, a client fault.

Results from `vmbaudcli run --mute --wav` captures: with 8 buffers of 50 ms the host receives a
clean sine; with 4 of 20 ms too, except for 10 ms of silence 20 ms into each stream. Windows Media
Player 9 (DirectSound, 44.1 kHz) plays the sample music without a gap. The host's queue stays at 45
to 90 ms, and its device buffer runs empty only at a stream's end.

Open points:

- XP's logon sound has a gap about 0.3 s in: the guest sends late during logon.
- A client hung holding the stream keeps the device from being removed when the host program exits;
  the next offer then does not start the device until XP restarts.
- The trim is proportional only, so the host's queue settles above 60 ms by the clock difference.

## Files

```
adapter.cpp     DriverEntry, AddDevice, StartDevice: the PortCls adapter with a wave and a topology filter
minwave.cpp     the WaveCyclic miniport and its filter description (wavtable.h)
minstream.cpp   the render stream: IMiniportWaveCyclicStream and IDmaChannel, pipe I/O, stream clock
mintopo.cpp     the topology miniport: one volume node (toptable.h)
common.cpp      adapter common object, power management stub, CUnknown, operator new, 64-bit division
helpers.cpp     property helpers
vmbaud.h        wire protocol, interface GUID, clock constants
vmbaud.inf      installs vmbaud as a sound device for VMBUS\{8b57f4e3-2a3c-4f6e-9c8d-1e5a70b9c4d2}
vmbaud-host.ps1 offers the sound device from the host and plays its PCM through winmm (PS 5.1, elevated)
testplay.c      XP console program that plays a tone through winmm: the test client
LICENSE         the MS-PL, which covers everything in this directory
tray/           the x64 host programs vmbaudtray.exe and vmbaudcli.exe:
  hvhost.cpp      Hyper-V WMI: VMs, their state changes, the host-only KVP settings
  pipechannel.cpp vmbuspiper.dll: offer, connect, read, write
  vmsession.cpp   one worker thread per VM: offers, wire protocol, CONSUMED, reconnecting
  audioout.cpp    one WASAPI stream (and volume mixer entry) per VM
  trayui.cpp      tray icon, settings dialog, autostart task; main.cpp, vmbaudtray.rc
  cli.cpp         vmbaudcli.exe: settings and a session from the command line
../common/portcls.def, ../common/ddk_compat.h  XP's portcls.sys import library; what portcls.h lacks under C++
```
