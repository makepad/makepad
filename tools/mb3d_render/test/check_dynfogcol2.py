import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

offset = 436
print(f"DynFogCol2: {list(data[offset:offset+3])}")
