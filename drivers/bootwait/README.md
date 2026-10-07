# bootwait.sys

XP looks for the boot partition (`IopMarkBootPartition`, bug check 0x7B
`INACCESSIBLE_BOOT_DEVICE` if it is missing) as soon as the boot drivers
have run. On Hyper-V Gen2 the boot disk sits behind the VMBus SCSI
controller, and the kernel debugger shows why it is not there by then:

- `vmbus.sys` starts during the boot driver phase and answers the first
  bus relations query with no children. It learns about its devices from
  the host's channel offers, which it processes in work items
  (`XPartReceiveMessageWorkItem`), and reports them later with
  `IoInvalidateDeviceRelations`.
- One of those work items binds itself to processor 0. The boot thread
  (`Phase1Initialization`) runs the whole boot driver phase on processor 0
  at priority 31, so the work item stays ready but never runs; the other
  processors are idle. At the bug check, the VMBus device node has no
  children at all and `\Driver\storvsc` has no device objects.
- Until all boot drivers are initialized, Plug and Play only processes
  device changes synchronously on the boot thread, after each boot driver.

On Gen1 the same XP boots from the emulated IDE disk, so nothing waits for
VMBus devices.

Apart from the registry repairs described below, which `DriverEntry` does,
bootwait only registers a boot driver
reinitialization routine (`IoRegisterBootDriverReinitialization`). The I/O
manager calls these routines after all boot drivers are initialized, when
Plug and Play already handles device changes on worker threads, and before
it creates the ARC names and looks for the boot partition. The routine
polls every 100 ms and sleeps in between, which frees processor 0. vmbus
then reports its children, and Plug and Play starts the storvsc adapter and
the disk while the boot thread waits. The routine returns as soon as the
boot partition exists and the mount manager knows its volume (see
[The boot volume's drive letter](#the-boot-volumes-drive-letter)), or after
`Services\bootwait\Parameters\TimeoutSeconds` (default 30, at most 600); on
a machine whose boot disk is already there it returns at once.

The boot partition is the one in `Control\SystemBootDevice` (e.g.
`multi(0)disk(0)rdisk(0)partition(1)`). Like the kernel's ARC name code,
bootwait finds the disk by its MBR signature, taken from the BIOS disk's
`Identifier` under `HARDWARE\DESCRIPTION\System\MultifunctionAdapter`
or from a `signature()` ARC path; without one it uses
`\Device\Harddisk<rdisk>`. It opens devices with `FILE_READ_ATTRIBUTES`
only, so it never mounts a volume.

On the Gen2 test VM (under the kernel debugger, before the mount manager
check below existed) it waited about 0.8 s:

```
bootwait: boot device multi(0)disk(0)rdisk(0)partition(1)
bootwait: BIOS disk 0 identifier ed9575aa-dc6edc6e-A
bootwait: waiting up to 30 s for partition 1 of the disk with signature dc6edc6e
bootwait: 0 disk(s) at 0 ms
bootwait: 1 disk(s) at 687 ms
bootwait: boot partition is \Device\Harddisk0\Partition1 (after 828 ms)
```

Text-mode setup booted from a CD has a `cdrom(<n>)` ARC path, and the boot
device the kernel looks for is the CD-ROM drive (on Gen2 a DVD drive on the
same VMBus SCSI controller). For such a path bootwait waits until there is
a CD-ROM device (`IoGetConfigurationInformation()->CdRomCount`) instead of
a partition, and logs `bootwait: a CD-ROM is there (after <n> ms)` or
`bootwait: no CD-ROM after <n> s, giving up`. The mount manager step below
only concerns a boot partition.

Install it with `bootwait.inf` (DefaultInstall), or offline as a kernel
service with `Type=1`, `Start=0` (boot) and
`ImagePath=system32\DRIVERS\bootwait.sys`. The load order group does not
matter: the routine runs after all boot drivers.

## The boot volume's drive letter

Right after the boot drivers, the I/O manager gives the boot partition its
drive letter (`IoAssignDriveLetters` asks the mount manager with
`IOCTL_MOUNTMGR_NEXT_DRIVE_LETTER`) and sets `NtSystemRoot` to it. The mount
manager hears of a volume when the volume manager registers the volume's
mounted device interface. For a volume that turns up after the boot drivers,
as the VMBus disk does, and whose device node is not installed yet (the
first boots of a new installation), that registration waits until Plug and
Play has installed the node. The boot volume then gets no drive letter at
all, `NtSystemRoot` stays at the default `C:\WINDOWS`, which does not exist,
and smss stops the boot with 0xC000021A (`STATUS_OBJECT_PATH_NOT_FOUND`).
This happens whichever partition XP is on.

So once the boot partition is there, bootwait asks the mount manager with
`IOCTL_MOUNTMGR_QUERY_POINTS` whether it knows
`\Device\Harddisk<n>\Partition<m>`. If it does, the routine returns. If not,
the routine announces the volume once with the mount manager's documented
`IOCTL_MOUNTMGR_VOLUME_ARRIVAL_NOTIFICATION` and polls until the mount
manager has registered it. The last line of its log is then
`bootwait: the mount manager has the boot volume (after <n> ms)`, with
`, announced by bootwait` if it had to announce the volume. A timeout in
this step is logged as
`bootwait: the mount manager does not know the boot volume after <n> s, giving up`,
one before the partition exists as `no boot partition after <n> s`.

## RepairStorvsc

An XP moved over from Gen1 has a second problem with the SCSI controller.
On the first Gen2 boot the controller is a new device node, which the kernel
binds to storvsc through the CriticalDeviceDatabase. User-mode Plug and Play
then finishes the installation with the best driver it finds, the
Integration Services' `storvsc.inf`, whose section for Windows XP installs a
NULL driver. That deletes the device's `Service` value; the running boot is
not affected, but the next one stops with 0x7B. XP prefers signed drivers
over unsigned ones regardless of how well they match, so an INF of our own
cannot take the device over, and the controller's VMBus instance GUID
differs from VM to VM, so the device node cannot be prepared in advance.

With `Services\bootwait\Parameters\RepairStorvsc` (REG_DWORD) set to
nonzero, `DriverEntry` walks `Enum\VMBUS\<device>\<instance>` and writes
`Service=storvsc` into every instance whose first hardware ID is
`VMBUS\{ba6163d9-04a1-4d29-b605-72e2ffb1dc7f}` (the SCSI controller class)
and that has no service. `ConfigFlags`, `Driver` and the driver key stay as
the NULL installation left them. The NULL INF also names the device
"Microsoft Hyper-V SCSI Controller (not supported)", so the repair writes a
`FriendlyName` too, which Device Manager shows instead. This runs before
vmbus.sys reports its children, so Plug and Play reads the repaired value:

```
bootwait: loaded, timeout 30 s, 1 device class(es) to repair
bootwait: Service=storvsc restored on VMBUS\{8b693a5d-...}\4&22ffa449&0&{8b693a5d-...} (status 00000000)
bootwait: FriendlyName "Microsoft Hyper-V SCSI Controller" set on VMBUS\{8b693a5d-...}\4&22ffa449&0&{8b693a5d-...} (status 00000000)
bootwait: boot partition is \Device\Harddisk0\Partition1 (after 328 ms)
```

The NULL installation also deletes the `Service` value of the controller
class's `CriticalDeviceDatabase\vmbus#{ba6163d9-...}` entry, through which
the kernel binds a device node it has not seen before to storvsc. Without
that value a disk that has booted once stops with 0x7B in any VM whose
controller is a new device node (another VM, an imported copy; the
controller's instance GUID is per VM). So bootwait writes the entry's
`Service` back as well, on every boot.

The device node does not exist before the first Gen2 boot, so the name
appears from the second boot on (the first one asks for a restart anyway).
The default is 0, and `bootwait.inf` leaves it off; the converter turns it
on.

## Other VMBus devices

The Integration Services' INFs install NULL drivers for more devices than
the SCSI controller on Windows XP, and name them "(not supported)". For those
that work with a driver or service that is installed by other means,
`Parameters\Devices` holds a table. Each subkey is one device class:

| Value          | Type   | Meaning |
|----------------|--------|---------|
| `HardwareID`   | REG_SZ | first hardware ID of the device, `VMBUS\{<type guid>}` |
| `Service`      | REG_SZ | optional: service written into nodes that have none |
| `FriendlyName` | REG_SZ | optional: name written into the node |
| `ClassGUID`, `Class` | REG_SZ | optional: class (and its name) written into nodes that have none |

The Integration Services leave a few devices of a Generation 2 VM without
any driver, among them the Activation component and the two Remote Desktop
channels of an Enhanced Session. XP installs its own NULL driver on them,
which leaves them nameless in the class "Other devices". The converter gives
them the names and the class "System" of the Windows 8 INF (`wvmic.inf`)
through this table. An INF of our own would do it properly, but it is
unsigned: XP lowers its rank to 0x8000, which makes Plug and Play show the
Found New Hardware wizard instead of installing it quietly, whatever the
driver signing policy says.

`RepairStorvsc` is the entry for the SCSI controller; an entry in the table
with the same hardware ID replaces it. An entry with a `Service` repairs
the Critical Device Database entry of its class too (see above).
`migrate/inject.ps1 -DeviceFix` writes the table. The strings have fixed
sizes: a `HardwareID` of at most 79 characters, `Service`, `ClassGUID` and
`Class` of at most 39, `FriendlyName` of at most 99. A longer string is not
shortened but left out, and logged to the kernel debugger: an entry with a
longer `HardwareID` is ignored, a longer `Service` or `FriendlyName` is not
written while the rest of the entry still is, and a longer `ClassGUID` or
`Class` leaves out both.

## Registry values

Plug and Play installs the Integration Services' devices again on the first
boot of a new VM, and the INF of each service writes its values again, for
example `Parameters\ServiceDll` of the services that run `icsvc.dll`.
Whatever was changed offline there is lost. `Parameters\Values\<n>` holds a
table of string values that bootwait writes on every boot, before the
service control manager reads them:

| Value  | Type   | Meaning |
|--------|--------|---------|
| `Key`  | REG_SZ | key below `CurrentControlSet`, e.g. `Services\vmicguestinterface\Parameters`; it has to exist |
| `Name` | REG_SZ | name of the value |
| `Data` | REG_SZ | the string; the value becomes REG_EXPAND_SZ if it contains a `%`, REG_SZ otherwise |

A value is only written if it differs. `Key` and `Data` are limited to 99
characters and `Name` to 39; an entry with longer strings is ignored.
`migrate/inject.ps1` fills this table for the Guest Service Interface's
`ServiceDll`.
