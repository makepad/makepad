#!/usr/bin/env python3
"""Assemble a persistent Arch USB on macOS using Python 3.14 and Apple tools.

Example:
  python3 tools/arch_usb/build.py --assets /path/to/arch-assets \
      --output /path/to/arch-image --device diskN --writer-app

Select --device with `diskutil list external physical`. An attached USB is
required for identity and size. After reconnect or reboot, rebuild the prepared
plan to avoid stale disk-number reuse. Inputs are supplied by the caller; this
command does not download anything.

The assets directory supplies arch-base.raw, resolved-packages.json, and seed/
containing mirror.tar.*, realtek-firmware.tar, fanatec.tar.gz and cargo-cache.tar.
Use --base-image for a different pristine raw Arch cloud image. All executable
build/setup/writer logic lives beside this script, independently of the cache.
The source snapshot is refreshed from this checkout unless --cached-source is set.

Outputs: clean arch-boot.raw, CIDATA ISO, backup GPT, a pinned write-plan.json,
and a validation report. Device identity is read-only metadata; this command
does not write the USB. The optional native writer checks the selected physical
USB and all hashes, replaces the image, verifies every written byte, and ejects
after macOS authentication.
"""
import argparse
import binascii
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import struct
import subprocess
import sys
import tarfile
import time
import uuid

from btrfs_read import Btrfs
from efi_boot import Fat, prepare
from iso9660 import validate_seed
from usb_device import snapshot_device

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
BLOCK = 512
WAIT_UNITS = ('systemd-networkd-wait-online.service',
              'systemd-time-wait-sync.service', 'pacman-init.service')

def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def copy(source, target):
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, target)

def package_source(seed, assets):
    names = subprocess.check_output(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=REPO).split(b'\0')
    if (REPO/'Cargo.lock').is_file():
        names.append(b'Cargo.lock')
    count = 0
    with tarfile.open(seed/'makepad-source.tgz', 'w:gz', compresslevel=3) as archive:
        for raw in sorted(set(names)):
            if not raw: continue
            relative = Path(os.fsdecode(raw))
            if any(part in ('local', '.claude', '.grok', '__pycache__') or part.startswith('target') for part in relative.parts): continue
            if relative.name.startswith('.env') or relative.suffix in ('.log', '.pem', '.key', '.pyc'): continue
            path = REPO/relative
            if path.is_relative_to(seed.parent) or path.is_relative_to(assets): continue
            if not path.is_file() and not path.is_symlink(): continue
            if path.is_symlink():
                try: path.resolve().relative_to(REPO)
                except ValueError: continue
            archive.add(path, arcname=str(relative), recursive=False)
            count += 1
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip()
    (seed/'source-revision.txt').write_text(f'Base HEAD: {head}\nWorking-tree snapshot: {time.strftime("%Y-%m-%dT%H:%M:%S%z")}\nFiles: {count}\n')
    print(f'Packaged current Makepad source: {count} files.', flush=True)

def prepare_seed(args, output):
    seed = output/'seed'
    if seed.exists(): shutil.rmtree(seed)
    seed.mkdir()
    original = args.assets/'seed'
    metadata = json.loads((args.assets/'resolved-packages.json').read_text())
    packages = (HERE/'packages.txt').read_text().splitlines()
    assert set(packages) <= {item['NAME'][0] for item in metadata}, 'Requested package absent from the supplied cache'
    assert 'pacman' in packages and not {'iwd', 'wireless-regdb', 'cloud-init'} & set(packages)
    for path in sorted(original.glob('mirror.tar.*')):
        copy(path, seed/('mirror-'+path.suffix[1:]+'.tar'))
    assert list(seed.glob('mirror-*.tar')), 'Offline package mirror is missing'
    for name in ('realtek-firmware.tar', 'fanatec.tar.gz', 'cargo-cache.tar'):
        copy(original/name, seed/('fanatec.tgz' if name == 'fanatec.tar.gz' else name))
    # Verify the source cache's original payload hashes before reusing it.
    cached_hashes = dict(line.split('  ', 1)[::-1] for line in (original/'SHA256SUMS').read_text().splitlines())
    for path in seed.iterdir():
        original_name = ('mirror.tar.'+path.stem.removeprefix('mirror-')) if path.name.startswith('mirror-') else ('fanatec.tar.gz' if path.name == 'fanatec.tgz' else path.name)
        assert digest(path) == cached_hashes[original_name], f'Cached payload changed: {path.name}'
    if args.cached_source:
        for name, target in (('makepad-source.tar.gz', 'makepad-source.tgz'), ('SOURCE-REVISION.txt', 'source-revision.txt')): copy(original/name, seed/target)
    else:
        package_source(seed, args.assets)
    cef_archive = args.cef_archive or args.assets/'cef-linux.tar.bz2'
    assert cef_archive.is_file(), f'Cached Linux CEF archive missing: {cef_archive}'
    # Keep the untouched distribution, including resources, helper libraries
    # and headers. A build never needs to fetch CEF from the internet.
    with tarfile.open(cef_archive, 'r:bz2') as archive:
        members = archive.getnames()
        roots = {Path(name).parts[0] for name in members if Path(name).parts}
        assert len(roots) == 1, 'Expected one CEF distribution directory'
        cef_root = next(iter(roots))
        assert not cef_root.startswith('.') and cef_root.endswith('_linux64')
        assert all(not Path(name).is_absolute() and '..' not in Path(name).parts for name in members)
        assert cef_root+'/Release/libcef.so' in members
        assert cef_root+'/include/cef_version.h' in members
    copy(cef_archive, seed/'cef-linux.bz2')
    (seed/'cef-directory.txt').write_text(cef_root+'\n')
    for name in ('firstboot.sh', 'provision.sh', 'mount-win.sh', 'status.sh', 'wm-session.sh'):
        copy(HERE/name, seed/name)
        subprocess.run(['/bin/bash', '-n', str(seed/name)], check=True)
    for name in ('makepad-wm.service', 'wm.env'):
        copy(HERE/name, seed/name)
    (seed/'requested-packages.txt').write_text('\n'.join(packages)+'\n')
    key = args.ssh_key.read_text().strip()
    assert key.startswith('ssh-ed25519 ') and '\n' not in key, 'Expected one Ed25519 public key'
    (seed/'authorized_keys').write_text(key+'\n')
    (seed/'sshd.conf').write_text('PubkeyAuthentication yes\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nAuthenticationMethods publickey\nPermitRootLogin no\nAllowUsers arch\n')
    (seed/'wired.network').write_text('[Match]\nName=eth* en*\n\n[Network]\nDHCP=ipv4\nIPv6AcceptRA=yes\n\n[Link]\nRequiredForOnline=no\n')
    (seed/'no-wireless.conf').write_text(''.join(f'blacklist {name}\ninstall {name} /bin/false\n' for name in ('bluetooth', 'btusb', 'cfg80211', 'mac80211')))
    (seed/'fanatec-access.rules').write_text('SUBSYSTEM=="usb", ATTR{idVendor}=="0eb7", GROUP="games", MODE="0660"\nSUBSYSTEM=="hidraw", ATTRS{idVendor}=="0eb7", GROUP="games", MODE="0660", TAG+="uaccess"\n')
    (seed/'makepad-provision.service').write_text('''[Unit]
Description=Install the cached Makepad development stack
After=sshd.service network.target
Wants=sshd.service
ConditionPathExists=!/var/lib/makepad-provision/complete

[Service]
Type=exec
ExecStart=/usr/local/sbin/makepad-provision
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
''')
    (seed/'makepad-mount-win.service').write_text('''[Unit]
Description=Mount the Windows NTFS volume read-only at /mnt/win
After=local-fs.target

[Service]
Type=oneshot
ExecStart=/usr/local/sbin/makepad-mount-win
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
''')
    # Execute the setup directly from durable read-only media. Avoid generated
    # write_files/runcmd files and their once-per-instance state on the root.
    command = '''set -eu
mkdir -p /run/makepad-seed
if ! mountpoint -q /run/makepad-seed; then
    device=/dev/disk/by-label/CIDATA
    test -e "$device" || device=/dev/disk/by-label/cidata
    mount -o ro "$device" /run/makepad-seed
fi
exec /usr/bin/bash /run/makepad-seed/firstboot.sh'''
    config = {'cloud_init_modules': ['bootcmd'], 'cloud_config_modules': [],
              'cloud_final_modules': [], 'growpart': {'mode': 'off'},
              'resize_rootfs': False, 'users': [],
              'bootcmd': [['/usr/bin/bash', '-c', command]]}
    (seed/'user-data').write_text('#cloud-config\n'+json.dumps(config, indent=2)+'\n')
    (seed/'meta-data').write_text(json.dumps({'instance-id': args.build_id, 'local-hostname': 'makepad-arch'})+'\n')
    # The installed renderer uses the interface's name, not nested match.name.
    (seed/'network-config').write_text(json.dumps({'version': 2, 'renderer': 'networkd', 'ethernets': {'eth0': {'dhcp4': True, 'dhcp6': False, 'optional': True}}}, indent=2)+'\n')
    small = [p for p in seed.iterdir() if p.name.endswith(('.sh', '.conf', '.network', '.rules', '.service')) or p.name in ('authorized_keys', 'realtek-firmware.tar')]
    (seed/'config.sha256').write_text(''.join(f'{digest(p)}  {p.name}\n' for p in sorted(small)))
    (seed/'sha256sums').write_text(''.join(f'{digest(p)}  {p.name}\n' for p in sorted(seed.iterdir())))
    iso = output/'makepad-seed.iso'
    subprocess.run(['hdiutil', 'makehybrid', '-o', str(iso), str(seed), '-iso', '-joliet', '-iso-volume-name', 'CIDATA', '-joliet-volume-name', 'CIDATA', '-ov'], check=True)
    expected = {p.name: digest(p) for p in seed.iterdir()}
    validate_seed(iso, expected)
    print('Plain ISO9660/Linux and Joliet filenames and contents verified.', flush=True)
    # Also read back through macOS' filesystem driver.
    attached = plistlib.loads(subprocess.check_output(['hdiutil', 'attach', '-readonly', '-nobrowse', '-plist', str(iso)]))
    mounted = next(entity for entity in attached['system-entities'] if 'mount-point' in entity)
    try:
        root = Path(mounted['mount-point'])
        for line in (seed/'sha256sums').read_text().splitlines():
            expected, name = line.split('  ', 1)
            assert digest(root/name) == expected, f'ISO readback mismatch: {name}'
    finally:
        subprocess.run(['hdiutil', 'detach', mounted['dev-entry']], check=True)
    print('ISO payload readback verified.', flush=True)
    return iso

def assemble(args, output, iso):
    source = args.base_image
    with source.open('rb') as stream:
        mbr = bytearray(stream.read(512)); header = bytearray(stream.read(512))
        assert header[:8] == b'EFI PART', 'Expected a pristine raw GPT Arch cloud image'
        size, crc = struct.unpack_from('<II', header, 12)
        check = bytearray(header[:size]); struct.pack_into('<I', check, 16, 0)
        assert binascii.crc32(check) == crc
        table_lba, count, entry_size, table_crc = struct.unpack_from('<QIII', header, 72)
        stream.seek(table_lba*BLOCK); entries = bytearray(stream.read(count*entry_size))
        assert binascii.crc32(entries) == table_crc and entry_size == 128 and count == 128
        assert not any(entries[3*entry_size:])
        efi_start, efi_end = struct.unpack_from('<QQ', entries, 128+32)
        root_start, root_end = struct.unpack_from('<QQ', entries, 256+32)
        fs = Btrfs(stream, root_start*BLOCK)
        for name in ('pacman', 'sshd', 'sudo', 'visudo', 'bash', 'networkctl', 'systemctl'):
            assert fs.read('/usr/bin/'+name)[:4] == b'\x7fELF'
        assert b'@includedir /etc/sudoers.d' in fs.read('/etc/sudoers')
        assert fs.read('/etc/systemd/system/multi-user.target.wants/systemd-networkd.service') == b'/usr/lib/systemd/system/systemd-networkd.service'
        for path in ('/var/lib/makepad-firstboot/complete', '/var/lib/makepad-access/v2-complete'):
            try: fs.lookup(path)
            except FileNotFoundError: pass
            else: raise ValueError('Base image has already been used; supply the pristine download')
        fs.lookup('/boot/vmlinuz-linux'); fs.lookup('/boot/initramfs-linux.img')
        stream.seek(efi_start*BLOCK); esp = stream.read((efi_end-efi_start+1)*BLOCK)
    masks = ' '.join('systemd.mask='+name for name in WAIT_UNITS)
    config = f'''set timeout=2
set timeout_style=menu
set default=0
terminal_input console
terminal_output console
menuentry 'Makepad Arch - Ethernet / USB' {{
    insmod part_gpt
    insmod btrfs
    search --no-floppy --fs-uuid --set=root {fs.uuid}
    echo 'Starting Arch. Bluetooth and Wi-Fi disabled. Login: arch'
    linux /boot/vmlinuz-linux root=UUID={fs.uuid} rw net.ifnames=0 rootflags=compress=zstd:1 console=tty1 loglevel=3 module_blacklist=bluetooth,btusb,cfg80211,mac80211 systemd.show_status=auto {masks} nvidia_drm.modeset=1 nvidia_drm.fbdev=1
    initrd /boot/initramfs-linux.img
}}
'''.encode()
    loader_hash = hashlib.sha256(Fat(esp).read('/EFI/BOOT/BOOTX64.EFI')).hexdigest()
    esp, _ = prepare(esp, loader_hash, config)
    assert 'ACCESS.IMG' not in [e[0] for e in Fat(esp).entries(Fat(esp).lookup('/EFI/BOOT')[2])]
    (output/'grub.cfg').write_bytes(config)
    sectors = args.disk_bytes//BLOCK; array_sectors = len(entries)//BLOCK
    last_usable = sectors-array_sectors-2
    seed_sectors = (iso.stat().st_size+BLOCK-1)//BLOCK
    seed_start = ((last_usable+1-seed_sectors)//2048)*2048
    assert source.stat().st_size//BLOCK < seed_start
    struct.pack_into('<Q', entries, 256+40, seed_start-1)
    new = bytearray(128)
    new[:16] = uuid.UUID('0fc63daf-8483-4772-8e79-3d69d8477de4').bytes_le
    new[16:32] = uuid.uuid4().bytes_le
    struct.pack_into('<QQQ', new, 32, seed_start, seed_start+seed_sectors-1, 0)
    label = 'Makepad setup (CIDATA)'.encode('utf-16le'); new[56:56+len(label)] = label
    entries[384:512] = new
    header[56:72] = uuid.uuid4().bytes_le
    struct.pack_into('<QQ', header, 24, 1, sectors-1)
    struct.pack_into('<Q', header, 48, last_usable)
    struct.pack_into('<I', header, 88, binascii.crc32(entries))
    def finish(h):
        struct.pack_into('<I', h, 16, 0)
        struct.pack_into('<I', h, 16, binascii.crc32(h[:size]))
    finish(header)
    backup = bytearray(header)
    struct.pack_into('<QQ', backup, 24, sectors-1, 1)
    struct.pack_into('<Q', backup, 72, sectors-array_sectors-1); finish(backup)
    struct.pack_into('<I', mbr, 446+12, min(sectors-1, 0xffffffff))
    boot = output/'arch-boot.raw'; copy(source, boot)
    with boot.open('r+b') as stream:
        stream.write(mbr); stream.write(header)
        stream.seek(table_lba*BLOCK); stream.write(entries)
        stream.seek(efi_start*BLOCK); stream.write(esp)
        stream.seek(source.stat().st_size-(array_sectors+1)*BLOCK)
        stream.write(bytes((array_sectors+1)*BLOCK))
        stream.flush(); os.fsync(stream.fileno())
    tail = output/'gpt-tail.bin'; tail.write_bytes(entries+backup)
    # Root bytes and the stock initramfs must remain precisely the pristine base.
    with source.open('rb') as before, boot.open('rb') as after:
        before.seek(root_start*BLOCK); after.seek(root_start*BLOCK)
        remaining = (root_end-root_start+1)*BLOCK
        while remaining:
            n = min(4*1024*1024, remaining)
            assert before.read(n) == after.read(n), 'Pristine Linux root was modified'
            remaining -= n
    components = [{'path': p.name, 'offset': offset, 'length': p.stat().st_size, 'sha256': digest(p)} for p, offset in ((boot, 0), (iso, seed_start*BLOCK), (tail, (sectors-array_sectors-1)*BLOCK))]
    partitions = []
    for number in range(4):
        entry = entries[number*128:(number+1)*128]
        first, last = struct.unpack_from('<QQ', entry, 32)
        partitions.append({'number': number+1, 'first_lba': first, 'last_lba': last, 'size_bytes': (last-first+1)*BLOCK, 'name': entry[56:].decode('utf-16le').rstrip('\0')})
    plan = {**args.device_info, 'components': components, 'partitions': partitions}
    (output/'write-plan.json').write_text(json.dumps(plan, indent=2)+'\n')
    return plan

def objc_nsstring(value):
    return '@' + json.dumps(value, ensure_ascii=False)

def write_writer_config(output):
    values = (
        ('Base', str(output)),
        ('Python', str(Path(sys.executable).resolve())),
        ('WriterHash', digest(output/'write_usb.py')),
        ('DeviceHelperHash', digest(output/'usb_device.py')),
        ('PlanHash', digest(output/'write-plan.json')),
    )
    (output/'writer-config.h').write_text(''.join(
        f'static NSString *const {name} = {objc_nsstring(value)};\n' for name, value in values
    ))

def writer_app(args, output):
    copy(HERE/'write_usb.py', output/'write_usb.py')
    copy(HERE/'usb_device.py', output/'usb_device.py')
    write_writer_config(output)
    source = output/'writer.m'
    copy(HERE/'writer.m', source)
    app = args.writer_app.expanduser().resolve()
    existing = plistlib.loads((app/'Contents/Info.plist').read_bytes()) if (app/'Contents/Info.plist').exists() else {}
    identifier = existing.get('CFBundleIdentifier', 'nl.makepad.ArchUSBWriter')
    assert identifier in ('nl.makepad.ArchUSBWriter', 'nl.makepad.ArchUSBRepair')
    binary = existing.get('CFBundleExecutable', 'MakepadUSBWriter')
    assert binary in ('MakepadUSBWriter', 'MakepadUSBRepair')
    executable = app/'Contents/MacOS'/binary; executable.parent.mkdir(parents=True, exist_ok=True)
    plist = {'CFBundleIdentifier': identifier, 'CFBundleExecutable': binary, 'CFBundleName': app.stem, 'CFBundlePackageType': 'APPL', 'CFBundleVersion': '3', 'CFBundleShortVersionString': '3.0', 'NSHighResolutionCapable': True}
    (app/'Contents/Info.plist').write_bytes(plistlib.dumps(plist))
    subprocess.run(['xcrun', 'clang', '-fobjc-arc', '-Wall', '-Wextra', '-Werror', '-framework', 'AppKit', str(source), '-o', str(executable)], check=True)
    subprocess.run(['codesign', '--force', '--sign', '-', '--identifier', identifier, '--requirements', f'=designated => identifier "{identifier}"', str(app)], check=True)
    subprocess.run(['codesign', '--verify', '--strict', str(app)], check=True)
    subprocess.run([str(executable), '--check-configuration'], check=True)

def main():
    if sys.flags.optimize:
        raise RuntimeError('Python optimization is not allowed; safety assertions must remain enabled')
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--base-image', type=Path)
    parser.add_argument('--ssh-key', type=Path, default=Path.home()/'.ssh/id_ed25519.pub')
    parser.add_argument('--disk-bytes', type=int, help='Optional size assertion; must match the attached USB')
    parser.add_argument('--device', required=True, help='Whole disk from diskutil list external physical (diskN)')
    parser.add_argument('--build-id', default='makepad-clean-'+time.strftime('%Y%m%d-%H%M%S'))
    parser.add_argument('--cached-source', action='store_true')
    parser.add_argument('--cef-archive', type=Path, help='Cached Linux x86_64 CEF tar.bz2 (default: ASSETS/cef-linux.tar.bz2)')
    parser.add_argument('--writer-app', type=Path, nargs='?', const=Path.home()/'Applications/Makepad USB Writer.app')
    args = parser.parse_args()
    args.assets = args.assets.expanduser().resolve(); output = args.output.expanduser().resolve()
    assert output != args.assets and args.assets not in output.parents and output not in args.assets.parents, 'Keep output separate from input assets'
    assert output != REPO and output not in REPO.parents, 'Keep output separate from the repository root'
    assert output != HERE and HERE not in output.parents and output not in HERE.parents, 'Keep output separate from the builder sources'
    args.base_image = (args.base_image or args.assets/'arch-base.raw').expanduser().resolve()
    args.ssh_key = args.ssh_key.expanduser().resolve()
    args.cef_archive = (args.cef_archive or args.assets/'cef-linux.tar.bz2').expanduser().resolve()
    for source in (args.base_image, args.ssh_key, args.cef_archive):
        assert source != output and output not in source.parents, 'Keep source inputs outside the output directory'
    args.device_info = snapshot_device(args.device)
    args.device = args.device_info['device']
    if args.disk_bytes is not None:
        assert args.disk_bytes == args.device_info['size'], f'--disk-bytes {args.disk_bytes} does not match attached device size {args.device_info["size"]}'
    args.disk_bytes = args.device_info['size']
    print(f'Selected {args.device}: {args.device_info["registry_name"]}, {args.disk_bytes} bytes', flush=True)
    output.mkdir(parents=True, exist_ok=True)
    assert args.disk_bytes % BLOCK == 0
    iso = prepare_seed(args, output)
    plan = assemble(args, output, iso)
    copy(HERE/'write_usb.py', output/'write_usb.py')
    copy(HERE/'usb_device.py', output/'usb_device.py')
    if args.writer_app:
        args.writer_app = args.writer_app.expanduser().resolve()
        writer_app(args, output)
    report = {'build_id': args.build_id, 'assembled_at': time.strftime('%Y-%m-%dT%H:%M:%S%z'), 'base_sha256': digest(args.base_image), 'write_plan_sha256': digest(output/'write-plan.json'), 'checks': ['cached payload SHA256', 'shell syntax', 'plain ISO9660/Linux and Joliet exact filenames and content hashes', 'full ISO readback hashes', 'pacman/SSH/sudo binaries in base', 'FAT loader/configuration readback', 'stock root and initramfs byte comparison', 'no access repair overlay'], 'hardware_boot': 'pending', 'bytes_to_write': sum(c['length'] for c in plan['components'])}
    (output/'build-complete.json').write_text(json.dumps(report, indent=2)+'\n')
    print(f'Clean image assembled and checked: {output}', flush=True)
    print('Hardware boot and SSH verification remain pending. No physical disk was written.', flush=True)

if __name__ == '__main__':
    main()
