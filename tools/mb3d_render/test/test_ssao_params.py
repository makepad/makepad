import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    print("ssao_r_count:", data[187])
    print("deao_max_l:", struct.unpack('<f', data[374:378])[0])
