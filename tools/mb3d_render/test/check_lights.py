import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

for i in range(3):
    offset = 500 + i * 32
    print(f"Light {i}:")
    print(f"  Loption: {data[offset]}")
    print(f"  LFunction: {data[offset+1]}")
    print(f"  Lamp: {int.from_bytes(data[offset+2:offset+4], 'little')}")
    print(f"  Lcolor: {list(data[offset+4:offset+7])}")
    print(f"  LightMapNr: {int.from_bytes(data[offset+7:offset+9], 'little')}")
