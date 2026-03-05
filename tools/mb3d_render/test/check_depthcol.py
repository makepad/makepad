import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

for offset in range(480, 500):
    print(f"{offset}: {list(data[offset:offset+3])}")
