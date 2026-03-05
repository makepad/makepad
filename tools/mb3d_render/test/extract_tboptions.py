import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    tboptions = struct.unpack('<I', data[440+9*4:440+9*4+4])[0]
    print("TBoptions:", hex(tboptions))
