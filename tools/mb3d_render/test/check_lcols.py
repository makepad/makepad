import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

offset = 700
for i in range(10):
    pos = int.from_bytes(data[offset:offset+2], 'little')
    cdif = list(data[offset+2:offset+5])
    cspe = list(data[offset+5:offset+8])
    print(f"{i}: pos={pos}, dif={cdif}, spe={cspe}")
    offset += 8
