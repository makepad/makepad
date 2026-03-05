import struct
import math

data = open('../../local/mb3d/cathedral.m3p', 'rb').read()

val = data[149]
quali = (val >> 4) & 3
print(f'Quality: {quali}')

def get_ray_count(quality):
    if quality == 0:
        return 3
    else:
        ray_count = 1
        abr = math.pi * 0.5 / (quality + 0.9)
        for y in range(1, quality + 1):
            dt1 = y * abr
            itmp = round(math.sin(dt1) * math.pi * 2 / abr)
            ray_count += itmp
        return ray_count

print(f'RayCount: {get_ray_count(quali)}')
