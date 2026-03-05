import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

offset = 432 + 2 + 1 + 1 + 3 + 1 + 36
tboptions = int.from_bytes(data[offset:offset+4], 'little')
print(f"TBoptions: {tboptions}")
print(f"TBoptions & 0x8000: {tboptions & 0x8000}")
