#!/usr/bin/env python3
"""Generate Makepad Run and WM Android launcher artwork with Python/Pillow.

Run from the repository root: python3 tools/app_icons/makepad_android/icons.py
Writes each crate's resources/android/res, SVG masters beside this script,
preview.png, and preview_row.png when Translator's launcher_row.png exists.

Like ../translator/icons.py, all renderers consume the same 108-unit paths;
the path, clipping and supersampling helpers are copied/adapted here, with
no dependency on that generator. Foreground artwork fits the radius-33 safe
circle centred at (54, 54). Only the background extends to the canvas edges.
Legacy icons and previews show the launcher's central 72-unit viewport.
"""

from dataclasses import dataclass, replace
from math import hypot
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFont


SIZE = 108
KAPPA = 0.5522847498307936
LAUNCHER_VIEWPORT = (18, 18, 90, 90)
DENSITIES = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
ANDROID_NS = "http://schemas.android.com/apk/res/android"
ROOT = Path(__file__).resolve().parents[3]
ARTWORK = Path(__file__).resolve().parent

# tools/makepad_builder/resources/logo_makepad.svg: rgb(255,92,57).
ORANGE = "#ff5c39"
# apps/wm/resources/icon.svg: flat average of #1b3354 -> #0b1627,
# with the original light and blue. Keep every output on the same flat palette.
NAVY = "#13253e"
LIGHT = "#e8eef6"
BLUE = "#4aa3e0"
WHITE = "#ffffff"
LAUNCHER_BACKGROUND = "#0b1627"


@dataclass(frozen=True)
class Shape:
    path: tuple
    fill: str
    cutouts: tuple = ()


@dataclass(frozen=True)
class IconDesign:
    name: str
    package: str
    resources: Path
    background: tuple
    foreground: tuple
    monochrome: tuple

    @property
    def artwork(self):
        return (*self.background, *self.foreground)


def polygon(*points):
    return (("M", *points[0]), *(("L", *p) for p in points[1:]), ("Z",))


def rectangle(x, y, width, height, radius=0, *, bottom_radius=None):
    """Clockwise rounded rectangle; square bottoms make solid title bars."""
    if not radius:
        return polygon((x, y), (x + width, y),
                       (x + width, y + height), (x, y + height))
    r, k = radius, radius * KAPPA
    b = radius if bottom_radius is None else bottom_radius
    bk = b * KAPPA
    return (
        ("M", x + r, y), ("L", x + width - r, y),
        ("C", x + width - r + k, y, x + width, y + r - k, x + width, y + r),
        ("L", x + width, y + height - b),
        ("C", x + width, y + height - b + bk,
         x + width - b + bk, y + height, x + width - b, y + height),
        ("L", x + b, y + height),
        ("C", x + b - bk, y + height, x, y + height - b + bk, x, y + height - b),
        ("L", x, y + r),
        ("C", x, y + r - k, x + r - k, y, x + r, y), ("Z",),
    )


CANVAS = rectangle(0, 0, SIZE, SIZE)


def designs():
    """One source of geometry for Android, SVG, legacy PNGs and previews."""
    background = (Shape(CANVAS, NAVY),)

    # Simplify the logo's six triangles into two peaks above alternating
    # up/down triangles. Wide gutters survive the 48 px launcher size.
    triangles = (
        polygon((43.5, 29), (34, 45.5), (53, 45.5)),
        polygon((64.5, 29), (55, 45.5), (74, 45.5)),
        polygon((33, 47.5), (23.5, 64), (42.5, 64)),
        polygon((35, 47.5), (54, 47.5), (44.5, 64)),
        polygon((55, 47.5), (74, 47.5), (64.5, 64)),
        polygon((75, 47.5), (65.5, 64), (84.5, 64)),
    )
    play = polygon((54, 57), (76, 69), (54, 81))
    play_clearance = polygon((50.5, 51.5), (82.5, 69), (50.5, 86.5))
    run_mark = (
        *(Shape(path, ORANGE, (play_clearance,)) for path in triangles),
        Shape(play, WHITE),
    )
    run = IconDesign(
        "run", "dev.makepad.run", ROOT / "tools/run/android_host/resources/android/res",
        background, run_mark, tuple(replace(shape, fill=WHITE) for shape in run_mark),
    )

    # WM's blue rear window/light title bar and light front window/orange
    # title bar, without the desktop artwork's tiny controls and text lines.
    # All outlines are filled paths: no thin strokes at any density.
    back = rectangle(28, 30, 43, 34, 7.5)
    back_title = rectangle(28, 30, 43, 10, 7.5, bottom_radius=0)
    front = rectangle(41, 44, 40, 32, 7)
    front_title = rectangle(41, 44, 40, 10, 7, bottom_radius=0)
    front_clearance = rectangle(38.5, 41.5, 45, 37, 9.5)
    back_pane = rectangle(32.5, 40, 34, 19.5, 2.5)
    front_pane = rectangle(45.5, 54, 31, 17.5, 2.5)
    wm = IconDesign(
        "wm", "nl.makepad.wm", ROOT / "apps/wm/resources/android/res",
        background,
        (Shape(back, BLUE, (front_clearance,)),
         Shape(back_title, LIGHT, (front_clearance,)),
         Shape(front, LIGHT), Shape(front_title, ORANGE)),
        # Hollow panes keep both windows legible when Android tints the mark.
        # Subtract the union of holes, including the front window's clearance.
        (Shape(back, WHITE, (back_pane, front_clearance)),
         Shape(front, WHITE, (front_pane,))),
    )
    return (run, wm)


def number(value):
    return f"{value:.6f}".rstrip("0").rstrip(".") or "0"


def path_data(path):
    # Absolute M/L/C/Z commands have identical SVG and Android syntax.
    return " ".join(command[0] + " ".join(number(v) for v in command[1:])
                    for command in path)


def flatten(path, tolerance):
    """Adaptively subdivide cubic curves within a raster subpixel tolerance."""
    points = []

    def midpoint(a, b):
        return ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)

    def cubic(a, b, c, d):
        dx, dy = d[0] - a[0], d[1] - a[1]
        length = hypot(dx, dy)
        distance = max(abs(dx * (p[1] - a[1]) - dy * (p[0] - a[0])) / length
                       if length else hypot(p[0] - a[0], p[1] - a[1])
                       for p in (b, c))
        if distance <= tolerance:
            points.append(d)
            return
        ab, bc, cd = midpoint(a, b), midpoint(b, c), midpoint(c, d)
        abc, bcd = midpoint(ab, bc), midpoint(bc, cd)
        centre = midpoint(abc, bcd)
        cubic(a, ab, abc, centre)
        cubic(centre, bcd, cd, d)

    for command in path:
        op, *values = command
        if op in ("M", "L"):
            points.append(tuple(values))
        elif op == "C":
            cubic(points[-1], tuple(values[:2]), tuple(values[2:4]), tuple(values[4:]))
        elif op == "Z":
            points.append(points[0])
        else:
            raise ValueError(f"Unsupported path command: {op}")
    return points


def raster(shapes, size, *, circle_mask=False, viewport=(0, 0, SIZE, SIZE)):
    """Draw shared filled paths with Pillow at 4x; downsample exactly once."""
    pixels = size * 4
    left, top, right, bottom = viewport
    scale_x, scale_y = pixels / (right - left), pixels / (bottom - top)
    result = Image.new("RGBA", (pixels, pixels))

    def points(path):
        return [((x - left) * scale_x, (y - top) * scale_y)
                for x, y in flatten(path, 0.15 / max(scale_x, scale_y))]

    for shape in shapes:
        mask = Image.new("L", result.size)
        draw = ImageDraw.Draw(mask)
        draw.polygon(points(shape.path), fill=255)
        for hole in shape.cutouts:
            draw.polygon(points(hole), fill=0)
        result.paste(shape.fill, (0, 0, pixels, pixels), mask)
    if circle_mask:
        mask = Image.new("L", result.size)
        ImageDraw.Draw(mask).ellipse((0, 0, pixels - 1, pixels - 1), fill=255)
        result.putalpha(ImageChops.multiply(result.getchannel("A"), mask))
    return result.resize((size, size), Image.Resampling.LANCZOS)


def launcher_raster(shapes, size, *, circle_mask=True):
    return raster(shapes, size, circle_mask=circle_mask, viewport=LAUNCHER_VIEWPORT)


def svg_shape(shape, index):
    element = f'  <path d="{path_data(shape.path)}" fill="{shape.fill}"/>\n'
    if not shape.cutouts:
        return element
    clips, groups = [], []
    # Nested outside-of-hole clips subtract a union even when holes overlap.
    for hole_index, hole in enumerate(shape.cutouts):
        name = f"cutout-{index}-{hole_index}"
        outside = path_data(CANVAS) + " " + path_data(hole)
        clips.append(f'    <clipPath id="{name}" clipPathUnits="userSpaceOnUse">'
                     f'<path d="{outside}" clip-rule="evenodd"/></clipPath>\n')
        groups.append(f'  <g clip-path="url(#{name})">\n')
    return ("  <defs>\n" + "".join(clips) + "  </defs>\n" + "".join(groups)
            + element + "  </g>\n" * len(groups))


def svg(shapes):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="108" height="108"'
            f' viewBox="0 0 {SIZE} {SIZE}">\n'
            + "".join(svg_shape(shape, i) for i, shape in enumerate(shapes)) + "</svg>\n")


def vector(shapes):
    lines = [f'<vector xmlns:android="{ANDROID_NS}"',
             '    android:width="108dp" android:height="108dp"',
             '    android:viewportWidth="108" android:viewportHeight="108">']
    for shape in shapes:
        indent = "    "
        for hole in shape.cutouts:
            outside = path_data(CANVAS) + " " + path_data(hole)
            lines.append(f"{indent}<group>")
            indent += "    "
            lines.append(f'{indent}<clip-path android:pathData="{outside}"'
                         ' android:fillType="evenOdd"/>')
        lines.append(f'{indent}<path android:pathData="{path_data(shape.path)}"'
                     f' android:fillColor="{shape.fill}"/>')
        for _ in shape.cutouts:
            indent = indent[:-4]
            lines.append(f"{indent}</group>")
    return "\n".join((*lines, "</vector>", ""))


def adaptive_xml():
    return (f'<adaptive-icon xmlns:android="{ANDROID_NS}">\n'
            '    <background android:drawable="@drawable/ic_launcher_background"/>\n'
            '    <foreground android:drawable="@drawable/ic_launcher_foreground"/>\n'
            '    <monochrome android:drawable="@drawable/ic_launcher_monochrome"/>\n'
            '</adaptive-icon>\n')


def preview(icons, large):
    result = Image.new("RGBA", (552, 576), LAUNCHER_BACKGROUND)
    draw = ImageDraw.Draw(result)
    title = ImageFont.load_default(size=20)
    caption = ImageFont.load_default(size=15)
    for design, x in zip(icons, (40, 304)):
        centre = x + 96
        draw.text((centre, 24), design.name.upper(), fill=LIGHT, font=title, anchor="mt")
        draw.text((centre, 51), design.package, fill=LIGHT, font=caption, anchor="mt")
        result.alpha_composite(large[design.name], (x, 84))
        draw.text((centre, 291), "192 px", fill=LIGHT, font=caption, anchor="mt")
        small = launcher_raster(design.artwork, 48)
        result.alpha_composite(small.resize((192, 192), Image.Resampling.NEAREST), (x, 332))
        draw.text((centre, 540), "48 px / 4x nearest", fill=LIGHT, font=caption, anchor="mt")
    return result


def launcher_composite(source, large):
    """Replace row slots 3/4 with 192 px renders, resized to 148 px circles."""
    result = source.copy()
    for name, centre in (("run", 667), ("wm", 922)):
        # Existing circles are centred at y=120.5 in the supplied row. Erase
        # their entire antialiased rims before placing the requested y=110
        # circles; otherwise an old white crescent remains below each icon.
        left, top, right, bottom = centre - 77, 43, centre + 78, 198
        patch = source.crop((left, top, right, bottom))
        wallpaper = Image.new("RGBA", patch.size)
        draw = ImageDraw.Draw(wallpaper)
        for y in range(patch.height):
            before = source.getpixel((left - 1, top + y))
            after = source.getpixel((right, top + y))
            for x in range(patch.width):
                weight = (x + 1) / (patch.width + 1)
                color = tuple(round(a + (b - a) * weight) for a, b in zip(before, after))
                draw.point((x, y), fill=color)
        old_circle = Image.new("L", patch.size)
        ImageDraw.Draw(old_circle).ellipse((2, 2, 153, 153), fill=255)
        patch.paste(wallpaper, (0, 0), old_circle)
        result.paste(patch, (left, top))
        replacement = large[name].resize((148, 148), Image.Resampling.LANCZOS)
        result.alpha_composite(replacement, (centre - 74, 110 - 74))
    return result


def main():
    written = []

    def write(path, content):
        path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, Image.Image):
            content.save(path, optimize=True)
        else:
            path.write_text(content, encoding="utf-8")
        written.append(path.relative_to(ROOT))

    icons = designs()
    large = {}
    for design in icons:
        write(ARTWORK / f"{design.name}.svg", svg(design.artwork))
        write(ARTWORK / f"{design.name}_monochrome.svg", svg(design.monochrome))
        for name in ("ic_launcher", "ic_launcher_round"):
            write(design.resources / "mipmap-anydpi-v26" / f"{name}.xml", adaptive_xml())
        for layer in ("background", "foreground", "monochrome"):
            write(design.resources / "drawable" / f"ic_launcher_{layer}.xml",
                  vector(getattr(design, layer)))
        for density, size in DENSITIES.items():
            folder = design.resources / f"mipmap-{density}"
            write(folder / "ic_launcher.png",
                  launcher_raster(design.artwork, size, circle_mask=False))
            write(folder / "ic_launcher_round.png", launcher_raster(design.artwork, size))
        large[design.name] = launcher_raster(design.artwork, 192)

    write(ARTWORK / "preview.png", preview(icons, large))
    row_path = ARTWORK.parent / "translator/launcher_row.png"
    if row_path.exists():
        with Image.open(row_path) as source:
            row = source.convert("RGBA")
        if row.size != (1080, 337):
            raise ValueError("launcher_row.png must be the supplied 1080x337 launcher row")
        write(ARTWORK / "preview_row.png", launcher_composite(row, large))

    print("Files written:")
    for path in written:
        print(path)


if __name__ == "__main__":
    main()
