#!/usr/bin/env python3
"""Android launcher icons from an app's icon.svg.

  python3 tools/app_icons/android_mipmaps.py apps/translator

Renders <app>/resources/icon.svg to 1024 px with macOS QuickLook, cuts it to
its rounded square (radius 224/1024, the corners QuickLook paints white) and
writes <app>/resources/icon_1024.png plus resources/android/res/mipmap-*/ic_launcher.png,
which `cargo makepad android` picks up. Authoring only; nothing at build time
runs it. Needs Pillow.
"""
import subprocess, sys, tempfile, pathlib
from PIL import Image, ImageDraw

SIZES = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}

def main(app):
    res = pathlib.Path(app) / "resources"
    svg = res / "icon.svg"
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(["qlmanage", "-t", "-s", "1024", "-o", tmp, str(svg)], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        png = pathlib.Path(tmp) / (svg.name + ".png")
        img = Image.open(png).convert("RGBA").resize((1024, 1024), Image.LANCZOS)
    mask = Image.new("L", (4096, 4096), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, 4095, 4095), radius=896, fill=255)
    mask = mask.resize((1024, 1024), Image.LANCZOS)
    img.putalpha(mask)
    img.save(res / "icon_1024.png")
    for name, px in SIZES.items():
        out = res / "android" / "res" / f"mipmap-{name}"
        out.mkdir(parents=True, exist_ok=True)
        img.resize((px, px), Image.LANCZOS).save(out / "ic_launcher.png")
    print("wrote", res / "icon_1024.png", "and", len(SIZES), "mipmaps")

if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
