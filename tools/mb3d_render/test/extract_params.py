import struct

data = open('local/mb3d/cathedral.m3p', 'rb').read()

def parse_light(offset):
    r, g, b, a = data[offset:offset+4]
    angle_xy = struct.unpack('<d', data[offset+4:offset+12])[0]
    angle_z = struct.unpack('<d', data[offset+12:offset+20])[0]
    return {'color': (r, g, b), 'angle_xy': angle_xy, 'angle_z': angle_z}

print("Light 0:", parse_light(504))
print("Light 1:", parse_light(536))

# Ambient colors
r, g, b, a = data[484:488]
print(f"Ambient Bottom: {r}, {g}, {b}")
r, g, b, a = data[488:492]
print(f"Ambient Top: {r}, {g}, {b}")

print("\nGradient:")
for i in range(788, 830, 6):
    r, g, b, a = data[i:i+4]
    pos = struct.unpack('<H', data[i+4:i+6])[0]
    print(f"Stop at {pos}: {r}, {g}, {b} (A={a})")
