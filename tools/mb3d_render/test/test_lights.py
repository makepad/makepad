import math

# angles in degrees
xy1 = 132
z1 = -2

xy2 = -35
z2 = 53

def to_vec(xy, z):
    xy_rad = math.radians(xy)
    z_rad = math.radians(z)
    
    # Assuming Z is up, X is right, Y is forward
    # Or maybe Y is up, X is right, Z is forward
    
    # Let's try standard spherical
    # x = cos(z) * cos(xy)
    # y = cos(z) * sin(xy)
    # z = sin(z)
    
    x = math.cos(z_rad) * math.cos(xy_rad)
    y = math.cos(z_rad) * math.sin(xy_rad)
    z_val = math.sin(z_rad)
    
    return x, y, z_val

print("Light 1:", to_vec(xy1, z1))
print("Light 2:", to_vec(xy2, z2))

