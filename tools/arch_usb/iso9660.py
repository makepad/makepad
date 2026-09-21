"""Inspect the flat seed in both plain ISO9660 and Joliet namespaces.

Linux can mount the plain namespace, which lowercases names and allows only
one dot. macOS prefers Joliet and performs case-insensitive lookups, so a Mac
mount alone cannot verify that Linux will find a referenced filename.
"""
import hashlib
import struct

def validate_seed(image, expected):
    namespaces = []
    with image.open('rb') as stream:
        for sector in range(16, 48):
            stream.seek(sector*2048)
            descriptor = stream.read(2048)
            assert descriptor[1:6] == b'CD001'
            kind = descriptor[0]
            if kind == 255: break
            if kind not in (1, 2): continue
            assert struct.unpack_from('<H', descriptor, 128)[0] == 2048
            start = struct.unpack_from('<I', descriptor, 158)[0]*2048
            size = struct.unpack_from('<I', descriptor, 166)[0]
            stream.seek(start); data = stream.read(size)
            offset = 0; entries = {}
            while offset < len(data):
                length = data[offset]
                if not length:
                    offset = (offset//2048+1)*2048
                    continue
                entry = data[offset:offset+length]
                raw = entry[33:33+entry[32]]
                offset += length
                if raw in (b'\0', b'\1'): continue
                assert not entry[25] & (2 | 128), 'Seed must contain flat, single-extent files'
                name = raw.decode('ascii' if kind == 1 else 'utf-16-be')
                name = name.removesuffix(';1').removesuffix('.')
                if kind == 1: name = name.lower()
                assert name not in entries
                entries[name] = (struct.unpack_from('<I', entry, 2)[0]*2048, struct.unpack_from('<I', entry, 10)[0])
            namespace = 'ISO9660/Linux' if kind == 1 else 'Joliet'
            assert set(entries) == set(expected), f'{namespace} names differ: missing={set(expected)-set(entries)}, extra={set(entries)-set(expected)}'
            for name, (position, remaining) in entries.items():
                stream.seek(position); digest = hashlib.sha256()
                while remaining:
                    block = stream.read(min(4*1024*1024, remaining))
                    assert block
                    digest.update(block); remaining -= len(block)
                assert digest.hexdigest() == expected[name], f'{namespace}: content differs for {name}'
            namespaces.append(namespace)
    assert namespaces == ['ISO9660/Linux', 'Joliet']
    return namespaces
