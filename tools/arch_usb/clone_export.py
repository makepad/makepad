#!/usr/bin/env python3
"""Read-only exporter, sent over SSH by build.py --clone (no remote install)."""
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import time


def main():
    root = Path(sys.argv[1]).resolve()
    if not (root/'Cargo.toml').is_file():
        raise RuntimeError(f'Not a Makepad checkout: {root}')
    binaries = {name: root/'target/release'/name for name in ('wm', 'makepad-ai-hub')}
    for name, path in binaries.items():
        with path.open('rb') as stream:
            if stream.read(4) != b'\x7fELF':
                raise RuntimeError(f'Missing Linux release binary: {name}')
    # These are the two standalone services. Hosted apps retain the normal
    # Cargo launch path; do not collect a set of prebuilt hosted app binaries.
    services = {}
    for name in ('makepad-wm.service', 'makepad-aihub.service'):
        properties = subprocess.check_output(
            ['systemctl', 'show', name, '-p', 'ActiveState', '-p', 'WorkingDirectory', '-p', 'ExecStart'], text=True)
        if 'ActiveState=active\n' not in properties or f'WorkingDirectory={root}\n' not in properties:
            raise RuntimeError(f'{name} is not active from {root}')
        services[name] = properties
    cargo = Path.home()/'.cargo'
    cef = (root/'local/cef-prebuilt/current-linux64').resolve(strict=True)
    manifest = {'exported_at': time.strftime('%Y-%m-%dT%H:%M:%S%z'),
                'source_root': str(root), 'services': services,
                'cef_directory': cef.name, 'binary_sha256': {}}
    listed = subprocess.check_output(
        ['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=root).split(b'\0')
    names = {Path(raw.decode('utf-8', 'surrogateescape')) for raw in listed if raw}
    if (root/'Cargo.lock').exists(): names.add(Path('Cargo.lock'))
    source_count = 0

    def add(archive, path, name):
        before = path.stat()
        def portable_link(info):
            if info.issym() and info.linkname.startswith('/'):
                info.linkname = os.path.relpath(path.resolve(), path.parent)
            return info
        archive.add(path, arcname=name, recursive=False, filter=portable_link)
        after = path.stat()
        if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
            raise RuntimeError(f'File changed during export; retry: {path}')

    with tarfile.open(fileobj=sys.stdout.buffer, mode='w|gz', compresslevel=3) as archive:
        for relative in sorted(names):
            if relative.is_absolute() or '..' in relative.parts:
                raise RuntimeError(f'Invalid source path: {relative}')
            if any(part in ('local', '.git', '.claude', '.grok', '__pycache__') or part.startswith('target') for part in relative.parts):
                continue
            if relative.name.startswith('.env') or relative.suffix in ('.log', '.pem', '.key', '.pyc'):
                continue
            path = root/relative
            if not path.is_file() and not path.is_symlink(): continue
            if path.is_symlink() and not path.resolve().is_relative_to(root): continue
            add(archive, path, 'source/'+relative.as_posix())
            source_count += 1
        for name, path in binaries.items():
            with path.open('rb') as stream:
                manifest['binary_sha256'][name] = hashlib.file_digest(stream, 'sha256').hexdigest()
            add(archive, path, 'binaries/'+name)
        # No Cargo credentials or host-specific Cargo configuration.
        for directory in ('registry', 'git'):
            base = cargo/directory
            if not base.is_dir(): continue
            for path in sorted(base.rglob('*')):
                if not path.is_file() and not path.is_symlink(): continue
                if path.is_symlink() and not path.resolve().is_relative_to(cargo): continue
                add(archive, path, 'cargo/'+path.relative_to(cargo).as_posix())
        for name, path in {
            'wm-session.sh': '/usr/local/bin/makepad-wm-session',
            'aihub-session.sh': '/usr/local/bin/makepad-aihub-session',
            'wm.env': '/etc/makepad/wm.env',
            'aihub.env': '/etc/makepad/aihub.env',
            'makepad-wm.service': '/etc/systemd/system/makepad-wm.service',
            'makepad-aihub.service': '/etc/systemd/system/makepad-aihub.service',
        }.items():
            add(archive, Path(path), 'config/'+name)
        manifest['source_files'] = source_count
        data = (json.dumps(manifest, indent=2)+'\n').encode()
        info = tarfile.TarInfo('manifest.json')
        info.size = len(data)
        archive.addfile(info, io.BytesIO(data))
    print(f'Exported {source_count} source files, WM/AI Hub services and Cargo cache.', file=sys.stderr)


if __name__ == '__main__':
    main()
