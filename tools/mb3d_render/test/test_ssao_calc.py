import math
import random

def get_rot_matrix(angle_x, angle_y, angle_z):
    cx = math.cos(angle_x)
    sx = math.sin(angle_x)
    cy = math.cos(angle_y)
    sy = math.sin(angle_y)
    cz = math.cos(angle_z)
    sz = math.sin(angle_z)
    
    return [
        [cy*cz, -cy*sz, sy],
        [cx*sz + sx*sy*cz, cx*cz - sx*sy*sz, -sx*cy],
        [sx*sz - cx*sy*cz, sx*cz + cx*sy*sz, cx*cy]
    ]

def get_ray_dirs(quality):
    dirs = []
    if quality == 0:
        abr = 60 * math.pi / 180
        for x in range(3):
            rot = get_rot_matrix(0, 0.5 * abr, x * math.pi * 2 / 3)
            dirs.append([-rot[2][0], -rot[2][1], -rot[2][2]])
    else:
        rot = get_rot_matrix(0, 0, 0)
        dirs.append([-rot[2][0], -rot[2][1], -rot[2][2]])
        abr = math.pi * 0.5 / (quality + 0.9)
        for y in range(1, quality + 1):
            dt1 = y * abr
            itmp = round(math.sin(dt1) * math.pi * 2 / abr)
            for x in range(itmp):
                rot = get_rot_matrix(0, dt1, x * math.pi * 2 / itmp)
                dirs.append([-rot[2][0], -rot[2][1], -rot[2][2]])
    return dirs

dirs = get_ray_dirs(2)
for i, d in enumerate(dirs):
    print(f'Ray {i}: {d[0]:.3f}, {d[1]:.3f}, {d[2]:.3f}')
