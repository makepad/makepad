#!/usr/bin/env python3

# TODO: support ttc

import argparse
import io
import os
from pathlib import Path
from uuid import uuid4

parser = argparse.ArgumentParser(description='Strip TrueType tables.')
parser.add_argument('target', metavar='TARGET',
                    choices=[
                        'glyf-outline',
                        'gvar-outline',
                        'cff-outline',
                        'cff2-outline',
                        'glyph-index',
                    ],
                    help='fuzz target')
parser.add_argument('out_dir', metavar='DIR', type=Path, nargs='?', help='output dir')
parser.add_argument('in_dirs', metavar='DIRS', type=Path, nargs='+', help='font dirs')
args = parser.parse_args()

if args.out_dir.exists():
    print('Error: Output directory already exists.')
    exit(1)

os.mkdir(args.out_dir)

required_tables = ['head', 'hhea', 'maxp']
if args.target == 'glyf-outline':
    required_tables.extend(['loca', 'glyf'])
elif args.target == 'gvar-outline':
    required_tables.extend(['loca', 'glyf', 'gvar', 'fvar'])
elif args.target == 'cff-outline':
    required_tables.append('CFF ')
elif args.target == 'cff2-outline':
    required_tables.extend(['CFF2', 'fvar'])
elif args.target == 'glyph-index':
    required_tables.append('cmap')

fonts = []
for dir in args.in_dirs:
    for root, _, files in os.walk(dir):
        for file in files:
            if file.endswith('ttf') or file.endswith('otf'):
                fonts.append(Path(root).joinpath(file))

for orig_font_path in fonts:
    with open(orig_font_path, 'rb') as in_file:
        font_data = in_file.read()

        # Parse header
        old_data = io.BytesIO(font_data)
        old_data.read(4)  # sfnt_version
        num_tables = int.from_bytes(old_data.read(2), byteorder='big', signed=False)
        old_data.read(6)  # search_range + entry_selector + range_shift

        tables = []

        # Parse Table Records
        for _ in range(0, num_tables):
            tag = old_data.read(4)
            old_data.read(4)  # check_sum
            offset = int.from_bytes(old_data.read(4), byteorder='big', signed=False)
            length = int.from_bytes(old_data.read(4), byteorder='big', signed=False)
            if tag.decode('ascii') in required_tables:
                tables.append((tag, font_data[offset:offset+length]))

        if len(tables) != len(required_tables):
            continue

        new_data = io.BytesIO()
        new_data.write(b'\x00\x01\x00\x00')  # magic
        new_data.write(int.to_bytes(len(tables), 2, byteorder='big', signed=False))  # number of tables
        new_data.write(b'\x00\x00\x00\x00\x00\x00')  # we don't care about those bytes
        # Write Table Records
        offset = 12 + len(tables) * 16
        for (table_tag, table_data) in tables:
            new_data.write(table_tag)
            new_data.write(b'\x00\x00\x00\x00')  # CRC, ttf-parser ignores it anyway
            new_data.write(int.to_bytes(offset, 4, byteorder='big', signed=False))
            new_data.write(int.to_bytes(len(table_data), 4, byteorder='big', signed=False))
            offset += len(table_data)

        for (_, table_data) in tables:
            new_data.write(table_data)

        with open(args.out_dir.joinpath(str(uuid4()) + '.ttf'), 'wb') as out_file:
            out_file.write(new_data.getbuffer())
