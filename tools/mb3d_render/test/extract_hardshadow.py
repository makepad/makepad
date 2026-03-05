import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    freebyte = data[440 + 16 + 13] # Lights[1].FreeByte
    print("Lights[1].FreeByte:", freebyte)
