#!/usr/bin/env python3
"""Tile a Godot --write-movie PNG sequence into one labelled contact sheet.

Godot writes f00000000.png, f00000001.png, ... one per frame. Reading them
individually burns an agent's context, so sample N evenly-spaced frames and
lay them out as a single image that shows motion over time.

usage: sheet.py <cap_dir> <out.png> [count=8] [cols=4] [tile_width=320]
"""

import glob
import os
import sys

from PIL import Image, ImageDraw

BAND = 16  # label strip above each tile


def main() -> int:
    cap = sys.argv[1]
    out = sys.argv[2]
    count = int(sys.argv[3]) if len(sys.argv) > 3 else 8
    cols = int(sys.argv[4]) if len(sys.argv) > 4 else 4
    tile_w = int(sys.argv[5]) if len(sys.argv) > 5 else 320

    frames = sorted(glob.glob(os.path.join(cap, "*.png")))
    if not frames:
        print(f"sheet: no frames in {cap}", file=sys.stderr)
        return 1

    n = min(count, len(frames))
    picks = [0] if n == 1 else [round(i * (len(frames) - 1) / (n - 1)) for i in range(n)]

    with Image.open(frames[0]) as probe:
        tile_h = round(tile_w * probe.height / probe.width)

    rows = (n + cols - 1) // cols
    sheet = Image.new("RGB", (cols * tile_w, rows * (tile_h + BAND)), (20, 20, 24))
    draw = ImageDraw.Draw(sheet)

    for i, frame_idx in enumerate(picks):
        with Image.open(frames[frame_idx]) as im:
            # NEAREST keeps pixel art legible when downscaled.
            tile = im.convert("RGB").resize((tile_w, tile_h), Image.NEAREST)
        x = (i % cols) * tile_w
        y = (i // cols) * (tile_h + BAND)
        sheet.paste(tile, (x, y + BAND))
        draw.text((x + 4, y + 3), f"frame {frame_idx}", fill=(190, 190, 200))

    sheet.save(out)
    print(f"sheet: {out} ({n} of {len(frames)} frames, {sheet.width}x{sheet.height})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
