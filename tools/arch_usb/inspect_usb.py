#!/usr/bin/env python3
"""Read persistent boot logs from the known USB through an authorized read-only fd."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import stat
import struct
import subprocess
import sys
import time
from btrfs_read import Btrfs, u32, u64
from usb_device import snapshot_device, verify_device

def main():
    if sys.flags.optimize:
        raise RuntimeError('Python optimization is not allowed; safety assertions must remain enabled')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--device', required=True)
    parser.add_argument('--device-fd', type=int, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        raise RuntimeError('Root is required to inspect the USB')
    fd = args.device_fd
    snapshot = snapshot_device(args.device, writable=False)
    device = snapshot['device']
    if fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_ACCMODE != os.O_RDONLY:
        raise ValueError('Device descriptor must be read-only')
    opened = os.fstat(fd)
    expected = os.stat(f'/dev/r{device}')
    if not stat.S_ISCHR(opened.st_mode) or opened.st_rdev != expected.st_rdev:
        raise ValueError('Passed descriptor is not the selected raw disk')
    snapshot = verify_device(snapshot, writable=False)
    device = snapshot['device']
    subprocess.run(['diskutil', 'unmountDisk', f'/dev/{device}'], check=True)
    output = args.output
    if output.exists():
        owner = output.stat()
    else:
        owner = output.parent.stat()
        output.mkdir(exist_ok=True)
        os.chown(output, owner.st_uid, owner.st_gid)
    report = {'time': time.strftime('%Y-%m-%dT%H:%M:%S%z'), 'device': device, 'read_only': True, 'files': {}}
    with os.fdopen(fd, 'rb', buffering=0) as disk:
        disk.seek(0)
        gpt = disk.read(4096)
        assert gpt[512:520] == b'EFI PART'
        root_start = struct.unpack_from('<Q', gpt, 1024+256+32)[0]
        fs = Btrfs(disk, root_start*512)
        report.update(root_uuid=fs.uuid, generation=u64(fs.sb, 72))
        paths = ['/var/log/makepad-firstboot.log', '/var/log/cloud-init.log', '/var/log/cloud-init-output.log',
                 '/var/log/makepad-provision.log', '/var/lib/makepad-firstboot/complete',
                 '/var/lib/makepad-provision/status', '/var/lib/cloud/data/status.json',
                 '/etc/sudoers.d/10-arch', '/etc/ssh/sshd_config.d/00-makepad.conf',
                 '/home/arch/.ssh/authorized_keys', '/etc/systemd/network/10-cloud-init-eth0.network']
        try:
            instance = fs.read('/var/lib/cloud/instance').decode().strip().split('/')[-1]
            assert '/' not in instance and instance not in ('', '.', '..')
            paths += [f'/var/lib/cloud/instances/{instance}/'+name for name in ('user-data.txt', 'cloud-config.txt', 'datasource')]
        except FileNotFoundError: pass
        for path in paths:
            try:
                data = fs.read(path, 32*1024*1024)
                target = output/path.strip('/').replace('/', '__')
                target.write_bytes(data); os.chown(target, owner.st_uid, owner.st_gid)
                report['files'][path] = {'bytes': len(data), 'saved': str(target)}
                print(f'{path}: {len(data)} bytes', flush=True)
            except FileNotFoundError:
                report['files'][path] = {'missing': True}
                print(f'{path}: missing', flush=True)
        passwd = fs.read('/etc/passwd').decode()
        report['arch_passwd_entry'] = next((line for line in passwd.splitlines() if line.startswith('arch:')), None)
        shadow = fs.read('/etc/shadow').decode()
        entry = next((line.split(':') for line in shadow.splitlines() if line.startswith('arch:')), None)
        report['arch_password_present'] = bool(entry and entry[1] and not entry[1].startswith(('!', '*')))
        for path in ('/home/arch', '/home/arch/.ssh', '/home/arch/.ssh/authorized_keys'):
            try:
                inode = fs.lookup(path)
                raw = next(data for key, data in fs.index[inode] if key[1] == 1)
                report.setdefault('permissions', {})[path] = {'uid': u32(raw, 44), 'gid': u32(raw, 48), 'mode': oct(u32(raw, 52))}
            except FileNotFoundError: pass
    path = output/'report.json'
    path.write_text(json.dumps(report, indent=2)+'\n'); os.chown(path, owner.st_uid, owner.st_gid)
    print('Persistent logs captured. USB was only read and remains connected.', flush=True)

if __name__ == '__main__':
    main()
