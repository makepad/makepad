import sys

with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()

freebyte = data[556]
print(f"Lights[1].FreeByte: {freebyte}")
print(f"iColOnOT: {2 + (freebyte & 3)}")
