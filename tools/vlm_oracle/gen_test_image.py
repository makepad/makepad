#!/usr/bin/env python3
"""Deterministic P6 PPM test images for VLM oracle comparison (stdlib only)."""
import struct, sys, math, os

def write_ppm(path, w, h, pixel_fn):
    buf = bytearray()
    for y in range(h):
        for x in range(w):
            r, g, b = pixel_fn(x, y)
            buf += bytes((max(0, min(255, int(r))), max(0, min(255, int(g))), max(0, min(255, int(b)))))
    with open(path, 'wb') as f:
        f.write(b'P6\n%d %d\n255\n' % (w, h))
        f.write(buf)
    print(path, w, 'x', h)

out_dir = os.path.dirname(os.path.abspath(__file__))

# 1. radar-like: map-ish background, green/yellow/red rain blobs (512x384, multiple of 32)
def radar(x, y):
    # pale map background with faint road grid
    r, g, b = 232, 236, 240
    if x % 64 < 2 or y % 64 < 2:
        r, g, b = 200, 200, 205
    # rain cells: three gaussian blobs of increasing intensity
    for (cx, cy, s, col) in [(140, 120, 55, (120, 200, 120)),
                             (300, 200, 70, (240, 220, 100)),
                             (330, 180, 30, (220, 80, 60))]:
        d2 = (x - cx) ** 2 + (y - cy) ** 2
        w = math.exp(-d2 / (2 * s * s))
        if w > 0.25:
            r = r * (1 - w) + col[0] * w
            g = g * (1 - w) + col[1] * w
            b = b * (1 - w) + col[2] * w
    return r, g, b
write_ppm(os.path.join(out_dir, 'radar_512x384.ppm'), 512, 384, radar)

# 2. small gradient + circle, exercises nothing fancy (256x256)
def grad(x, y):
    inside = (x - 128) ** 2 + (y - 96) ** 2 < 48 ** 2
    return (255, 64, 32) if inside else (x % 256, y % 256, (x + y) % 256)
write_ppm(os.path.join(out_dir, 'grad_256x256.ppm'), 256, 256, grad)

# 3. non-32-aligned size to exercise smart-resize (500x375)
write_ppm(os.path.join(out_dir, 'radar_500x375.ppm'), 500, 375,
          lambda x, y: radar(x * 512 // 500, y * 384 // 375))
