"""Read and verify the identity of an explicitly selected macOS USB disk.

An IOMedia registry ID pins one attachment, not a reusable disk number. A plan
must be prepared again after reconnecting the drive or rebooting the Mac.
Nothing in this module opens, unmounts, or writes a device.
"""
import plistlib
import re
import subprocess


def normalize_device(value):
    match = re.fullmatch(r'(?:/dev/r?)?(disk[0-9]+)', str(value))
    if not match:
        raise ValueError('Select a whole disk such as diskN or /dev/diskN, not a partition')
    return match[1]


def _plist(command):
    return plistlib.loads(subprocess.check_output(command))


def _media_ids(nodes, device):
    result = set()
    for node in nodes:
        if node.get('BSD Name') == device and node.get('Whole') is True:
            value = node.get('IORegistryEntryID')
            if isinstance(value, int) and value > 0:
                result.add(value)
        result.update(_media_ids(node.get('IORegistryEntryChildren', []), device))
    return result


def snapshot_device(device, *, writable=True):
    device = normalize_device(device)
    info = _plist(['/usr/sbin/diskutil', 'info', '-plist', device])
    if not (info.get('DeviceIdentifier') == device
            and info.get('WholeDisk') is True and info.get('Internal') is False
            and info.get('VirtualOrPhysical') == 'Physical'
            and info.get('BusProtocol') == 'USB'):
        raise ValueError('The target must be a whole external physical USB disk')
    if writable and info.get('Writable') is not True:
        raise ValueError('The selected USB disk is not writable')
    size = info.get('TotalSize')
    if (info.get('DeviceBlockSize') != 512 or type(size) is not int
            or size <= 0 or size % 512):
        raise ValueError('The Arch image layout requires a disk with 512-byte logical sectors')
    name = info.get('IORegistryEntryName')
    if not isinstance(name, str) or not name:
        raise ValueError('The USB disk has no reported media name')
    entries = _plist(['/usr/sbin/ioreg', '-a', '-r', '-c', 'IOMedia'])
    ids = _media_ids(entries, device)
    if len(ids) != 1:
        raise ValueError('Cannot identify exactly one current USB disk attachment')
    boot = subprocess.check_output(
        ['/usr/sbin/sysctl', '-n', 'kern.bootsessionuuid'], text=True).strip()
    if not boot:
        raise ValueError('Cannot identify the current macOS boot session')
    return {'device': device, 'size': size, 'block_size': 512,
            'registry_name': name, 'media_entry_id': ids.pop(),
            'boot_session_uuid': boot}


def verify_device(plan, *, writable=True):
    current = snapshot_device(plan['device'], writable=writable)
    if any(plan.get(key) != value for key, value in current.items()):
        raise ValueError('The selected USB disk or its attachment changed; prepare a new plan')
    return current
