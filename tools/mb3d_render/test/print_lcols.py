import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    offset = 692
    print("LCols:")
    for i in range(10):
        pos, = struct.unpack('<H', data[offset:offset+2])
        dif = list(data[offset+2:offset+6])
        spe = list(data[offset+6:offset+10])
        print(f"  {i}: pos={pos}, dif={dif}, spe={spe}")
        offset += 10
