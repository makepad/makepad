#!/usr/bin/env python3
"""Write a prepared Arch image to its explicitly selected and verified USB disk.

Use --check-device for read-only identity verification. --write requires macOS
administrator authentication; the native writer also supplies its raw disk fd.
"""
import argparse
import binascii
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import stat
import struct
import subprocess
import time

from usb_device import normalize_device, verify_device


def require(condition, message):
    if not condition:
        raise ValueError(message)


def load_plan(base):
    plan = json.loads((base/'write-plan.json').read_text())
    require(normalize_device(plan['device']) == plan['device'], 'Invalid plan device')
    require(type(plan['size']) is int and plan['size'] > 0
            and plan['size'] % 512 == 0 and plan['block_size'] == 512, 'Invalid disk geometry')
    components = plan['components']
    require(isinstance(components, list) and len(components) == 3, 'Expected three image components')
    names = set()
    ranges = []
    for component in components:
        name = component['path']
        require(isinstance(name, str) and name not in ('', '.', '..')
                and Path(name).name == name, 'Component paths must be filenames beside the plan')
        path = base/name
        require(not path.is_symlink() and path.resolve().parent == base,
                'An image component must stay inside the prepared output directory')
        offset, length = component['offset'], component['length']
        require(type(offset) is int and type(length) is int
                and offset >= 0 and length > 0 and offset % 512 == length % 512 == 0
                and offset + length <= plan['size'], 'Invalid image component extent')
        require(path.is_file() and path.stat().st_size == length, f'Component size changed: {name}')
        require(name not in names, 'Duplicate image component')
        names.add(name)
        ranges.append((offset, offset + length))
    ranges.sort()
    require(all(left[1] <= right[0] for left, right in zip(ranges, ranges[1:])),
            'Image component extents overlap')
    return plan


def check_descriptor(fd, device):
    opened = os.fstat(fd)
    expected = os.stat(f'/dev/r{device}')
    require(stat.S_ISCHR(opened.st_mode) and opened.st_rdev == expected.st_rdev,
            'Passed descriptor is not the selected raw disk')
    require(fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_ACCMODE == os.O_RDWR,
            'The raw disk descriptor must allow read-back verification')


def write(base, plan, device_fd):
    require(os.geteuid() == 0, 'Use macOS administrator authentication for the USB write')
    info = verify_device(plan)
    device = info['device']
    print(f'Confirmed {device}: {info["registry_name"]}, {info["size"]} bytes', flush=True)
    for component in plan['components']:
        with (base/component['path']).open('rb') as stream:
            require(hashlib.file_digest(stream, 'sha256').hexdigest() == component['sha256'],
                    f'Image hash changed: {component["path"]}')
    if device_fd is not None:
        check_descriptor(device_fd, device)
    # Hashing large images takes time. Recheck the actual attachment afterwards.
    verify_device(plan)
    print('All image hashes verified. Unmounting the selected USB disk.', flush=True)
    subprocess.run(['/usr/sbin/diskutil', 'unmountDisk', f'/dev/{device}'], check=True)
    verify_device(plan)
    chunk_size = 4*1024*1024
    with (os.fdopen(device_fd, 'r+b', buffering=0) if device_fd is not None
          else open(f'/dev/r{device}', 'r+b', buffering=0)) as output:
        check_descriptor(output.fileno(), device)
        verify_device(plan)
        for component in plan['components']:
            print(f'Writing {component["path"]}: {component["length"]} bytes at {component["offset"]}', flush=True)
            output.seek(component['offset'])
            with (base/component['path']).open('rb') as stream:
                remaining = component['length']
                while remaining:
                    block = stream.read(min(chunk_size, remaining))
                    if not block:
                        raise RuntimeError('Image component shrank during writing')
                    view = memoryview(block)
                    while view:
                        done = output.write(view)
                        if not done:
                            raise RuntimeError('Short disk write')
                        view = view[done:]
                    remaining -= len(block)
                require(not stream.read(1), 'Image component grew during writing')
            try:
                os.fsync(output.fileno())
            except OSError as error:
                if error.errno not in (errno.EINVAL, errno.ENOTSUP, errno.ENOTTY):
                    raise
        try:
            fcntl.fcntl(output.fileno(), 51)  # F_FULLFSYNC on macOS
        except OSError:
            subprocess.run(['/bin/sync'], check=True)
        print('Reading back every written byte for SHA256 verification.', flush=True)
        for component in plan['components']:
            output.seek(component['offset'])
            remaining = component['length']
            digest = hashlib.sha256()
            while remaining:
                block = output.read(min(chunk_size, remaining))
                if not block:
                    raise RuntimeError('Short disk read-back')
                digest.update(block)
                remaining -= len(block)
            require(digest.hexdigest() == component['sha256'],
                    f'Read-back mismatch: {component["path"]}')
            print(f'Verified {component["path"]}', flush=True)
        for lba in (1, plan['size']//512-1):
            output.seek(lba*512)
            header = bytearray(output.read(512))
            require(len(header) == 512 and header[:8] == b'EFI PART', 'Invalid GPT header')
            size, crc = struct.unpack_from('<II', header, 12)
            require(92 <= size <= 512, 'Invalid GPT header size')
            struct.pack_into('<I', header, 16, 0)
            require(binascii.crc32(header[:size]) == crc, 'GPT header checksum mismatch')
            table, count, entry_size, entries_crc = struct.unpack_from('<QIII', header, 72)
            require(count == 128 and entry_size == 128
                    and table*512 + count*entry_size <= plan['size'], 'Invalid GPT entry table')
            output.seek(table*512)
            entries = output.read(count*entry_size)
            require(len(entries) == count*entry_size and binascii.crc32(entries) == entries_crc,
                    'GPT entry checksum mismatch')
    print('Both GPT copies and all payloads verified.', flush=True)
    subprocess.run(['/usr/sbin/diskutil', 'eject', f'/dev/{device}'], check=True)
    record = {'device': device, 'media': info['registry_name'], 'size': info['size'],
              'verified_components': plan['components'],
              'completed_at': time.strftime('%Y-%m-%dT%H:%M:%S%z')}
    (base/'write-complete.json').write_text(json.dumps(record, indent=2)+'\n')
    print('USB write verified and drive ejected.', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument('--check-device', action='store_true', help='Read identity only; never open or unmount the disk')
    action.add_argument('--write', action='store_true')
    parser.add_argument('--device-fd', type=int)
    args = parser.parse_args()
    if args.device_fd is not None and not args.write:
        parser.error('--device-fd is only valid with --write')
    base = Path(__file__).resolve().parent
    plan = load_plan(base)
    if args.check_device:
        print(json.dumps(verify_device(plan)))
    else:
        write(base, plan, args.device_fd)


if __name__ == '__main__':
    main()
