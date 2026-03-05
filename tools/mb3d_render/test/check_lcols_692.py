import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

offset = 692
for i in range(10):
    pos = int.from_bytes(data[offset:offset+2], 'little')
    cdif = list(data[offset+2:offset+6])
    cspe = list(data[offset+6:offset+10])
    print(f"{i}: pos={pos}, dif={cdif}, spe={cspe}")
    offset += 10
