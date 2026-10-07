# Sound (vmbaud.sys)

Hyper-V gives a guest no sound device; Windows guests get sound only through
an enhanced session (RDP), which needs a newer Windows inside. vmbaud.sys is
a sound card for XP whose back end is a program on the host: the guest's
audio stack plays into a PortCls WaveCyclic render device, the driver sends
the PCM over a VMBus pipe (see [vmbecho](../vmbecho/README.md)), and `vmbaudtray.exe`
plays it on the host's default output.

The host program offers the device (interface type
`{8b57f4e3-2a3c-4f6e-9c8d-1e5a70b9c4d2}`, a fixed instance) to each running
VM that has sound switched on. On the first offer XP shows Found New
Hardware; install `vmbaud.inf` and `vmbaud.sys` from a CD or folder.
A device that first appears after setup can't be installed without that
wizard on XP when its driver is unsigned: Plug and Play's non-interactive
install refuses unsigned files whatever the signing policy. So `hvkit
setup-cd --vmbaud` installs it ahead of time instead: `guest/predev`, run
from the CD's `cmdlines.txt` near the end of GUI-mode setup, creates the
device node and installs `vmbaud.inf` on it, and the first offer just
starts the driver. `hvkit inject --vmbaud` only puts the files into
DevicePath, where the wizard finds them. The device is "VMBus PCM
Audio"; winmm lists it as `VMbaud_Wave`. It is there while the host program
serves the VM: exiting the program removes it.

The render pin takes PCM, 16 bits, 1 or 2 channels, 8 to 48 kHz. There is no
capture pin yet.

## The host program

`vmbaudtray.exe` runs elevated (offering a pipe needs a full administrator
token) with an icon in the notification area. Its settings window lists the
VMs in three groups, sound on, running and off; the checkbox in front of a
VM switches its sound on, and the slider and Mute below act on the selected
VM. Each VM gets its own entry in the Windows volume mixer, named after it.
"Start with Windows" registers a Task Scheduler task that starts the program
at logon with the highest privileges, so without a UAC prompt. Closing the
window hides it; Exit is in the icon's menu.

The settings are kept with the VM as host-only KVP items (not exchanged with
the guest; they move with an export):

| item | values | absent |
|---|---|---|
| `vmbaud.enabled` | `0`, `1` | `0` |
| `vmbaud.volume` | `0` to `100` | `100` |
| `vmbaud.mute` | `0`, `1` | `0` |

The program offers the device as soon as such a VM runs; XP picks it up when
vmbaud.sys starts (an offer made while the firmware runs is fine). When the
channel breaks, after a guest reboot for one, it offers again; a new offer
the guest does not open within 10 s (doubling up to 60 s) is withdrawn and
made again, except while the VM is rebooting. `--wait-ic` holds each offer
until the Heartbeat or KVP integration service reports OK. The program logs
one line per event to `%TEMP%\vmbaudtray.log`.

`vmbaudcli.exe` does the same from a console:

```
vmbaudcli list                                     # VMs, state, settings
vmbaudcli set <vm> enabled|volume|mute <value>     # writes the KVP item
vmbaudcli unset <vm> enabled|volume|mute
vmbaudcli run <vm> [seconds] [--mute] [--wav prefix]   # elevated; not while vmbaudtray serves the VM
```

`run` serves one VM in the foreground and logs to the console. With
`--mute` the stream plays on the real device, clock included, with its
session muted; `--wav` writes each stream as the guest sent it to
`prefix-<n>.wav`. `vmbaud-host.ps1` is the earlier PowerShell host (winmm,
one VM, `-LogFile` capture), kept for scripting:

```
.\vmbaud-host.ps1 -VMName <vm> [-Volume 0..100] [-LogFile capture.wav]   # elevated
```

## Wire protocol

One pipe message per write, an 8-byte header (`u32 type`, `u32 size` of the
payload that follows), little endian:

| type | name | direction | payload |
|---|---|---|---|
| 1 | FORMAT | guest to host | `u32 rate, u32 channels, u32 bits`: a stream starts |
| 2 | PCM_OUT | guest to host | interleaved PCM |
| 3 | CONSUMED | host to guest | `u64 played, u32 queued`: bytes of this stream played and still queued on the host |

The guest sends FORMAT when a stream goes from KSSTATE_STOP to ACQUIRE; both
sides count the stream's bytes from there. The host sends CONSUMED every
10 ms while a stream plays.

## The clock

The play position that the driver reports (`GetPosition`) decides how fast
the guest's audio stack hands it data, so it is the clock of the stream. It
must advance smoothly: XP's WaveCyclic port keeps only about 40 ms written
ahead of the position, and a position that moves in steps of tens of
milliseconds makes kmixer use up its client's data and fill the gaps with
silence. It must also follow the host's sound card in the long run, or the
host's queue grows or runs dry.

So the position is a clock on the guest's performance counter at the nominal
byte rate, corrected by up to 0.5%. The host reports how much PCM it has
queued; the driver smooths that over about 16 reports and slows its clock
when the queue is deeper than 60 ms, and speeds it up when it is shallower.
The audio is not resampled. The clock stops at what has been sent, as the
host cannot play data it does not have.

Two other models were tried first. A position that follows only what the
host has played lags by the whole round trip (the host's own buffering and
the reports' interval): the guest could send only a third of real time and
the host kept running dry. A position that follows what the host has
received moves in steps, with the kmixer silence described above.

The host queues the guest's PCM, starts playing once 60 ms are queued (again
after running dry), and starts anyway when the guest stops sending for
30 ms. vmbaudtray plays through a shared-mode WASAPI stream with a 50 ms
device buffer, refilled on every wake-up of its MMCSS ("Pro Audio") worker;
vmbaud-host.ps1 joins the messages into 20 ms winmm buffers.

## Notes on the implementation

- Data path: the port calls `IDmaChannel::CopyTo` with the client's PCM, and
  the driver sends it as is (the MSVAD/Scream approach); the DMA buffer is
  allocated only because the port asks for one. XP's port calls `CopyTo`
  already in KSSTATE_PAUSE, while the client fills the buffer before it
  starts the stream.
- The stream object is both the `IMiniportWaveCyclicStream` and the
  `IDmaChannel` the port gets from `NewStream`.
- The counts and the write list are used at PASSIVE_LEVEL (the thread that
  reads CONSUMED) and at DISPATCH_LEVEL (the port), so they are under a spin
  lock that raises IRQL. Stopping a stream cancels its pending writes outside
  that lock: vmbus.sys completes a cancelled pipe write inside `IoCancelIrp`,
  and the completion routine takes the lock.
- Formats: the render pin takes only PCM in a WAVEFORMATEX(TENSIBLE), and
  `NewStream`, `SetFormat` and the proposed-format property all check it.
  DirectSound offers the pin its hardware buffer formats
  (`KSDATAFORMAT_SPECIFIER_DSOUND`) at rates down to 100 Hz; refused, it
  mixes in software through kmixer, which converts any format to one the pin
  takes. kmixer also changes the format after STOP -> ACQUIRE (DirectSound
  opens at 48 kHz, then plays 44.1 kHz), so `SetFormat` past STOP sends a new
  FORMAT and the counts restart on both sides.
- If the host stops reading the pipe, at most 64 writes (about 640 ms) stay
  pending; after that PCM is dropped and the stream clock keeps running, like
  a card with nothing plugged in, so the guest's programs do not hang.
- C++ with this toolchain: `ddk/portcls.h` needs `DECLSPEC_NOVTABLE`,
  `DECLSPEC_NOTHROW`, `TCHAR` and the `KSRTAUDIO_*` structures under C++
  (`drivers/common/ddk_compat.h`, included after `ntddk.h` and before `portcls.h`),
  and `-fno-exceptions` is required: with exceptions `STDMETHOD` is
  `noexcept` and `STDMETHODIMP_` is not. `operator delete` comes from
  `stdunk.h`; `operator new` and the 64-bit division helpers are in
  `common.cpp`.
- The toolchain has no import library for portcls.sys; the Makefile makes
  one from `drivers/common/portcls.def`. XP's forwarding routine is
  `PcForwardIrpSynchronous`, and `PcTerminateAdapterDriver` does not exist.

## Testing

Tested on Hyper-V Gen2 through CSMWrap, XP SP3, Integration Services
6.3.9600.16384, Windows 11 host. `testplay.exe` (built by `make`) plays a sine
tone through winmm and prints one line per buffer:

```
c:\testplay -s 20              (20 s, 4 buffers of 20 ms queued)
c:\testplay -s 5 -n 8 -m 50     (5 s, 8 buffers of 50 ms queued)
c:\testplay -T                  (the clock tick; XP's default is 15.6 ms)
```

Like a player, testplay waits on an event for finished buffers. `-p` makes
it poll with `Sleep(2)` instead, which sleeps a whole clock tick: with
4 buffers of 20 ms kmixer then runs dry and inserts 10 ms of silence every
quarter second or so, which is the client's doing, not the card's. `-t`
raises the clock to 1 ms while playing.

Checked with `vmbaudcli run --mute --wav` captures (a sine is easy to check
for silence and discontinuities): with 8 buffers of 50 ms the PCM that
reaches the host is a clean sine; with 4 buffers of 20 ms it is too, but for
10 ms of silence 20 ms into each stream. Windows Media Player 9 (DirectSound,
44.1 kHz) plays the sample music through without a gap. The host's queue stays between 45
and 90 ms, and its device buffer runs empty only at the end of a stream.
Open points:

- XP's logon sound has a gap about 0.3 s in: during logon the guest sends
  late.
- A client that hangs holding the stream keeps the device from being removed
  when the host program exits; the next offer then does not start the device
  until XP restarts.
- The trim is proportional only, so the host's queue settles above the
  60 ms target by the clock difference.
