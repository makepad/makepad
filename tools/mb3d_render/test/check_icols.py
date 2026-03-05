import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

offset = 792
for i in range(4):
    pos = int.from_bytes(data[offset:offset+2], 'little')
    color = list(data[offset+2:offset+6])
    print(f"{i}: pos={pos}, color={color}")
    offset += 6
