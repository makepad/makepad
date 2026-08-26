#!/usr/bin/env python3
"""Generates the mixer app icon at every size the platform embeds.

The icon IS this script: three channel faders on a dark desk, each riding a
different level, drawn analytically and supersampled 4x. Run it from the
repo root after changing anything here:

    python3 apps/mixer/resources/make_icon.py

Writes icon_{32,64,128,256,512,1024}.png and icon.ico next to itself.
Stdlib only (zlib) — no image library, no checked-in art to go stale.
"""
import os
import struct
import zlib

SS = 4  # supersampling factor

BG_TOP = (0x18, 0x22, 0x30)
BG_BOTTOM = (0x08, 0x0b, 0x11)
RIM = (0x2f, 0x3b, 0x4d)
SLOT = (0x1c, 0x24, 0x31)
CAP = (0xdc, 0xe3, 0xed)
CAP_EDGE = (0x8d, 0x97, 0xa5)
CAP_LINE = (0xff, 0xff, 0xff)
# level colours, left to right: the app's own blue, amber, green
LEVEL = [(0x2f, 0x6f, 0xe0), (0xf0, 0xb4, 0x29), (0x2f, 0xbf, 0x6b)]
# fader centre x, and how far up its cap sits (0 = bottom, 1 = top)
FADERS = [(0.276, 0.30), (0.500, 0.62), (0.724, 0.44)]

TRACK_TOP, TRACK_BOTTOM = 0.185, 0.815


def rounded_box(x, y, w, h, r):
    """Signed distance to a rounded box, negative inside."""
    def f(px, py):
        dx = abs(px - (x + w / 2)) - (w / 2 - r)
        dy = abs(py - (y + h / 2)) - (h / 2 - r)
        ax, ay = max(dx, 0.0), max(dy, 0.0)
        return (ax * ax + ay * ay) ** 0.5 + min(max(dx, dy), 0.0) - r
    return f


def blend(dst, src, a):
    return tuple(int(round(d + (s - d) * a)) for d, s in zip(dst, src))


def render(size):
    n = size * SS
    px = [[(0, 0, 0, 0) for _ in range(n)] for _ in range(n)]

    body = rounded_box(0.0, 0.0, 1.0, 1.0, 0.225)
    slots = []
    caps = []
    for (cx, level), colour in zip(FADERS, LEVEL):
        sw = 0.052
        slots.append((
            rounded_box(cx - sw / 2, TRACK_TOP, sw, TRACK_BOTTOM - TRACK_TOP, sw / 2),
            colour,
            level,
        ))
        cap_h = 0.105
        cap_y = TRACK_BOTTOM - cap_h / 2 - level * (TRACK_BOTTOM - TRACK_TOP - cap_h)
        cw = 0.235
        caps.append((
            rounded_box(cx - cw / 2, cap_y - cap_h / 2, cw, cap_h, 0.026),
            cap_y,
        ))

    aa = 1.0 / n  # one output pixel of feather, in unit space

    for iy in range(n):
        v = (iy + 0.5) / n
        row = px[iy]
        for ix in range(n):
            u = (ix + 0.5) / n

            d = body(u, v)
            if d > aa:
                continue
            cover = min(1.0, max(0.0, (aa - d) / (2 * aa) + 0.5))
            t = (v - 0.06) / 0.94
            t = min(1.0, max(0.0, t))
            col = blend(BG_TOP, BG_BOTTOM, t)
            # rim light along the top edge of the body
            rim = min(1.0, max(0.0, (0.012 - abs(d)) / 0.012)) * (1.0 - t) * 0.55
            col = blend(col, RIM, rim)

            for (slot, colour, level), (cap, cap_y) in zip(slots, caps):
                ds = slot(u, v)
                if ds < aa:
                    a = min(1.0, max(0.0, (aa - ds) / (2 * aa) + 0.5))
                    # the slot is lit from the cap down: that is the level
                    lit = v > cap_y
                    col = blend(col, colour if lit else SLOT, a)

                dc = cap(u, v)
                if dc < aa:
                    a = min(1.0, max(0.0, (aa - dc) / (2 * aa) + 0.5))
                    shade = CAP if abs(dc) > 0.006 else CAP_EDGE
                    col = blend(col, shade, a)
                    if abs(v - cap_y) < 0.006:
                        col = blend(col, CAP_LINE, 0.9)

            row[ix] = (col[0], col[1], col[2], int(round(255 * cover)))

    # box-downsample to the requested size
    out = bytearray()
    for y in range(size):
        out.append(0)  # PNG filter: none
        for x in range(size):
            r = g = b = a = 0
            for sy in range(SS):
                for sx in range(SS):
                    pr, pg, pb, pa = px[y * SS + sy][x * SS + sx]
                    r += pr * pa
                    g += pg * pa
                    b += pb * pa
                    a += pa
            if a:
                out += bytes((r // a, g // a, b // a, a // (SS * SS)))
            else:
                out += b"\0\0\0\0"
    return bytes(out)


def png(size, raw):
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff)
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


def ico(entries):
    """ICO container holding PNG payloads (Vista+ reads these directly)."""
    head = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    dir_bytes, blob = b"", b""
    for size, data in entries:
        dim = 0 if size >= 256 else size
        dir_bytes += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        blob += data
        offset += len(data)
    return head + dir_bytes + blob


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    made = {}
    for size in (32, 64, 128, 256, 512, 1024):
        data = png(size, render(size))
        made[size] = data
        with open(os.path.join(here, f"icon_{size}.png"), "wb") as f:
            f.write(data)
        print(f"icon_{size}.png  {len(data)} bytes")
    with open(os.path.join(here, "icon.ico"), "wb") as f:
        f.write(ico([(s, made[s]) for s in (32, 64, 128, 256)]))
    print("icon.ico written")


if __name__ == "__main__":
    main()
