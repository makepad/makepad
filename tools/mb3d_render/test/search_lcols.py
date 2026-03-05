import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

for i in range(len(data) - 8):
    if data[i:i+3] == b'\xc0\xc0\xc0':
        print(f"Found 192,192,192 at {i}")
