import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    offset = 500 # Wait, Lights starts at 508? Let's check 500
    for offset in range(500, 520):
        print(f"Offset {offset}: {list(data[offset:offset+16])}")
