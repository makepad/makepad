#!/usr/bin/env python3
"""Restore an installed clone's model trees to an already mounted Btrfs USB.

Run as root on the Linux clone. This does not format or repartition anything.
Example: sudo python3 restore_models.py --destination /run/makepad-usb \
    --seed /run/makepad-usb-seed --serial 03005423082225201936
The Btrfs send/receive copy retains shared extents, so repeated model files
do not expand beyond the USB's capacity. No model download is performed.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import time


def run(*args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def output(*args):
    return subprocess.check_output([str(arg) for arg in args], text=True).strip()


def inventory(root):
    result = {}
    for parent, directories, files in os.walk(root, followlinks=False):
        for name in sorted(directories + files):
            path = Path(parent)/name
            info = path.lstat()
            if stat.S_ISLNK(info.st_mode):
                entry = ['link', os.readlink(path)]
            elif stat.S_ISREG(info.st_mode):
                entry = ['file', info.st_size]
            elif stat.S_ISDIR(info.st_mode):
                entry = ['directory']
            else:
                raise ValueError(f'Unexpected model file type: {path}')
            result[str(path.relative_to(root))] = entry
    return result


def check_links(root, weights):
    count = 0
    for name, entry in inventory(weights).items():
        if entry[0] != 'link': continue
        logical = Path('/home/arch/.makepad/weights')/name
        target = Path(entry[1])
        if not target.is_absolute(): target = logical.parent/target
        target = Path(os.path.normpath(target))
        if not target.is_relative_to('/home/arch'):
            raise ValueError(f'Model link leaves the cloned home: {name} -> {target}')
        # Absolute targets must be checked inside the USB, not the live host.
        mapped = root/target.relative_to('/')
        if not mapped.resolve().is_relative_to(root.resolve()) or not mapped.is_file():
            raise ValueError(f'Model link does not resolve inside {root}: {name} -> {target}')
        count += 1
    return count


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--seed', type=Path, required=True)
    parser.add_argument('--serial', required=True, help='Expected external USB serial from lsblk')
    parser.add_argument('--source-home', type=Path, default=Path('/home/arch'))
    args = parser.parse_args()
    if os.geteuid() != 0: parser.error('Run as root on the Linux clone')
    destination = args.destination.resolve(strict=True)
    seed = args.seed.resolve(strict=True)
    home = args.source_home.resolve(strict=True)
    mount = json.loads(output('findmnt', '-J', '-M', destination))['filesystems'][0]
    if mount['fstype'] != 'btrfs' or destination == Path('/'):
        raise ValueError('Destination must be a separately mounted Btrfs USB root')
    device = mount['source'].split('[', 1)[0]
    parent = output('lsblk', '-ndo', 'PKNAME', device)
    if not parent or output('lsblk', '-ndo', 'TRAN', '/dev/'+parent) != 'usb':
        raise ValueError('Destination is not on a USB disk')
    if output('lsblk', '-ndo', 'SERIAL', '/dev/'+parent) != args.serial:
        raise ValueError('USB serial does not match the selected drive')
    seed_mount = json.loads(output('findmnt', '-J', '-M', seed))['filesystems'][0]
    if output('lsblk', '-ndo', 'PKNAME', seed_mount['source']) != parent or seed_mount['fstype'] != 'iso9660':
        raise ValueError('Seed must be the read-only ISO partition on the same USB')
    if not (seed/'clone.json').is_file() or not (seed/'ssh.sh').is_file():
        raise ValueError('USB seed lacks the cloned runtime and new SSH setup')
    if output('findmnt', '-no', 'FSTYPE', '-T', home) != 'btrfs':
        raise ValueError('Source models must be on Btrfs to retain shared extents')
    model_destination = destination/'home/arch/models'
    weights_destination = destination/'home/arch/.makepad/weights'
    for path in (model_destination, weights_destination):
        if path.exists(): raise ValueError(f'Refusing to replace an existing model tree: {path}')
    models = home/'models'
    weights = home/'.makepad/weights'
    check_links(Path('/'), weights)
    run('btrfs', 'filesystem', 'resize', 'max', destination)
    usage = output('btrfs', 'filesystem', 'du', '-s', '--raw', models).splitlines()[-1].split()
    required = int(usage[1]) + int(usage[2]) + 35_000_000_000
    free = shutil.disk_usage(destination).free
    if free < required:
        raise ValueError(f'Models plus OS reserve need {required} bytes; USB has {free} free')
    state = Path('/var/lib/makepad-model-transfer')
    stage = state/'payload'
    receive_directory = destination/'var/lib/makepad-model-transfer'
    if stage.exists() or receive_directory.exists():
        raise ValueError('A previous transfer exists; inspect it before restarting')
    state.mkdir(parents=True, exist_ok=True)
    receive_directory.mkdir(parents=True)
    run('btrfs', 'subvolume', 'create', stage)
    print('Creating a stable model snapshot using shared extents.', flush=True)
    run('cp', '-a', '--reflink=always', models, stage/'models')
    run('cp', '-a', '--reflink=always', weights, stage/'weights')
    # Each booted machine generates its own AI Hub identity and lock file.
    for name in ('node-key', 'service.lock'):
        (stage/'weights'/name).unlink(missing_ok=True)
    run('btrfs', 'property', 'set', '-ts', stage, 'ro', 'true')
    expected = {'models': inventory(stage/'models'), 'weights': inventory(stage/'weights')}
    print(f'Model snapshot ready; about {required-35_000_000_000:,} unique bytes plus model links.', flush=True)
    (state/'inventory.json').write_text(json.dumps(expected, indent=2)+'\n')
    started = time.monotonic()
    last = started
    transferred = 0
    sender = subprocess.Popen(['btrfs', 'send', '--proto', '2', '--compressed-data', str(stage)], stdout=subprocess.PIPE)
    receiver = subprocess.Popen(['btrfs', 'receive', str(receive_directory)], stdin=subprocess.PIPE)
    try:
        while block := sender.stdout.read(4*1024*1024):
            receiver.stdin.write(block)
            transferred += len(block)
            now = time.monotonic()
            if now-last >= 10:
                print(f'Transferred {transferred/1e9:.1f} GB in {now-started:.0f}s ({transferred/(now-started)/1e6:.0f} MB/s).', flush=True)
                last = now
        receiver.stdin.close()
        if sender.wait() != 0 or receiver.wait() != 0:
            raise RuntimeError('Btrfs model transfer failed; source snapshot retained')
    finally:
        for process in (sender, receiver):
            if process.poll() is None:
                process.terminate()
                process.wait()
    received = receive_directory/'payload'
    for name in ('models', 'weights'):
        if inventory(received/name) != expected[name]:
            raise ValueError(f'Received {name} inventory differs from the source snapshot')
    model_destination.parent.mkdir(parents=True, exist_ok=True)
    weights_destination.parent.mkdir(parents=True, exist_ok=True)
    owner = home.stat()
    for path in (model_destination.parent, weights_destination.parent):
        os.chown(path, owner.st_uid, owner.st_gid)
    run('cp', '-a', '--reflink=always', received/'models', model_destination)
    run('cp', '-a', '--reflink=always', received/'weights', weights_destination)
    for name, path in (('models', model_destination), ('weights', weights_destination)):
        if inventory(path) != expected[name]:
            raise ValueError(f'Installed {name} inventory differs from the source snapshot')
    links = check_links(destination, weights_destination)
    print(f'Model paths and sizes match; all {links} model links resolve inside the USB.', flush=True)
    # Install and enable the same units that first-boot provisioning uses.
    # Conditions keep them inactive until package setup has completed.
    for unit in ('makepad-wm.service', 'makepad-aihub.service'):
        target = destination/'etc/systemd/system'/unit
        shutil.copyfile(seed/unit, target)
        target.chmod(0o644)
        run('systemctl', '--root='+str(destination), 'enable', unit)
        run('systemctl', '--root='+str(destination), 'is-enabled', unit)
    for name in ('wm-session.sh', 'aihub-session.sh'):
        target = destination/'usr/local/bin'/('makepad-'+name.removesuffix('.sh'))
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(seed/name, target)
        target.chmod(0o755)
    config = destination/'etc/makepad'
    config.mkdir(parents=True, exist_ok=True)
    for name in ('wm.env', 'aihub.env'):
        shutil.copyfile(seed/name, config/name)
    run('btrfs', 'subvolume', 'delete', received)
    receive_directory.rmdir()
    run('btrfs', 'filesystem', 'sync', destination)
    print('Reading back the USB filesystem with a read-only Btrfs scrub.', flush=True)
    run('btrfs', 'scrub', 'start', '-B', '-r', destination)
    report = {'serial': args.serial, 'stream_bytes': transferred, 'model_links': links,
              'model_files': sum(e[0] == 'file' for e in expected['models'].values()),
              'model_logical_bytes': sum(e[1] for e in expected['models'].values() if e[0] == 'file'),
              'wm_autostart': True, 'aihub_autostart': True,
              'completed_at': time.strftime('%Y-%m-%dT%H:%M:%S%z')}
    path = destination/'var/lib/makepad-model-restore.json'
    path.write_text(json.dumps(report, indent=2)+'\n')
    run('btrfs', 'filesystem', 'sync', destination)
    run('btrfs', 'subvolume', 'delete', stage)
    print('Model restoration verified. WM and AI Hub are enabled for boot.', flush=True)


if __name__ == '__main__':
    main()
