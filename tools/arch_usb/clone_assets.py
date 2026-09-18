"""Refresh the USB's offline assets from an installed clone, without installs."""
import base64
import hashlib
import json
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def ssh(host, *command):
    if not re.fullmatch(r'(?:[a-zA-Z0-9_.-]+@)?[a-zA-Z0-9][a-zA-Z0-9_.-]*', host):
        raise ValueError('Use a known SSH hostname or user@hostname for --clone')
    return ['ssh', '-T', '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=yes',
            '-o', 'UpdateHostKeys=no', '-o', 'ConnectTimeout=10', host, shlex.join(command)]


def read_database(path, repo):
    records = {}
    with tarfile.open(path, 'r:*') as archive:
        for member in archive:
            if not member.name.endswith('/desc'): continue
            record = {'repo': repo}
            for section in archive.extractfile(member).read().decode().strip().split('\n\n'):
                lines = section.splitlines()
                record[lines[0].strip('%')] = lines[1:]
            records[record['NAME'][0]] = record
    return records


def refresh_packages(assets, host, requested):
    """Fill gaps only from matching signed archives; retain one repo snapshot."""
    mirror = assets/'package-mirror'
    database = {}
    for repo in ('core', 'extra'):
        path = mirror/repo/'os/x86_64'/f'{repo}.db'
        remote = subprocess.check_output(ssh(host, 'sha256sum', f'/var/lib/pacman/sync/{repo}.db'), text=True).split()[0]
        if digest(path) != remote:
            raise ValueError(f'{repo}.db differs on the clone; refusing to mix package snapshots')
        database.update(read_database(path, repo))
    selected = {p['NAME'][0]: p for p in json.loads((assets/'resolved-packages.json').read_text())}
    for name, package in selected.items():
        if any(package.get(key) != database[name].get(key) for key in ('VERSION', 'FILENAME', 'SHA256SUM')):
            raise ValueError(f'Cached metadata differs from its repository snapshot: {name}')
    original_names = set(selected)
    queue = list(requested)
    while queue:
        name = queue.pop(0)
        if name in selected: continue
        package = database[name]
        selected[name] = package
        for expression in package.get('DEPENDS', []):
            dependency = re.split('[<>=]', expression)[0]
            if dependency in selected or any(
                dependency == re.split('[<>=]', provided)[0]
                for item in selected.values() for provided in item.get('PROVIDES', [])):
                continue
            if dependency in database:
                queue.append(dependency)
            else:
                providers = [n for n, item in database.items() if any(
                    dependency == re.split('[<>=]', value)[0] for value in item.get('PROVIDES', []))]
                if len(providers) != 1:
                    raise ValueError(f'Choose a provider for {name}: {expression}: {providers}')
                queue.extend(providers)
    changed = set(selected) != original_names
    with tempfile.TemporaryDirectory(prefix='clone-packages-', dir=assets) as temp:
        stage = Path(temp)
        for name, package in sorted(selected.items()):
            filename = package['FILENAME'][0]
            path = mirror/package['repo']/'os/x86_64'/filename
            signature = base64.b64decode(package['PGPSIG'][0])
            for suffix in ('', '.sig'):
                target = path.with_name(filename+suffix)
                valid = target.is_file() and (
                    target.read_bytes() == signature if suffix else digest(target) == package['SHA256SUM'][0])
                if valid: continue
                # The clone retains both the original mirror and later pacman
                # downloads. Neither path executes or installs the package.
                candidates = [f'/var/cache/pacman/pkg/{filename}{suffix}',
                              f'/var/cache/makepad-mirror/{package["repo"]}/os/x86_64/{filename}{suffix}']
                fetched = stage/(filename+suffix)
                for remote in candidates:
                    with fetched.open('wb') as stream:
                        result = subprocess.run(ssh(host, 'cat', remote), stdout=stream, stderr=subprocess.PIPE)
                    if result.returncode == 0: break
                else:
                    raise ValueError(f'Package missing from the installed clone: {filename}{suffix}')
                if (fetched.read_bytes() != signature if suffix else digest(fetched) != package['SHA256SUM'][0]):
                    raise ValueError(f'Clone archive/signature differs from the snapshot: {filename}{suffix}')
                target.parent.mkdir(parents=True, exist_ok=True)
                fetched.replace(target)
                changed = True
        print(f'Verified {len(selected)} cached packages and their signature files.', flush=True)
        if not changed: return
        # Stage the complete replacement before changing the seed's manifest.
        tar_path = stage/'mirror.tar'
        subprocess.run(['tar', '-cf', str(tar_path), '-C', str(mirror), '.'], check=True)
        chunks = []
        with tar_path.open('rb') as source:
            index = 0
            while block := source.read(4*1024*1024):
                name = f'mirror.tar.{chr(97+index//26)}{chr(97+index%26)}'
                path = stage/name
                with path.open('wb') as output:
                    remaining = 1024**3
                    while block:
                        output.write(block)
                        remaining -= len(block)
                        if not remaining: break
                        block = source.read(min(remaining, 4*1024*1024))
                chunks.append(path)
                index += 1
        seed = assets/'seed'
        sums = {name: value for value, name in (line.split('  ', 1) for line in (seed/'SHA256SUMS').read_text().splitlines())}
        sums = {name: value for name, value in sums.items() if not name.startswith('mirror.tar.')}
        for chunk in chunks:
            sums[chunk.name] = digest(chunk)
        for old in seed.glob('mirror.tar.*'): old.unlink()
        for chunk in chunks: chunk.replace(seed/chunk.name)
        (seed/'SHA256SUMS').write_text(''.join(f'{value}  {name}\n' for name, value in sorted(sums.items())))
        (assets/'resolved-packages.json').write_text(json.dumps([selected[n] for n in sorted(selected)], indent=2)+'\n')
        print('Refreshed the offline mirror and its manifest.', flush=True)


def refresh_source(assets, host, root):
    with tempfile.TemporaryDirectory(prefix='clone-source-', dir=assets) as temp:
        stage = Path(temp)
        payload = stage/'payload.tgz'
        print(f'Exporting the working source, WM/AI Hub and Cargo cache from {host}:{root}.', flush=True)
        with payload.open('wb') as output:
            subprocess.run(ssh(host, 'python3', '-', root),
                           input=(HERE/'clone_export.py').read_bytes(), stdout=output, check=True)
        unpacked = stage/'unpacked'
        unpacked.mkdir()
        with tarfile.open(payload, 'r:gz') as archive:
            archive.extractall(unpacked, filter='data')
        # The delivered source must also retain this regeneration entry point,
        # even when the working clone predates it.
        builder = unpacked/'source/tools/arch_usb'
        builder.mkdir(parents=True, exist_ok=True)
        for path in HERE.iterdir():
            if path.is_file() and path.suffix in ('.py', '.sh', '.m', '.env', '.rules', '.service', '.txt'):
                shutil.copy2(path, builder/path.name)
        manifest = json.loads((unpacked/'manifest.json').read_text())
        manifest['host'] = host
        for name, expected in manifest['binary_sha256'].items():
            if name not in ('wm', 'makepad-ai-hub') or digest(unpacked/'binaries'/name) != expected:
                raise ValueError(f'Exported service binary hash mismatch: {name}')
        source = stage/'prepared'
        source.mkdir()
        with tarfile.open(source/'makepad-source.tgz', 'w:gz', compresslevel=3) as archive:
            archive.add(unpacked/'source', arcname='.')
        with tarfile.open(source/'cargo-cache.tar', 'w') as archive:
            archive.add(unpacked/'cargo', arcname='.')
        shutil.copytree(unpacked/'binaries', source/'binaries')
        shutil.copytree(unpacked/'config', source/'config')
        for path in (source/'config').glob('*.sh'):
            subprocess.run(['/bin/bash', '-n', str(path)], check=True)
        revision = f'Installed clone: {host}:{root}\nExported: {manifest["exported_at"]}\nFiles: {manifest["source_files"]}\n'
        (source/'source-revision.txt').write_text(revision)
        manifest['files'] = {str(path.relative_to(source)): digest(path) for path in source.rglob('*') if path.is_file()}
        (source/'manifest.json').write_text(json.dumps(manifest, indent=2)+'\n')
        target = assets/'clone'
        if target.exists():
            if not (target/'manifest.json').is_file():
                raise ValueError(f'Refusing to replace an unrecognized clone cache: {target}')
            shutil.rmtree(target)
        source.replace(target)
        print('Installed clone snapshot cached and verified.', flush=True)


def verify_source(assets):
    source = assets/'clone'
    manifest = json.loads((source/'manifest.json').read_text())
    for name, expected in manifest['files'].items():
        path = source/name
        if not path.resolve().is_relative_to(source.resolve()) or digest(path) != expected:
            raise ValueError(f'Cached clone changed: {name}')
    return source, manifest
