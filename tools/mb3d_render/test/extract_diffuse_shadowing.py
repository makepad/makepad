import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    # Light[3].AdditionalByteEx
    offset = 508 + 3 * 32 + 14
    print("Light[3].AdditionalByteEx:", data[offset])
