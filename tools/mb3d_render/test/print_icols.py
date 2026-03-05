import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    offset = 792
    print("ICols:")
    for i in range(4):
        pos, = struct.unpack('<H', data[offset:offset+2])
        col = list(data[offset+2:offset+6])
        print(f"  {i}: pos={pos}, col={col}")
        offset += 6
