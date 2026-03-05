import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    offset = 440 + 60
    depthcol = data[offset:offset+3]
    print("DepthCol:", list(depthcol))
