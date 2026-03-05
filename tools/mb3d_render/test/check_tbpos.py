import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

offset = 440
for i in range(3, 12):
    val = int.from_bytes(data[offset:offset+4], 'little', signed=True)
    print(f"TBpos[{i}]: {val}")
    offset += 4
