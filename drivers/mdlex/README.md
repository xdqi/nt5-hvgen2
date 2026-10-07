# Hyper-V Dynamic Memory (mdlex.sys and dmvsc)

Hyper-V Dynamic Memory lets the host grow and shrink a VM's RAM at runtime
through a balloon driver. Microsoft's balloon driver for the guest is
`dmvsc.sys`, shipped in the Integration Services. The last IC release that
still supports XP (6.3.9600.16384) contains a `dmvsc.sys` built for Windows
Server 2003 SP1, and its INF deliberately installs a "(not supported)" NULL
driver on XP. With two small adjustments that `dmvsc.sys` runs on XP SP3 and
Dynamic Memory works (balloon only; XP cannot hot-add memory).

`dmvsc.sys` is a KMDF driver that talks the Dynamic Memory protocol over its
VMBus channel (`{525074dc-8985-46e2-8057-a307dc18a502}`) using
`vmbkmcl.sys`/`wdfldr.sys`, which the Integration Services already put on the
disk. The only things XP's kernel does not give it are two ntoskrnl exports:

- **`MmAllocatePagesForMdlEx`** (Server 2003 SP1+). XP does not export it at
  all, so `dmvsc.sys` will not even load. It is the balloon-inflate allocator:
  `dmvsc` calls it to take pages away from the guest and hand the page runs to
  the host. XP has the older `MmAllocatePagesForMdl`, whose first four
  arguments are identical.
- **`MmAddPhysicalMemory`**. XP exports it, but XP cannot usefully hot-add
  memory, and - crucially - the host does **not** issue balloon requests
  unless the guest advertises the hot-add capability (the Linux and macOS
  balloon drivers advertise hot-add for exactly this reason and then refuse
  the host's hot-add requests). `dmvsc` decides whether to advertise hot-add
  from a start-time probe: it "adds" one already-present page with
  `MmAddPhysicalMemory` and advertises hot-add only if that returns success.
  On XP the real routine does not, so `dmvsc` advertises hot-add = 0, the host
  rejects the capabilities (STATUS `0xC000A013`), and the device fails to
  start (`CM_PROB_FAILED_POST_START`).

**mdlex.sys** (this repository, MIT) is a tiny kernel export driver that
provides both routines:

- `MmAllocatePagesForMdlEx` is implemented over XP's `MmAllocatePagesForMdl`.
  It honours `MM_ALLOCATE_FULLY_REQUIRED` (frees a short allocation and returns
  NULL rather than a partial MDL) and `MM_DONT_ZERO_ALLOCATION` (zeroes the
  pages otherwise); Dynamic Memory always passes `MM_DONT_ZERO_ALLOCATION`.
- `MmAddPhysicalMemory` returns `STATUS_SUCCESS` for the one-page capability
  probe (so `dmvsc` advertises hot-add and the host accepts the capabilities)
  and `STATUS_INVALID_PARAMETER_1` for any larger request - a real hot-add,
  which the host issues only when Maximum > Startup. XP cannot add physical
  memory. `dmvsc` maps exactly that status to "zero pages added" and answers
  the host's request with it, as a balloon-only guest should; any other failure
  status, `STATUS_NOT_SUPPORTED` for one, makes it send no answer and stop its
  message loop, which ends Dynamic Memory in the guest. (Found by reading
  `dmvsc`'s code. The probe only treats `STATUS_NOT_SUPPORTED` as "no
  hot-add", so it is not affected.)

`migrate/Patch-Dmvsc.ps1` rebinds those two imports in a caller-supplied
`dmvsc.sys` from ntoskrnl to `mdlex.sys`, touching no code: it appends a new
`.dmx` section with a fresh import-descriptor array (the four original
descriptors verbatim plus one per rebound import whose `FirstThunk` reuses the
existing IAT slot the code already calls), repoints each rebound ntoskrnl INT
entry to an already-imported export so the loader can still snap ntoskrnl's
thunk (the `mdlex` descriptors, processed afterwards, overwrite those two IAT
slots), repoints the import directory and fixes the PE checksum. The four
original IAT slots keep their addresses, so every other import and all code are
unchanged. The script verifies the input SHA-256 (IC 6.3.9600.16384
`dmvsc.sys`) before patching and never redistributes the Microsoft binary -
the caller supplies its own copy and keeps the output locally. It runs under
Windows PowerShell 5.1, where the converter runs.

```
# on the host, against your own IC 6.3.9600.16384 dmvsc.sys:
.\migrate\Patch-Dmvsc.ps1 -InputPath <IS>\dmvsc\dmvsc.sys -OutputPath out\dmvsc.sys
```

## Installing

`hvkit setup-cd` installs Dynamic Memory with XP from a setup CD, and `hvkit
inject` adds it to an installed XP offline (see hvkit/README.md); enable
Dynamic Memory on the VM before installing from the CD, or the device is not
there for setup to install. What they do to an installed system, by hand while
experimenting:

- Files: the patched `dmvsc.sys` and `mdlex.sys` into `%SystemRoot%\system32\
  drivers\`, and the Integration Services' `dmvscres.dll` into
  `%SystemRoot%\system32\` (the event-log message resource).
- Service `dmvsc` (`HKLM\SYSTEM\CurrentControlSet\Services\dmvsc`):
  `Type=1` (kernel), `Start=3` (demand), `ErrorControl=1`,
  `ImagePath=system32\DRIVERS\dmvsc.sys`; optional event-log source under
  `Services\EventLog\System\dmvsc` (`EventMessageFile=%SystemRoot%\System32\
  IoLogMsg.dll;%SystemRoot%\System32\dmvscres.dll`, `TypesSupported=7`).
  No service is needed for `mdlex.sys`: the kernel loads it automatically as a
  dependency of the import-patched `dmvsc.sys` and snaps the two imports to it.
- Bind the Dynamic Memory device to `dmvsc`. Its first hardware ID is
  `VMBUS\{525074dc-8985-46e2-8057-a307dc18a502}`; the IC INF left the devnode's
  `Service` empty (the NULL install) and the Critical Device Database entry
  `Control\CriticalDeviceDatabase\vmbus#{525074dc-8985-46e2-8057-a307dc18a502}`
  without a `Service`. Write `Service=dmvsc` into that CDDB key (so a never-seen
  instance binds on first boot) and into the existing `Enum\VMBUS\{525074dc-
  ...}\<instance>` node. User-mode Plug and Play may re-apply the NULL section
  on a later boot, so the generalized `bootwait.sys` re-asserts the `Service`
  (and the friendly name `Microsoft Hyper-V Dynamic Memory`) each boot, like it
  does for the SCSI controller.

## What works

Tested on Hyper-V Gen2 through CSMWrap, XP SP3, Integration Services
6.3.9600.16384, Dynamic Memory enabled (startup 2 GB, minimum 512 MB):
`dmvsc.sys` loads and starts (no yellow bang, no bug check), the host reports
`MemoryStatus = OK` and a live `MemoryDemand`, and the assigned memory tracks
demand - it balloons down to the 512 MB minimum while XP idles (demand
~90 MB) and rises again when a workload raises demand (e.g. ~770 MB assigned
at ~610 MB demand). XP stays stable. With Maximum > Startup the VM boots and
balloons normally; a real hot-add is answered with "no pages added" (XP cannot
add RAM), so the VM never grows above its startup size - it is balloon-only.
That answer to a real hot-add request follows from `dmvsc`'s code but has not
been exercised on a VM; the converter sets Maximum = Startup, which never
produces such a request.
