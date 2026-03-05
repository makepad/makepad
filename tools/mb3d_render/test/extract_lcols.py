import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    offset = 440 + 60 + 3 + 1 + 3 + 1 + 3 + 3 + 1 + 6*32
    print("LCols offset:", offset)
