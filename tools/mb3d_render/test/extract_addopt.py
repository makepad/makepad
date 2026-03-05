import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    addopt = data[436]
    print("AdditionalOptions:", addopt)
