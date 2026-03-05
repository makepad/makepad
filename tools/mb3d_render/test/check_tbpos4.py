import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

offset = 432 + 2 + 1 + 1 + 3 + 1 + 4
tbpos4 = int.from_bytes(data[offset:offset+4], 'little', signed=True)
print(f"TBpos[4]: {tbpos4}")
