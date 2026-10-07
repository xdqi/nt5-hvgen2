# bootwait.sys

A boot-start driver that holds XP's boot until the boot disk on the Hyper-V Gen2 VMBus SCSI
controller (or the setup CD) is there and the mount manager knows the boot volume, avoiding bug
checks 0x7B and 0xC000021A. It also repairs device nodes and registry values that the Integration
Services' INFs break on XP. The converter ([migrate](../../migrate/README.md)) and the hvkit setup
CD install it. Its values are under `Services\bootwait\Parameters`.

## Waiting for the boot disk

XP looks for the boot partition (`IopMarkBootPartition`; 0x7B `INACCESSIBLE_BOOT_DEVICE` if it is
missing) right after the boot drivers. On Gen1 that is the emulated IDE disk; on Gen2 the VMBus
disk is not there yet. vmbus.sys reports its devices (`IoInvalidateDeviceRelations`) only after
work items (`XPartReceiveMessageWorkItem`) have processed the host's channel offers, and one of
them is bound to processor 0, which the boot thread (`Phase1Initialization`) holds at priority 31
for the whole boot driver phase. Until that phase ends, Plug and Play also handles device changes
only synchronously on the boot thread.

bootwait's boot driver reinitialization routine (`IoRegisterBootDriverReinitialization`) runs after
all boot drivers are initialized, when Plug and Play works on worker threads, and before the I/O
manager creates the ARC names and looks for the boot partition. It polls every 100 ms and sleeps in
between, freeing processor 0, so vmbus reports its children and Plug and Play starts storvsc and
the disk. It returns once the boot partition exists and the mount manager knows its volume (at once
if the disk is already there), or after `TimeoutSeconds` (REG_DWORD, default 30, at most 600).

- **Boot partition**: the one in `Control\SystemBootDevice` (e.g.
  `multi(0)disk(0)rdisk(0)partition(1)`). As in the kernel's ARC name code, the disk is found by
  its MBR signature, from the BIOS disk's `Identifier` under
  `HARDWARE\DESCRIPTION\System\MultifunctionAdapter` or a `signature()` ARC path, else as
  `\Device\Harddisk<rdisk>`. Devices are opened with `FILE_READ_ATTRIBUTES` only, so no volume is
  mounted.
- **Setup CD**: for a `cdrom(<n>)` ARC path (text-mode setup from a CD; on Gen2 a DVD drive on the
  same controller) it waits for a CD-ROM device (`IoGetConfigurationInformation()->CdRomCount`)
  instead, with no mount manager step.
- **Drive letter**: right after the boot drivers, `IoAssignDriveLetters` gets the boot volume's
  letter from the mount manager (`IOCTL_MOUNTMGR_NEXT_DRIVE_LETTER`) and sets `NtSystemRoot`. A
  volume that appears after the boot drivers on a device node not installed yet (the first boots of
  a new installation) reaches the mount manager only once Plug and Play has installed the node. The
  boot volume then gets no letter at all, `NtSystemRoot` stays at the nonexistent `C:\WINDOWS` and
  smss stops with 0xC000021A (`STATUS_OBJECT_PATH_NOT_FOUND`), whichever partition XP is on. So
  bootwait asks the mount manager (`IOCTL_MOUNTMGR_QUERY_POINTS`) about
  `\Device\Harddisk<n>\Partition<m>` and, if it is unknown, announces it once with the documented
  `IOCTL_MOUNTMGR_VOLUME_ARRIVAL_NOTIFICATION` and polls until it is registered.

On the Gen2 test VM (kernel debugger, before the mount manager step existed) it waited 0.8 s:

```
bootwait: boot device multi(0)disk(0)rdisk(0)partition(1)
bootwait: BIOS disk 0 identifier ed9575aa-dc6edc6e-A
bootwait: waiting up to 30 s for partition 1 of the disk with signature dc6edc6e
bootwait: 0 disk(s) at 0 ms
bootwait: 1 disk(s) at 687 ms
bootwait: boot partition is \Device\Harddisk0\Partition1 (after 828 ms)
```

The log now ends with `the mount manager has the boot volume (after <n> ms)` (plus `, announced by
bootwait` if it had to) or `a CD-ROM is there (after <n> ms)`; on timeout with `no boot partition`,
`the mount manager does not know the boot volume` or `no CD-ROM`, then `after <n> s, giving up`.

Install it with `bootwait.inf` (DefaultInstall), or offline as a kernel service with `Type=1`,
`Start=0` (boot) and `ImagePath=system32\DRIVERS\bootwait.sys`, in any load order group.

## Repairing VMBus device nodes

On XP the Integration Services' INFs install NULL drivers named "... (not supported)" on several
VMBus devices. `DriverEntry` repairs these nodes on every boot, before vmbus.sys reports its
children, so Plug and Play reads the repaired values. An INF of our own cannot: XP prefers signed
drivers whatever the match and ranks an unsigned one 0x8000, which brings up the Found New Hardware
wizard whatever the signing policy.

**`RepairStorvsc`** (REG_DWORD, default 0; off in `bootwait.inf`, on in the converter). On an XP
moved from Gen1, the first Gen2 boot binds the new SCSI controller node to storvsc through the
CriticalDeviceDatabase; user-mode Plug and Play then installs `storvsc.inf`'s XP section, a NULL
driver, which deletes `Service` from the node (the next boot stops with 0x7B) and from
`CriticalDeviceDatabase\vmbus#{ba6163d9-...}` (so a disk that has booted once stops with 0x7B in
any VM whose controller is a new node: another VM, an imported copy). The node cannot be prepared
in advance, as its instance GUID is per VM. When the value is nonzero, bootwait:

- writes `Service=storvsc` into every `Enum\VMBUS\<device>\<instance>` without a service whose
  first hardware ID is `VMBUS\{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f}` (the SCSI controller class),
  leaving `ConfigFlags`, `Driver` and the driver key alone;
- writes the CriticalDeviceDatabase entry's `Service` back;
- sets `FriendlyName` "Microsoft Hyper-V SCSI Controller" for Device Manager, from the second Gen2
  boot on (the node does not exist before the first, which asks for a restart anyway).

```
bootwait: loaded, timeout 30 s, 1 device class(es) and 0 value(s) to repair
bootwait: Service=storvsc restored on VMBUS\{8b693a5d-...}\4&22ffa449&0&{8b693a5d-...} (status 00000000)
bootwait: FriendlyName "Microsoft Hyper-V SCSI Controller" set on VMBUS\{8b693a5d-...}\4&22ffa449&0&{8b693a5d-...} (status 00000000)
bootwait: boot partition is \Device\Harddisk0\Partition1 (after 328 ms)
```

**`Devices`** does the same for devices that work with a driver or service installed by other
means, one subkey per device class:

| Value          | Type   | Meaning |
|----------------|--------|---------|
| `HardwareID`   | REG_SZ | first hardware ID of the device, `VMBUS\{<type guid>}` |
| `Service`      | REG_SZ | optional: service written into nodes that have none |
| `FriendlyName` | REG_SZ | optional: name written into the node |
| `ClassGUID`, `Class` | REG_SZ | optional: class (and its name) written into nodes that have none |

An entry with a `Service` also repairs its class's CriticalDeviceDatabase entry; one with the SCSI
controller's ID replaces `RepairStorvsc`. Limits: `HardwareID` 79 characters, `Service`,
`ClassGUID`, `Class` 39, `FriendlyName` 99. A longer string is left out (not shortened) and logged:
a long `HardwareID` drops the entry, a long `Service` or `FriendlyName` only itself, a long
`ClassGUID` or `Class` both. `migrate/inject.ps1 -DeviceFix` writes the table. The converter uses
it to give devices that the Integration Services leave without a driver on Gen2 (among them the
Activation component and the two Enhanced Session Remote Desktop channels, otherwise nameless in
"Other devices") the names and class "System" of the Windows 8 INF `wvmic.inf`.

## Registry values

On a new VM's first boot, Plug and Play reinstalls the Integration Services' devices and their INFs
rewrite their values (e.g. `Parameters\ServiceDll` of the services that run `icsvc.dll`), losing
offline changes. bootwait writes the strings in `Values\<n>` on every boot, before the service
control manager reads them, where they differ:

| Value  | Type   | Meaning |
|--------|--------|---------|
| `Key`  | REG_SZ | key below `CurrentControlSet`, e.g. `Services\vmicguestinterface\Parameters`; it has to exist |
| `Name` | REG_SZ | name of the value |
| `Data` | REG_SZ | the string; the value becomes REG_EXPAND_SZ if it contains a `%`, REG_SZ otherwise |

`Key` and `Data` take at most 99 characters, `Name` 39; an entry with a longer string is ignored.
`migrate/inject.ps1` uses it for the Guest Service Interface's `ServiceDll`.

## Files

```
bootwait.c    the reinitialization routine: device repair, waiting, mount manager announcement
bootwait.inf  installs bootwait as a boot-start service on an installed XP
```
