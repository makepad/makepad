#!/usr/bin/env python3
"""Generate Translator's app artwork using only Python and Pillow.

Run from the repository root: python3 tools/app_icons/translator/icons.py
Writes the master SVG, desktop PNG/ICO icons, existing theme variants,
Android resources and a 512 px preview. All artwork starts in the same
108-unit coordinate space; glyphs are drawn paths, never fonts.
No external renderer is required.

DESIGN selects the artwork; "current" preserves the original two bubbles.
Use --preview to compare bubble, split and ring without writing app resources.
The comparison row uses launcher_row.png beside this script as its source.
"""

import argparse
from dataclasses import dataclass, replace
from math import hypot
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw


SIZE = 108
DESIGN = "bubble"
CANDIDATES = ("bubble", "split", "ring")
# Adaptive icons expose the central 72 dp; the 66 dp safe circle sits inside it.
LAUNCHER_VIEWPORT = (18, 18, 90, 90)
PAPER = "#f4f1eb"
INK = "#1f2b46"
LATIN = "#f6f1e8"
VERMILION = "#e8502f"
WHITE = "#ffffff"
KAPPA = 0.5522847498307936
ANDROID_NS = "http://schemas.android.com/apk/res/android"
ROOT = Path(__file__).resolve().parents[3]
RESOURCES = ROOT / "apps/translator/resources"
ARTWORK = Path(__file__).resolve().parent


@dataclass(frozen=True)
class Shape:
    path: tuple
    fill: str = ""
    stroke: str = ""
    width: float = 0
    cutouts: tuple = ()


@dataclass(frozen=True)
class IconDesign:
    background: tuple
    foreground: tuple
    monochrome: tuple

    @property
    def artwork(self):
        return (*self.background, *self.foreground)


def rectangle(x, y, w, h, radius=0, *, tail=()):
    """Clockwise rounded rectangle; optional tail points along its bottom."""
    if not radius:
        return polygon((x, y), (x + w, y), (x + w, y + h), *tail, (x, y + h))
    r, k = radius, radius * KAPPA
    return (
        ("M", x + r, y), ("L", x + w - r, y),
        ("C", x + w - r + k, y, x + w, y + r - k, x + w, y + r),
        ("L", x + w, y + h - r),
        ("C", x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h),
        *(("L", *point) for point in tail),
        ("L", x + r, y + h),
        ("C", x + r - k, y + h, x, y + h - r + k, x, y + h - r),
        ("L", x, y + r),
        ("C", x, y + r - k, x + r - k, y, x + r, y), ("Z",),
    )


def polygon(*points):
    return (("M", *points[0]), *(("L", *p) for p in points[1:]), ("Z",))


def stroke(start, end, color, width):
    return Shape((("M", *start), ("L", *end)), stroke=color, width=width)


def curve(start, control, end, color, width):
    return Shape((("M", *start), ("Q", *control, *end)), stroke=color, width=width)


# The only definition of the mark. Even the tails and round stroke caps stay
# inside the central 66-unit safe circle (centre 54,54; radius 33).
BACK = Shape(rectangle(27.5, 27.5, 40, 35, 12,
                      tail=((48, 62.5), (34, 69), (40, 62.5))), INK)
A = (
    stroke((34, 51), (40.5, 34.5), LATIN, 4.2),
    stroke((40.5, 34.5), (47, 51), LATIN, 4.2),
    stroke((36.6, 44.5), (44.4, 44.5), LATIN, 4.2),
)
FRONT = Shape(rectangle(49, 47, 33, 30, 9,
                       tail=((72, 77), (72, 81), (64, 77))), VERMILION)
WEN = (
    stroke((64, 52), (65, 53.5), WHITE, 3.2),  # dot
    stroke((55.5, 57.5), (75.5, 57.5), WHITE, 3.4),  # bar
    stroke((59, 60), (75, 72), WHITE, 3.4),  # falling right
    stroke((72, 59.5), (56, 72), WHITE, 3.4),  # falling left
)
CURRENT_MARK = (BACK, *A, FRONT, *WEN)
GLYPHS = (*A, *WEN)
CANVAS = rectangle(0, 0, SIZE, SIZE)

# Code's master uses a 100 px margin and an 824 px tile with 190 px corners.
MASTER_TILE = Shape(rectangle(100 * SIZE / 1024, 100 * SIZE / 1024,
                              824 * SIZE / 1024, 824 * SIZE / 1024,
                              190 * SIZE / 1024), PAPER)
LEGACY_TILE = Shape(rectangle(0, 0, SIZE, SIZE, 23), PAPER)
DENSITIES = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}


def number(value):
    return f"{value:.6f}".rstrip("0").rstrip(".") or "0"


def path_data(path):
    # SVG and Android share these absolute M/L/Q/C/Z commands verbatim.
    return " ".join(command[0] + " ".join(number(v) for v in command[1:])
                    for command in path)


def round_cap(point, tangent):
    """A semicircle from the left normal to the right, around the tangent."""
    x, y = point
    tx, ty = tangent  # Tangent length is the stroke radius.
    nx, ny = -ty, tx
    k = KAPPA
    return (
        ("C", x + nx + k * tx, y + ny + k * ty,
         x + tx + k * nx, y + ty + k * ny, x + tx, y + ty),
        ("C", x + tx - k * nx, y + ty - k * ny,
         x - nx + k * tx, y - ny + k * ty, x - nx, y - ny),
    )


def quadratic_point(start, control, end, t):
    return tuple((1 - t) ** 2 * a + 2 * (1 - t) * t * b + t ** 2 * c
                 for a, b, c in zip(start, control, end))


def curve_outline(shape, tolerance=0.001):
    """Fit Q segments to both normal offsets, then close with round caps.

    Quadratic offsets are not themselves quadratics. Subdivide until fitted
    quarter points agree within 0.001 units (under 0.01 px at 1024 px).
    """
    start = shape.path[0][1:]
    control, end = shape.path[1][1:3], shape.path[1][3:]
    radius = shape.width / 2

    def tangent(t):
        dx, dy = ((1 - t) * (b - a) + t * (c - b)
                  for a, b, c in zip(start, control, end))
        if not hypot(dx, dy):
            dx, dy = end[0] - start[0], end[1] - start[1]
        length = hypot(dx, dy)
        return (dx / length * radius, dy / length * radius) if length else (radius, 0)

    def offset(t, side):
        x, y = quadratic_point(start, control, end, t)
        tx, ty = tangent(t)
        return (x - side * ty, y + side * tx)

    def side_path(a, b, side):
        first, last = offset(a, side), offset(b, side)
        middle = (a + b) / 2
        centre = offset(middle, side)
        handle = tuple(2 * m - (s + e) / 2 for s, m, e in zip(first, centre, last))
        error = 0
        for t in (0.25, 0.75):
            exact = offset(a + (b - a) * t, side)
            fitted = quadratic_point(first, handle, last, t)
            error = max(error, hypot(exact[0] - fitted[0], exact[1] - fitted[1]))
        if error <= tolerance:
            return (("Q", *handle, *last),)
        return (*side_path(a, middle, side), *side_path(middle, b, side))

    return (
        ("M", *offset(0, 1)), *side_path(0, 1, 1),
        *round_cap(end, tangent(1)), *side_path(1, 0, -1),
        *round_cap(start, tuple(-v for v in tangent(0))), ("Z",),
    )


def capsule(shape):
    """Outline one round-capped glyph stroke, for raster and negative space."""
    if shape.path[1][0] == "Q":
        return curve_outline(shape)
    _, sx, sy = shape.path[0]
    _, ex, ey = shape.path[1]
    length = hypot(ex - sx, ey - sy)
    radius = shape.width / 2
    if not length:
        return rectangle(sx - radius, sy - radius, shape.width, shape.width, radius)
    tx, ty = (ex - sx) / length * radius, (ey - sy) / length * radius
    nx, ny = -ty, tx
    return (
        ("M", sx + nx, sy + ny), ("L", ex + nx, ey + ny),
        *round_cap((ex, ey), (tx, ty)),
        ("L", sx - nx, sy - ny),
        *round_cap((sx, sy), (-tx, -ty)), ("Z",),
    )


def designs():
    """One source of paths for SVG, Android vectors and Pillow at every size."""
    current_holes = tuple(capsule(glyph) for glyph in GLYPHS)
    current = IconDesign(
        (Shape(CANVAS, PAPER),), CURRENT_MARK,
        tuple(Shape(bubble.path, WHITE, cutouts=current_holes) for bubble in (BACK, FRONT)),
    )

    # All foreground edges, including round caps and tails, fit radius 33.
    # Large corners let this 60-unit-wide bubble fill the circular safe zone.
    bubble = Shape(rectangle(24, 26, 60, 50, 20,
                             tail=((51, 76), (39, 82), (40, 76))), PAPER)
    bubble_wen = (
        stroke((52.5, 29.5), (55.5, 35), VERMILION, 7),  # dot
        stroke((35, 41.5), (73, 41.5), VERMILION, 7),  # bar
        curve((62, 45.5), (55, 63), (35, 72.5), VERMILION, 7),  # falling left
        curve((44, 46), (55, 62), (74, 71), VERMILION, 7),  # falling right
    )

    # The diagonal follows x + y = 108. Each glyph stays wholly on its half.
    split_a = (
        stroke((29, 52), (39.5, 29), PAPER, 5.5),
        stroke((39.5, 29), (50, 52), PAPER, 5.5),
        stroke((33, 45), (46, 45), PAPER, 5.5),
    )
    split_wen = (
        stroke((67, 50), (68, 51.5), WHITE, 5),
        stroke((56, 59), (78, 59), WHITE, 5.5),
        stroke((61, 62), (75, 74), WHITE, 5.5),
        stroke((73, 62), (56, 75), WHITE, 5.5),
    )

    # A six-unit circular rim with a short, integral lower-left speech tail.
    ring_outline = (
        ("M", 54, 21),
        ("C", 54 + 30 * KAPPA, 21, 84, 51 - 30 * KAPPA, 84, 51),
        ("C", 84, 51 + 30 * KAPPA, 54 + 30 * KAPPA, 81, 54, 81),
        ("C", 51.7, 81, 49.4, 80.74, 47.2, 80.23),
        ("L", 36, 81), ("L", 37.6, 76.12),
        ("C", 29.32, 70.73, 24, 61.47, 24, 51),
        ("C", 24, 51 - 30 * KAPPA, 54 - 30 * KAPPA, 21, 54, 21), ("Z",),
    )
    ring = Shape(ring_outline, PAPER, cutouts=(rectangle(30, 27, 48, 48, 24),))
    ring_wen = (
        stroke((53, 35), (54, 36.5), INK, 6),
        stroke((42, 45.5), (66, 45.5), INK, 6),
        stroke((47, 49), (65, 64), INK, 6),
        stroke((61.5, 49), (43, 64), INK, 6),
    )

    def white(shapes):
        return tuple(replace(shape, fill=WHITE if shape.fill else "",
                             stroke=WHITE if shape.stroke else "") for shape in shapes)

    return {
        "current": current,
        "bubble": IconDesign(
            (Shape(CANVAS, INK),), (bubble, *bubble_wen),
            (Shape(bubble.path, WHITE, cutouts=tuple(capsule(g) for g in bubble_wen)),),
        ),
        "split": IconDesign(
            (Shape(CANVAS, INK), Shape(polygon((0, 108), (108, 0), (108, 108)), VERMILION)),
            (*split_a, *split_wen), white((*split_a, *split_wen)),
        ),
        "ring": IconDesign(
            (Shape(CANVAS, VERMILION),), (ring, *ring_wen), white((ring, *ring_wen)),
        ),
    }


DESIGNS = designs()
SELECTED = DESIGNS[DESIGN]
MARK = SELECTED.foreground


def flatten(path, tolerance):
    """Flatten quadratic/cubic curves within a raster subpixel tolerance."""
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
        elif op == "Q":
            start, control, end = points[-1], tuple(values[:2]), tuple(values[2:])
            # Degree elevation is exact; reuse the adaptive cubic sampler.
            first = tuple(a + 2 * (b - a) / 3 for a, b in zip(start, control))
            second = tuple(c + 2 * (b - c) / 3 for b, c in zip(control, end))
            cubic(start, first, second, end)
        elif op == "C":
            cubic(points[-1], tuple(values[:2]), tuple(values[2:4]), tuple(values[4:]))
        elif op == "Z":
            points.append(points[0])
        else:
            raise ValueError(f"Unsupported path command: {op}")
    return points


def raster(shapes, size, *, circle_mask=False, viewport=(0, 0, SIZE, SIZE)):
    """Draw the shared paths with ImageDraw at 4x, then downsample once."""
    pixels = size * 4
    left, top, right, bottom = viewport
    scale_x, scale_y = pixels / (right - left), pixels / (bottom - top)
    result = Image.new("RGBA", (pixels, pixels))
    draw = ImageDraw.Draw(result)

    def points(path):
        return [((x - left) * scale_x, (y - top) * scale_y)
                for x, y in flatten(path, 0.15 / max(scale_x, scale_y))]

    def paint(target, shape, color):
        if shape.stroke and shape.path[1][0] == "Q":
            sampled = points(shape.path)
            target.line(sampled, fill=color, width=max(1, round(shape.width * scale_x)),
                        joint="curve")
            rx, ry = shape.width * scale_x / 2, shape.width * scale_y / 2
            # Cover every join as well as the ends: Pillow can leave slivers
            # between thick line segments at shallow angles.
            for x, y in sampled:
                target.ellipse((x - rx, y - ry, x + rx, y + ry), fill=color)
        else:
            outline = capsule(shape) if shape.stroke else shape.path
            target.polygon(points(outline), fill=color)

    for shape in shapes:
        color = shape.stroke or shape.fill
        if shape.cutouts:
            mask = Image.new("L", result.size)
            mask_draw = ImageDraw.Draw(mask)
            paint(mask_draw, shape, 255)
            # Subtract the union: overlapping glyph strokes stay transparent.
            for hole in shape.cutouts:
                mask_draw.polygon(points(hole), fill=0)
            result.paste(color, (0, 0, pixels, pixels), mask)
        else:
            paint(draw, shape, color)
    if circle_mask:
        mask = Image.new("L", result.size)
        ImageDraw.Draw(mask).ellipse((0, 0, pixels - 1, pixels - 1), fill=255)
        result.putalpha(ImageChops.multiply(result.getchannel("A"), mask))
    return result.resize((size, size), Image.Resampling.LANCZOS)


def svg_shape(shape, index=0):
    attributes = f'd="{path_data(shape.path)}" fill="{shape.fill or "none"}"'
    if shape.stroke:
        attributes += (f' stroke="{shape.stroke}" stroke-width="{number(shape.width)}"'
                       ' stroke-linecap="round" stroke-linejoin="round"')
    if not shape.cutouts:
        return f"  <path {attributes}/>\n"
    # Intersect outside-of-hole clips, rather than XOR crossing glyph strokes.
    clips = []
    groups = []
    for hole_index, hole in enumerate(shape.cutouts):
        name = f"cutout-{index}-{hole_index}"
        outside = path_data(CANVAS) + " " + path_data(hole)
        clips.append(f'    <clipPath id="{name}" clipPathUnits="userSpaceOnUse">'
                     f'<path d="{outside}" clip-rule="evenodd"/></clipPath>\n')
        groups.append(f'  <g clip-path="url(#{name})">\n')
    return ("  <defs>\n" + "".join(clips) + "  </defs>\n" + "".join(groups)
            + f"  <path {attributes}/>\n" + "  </g>\n" * len(groups))


def svg(shapes, size=64):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}"'
            f' viewBox="0 0 {SIZE} {SIZE}">\n'
            + "".join(svg_shape(shape, i) for i, shape in enumerate(shapes)) + "</svg>\n")


def theme_shapes(style):
    # The tile wears the design's ground colour: paper for the original two
    # bubbles, the design's own background otherwise (a paper bubble on a
    # paper tile would vanish). The platform frames stay per style, so each
    # desktop style still has its own artwork.
    ground = PAPER if DESIGN == "current" else SELECTED.background[0].fill
    # Decorations follow Code's platform families, still in 108-unit space.
    if style == "macos":
        tile = rectangle(5.0625, 5.0625, 97.875, 96.1875, 21.9375)
        return (Shape(rectangle(5.0625, 7.59375, 97.875, 96.1875, 21.9375), "#d8d4ce"),
                Shape(tile, ground), *MARK)
    if style == "ios":
        tile = (("M", 28.6875, 1.6875), ("L", 79.3125, 1.6875),
                ("C", 99.5625, 1.6875, 106.3125, 8.4375, 106.3125, 28.6875),
                ("L", 106.3125, 79.3125),
                ("C", 106.3125, 99.5625, 99.5625, 106.3125, 79.3125, 106.3125),
                ("L", 28.6875, 106.3125),
                ("C", 8.4375, 106.3125, 1.6875, 99.5625, 1.6875, 79.3125),
                ("L", 1.6875, 28.6875),
                ("C", 1.6875, 8.4375, 8.4375, 1.6875, 28.6875, 1.6875), ("Z",))
        return (Shape(tile, ground), *MARK)
    if style == "android":
        return (Shape(rectangle(1.6875, 1.6875, 104.625, 104.625, 52.3125), ground), *MARK)
    if style == "windows":
        return (Shape(rectangle(8.4375, 10.125, 79.3125, 79.3125, 6.75), "#d8c4bb"),
                Shape(rectangle(11.8125, 13.5, 79.3125, 79.3125, 6.75), PAPER if ground == INK else INK),
                Shape(rectangle(15.1875, 16.875, 79.3125, 79.3125, 6.75), ground), *MARK)
    if style in ("nextstep", "windows-2000"):
        return (Shape(CANVAS, "#000000"),
                Shape(rectangle(1.6875, 1.6875, 104.625, 104.625), "#aaaaaa"),
                Shape(polygon((1.6875, 1.6875), (106.3125, 1.6875), (102.9375, 5.0625),
                              (5.0625, 5.0625), (5.0625, 102.9375), (1.6875, 106.3125)), WHITE),
                Shape(polygon((1.6875, 106.3125), (106.3125, 106.3125), (106.3125, 1.6875),
                              (102.9375, 5.0625), (102.9375, 102.9375), (5.0625, 102.9375)), "#555555"),
                Shape(rectangle(10.125, 10.125, 87.75, 87.75), ground), *MARK)
    # Newly added theme directories with a Translator icon keep the master tile.
    return (MASTER_TILE, *MARK)


def omarchy_svg():
    # Single-ink outlines with the theme's dark bubble interiors. Opaque fills
    # hide the overlap without requiring SVG clipping support in Makepad.
    ink = "#d5dce8"
    if DESIGN != "current":
        return svg(tuple(replace(s, fill=ink if s.fill else "", stroke=ink if s.stroke else "")
                         for s in SELECTED.monochrome))
    return svg(tuple(Shape(s.path, "#1a1b26", ink, 2.4) if s.fill else
                     Shape(s.path, stroke=ink, width=s.width * 0.75) for s in MARK))


def windows_2000_svg():
    # Derive a classic pixel variant from the shared geometry, never a second
    # hand-maintained pixel drawing. A 32 px grid keeps both glyphs readable.
    palette = Image.new("P", (1, 1))
    colors = [(0, 0, 0), (128, 0, 0), (0, 128, 0), (128, 128, 0),
              (0, 0, 128), (128, 0, 128), (0, 128, 128), (192, 192, 192),
              (128, 128, 128), (255, 0, 0), (0, 255, 0), (255, 255, 0),
              (0, 0, 255), (255, 0, 255), (0, 255, 255), (255, 255, 255)]
    palette.putpalette([channel for color in colors for channel in color] + [0] * (768 - 48))
    pixels = raster(theme_shapes("windows-2000"), 32).convert("RGB").quantize(
        palette=palette, dither=Image.Dither.NONE).convert("RGB")
    runs = {}
    unit = SIZE / pixels.width
    for y in range(pixels.height):
        x = 0
        while x < pixels.width:
            color = pixels.getpixel((x, y))
            end = x + 1
            while end < pixels.width and pixels.getpixel((end, y)) == color:
                end += 1
            fill = "#" + "".join(f"{channel:02x}" for channel in color)
            runs.setdefault(fill, []).extend(rectangle(x * unit, y * unit, (end - x) * unit, unit))
            x = end
    # One compound path per palette colour keeps the SVG's draw count small.
    paths = [Shape(tuple(path), fill) for fill, path in runs.items()]
    return svg(paths).replace('viewBox="', 'shape-rendering="crispEdges" viewBox="', 1)


def vector(shapes=None, *, monochrome=False):
    if shapes is None:
        shapes = SELECTED.monochrome if monochrome else MARK
    lines = [f'<vector xmlns:android="{ANDROID_NS}"',
             '    android:width="108dp" android:height="108dp"',
             '    android:viewportWidth="108" android:viewportHeight="108">']
    for shape in shapes:
        indent = "    "
        for cutout in shape.cutouts:
            hole = path_data(CANVAS) + " " + path_data(cutout)
            lines.append(f'{indent}<group>')
            indent += "    "
            lines.append(f'{indent}<clip-path android:pathData="{hole}" android:fillType="evenOdd"/>')
        attributes = f'android:pathData="{path_data(shape.path)}"'
        if shape.stroke:
            attributes += (f' android:fillColor="#00000000" android:strokeColor="{shape.stroke}"'
                           f' android:strokeWidth="{number(shape.width)}"'
                           ' android:strokeLineCap="round" android:strokeLineJoin="round"')
        else:
            attributes += f' android:fillColor="{shape.fill}"'
        lines.append(f'{indent}<path {attributes}/>')
        for _ in shape.cutouts:
            indent = indent[:-4]
            lines.append(f'{indent}</group>')
    return "\n".join((*lines, "</vector>", ""))


def background_xml(design):
    if len(design.background) == 1:
        return (f'<shape xmlns:android="{ANDROID_NS}" android:shape="rectangle">\n'
                f'    <solid android:color="{design.background[0].fill}"/>\n</shape>\n')
    return vector(design.background)


def adaptive_xml():
    return (f'<adaptive-icon xmlns:android="{ANDROID_NS}">\n'
            '    <background android:drawable="@drawable/ic_launcher_background"/>\n'
            '    <foreground android:drawable="@drawable/ic_launcher_foreground"/>\n'
            '    <monochrome android:drawable="@drawable/ic_launcher_monochrome"/>\n'
            '</adaptive-icon>\n')


def launcher_raster(shapes, size):
    return raster(shapes, size, circle_mask=True, viewport=LAUNCHER_VIEWPORT)


def launcher_composite(source, icon):
    # The supplied 1080x337 row has its second circle at (412.5, 120.5),
    # including antialiasing, rather than the approximate y=110 reference.
    box = (338, 46, 487, 195)
    left, top, right, bottom = box
    replacement = icon.resize((right - left, bottom - top), Image.Resampling.LANCZOS)
    patch = source.crop(box)
    # Restore wallpaper underneath the antialiased rim so the old paper circle
    # cannot leave a pale halo. Preserve all pixels outside the replacement.
    alpha = replacement.getchannel("A")
    for y in range(patch.height):
        before = source.getpixel((left - 1, top + y))
        after = source.getpixel((right, top + y))
        for x in range(patch.width):
            if alpha.getpixel((x, y)):
                weight = (x + 1) / (patch.width + 1)
                color = tuple(round(a + (b - a) * weight) for a, b in zip(before, after))
                patch.putpixel((x, y), color)
    patch.alpha_composite(replacement)
    result = source.copy()
    result.paste(patch, (left, top))
    return result


def preview(write):
    """Write candidate assets and launcher comparisons, never app resources."""
    with Image.open(ARTWORK / "launcher_row.png") as source:
        row = source.convert("RGBA")
    if row.size != (1080, 337):
        raise ValueError("launcher_row.png must be the supplied 1080x337 launcher row")
    strip = Image.new("RGBA", (48 * len(CANDIDATES), 48))
    for index, name in enumerate(CANDIDATES):
        design = DESIGNS[name]
        folder = ARTWORK / "candidates" / name
        large = launcher_raster(design.artwork, 192)
        small = launcher_raster(design.artwork, 48)
        write(folder / "icon.svg", svg(design.artwork, 108))
        write(folder / "ic_launcher.xml", adaptive_xml())
        write(folder / "ic_launcher_background.xml", background_xml(design))
        write(folder / "ic_launcher_foreground.xml", vector(design.foreground))
        write(folder / "ic_launcher_monochrome.xml", vector(design.monochrome))
        write(folder / "monochrome.svg", svg(design.monochrome, 108))
        write(folder / "launcher_192.png", large)
        write(folder / "launcher_48.png", small)
        for size in (192, 48):
            write(folder / f"monochrome_{size}.png", launcher_raster(design.monochrome, size))
        write(ARTWORK / f"preview_{name}.png", launcher_composite(row, large))
        strip.paste(small, (index * 48, 0))
    write(ARTWORK / "preview_all.png", strip.resize((strip.width * 4, strip.height * 4),
                                                   Image.Resampling.NEAREST))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--preview", action="store_true",
                        help="compare all three candidates without changing app resources")
    args = parser.parse_args()
    written = []

    def write(path, content, **save_options):
        path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, Image.Image):
            content.save(path, optimize=True, **save_options)
        else:
            path.write_text(content, encoding="utf-8")
        written.append(path.relative_to(ROOT))

    if args.preview:
        preview(write)
        print("Files written:")
        for path in written:
            print(path)
        return

    master = (MASTER_TILE, *MARK) if DESIGN == "current" else SELECTED.artwork
    write(RESOURCES / "icon.svg", svg(master, 1024))
    for size in (32, 64, 128, 1024):
        write(RESOURCES / f"icon_{size}.png", raster(master, size))
    ico_images = [raster(master, size) for size in (16, 24, 32, 48, 64, 128, 256)]
    write(RESOURCES / "icon.ico", ico_images[-1], format="ICO",
          sizes=[icon.size for icon in ico_images], append_images=ico_images[:-1])
    for path in sorted((ROOT / "widgets/themes").glob("*/icons/translator.svg")):
        style = path.parents[1].name
        content = (omarchy_svg() if style == "omarchy" else
                   windows_2000_svg() if style == "windows-2000" else svg(theme_shapes(style)))
        write(path, content)

    android = RESOURCES / "android/res"
    for name in ("ic_launcher", "ic_launcher_round"):
        write(android / "mipmap-anydpi-v26" / f"{name}.xml", adaptive_xml())
    write(android / "drawable/ic_launcher_background.xml", background_xml(SELECTED))
    write(android / "drawable/ic_launcher_foreground.xml", vector())
    write(android / "drawable/ic_launcher_monochrome.xml", vector(monochrome=True))
    for density, size in DENSITIES.items():
        legacy = (LEGACY_TILE, *MARK) if DESIGN == "current" else SELECTED.artwork
        write(android / f"mipmap-{density}/ic_launcher.png", raster(legacy, size))
        round_icon = (raster(SELECTED.artwork, size, circle_mask=True) if DESIGN == "current"
                      else launcher_raster(SELECTED.artwork, size))
        write(android / f"mipmap-{density}/ic_launcher_round.png", round_icon)
    write(RESOURCES / "icon_preview.png", raster(master, 512))

    print("Files written:")
    for path in written:
        print(path)


if __name__ == "__main__":
    main()
