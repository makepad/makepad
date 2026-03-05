import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

offset = 244
print(f"bOptions1: {data[offset]}")
print(f"bOptions2: {data[offset+1]}")
print(f"bOptions3: {data[offset+2]}")
