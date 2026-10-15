#!/usr/bin/env python3
"""Writes spinner.gif, the animated GIF fixture of the hello_world example.

    python3 make_spinner_gif.py            # writes spinner.gif next to this script

A 96 x 96 looping GIF89a with 12 frames: eight dots on a ring, one bright dot
walking around it with a fading tail. Python standard library only; the LZW
encoder below is the plain GIF variable-width one.
"""

import math
import os
import struct

SIZE = 96
FRAMES = 12
DOTS = 8
DELAY_CS = 8  # frame delay in 1/100 s

# Palette: background, then a ramp from dim to bright dot colour.
PALETTE = [
    (0x1e, 0x1f, 0x26),
    (0x34, 0x37, 0x45),
    (0x4a, 0x5a, 0x80),
    (0x60, 0x80, 0xc0),
    (0x7c, 0xa8, 0xf0),
    (0xb0, 0xd0, 0xff),
    (0xff, 0xff, 0xff),
    (0x00, 0x00, 0x00),
]
MIN_CODE_SIZE = 3  # 8 palette entries


def frame_pixels(frame):
    pixels = bytearray(SIZE * SIZE)
    centre = (SIZE - 1) / 2.0
    ring = SIZE * 0.32
    radius = SIZE * 0.09
    head = frame * DOTS / FRAMES
    for dot in range(DOTS):
        angle = 2.0 * math.pi * dot / DOTS - math.pi / 2.0
        cx = centre + ring * math.cos(angle)
        cy = centre + ring * math.sin(angle)
        behind = (head - dot) % DOTS  # 0 = the head, larger = further back in the tail
        colour = max(1, 6 - int(behind * 1.4))
        r2 = radius * radius
        for y in range(int(cy - radius) - 1, int(cy + radius) + 2):
            for x in range(int(cx - radius) - 1, int(cx + radius) + 2):
                if 0 <= x < SIZE and 0 <= y < SIZE and (x - cx) ** 2 + (y - cy) ** 2 <= r2:
                    pixels[y * SIZE + x] = colour
    return bytes(pixels)


def lzw_encode(pixels, min_code_size):
    clear = 1 << min_code_size
    end = clear + 1
    out = bytearray()
    bit_buffer = 0
    bit_count = 0

    def emit(code, width):
        nonlocal bit_buffer, bit_count
        bit_buffer |= code << bit_count
        bit_count += width
        while bit_count >= 8:
            out.append(bit_buffer & 0xFF)
            bit_buffer >>= 8
            bit_count -= 8

    def reset():
        return {bytes([i]): i for i in range(clear)}, end + 1, min_code_size + 1

    table, next_code, width = reset()
    emit(clear, width)
    prefix = b""
    for value in pixels:
        candidate = prefix + bytes([value])
        if candidate in table:
            prefix = candidate
            continue
        emit(table[prefix], width)
        if next_code == 4096:
            emit(clear, width)
            table, next_code, width = reset()
        else:
            table[candidate] = next_code
            if next_code == (1 << width) and width < 12:
                width += 1
            next_code += 1
        prefix = bytes([value])
    if prefix:
        emit(table[prefix], width)
    emit(end, width)
    if bit_count:
        out.append(bit_buffer & 0xFF)
    return bytes(out)


def sub_blocks(data):
    out = bytearray()
    for start in range(0, len(data), 255):
        chunk = data[start:start + 255]
        out.append(len(chunk))
        out += chunk
    out.append(0)
    return bytes(out)


def build():
    gif = bytearray(b"GIF89a")
    # Logical screen: global colour table of 2^(2+1) = 8 entries.
    gif += struct.pack("<HHBBB", SIZE, SIZE, 0x80 | 0x70 | (MIN_CODE_SIZE - 1), 0, 0)
    for r, g, b in PALETTE:
        gif += bytes((r, g, b))
    # Loop forever.
    gif += b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00"
    for frame in range(FRAMES):
        gif += struct.pack("<BBBBHBB", 0x21, 0xF9, 4, 0x04, DELAY_CS, 0, 0)
        gif += struct.pack("<BHHHHB", 0x2C, 0, 0, SIZE, SIZE, 0)
        gif.append(MIN_CODE_SIZE)
        gif += sub_blocks(lzw_encode(frame_pixels(frame), MIN_CODE_SIZE))
    gif.append(0x3B)
    return bytes(gif)


if __name__ == "__main__":
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "spinner.gif")
    with open(path, "wb") as f:
        f.write(build())
    print(path)
