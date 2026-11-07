#!/usr/bin/python3 -I
"""Installer backend. scan/plan/simulate never open a disk for writing.

Real installs require root, the root-owned installed helper, a marked booted USB,
fresh device identities, an unused destination, and an exact erase confirmation.
No caller-provided shell commands or mount paths are accepted.
"""
import argparse
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import uuid

MIB = 1024 * 1024
INSTALLED = Path('/usr/lib/makepad-arch-installer/helper.py')
MARKER = Path('/etc/makepad/installer-source.json')
SNAPSHOTS = Path('/.makepad-install-snapshots')
EFI = 'c12a7328-f81f-11d2-ba4b-00a0c93ec93b'
BIOS = '21686148-6449-6e6f-744e-656564454649'
ENV = {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C'}


def emit(kind, **fields):
    print(json.dumps(dict(kind=kind, **fields)), flush=True)


def run(*args, **kwargs):
    p = subprocess.run([str(a) for a in args], env=ENV, text=True,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kwargs)
    if p.returncode:
        raise RuntimeError(f'{args[0]} failed: {(p.stderr or p.stdout).strip()}')
    return p.stdout.strip()


def descendants(disk):
    yield disk
    for child in disk.get('children', []):
        yield from descendants(child)


def devices():
    fields = 'NAME,PATH,KNAME,TYPE,TRAN,SIZE,RO,RM,MODEL,SERIAL,WWN,MAJ:MIN,LOG-SEC,PARTTYPE,FSTYPE,UUID,MOUNTPOINTS'
    return json.loads(run('lsblk', '--json', '--bytes', '--output', fields))['blockdevices']


def identity(disk):
    return {k: disk.get(k) for k in ('path', 'maj:min', 'size', 'serial', 'wwn', 'model', 'log-sec')}


def fingerprint(disk):
    return hashlib.sha256(json.dumps(identity(disk), sort_keys=True).encode()).hexdigest()


def root_device(disks):
    root = json.loads(run('findmnt', '--json', '--target', '/', '--output', 'SOURCE,FSTYPE,UUID,FSROOT'))['filesystems'][0]
    path = os.path.realpath(root['source'].split('[')[0])
    for disk in disks:
        for part in descendants(disk):
            if part['path'] == path:
                return disk, part, root
    raise RuntimeError('The running system is not on a supported physical disk.')


def source_info(disks):
    disk, part, root = root_device(disks)
    if disk.get('tran') != 'usb' or root['fstype'] != 'btrfs' or root.get('fsroot') != '/':
        raise RuntimeError('Boot this computer from the Makepad USB to enable installation.')
    marker_stat = MARKER.stat()
    if marker_stat.st_uid != 0 or marker_stat.st_mode & 0o022 or MARKER.is_symlink():
        raise RuntimeError('The installer USB marker must be owned by root and not writable by other users.')
    if json.loads(MARKER.read_text()).get('root_uuid') != root['uuid']:
        raise RuntimeError('The installer marker does not match the booted USB filesystem.')
    parts = disk.get('children', [])
    if len(parts) != 4 or any(p['type'] != 'part' for p in parts):
        raise RuntimeError('This installer needs the Makepad USB layout: BIOS, EFI, Btrfs, and setup data.')
    if ((parts[0].get('parttype') or '').lower() != BIOS
            or (parts[1].get('parttype') or '').lower() != EFI
            or parts[2]['path'] != part['path']
            or parts[3].get('fstype') != 'iso9660'):
        raise RuntimeError('The USB partition layout is not supported; no disk will be changed.')
    return disk, parts, root


def layout(source_parts, destination):
    sector = int(destination.get('log-sec') or 512)
    if sector not in (512, 4096):
        raise RuntimeError('Unsupported destination sector size.')
    align = MIB // sector
    last = int(destination['size']) // sector - math.ceil(16384 / sector) - 2
    bios_start, bios_end = align, 2 * align - 1
    efi_start = bios_end + 1
    efi_end = efi_start + math.ceil(source_parts[1]['size'] / MIB) * align - 1
    seed_sectors = math.ceil(source_parts[3]['size'] / sector)
    seed_start = ((last + 1 - seed_sectors) // align) * align
    root_start, root_end = efi_end + 1, seed_start - 1
    root_bytes = (root_end - root_start + 1) * sector
    if root_bytes < source_parts[2]['size']:
        raise RuntimeError('The destination is too small for this USB. Choose a larger drive.')
    return [(bios_start, bios_end, 'ef02'), (efi_start, efi_end, 'ef00'),
            (root_start, root_end, '8300'), (seed_start, last, '0700')]


def blocked_reason(disk, root_disk, source, parts):
    if disk['type'] != 'disk':
        return 'Not a whole physical disk'
    if disk['path'] == root_disk['path']:
        return 'Running system / source USB — protected'
    if disk.get('ro'):
        return 'Read-only drive'
    if not disk.get('serial') and not disk.get('wwn'):
        return 'No stable hardware identity; installation is blocked'
    for child in descendants(disk):
        if any(child.get('mountpoints') or []):
            return 'Mounted or used as swap — unmount it first'
        if child['type'] not in ('disk', 'part'):
            return 'An active storage mapping uses this drive'
        holders = Path('/sys/class/block') / child['kname'] / 'holders'
        if not holders.is_dir() or any(holders.iterdir()):
            return 'The drive is in use, or its usage could not be checked'
    if source is not None:
        try:
            layout(parts, disk)
        except RuntimeError as err:
            return str(err)
    return ''


def describe(disk, reason=''):
    return dict(path=disk['path'], name=(disk.get('model') or disk['name']).strip(),
                size=disk['size'], serial=disk.get('serial') or disk.get('wwn') or '',
                fingerprint=fingerprint(disk), reason=reason,
                partitions=[dict(path=p['path'], size=p['size'], filesystem=p.get('fstype') or 'Unknown',
                                 mounts=p.get('mountpoints') or []) for p in disk.get('children', [])])


def demo_scan():
    # These are deliberately not /dev paths. Simulation never turns them into block devices.
    source = dict(path='simulation:usb', name='Makepad installer USB (simulated)', size=512_000_000_000,
                  serial='DEMO-USB', fingerprint='simulation-source', reason='Protected source USB')
    target = dict(path='simulation:internal', name='Internal SSD (simulated)', size=1_000_000_000_000,
                  serial='DEMO-INTERNAL', fingerprint='simulation-target', reason='', partitions=[
                      dict(path='EFI boot partition', size=512*MIB, filesystem='FAT32', mounts=[]),
                      dict(path='Existing system and files', size=900_000_000_000, filesystem='NTFS', mounts=[])])
    return dict(kind='scan', simulation=True, source=source, source_error='', destinations=[target],
                description='Linux, Makepad WM, all apps, AI models, and Source Library')


def scan(simulation=False):
    if simulation:
        result = demo_scan()
        # Real disks are shown for orientation only; none is a simulation destination.
        if sys.platform == 'linux':
            for d in devices():
                if d['type'] == 'disk':
                    result['destinations'].append(describe(d, 'Real drive — protected in simulation'))
        return result
    disks = devices()
    root_disk, _, _ = root_device(disks)
    source, parts, error = None, [], ''
    try:
        source, parts, _ = source_info(disks)
    except (RuntimeError, OSError, ValueError, KeyError) as err:
        error = str(err)
    return dict(kind='scan', simulation=False, source=describe(source) if source else None,
                source_error=error, destinations=[describe(d, blocked_reason(d, root_disk, source, parts))
                                                 for d in disks if d['type'] == 'disk'],
                description='The complete installed system, apps, models, and library data')


def check_request(request, simulation=False):
    state = scan(simulation)
    if state['source_error']:
        raise RuntimeError(state['source_error'])
    target = next((d for d in state['destinations'] if d['path'] == request.get('destination')), None)
    if target is None or target['reason']:
        raise RuntimeError(target['reason'] if target else 'The selected drive is no longer connected.')
    if target['fingerprint'] != request.get('destination_fingerprint'):
        raise RuntimeError('The selected drive changed. Scan again and review its identity.')
    if state['source']['fingerprint'] != request.get('source_fingerprint'):
        raise RuntimeError('The source USB changed. Scan again.')
    if request.get('cleanup') not in ('fast', 'full'):
        raise RuntimeError('Choose fast boot cleanup or full overwrite.')
    return state, target


def plan(request, simulation=False):
    state, target = check_request(request, simulation)
    return dict(kind='plan', simulation=simulation, source=state['source'], destination=target,
                cleanup=request['cleanup'], confirmation='ERASE ' + target['path'], steps=[
                    'Check both drive identities and take read-only USB snapshots',
                    ('Overwrite every addressable sector with zeros' if request['cleanup'] == 'full'
                     else 'Clear old boot partitions, boot sectors, GPT/MBR tables, and filesystem signatures'),
                    'Create fresh partitions using the destination drive capacity',
                    'Copy the complete USB system, apps, models, and library',
                    'Install GRUB for UEFI and BIOS, with Makepad selected to boot automatically',
                    'Verify the copy, flush writes, and unmount the new installation'])


def simulate(request):
    p = plan(request, True)
    emit('started', simulation=True)
    for index, step in enumerate(p['steps']):
        for tick in range(9):
            emit('progress', phase=step, percent=round((index + tick/9) / len(p['steps']) * 100),
                 detail='SIMULATION — no disks are opened for writing')
            time.sleep(0.10)
    emit('complete', simulation=True, percent=100,
         detail='Simulation complete. No drive, partition, or boot sector was changed.')


def stream(args, phase, start, span, total_bytes=None, **kwargs):
    """Consume bounded progress lines without retaining the full rsync transcript."""
    p = subprocess.Popen([str(a) for a in args], env=ENV, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT, **kwargs)
    line, last_line, last_emit = bytearray(), '', 0.0
    try:
        while True:
            chunk = p.stdout.read(1)
            if not chunk:
                break
            if chunk not in (b'\n', b'\r'):
                line.extend(chunk)
                continue
            text = line.decode('utf-8', errors='replace').strip()
            line.clear()
            if not text:
                continue
            last_line = text
            percent = re.search(r'(\d+)%', text)
            written = re.match(r'([\d,]+) bytes', text)
            fraction = int(percent[1])/100 if percent else (
                min(int(written[1].replace(',', ''))/total_bytes, 1) if written and total_bytes else 0)
            if time.monotonic() - last_emit >= 0.3:
                emit('progress', phase=phase, percent=round(start + span*fraction), detail=text)
                last_emit = time.monotonic()
        if p.wait():
            raise RuntimeError(f'{phase}: {last_line or "command failed"}')
    finally:
        if p.poll() is None:
            p.terminate()
            p.wait()


def overwrite(fd, size, phase, start, span):
    stream(['dd', 'if=/dev/zero', f'of=/proc/self/fd/{fd}', 'bs=16M', f'count={size}',
            'iflag=count_bytes', 'oflag=direct', 'conv=fsync', 'status=progress'],
           phase, start, span, size, pass_fds=(fd,))


def clear_edges(fd, size):
    length = min(8*MIB, size)
    zero = bytes(MIB)
    for start, end in [(0, length), (max(length, size-length), size)]:
        offset = start
        while offset < end:
            count = os.pwrite(fd, zero[:min(len(zero), end-offset)], offset)
            if not count:
                raise RuntimeError('The disk stopped accepting writes.')
            offset += count
    os.fsync(fd)


def open_block(device):
    fd = os.open(device['path'], os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW)
    st = os.fstat(fd)
    expected = ':'.join((str(os.major(st.st_rdev)), str(os.minor(st.st_rdev))))
    if not stat.S_ISBLK(st.st_mode) or expected != device['maj:min']:
        os.close(fd)
        raise RuntimeError('The device identity changed before opening it.')
    return fd


def usb_boot_files(efi, root_uuid):
    loader = (efi/'EFI/BOOT/BOOTX64.EFI').read_bytes()
    config_path = efi/'EFI/BOOT/grub.cfg'
    configuration = config_path.read_text()
    # This is the Makepad USB's adjacent-config loader. A different loader
    # could retain an embedded USB UUID and silently boot the source drive.
    if (not loader.startswith(b'MZ') or b'normal $cmdpath/grub.cfg\n' not in loader
            or f'--set=root {root_uuid}' not in configuration
            or f'root=UUID={root_uuid}' not in configuration
            or '/boot/vmlinuz-linux' not in configuration
            or 'initrd /boot/initramfs-linux.img' not in configuration):
        raise RuntimeError('The USB does not have the supported relocatable Makepad bootloader.')
    run('grub-script-check', config_path)
    return loader, configuration


def preflight(source, source_parts):
    required = ['btrfs', 'rsync', 'sgdisk', 'wipefs', 'mkfs.btrfs', 'mkfs.fat', 'mount', 'umount',
                'grub-install', 'grub-script-check', 'grub-editenv', 'grub-fstest',
                'udevadm', 'blockdev', 'dd', 'blkid', 'sync', 'cp']
    if Path('/sys/firmware/efi').is_dir():
        required.append('efibootmgr')
    missing = [name for name in required if not Path('/usr/bin', name).is_file()]
    if missing:
        raise RuntimeError('Required tools are missing: ' + ', '.join(missing))
    if Path('/sys/firmware/efi').is_dir():
        run('efibootmgr')  # Refuse before erasure when firmware variables are inaccessible.
    if 'Total devices 1' not in run('btrfs', 'filesystem', 'show', '--raw', '/'):
        raise RuntimeError('Only a single-device USB filesystem is supported.')
    if any(line.strip() for line in run('btrfs', 'subvolume', 'list', '-s', '/').splitlines()):
        raise RuntimeError('Remove or export existing USB snapshots before installing; they can require additional capacity.')
    subvolumes = []
    for line in run('btrfs', 'subvolume', 'list', '/').splitlines():
        rel = line.split(' path ', 1)[1]
        path = Path(rel)
        if path.is_absolute() or '..' in path.parts or '<FS_TREE>' in path.parts:
            raise RuntimeError('Unsupported subvolume path: ' + rel)
        subvolumes.append(rel)
    if SNAPSHOTS.exists():
        raise RuntimeError('A previous installer snapshot directory remains. Resolve that installation before retrying.')
    # Swap must be recreated rather than snapshotted/copied while it is active.
    if Path('/swap').is_dir() and any(p.name != 'swapfile' for p in Path('/swap').iterdir()):
        raise RuntimeError('The swap directory contains other files; move them before installation.')
    for line in Path('/etc/fstab').read_text().splitlines():
        fields = line.split()
        if not fields or fields[0].startswith('#'):
            continue
        if fields[1] not in ('/', '/boot/efi') and not (fields[0] == '/swap/swapfile' and fields[2] == 'swap'):
            raise RuntimeError('Additional fstab mounts need a custom installation: ' + fields[1])
    for path in ['/boot/grub/grub.cfg', '/boot/vmlinuz-linux', '/boot/initramfs-linux.img']:
        if not Path(path).is_file():
            raise RuntimeError('A required USB boot file is missing: ' + path)
    for platform in ('x86_64-efi', 'i386-pc'):
        if not Path('/usr/lib/grub', platform).is_dir():
            raise RuntimeError('GRUB boot modules are missing: ' + platform)
    # Validate the working USB loader before any destination erasure.
    with tempfile.TemporaryDirectory(prefix='makepad-boot-preflight-', dir='/run') as directory:
        run('mount', '-o', 'ro', source_parts[1]['path'], directory)
        try:
            usb_boot_files(Path(directory), source_parts[2]['uuid'])
        finally:
            run('umount', directory)
    # Ensure only virtual/system mounts and this USB's subvolumes are omitted by the snapshot.
    mounts = json.loads(run('findmnt', '--json', '--list', '--output', 'TARGET,FSTYPE,SOURCE'))['filesystems']
    for mount in mounts:
        target = mount['target']
        if any(target == p or target.startswith(p+'/') for p in ('/dev', '/proc', '/sys', '/run', '/tmp')):
            continue
        if target in ('/', '/boot/efi') or mount['fstype'] in ('tmpfs', 'cgroup2', 'efivarfs'):
            continue
        if mount['source'].split('[')[0] == source_parts[2]['path']:
            continue
        raise RuntimeError('Unmount the extra filesystem before installing: ' + target)
    return sorted(subvolumes, key=lambda p: (p.count('/'), p))


def copy_tree(source, destination, exclusions, phase, start, span):
    args = ['rsync', '-aHAXS', '--numeric-ids', '--one-file-system', '--omit-dir-times']
    args += [f'--exclude=/{name}' for name in exclusions]
    stream(args + ['--info=progress2', str(source)+'/', str(destination)+'/'], phase, start, span)
    difference = run(*args, '--dry-run', '--itemize-changes', str(source)+'/', str(destination)+'/')
    if difference:
        raise RuntimeError('The copied file tree did not verify: ' + difference[:2000])


def verify_boot_files(dest, root_device):
    # rsync -S can leave a trailing Btrfs hole in a kernel image. Linux reads
    # those bytes as zeroes, but GRUB can stop early and refuse to boot it.
    files = sorted(path for path in (dest/'boot').iterdir()
                   if path.is_file() and not path.is_symlink()
                   and (path.name.startswith('vmlinuz') or path.suffix == '.img'))
    for path in files:
        with path.open('rb') as source:
            hole = os.lseek(source.fileno(), 0, os.SEEK_HOLE)
        if hole < path.stat().st_size:
            fd, name = tempfile.mkstemp(prefix='.'+path.name+'-', dir=path.parent)
            os.close(fd)
            temporary = Path(name)
            try:
                run('cp', '--reflink=never', '--sparse=never', '--preserve=all', '--', path, temporary)
                with temporary.open('rb') as copied:
                    if (temporary.stat().st_size != path.stat().st_size
                            or os.lseek(copied.fileno(), 0, os.SEEK_HOLE) < temporary.stat().st_size):
                        raise RuntimeError('Could not materialize the boot file: ' + path.name)
                    os.fsync(copied.fileno())
                os.replace(temporary, path)
            finally:
                temporary.unlink(missing_ok=True)
    # Read through GRUB's actual filesystem driver after committing Btrfs
    # metadata; Linux file checks alone do not catch this boot failure.
    run('sync', '-f', dest)
    for path in files:
        run('grub-fstest', root_device, 'cmp', '/boot/'+path.name, path)


def configure_boot(dest, efi, source_efi, target, expected_fingerprint, source_uuid, root_uuid):
    """Called only by the authenticated real installer, after copying the target."""
    current = next(d for d in devices() if d['path'] == target['path'])
    if fingerprint(current) != expected_fingerprint:
        raise RuntimeError('The destination changed before installing its bootloader.')
    defaults = dest/'etc/default/grub'
    contents = defaults.read_text()
    settings = dict(GRUB_DEFAULT='0', GRUB_TIMEOUT='3', GRUB_TIMEOUT_STYLE='hidden',
                    GRUB_SAVEDEFAULT='false', GRUB_DISABLE_OS_PROBER='true',
                    GRUB_DISTRIBUTOR='"Makepad"')
    for key, value in settings.items():
        contents = re.sub(r'^\s*'+key+r'=.*\n?', '', contents, flags=re.MULTILINE)
        contents += f'\n{key}={value}\n'
    defaults.write_text(contents)
    # Preserve the boot chain that already started this computer. Rebuilding
    # GRUB and its configuration can change firmware memory use and make a
    # large, otherwise working portable initramfs fail with out of memory.
    # Its storage drivers are already included; the root UUID is a boot arg.
    loader, configuration = usb_boot_files(source_efi, source_uuid)
    configuration = configuration.replace(source_uuid, root_uuid)
    configuration = re.sub(r'\bresume(?:_offset)?=[^\s"\']+', '', configuration)
    configuration = re.sub(r'^set timeout=.*$', 'set timeout=3', configuration, flags=re.MULTILINE)
    configuration = configuration.replace("menuentry 'Makepad Arch - Ethernet / USB'", "menuentry 'Makepad Linux'")
    if source_uuid in configuration:
        raise RuntimeError('The installed boot configuration still references the USB.')
    for directory, name in (('BOOT', 'BOOTX64.EFI'), ('Makepad', 'grubx64.efi')):
        output = efi/'EFI'/directory
        output.mkdir(parents=True, exist_ok=True)
        (output/name).write_bytes(loader)
        (output/'grub.cfg').write_text(configuration)
        run('grub-script-check', output/'grub.cfg')
        if (output/name).read_bytes() != loader:
            raise RuntimeError('The copied EFI bootloader did not verify.')
    (dest/'boot/grub/grub.cfg').write_text(configuration)
    run('grub-editenv', dest/'boot/grub/grubenv', 'create')
    # BIOS needs a new core image embedded in the destination's BIOS partition.
    run('grub-install', '--target=i386-pc', '--boot-directory='+str(dest/'boot'), target['path'])
    run('grub-script-check', dest/'boot/grub/grub.cfg')
    configuration = (dest/'boot/grub/grub.cfg').read_text()
    if (root_uuid not in configuration or 'set timeout=3' not in configuration
            or not (efi/'EFI/BOOT/BOOTX64.EFI').is_file()
            or not (efi/'EFI/Makepad/grubx64.efi').is_file()):
        raise RuntimeError('The installed boot configuration did not verify.')
    root_partition = next(p for p in descendants(current) if p.get('uuid') == root_uuid)
    verify_boot_files(dest, root_partition['path'])
    if Path('/sys/firmware/efi').is_dir():
        before = run('efibootmgr')
        existing = set(re.findall(r'^Boot([0-9A-Fa-f]{4})\*?\s', before, re.MULTILINE))
        run('efibootmgr', '--create', '--disk', target['path'], '--part', '2',
            '--label', 'Makepad', '--loader', r'\EFI\Makepad\grubx64.efi')
        after = run('efibootmgr')
        entries = set(re.findall(r'^Boot([0-9A-Fa-f]{4})\*?\s+Makepad(?:\s|$)', after, re.MULTILINE))
        created = entries - existing
        if len(created) != 1:
            raise RuntimeError('The new Makepad firmware boot entry could not be identified.')
        entry = created.pop()
        order = re.search(r'^BootOrder:\s*([0-9A-Fa-f,]+)', after, re.MULTILINE)
        remaining = [item for item in order[1].split(',') if item.upper() != entry.upper()] if order else []
        run('efibootmgr', '--bootorder', ','.join([entry] + remaining))
        run('efibootmgr', '--bootnext', entry)
        verified = run('efibootmgr')
        if not re.search(r'^BootOrder:\s*'+entry+r'(?:,|$)', verified, re.MULTILINE | re.IGNORECASE):
            raise RuntimeError('The firmware did not retain Makepad as the first boot entry.')


def install(request):
    if os.geteuid() != 0:
        raise RuntimeError('Administrator authentication is required.')
    if Path(__file__).resolve() != INSTALLED:
        raise RuntimeError('Real installation is only available through the installed root-owned helper.')
    for path in (INSTALLED, INSTALLED.parent):
        st = path.stat()
        if st.st_uid != 0 or st.st_mode & 0o022:
            raise RuntimeError('The privileged helper is not safely installed.')
    state, destination = check_request(request)
    if request.get('confirmation') != 'ERASE ' + destination['path'] or request.get('acknowledged') is not True:
        raise RuntimeError('The exact drive name and erase acknowledgement are required.')
    all_disks = devices()
    source, source_parts, root = source_info(all_disks)
    target = next(d for d in all_disks if d['path'] == destination['path'])
    partition_layout = layout(source_parts, target)
    subvolumes = preflight(source, source_parts)
    fd = open_block(target)
    mounts, snapshots, work = [], [], None
    finished = False
    own_snapshot_dir = False
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        check_request(request)  # Recheck mounts, swap, holders, identities immediately before preparing.
        work = Path(tempfile.mkdtemp(prefix='makepad-install-', dir='/run'))
        SNAPSHOTS.mkdir(mode=0o700)
        own_snapshot_dir = True
        emit('started', simulation=False)
        emit('progress', phase='Protecting the USB source', percent=1, detail='Taking read-only snapshots')
        for index, rel in enumerate([''] + [p for p in subvolumes if p != 'swap']):
            snap = SNAPSHOTS / str(index)
            run('btrfs', 'subvolume', 'snapshot', '-r', Path('/') / rel, snap)
            snapshots.append((rel, snap))
        # Capture the old partition identities before clearing their signatures.
        check_request(request)
        if request['cleanup'] == 'full':
            overwrite(fd, target['size'], 'Overwriting every sector', 3, 25)
        else:
            emit('progress', phase='Clearing old boot structures', percent=3,
                 detail='Removing boot partitions, filesystem signatures, and both partition tables')
            for old in target.get('children', []):
                child_fd = open_block(old)
                try:
                    if (old.get('parttype') or '').lower() in (EFI, BIOS):
                        overwrite(child_fd, old['size'], 'Clearing an old boot partition', 3, 15)
                    else:
                        run('wipefs', '--all', '--force', old['path'])
                        clear_edges(child_fd, old['size'])
                finally:
                    os.close(child_fd)
            run('wipefs', '--all', '--force', target['path'])
            clear_edges(fd, target['size'])
        emit('progress', phase='Creating the new drive layout', percent=29, detail='Fresh GPT, EFI, Btrfs, and setup partitions')
        # The open descriptor pins the selected block device, including through partition-table updates.
        args = ['sgdisk', '--clear']
        for index, (start, end, kind) in enumerate(partition_layout, 1):
            args += [f'--new={index}:{start}:{end}', f'--typecode={index}:{kind}']
        run(*args, f'/proc/self/fd/{fd}', pass_fds=(fd,))
        run('blockdev', '--rereadpt', target['path'])
        # udev takes a shared flock on the whole disk before processing any
        # partition. Waiting with our exclusive lock held deadlocks at 29%.
        # Keep the descriptor (device identity) and installer-wide lock, but
        # release the disk lock before notifying udev. An unlock alone does
        # not trigger a rescan while this writable descriptor remains open.
        fcntl.flock(fd, fcntl.LOCK_UN)
        disk_sysfs = Path('/sys/class/block') / target['kname']
        partition_sysfs = [p for p in disk_sysfs.iterdir() if (p/'partition').is_file()]
        if len(partition_sysfs) != len(partition_layout):
            raise RuntimeError('The kernel did not expose the new partitions; no filesystems were formatted.')
        # Wait only for this disk's events, not unrelated hardware queues.
        run('udevadm', 'trigger', '--action=change', '--settle', disk_sysfs, *partition_sysfs)
        refreshed = next(d for d in devices() if d['path'] == target['path'])
        if fingerprint(refreshed) != destination['fingerprint']:
            raise RuntimeError('The destination device changed during partitioning.')
        new_parts = refreshed.get('children', [])
        if len(new_parts) != 4 or any(any(p.get('mountpoints') or []) for p in new_parts):
            raise RuntimeError('The new partitions are missing or were mounted by another program.')
        run('mkfs.fat', '-F', '32', '-n', 'MAKEPAD_EFI', new_parts[1]['path'])
        run('mkfs.btrfs', '-f', '-L', 'Makepad', new_parts[2]['path'])
        dest = work / 'root'
        dest.mkdir()
        run('mount', '-o', 'compress=zstd:1', new_parts[2]['path'], dest)
        mounts.append(dest)
        excluded = ['dev/***', 'proc/***', 'sys/***', 'run/***', 'tmp/***', '.makepad-install-snapshots/***',
                    'swap/***', 'boot/efi/***'] + [p+'/***' for p in subvolumes]
        copy_tree(snapshots[0][1], dest, excluded, 'Copying the complete USB installation', 31, 49)
        for rel, snap in snapshots[1:]:
            output = dest / rel
            if output.exists():
                output.rmdir()  # Only the snapshot's empty subvolume stub may be replaced.
            output.parent.mkdir(parents=True, exist_ok=True)
            run('btrfs', 'subvolume', 'create', output)
            children = [p[len(rel)+1:]+'/***' for p in subvolumes if p.startswith(rel+'/')]
            copy_tree(snap, output, children, 'Copying system subvolumes', 80, 2)
        for name in ('dev', 'proc', 'sys', 'run', 'tmp', 'boot/efi'):
            (dest/name).mkdir(parents=True, exist_ok=True)
        os.chmod(dest/'tmp', 0o1777)
        if Path('/swap/swapfile').is_file():
            swap = dest/'swap'
            if swap.exists():
                swap.rmdir()
            run('btrfs', 'subvolume', 'create', swap)
            run('btrfs', 'filesystem', 'mkswapfile', '--size', str(Path('/swap/swapfile').stat().st_size), swap/'swapfile')
        emit('progress', phase='Copying USB setup data', percent=83, detail='Preserving the complete setup partition')
        seed_fd = open_block(new_parts[3])
        try:
            stream(['dd', 'if='+source_parts[3]['path'], f'of=/proc/self/fd/{seed_fd}', 'bs=16M',
                    'conv=fsync', 'status=progress'], 'Copying USB setup data', 83, 5,
                   source_parts[3]['size'], pass_fds=(seed_fd,))
        finally:
            os.close(seed_fd)
        efi = dest/'boot/efi'
        run('mount', new_parts[1]['path'], efi)
        mounts.append(efi)
        source_efi = work/'source-efi'
        source_efi.mkdir()
        run('mount', '-o', 'ro', source_parts[1]['path'], source_efi)
        mounts.append(source_efi)
        run('rsync', '-rt', str(source_efi)+'/', str(efi)+'/')
        root_uuid = run('blkid', '-s', 'UUID', '-o', 'value', new_parts[2]['path'])
        efi_uuid = run('blkid', '-s', 'UUID', '-o', 'value', new_parts[1]['path'])
        if not root_uuid or root_uuid == root['uuid'] or not efi_uuid:
            raise RuntimeError('The new filesystem identities could not be verified.')
        fstab = [f'UUID={root_uuid} / btrfs defaults,compress=zstd:1 0 0',
                 f'UUID={efi_uuid} /boot/efi vfat umask=0077 0 2']
        if (dest/'swap/swapfile').exists():
            fstab.append('/swap/swapfile none swap defaults 0 0')
        (dest/'etc/fstab').write_text('\n'.join(fstab)+'\n')
        # Preserve USB kernel options, but never its root/resume device.
        for path in (dest/'boot/grub/grub.cfg', dest/'etc/default/grub'):
            if path.is_file():
                text = path.read_text().replace(root['uuid'], root_uuid)
                text = re.sub(r'\bresume(?:_offset)?=[^\s"\']+', '', text)
                path.write_text(text)
        (dest/'etc/makepad/installer-source.json').unlink(missing_ok=True)
        emit('progress', phase='Installing GRUB and automatic boot', percent=90,
             detail='Fresh UEFI and BIOS bootloaders; Makepad boots by default after three seconds')
        configure_boot(dest, efi, source_efi, target, destination['fingerprint'], root['uuid'], root_uuid)
        emit('progress', phase='Flushing and unmounting', percent=97, detail='Finishing all writes before the drive can be used')
        run('sync', '-f', dest)
        finished = True
    finally:
        failures = []
        for mount in reversed(mounts):
            try:
                run('umount', mount)
            except RuntimeError as err:
                failures.append(str(err))
        for _, snap in reversed(snapshots):
            try:
                run('btrfs', 'subvolume', 'delete', snap)
            except RuntimeError as err:
                failures.append(str(err))
        if own_snapshot_dir and SNAPSHOTS.exists() and not any(SNAPSHOTS.iterdir()):
            SNAPSHOTS.rmdir()
        if work and not failures:
            shutil.rmtree(work)
        os.close(fd)
        if failures:
            raise RuntimeError('Cleanup needs attention: ' + '; '.join(failures))
    if finished:
        emit('complete', simulation=False, percent=100,
             detail='Installation complete. Shut down, unplug the USB, and boot the internal drive.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['scan', 'plan', 'simulate', 'install'])
    parser.add_argument('--simulation', action='store_true')
    parser.add_argument('--request', default='{}')
    args = parser.parse_args()
    try:
        request = json.loads(args.request)
        if not isinstance(request, dict):
            raise RuntimeError('The installation request must be a JSON object.')
        if args.command == 'scan':
            print(json.dumps(scan(args.simulation)), flush=True)
        elif args.command == 'plan':
            print(json.dumps(plan(request, args.simulation)), flush=True)
        elif args.command == 'simulate':
            simulate(request)
        else:
            if args.simulation:
                raise RuntimeError('A simulation request can never enter the real installer.')
            if os.geteuid() != 0:
                raise RuntimeError('Administrator authentication is required.')
            lock_fd = os.open('/run/makepad-arch-installer.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
            with os.fdopen(lock_fd, 'w') as lock:
                lock_stat = os.fstat(lock.fileno())
                if lock_stat.st_uid != 0 or lock_stat.st_mode & 0o022:
                    raise RuntimeError('The installation lock is not owned by root.')
                fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                install(request)
    except (RuntimeError, OSError, ValueError, KeyError, StopIteration) as err:
        emit('error', detail=str(err))
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
