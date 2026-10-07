# Hyper-V Dynamic Memory (mdlex.sys and dmvsc)

Hyper-V Dynamic Memory grows and shrinks a VM's RAM at runtime through a balloon driver in the
guest, Microsoft's `dmvsc.sys`. The last Integration Services release for XP (6.3.9600.16384) has a
`dmvsc.sys` built for Server 2003 SP1, but its INF installs a "(not supported)" NULL driver on XP.
With mdlex.sys (this repository, MIT) and an import patch, that `dmvsc.sys` runs on XP SP3 and
Dynamic Memory works, balloon only: XP cannot hot-add memory. The converter (`-DynamicMemory`),
`hvkit setup-cd` (with XP) and `hvkit inject` (offline, into an installed XP) install it.

## What mdlex.sys provides

`dmvsc.sys` is a KMDF driver that talks the Dynamic Memory protocol over its VMBus channel
(`{525074dc-8985-46e2-8057-a307dc18a502}`) through `vmbkmcl.sys`/`wdfldr.sys`, which the
Integration Services already install. XP lacks only two ntoskrnl exports; mdlex.sys, a small kernel
export driver, provides them:

- **`MmAllocatePagesForMdlEx`** (Server 2003 SP1+), with which `dmvsc` inflates the balloon: it
  takes pages from the guest and hands the page runs to the host. XP does not export it, so
  `dmvsc.sys` does not load. mdlex implements it over XP's `MmAllocatePagesForMdl` (same first four
  arguments) and honours `MM_ALLOCATE_FULLY_REQUIRED` (a short allocation is freed and NULL
  returned, not a partial MDL) and `MM_DONT_ZERO_ALLOCATION` (otherwise the pages are zeroed;
  Dynamic Memory always passes it).
- **`MmAddPhysicalMemory`**. XP exports it but cannot usefully hot-add memory. The host sends no
  balloon requests unless the guest advertises hot-add (which is why the Linux and macOS balloon
  drivers advertise it and then refuse hot-add requests), and `dmvsc` advertises it only if a
  start-time probe that "adds" one existing page succeeds. XP's routine fails it, so the host
  rejects the capabilities (`0xC000A013`) and the device fails to start
  (`CM_PROB_FAILED_POST_START`). mdlex returns `STATUS_SUCCESS` for the one-page probe and
  `STATUS_INVALID_PARAMETER_1` for anything larger: a real hot-add, sent only when Maximum >
  Startup, for which `dmvsc` answers "zero pages added", as a balloon-only guest should. Any other
  failure status (e.g. `STATUS_NOT_SUPPORTED`) makes `dmvsc` send no answer and stop its message
  loop, ending Dynamic Memory in the guest; the probe treats only `STATUS_NOT_SUPPORTED` as "no
  hot-add". (Both from reading `dmvsc`'s code.)

## Patching dmvsc.sys

`hvkit patch dmvsc` (hvkit.exe on Windows) rebinds the two imports from ntoskrnl to `mdlex.sys`
without touching code. A new `.dmx` section holds a new import descriptor array: the four original
descriptors verbatim, plus one per rebound import whose `FirstThunk` is the IAT slot the code
already calls. The rebound
ntoskrnl INT entries point at already-imported exports, so the loader can still snap ntoskrnl's
thunks before the later `mdlex` descriptors overwrite those two slots. The four original IAT slots
keep their addresses, so other imports and code are unchanged; the PE checksum is fixed. The
recipe checks the input's SHA-256 (IC 6.3.9600.16384 `dmvsc.sys`); the caller supplies the
Microsoft binary and keeps the output, nothing is redistributed.

```
hvkit patch dmvsc <IS>/dmvsc/dmvsc.sys -o out/dmvsc.sys   # your own IC 6.3.9600.16384 dmvsc.sys
```

## Installing

With `hvkit setup-cd` ([hvkit](../../hvkit/README.md)), enable Dynamic Memory on the VM before
installing, or the device is not there for setup. By hand, an installed system needs:

- Files: the patched `dmvsc.sys` and `mdlex.sys` in `%SystemRoot%\system32\drivers\`, the
  Integration Services' `dmvscres.dll` (event log messages) in `%SystemRoot%\system32\`.
- Service `dmvsc`: `Type=1` (kernel), `Start=3` (demand), `ErrorControl=1`,
  `ImagePath=system32\DRIVERS\dmvsc.sys`; optionally an event log source
  `Services\EventLog\System\dmvsc` with
  `EventMessageFile=%SystemRoot%\System32\IoLogMsg.dll;%SystemRoot%\System32\dmvscres.dll` and
  `TypesSupported=7`. mdlex.sys needs no service: the kernel loads it as an import of `dmvsc.sys`.
- `Service=dmvsc` in `Control\CriticalDeviceDatabase\vmbus#{525074dc-8985-46e2-8057-a307dc18a502}`
  (so a new instance binds on its first boot) and in the existing
  `Enum\VMBUS\{525074dc-...}\<instance>` node; the IC INF's NULL install left both without one.
  User-mode Plug and Play may re-apply the NULL section on a later boot, so a
  [bootwait](../bootwait/README.md) `Devices` entry re-asserts `Service` and the name "Microsoft
  Hyper-V Dynamic Memory" on every boot.

## What works

Tested on Gen2 through CSMWrap (XP SP3, IC 6.3.9600.16384, startup 2 GB, minimum 512 MB):
`dmvsc.sys` starts without a problem code or bug check, the host reports `MemoryStatus = OK` and a
live `MemoryDemand`, and assigned memory follows demand, down to 512 MB while XP idles (~90 MB
demand) and up under load (e.g. ~770 MB at ~610 MB demand). XP stays stable. With Maximum >
Startup the VM boots and balloons but never grows above startup; the "no pages added" answer to a
real hot-add is from `dmvsc`'s code, not yet seen on a VM. The converter sets Maximum = Startup,
so the host never sends one.

## Files

```
mdlex.c    MmAllocatePagesForMdlEx and MmAddPhysicalMemory for dmvsc.sys
mdlex.def  export names (undecorated, --kill-at)
```
