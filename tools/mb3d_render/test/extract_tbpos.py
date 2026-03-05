import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    tbpos = struct.unpack('<9i', data[440:440+9*4])
    print("TBpos[3..11]:", tbpos)
