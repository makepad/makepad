import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    iters, = struct.unpack('<I', data[12:16])
    print("iterations:", iters)
