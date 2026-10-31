#!/usr/bin/env python3
"""Makepad Code's app icon: the source of every variant.

The mark is a terminal prompt: an ion-cyan chevron and a signal-magenta
block cursor on a deep ink screen with faint scanlines, the app's own
palette (apps/code/src/ui/theme.rs). This script writes:

  apps/code/resources/icon.svg                  1024 master (render.sh -> icon_1024.png)
  widgets/themes/<style>/icons/code.svg         the seven desktop-style variants

Run from the repository root: python3 tools/app_icons/code/icons.py
Then: sh tools/app_icons/render.sh apps/code/resources/icon.svg and copy
icon_1024.png to code-icon.png (the Builder's bundle icon). Original
artwork made for this project.
"""
import os

AGENT = "#3ce6f2"
AGENT_HI = "#9df6fb"
HUMAN = "#ff4f9e"
INK_TOP = "#0e1a26"
INK_BOT = "#04060a"


def defs(prefix=""):
    return (
        "<defs>"
        f'<linearGradient id="code-ink" x2="0.7" y2="1"><stop stop-color="{INK_TOP}"/><stop offset="1" stop-color="{INK_BOT}"/></linearGradient>'
        f'<linearGradient id="code-chev" x2="0.4" y2="1"><stop stop-color="{AGENT_HI}"/><stop offset="1" stop-color="{AGENT}"/></linearGradient>'
        '<linearGradient id="code-tile" x2="0.7" y2="1"><stop stop-color="#ff5aa8"/><stop offset="1" stop-color="#6a2bd9"/></linearGradient>'
        f"{prefix}</defs>"
    )


def mark(x, y, w, h, *, glow=True, scan=True, flat=False):
    """The prompt inside a screen at (x, y, w, h), in that box's units."""
    sw = w * 0.115
    p = lambda fx, fy: f"{x + w * fx:.2f} {y + h * fy:.2f}"
    chev = f"M{p(0.245, 0.31)} L{p(0.475, 0.5)} L{p(0.245, 0.69)}"
    cx, cy, cw, ch = x + w * 0.585, y + h * 0.33, w * 0.17, h * 0.34
    out = []
    if scan:
        for fy in (0.22, 0.78):
            out.append(f'<rect x="{x + w * 0.12:.2f}" y="{y + h * fy:.2f}" width="{w * 0.76:.2f}" height="{h * 0.022:.2f}" fill="{AGENT}" opacity="0.14"/>')
    if glow:
        # A soft halo: two faint layers, wide then narrow.
        for k, a in ((2.3, 0.06), (1.55, 0.11)):
            out.append(f'<path d="{chev}" fill="none" stroke="{AGENT}" stroke-width="{sw * k:.2f}" stroke-linecap="round" stroke-linejoin="round" opacity="{a}"/>')
        for pad, a in ((0.05, 0.07), (0.024, 0.13)):
            out.append(f'<rect x="{cx - w * pad:.2f}" y="{cy - h * pad:.2f}" width="{cw + 2 * w * pad:.2f}" height="{ch + 2 * h * pad:.2f}" rx="{w * pad:.2f}" fill="{HUMAN}" opacity="{a}"/>')
    cap = "butt" if flat else "round"
    join = "miter" if flat else "round"
    stroke = AGENT if flat else "url(#code-chev)"
    out.append(f'<path d="{chev}" fill="none" stroke="{stroke}" stroke-width="{sw:.2f}" stroke-linecap="{cap}" stroke-linejoin="{join}"/>')
    out.append(f'<rect x="{cx:.2f}" y="{cy:.2f}" width="{cw:.2f}" height="{ch:.2f}" rx="{0 if flat else w * 0.015:.2f}" fill="{HUMAN}"/>')
    return "".join(out)


def svg64(body, extra_defs=""):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">{defs(extra_defs)}{body}</svg>\n'


SCREEN = (9, 10, 43, 42)


def screen(rx=5):
    x, y, w, h = SCREEN
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="url(#code-ink)"/>' + mark(x, y, w, h)


def macos():
    # A full-bleed ink tile, the prompt filling it.
    return svg64(
        '<g opacity="0.12"><rect x="3" y="4.5" width="58" height="57" rx="13" fill="#101521"/></g>'
        '<rect x="3" y="3" width="58" height="57" rx="13" fill="url(#code-ink)"/>'
        f'<rect x="3.5" y="3.5" width="57" height="56" rx="12.5" fill="none" stroke="{AGENT}" stroke-opacity="0.22"/>'
        + mark(3, 3, 58, 57)
    )


def ios():
    return svg64(
        '<path d="M17 1 H47 C59 1 63 5 63 17 V47 C63 59 59 63 47 63 H17 C5 63 1 59 1 47 V17 C1 5 5 1 17 1 Z" fill="url(#code-tile)"/>'
        + screen()
    )


def android():
    return svg64('<circle cx="32" cy="32" r="31" fill="#ffe3f0"/><g transform="translate(5 5) scale(0.84)">' + screen() + "</g>")


def windows():
    return svg64(
        '<rect x="5" y="6" width="42" height="43" rx="4" fill="#ffc6e0"/>'
        '<rect x="7" y="8" width="42" height="43" rx="4" fill="#c63f86"/>'
        + screen()
    )


def nextstep():
    x, y, w, h = SCREEN
    return svg64(
        '<rect width="64" height="64" fill="#000000"/><rect x="1" y="1" width="62" height="62" fill="#aaaaaa"/>'
        '<path d="M1 1 H63 L61 3 H3 V61 L1 63 Z" fill="#ffffff"/><path d="M1 63 H63 V1 L61 3 V61 H3 Z" fill="#555555"/>'
        '<rect x="7" y="8" width="47" height="46" fill="#555555"/>'
        f'<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="#0b1119"/>' + mark(x, y, w, h, glow=False, scan=False, flat=True)
    )


def omarchy():
    # Line art in the set's single ink: the chevron and a cursor bar.
    ink = "#d5dce8"
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">'
        f'<path d="M14 18 L29 32 L14 46" fill="none" stroke="{ink}" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>'
        f'<rect x="35" y="20" width="11" height="24" rx="1.5" fill="none" stroke="{ink}" stroke-width="3"/>'
        "</svg>\n"
    )


# windows-2000: 16x16 pixels in the classic 16-colour palette.
W2K = [
    "................",
    ".WWWWWWWWWWWWWW.",
    ".WKKKKKKKKKKKKG.",
    ".WKKKKKKKKKKKKG.",
    ".WKKKKKKKKKKKKG.",
    ".WKCCKKKKMMKKKG.",
    ".WKKCCKKKMMKKKG.",
    ".WKKKCCKKMMKKKG.",
    ".WKKCCKKKMMKKKG.",
    ".WKCCKKKKMMKKKG.",
    ".WKKKKKKKKKKKKG.",
    ".WKKKKKKKKKKKKG.",
    ".WKKKKKKKKKKKKG.",
    ".WKKKKKKKKKKKKG.",
    ".GGGGGGGGGGGGGG.",
    "................",
]
W2K_INK = {"W": "#ffffff", "K": "#000000", "G": "#808080", "C": "#00ffff", "M": "#ff00ff"}


def windows_2000():
    rects = []
    for y, row in enumerate(W2K):
        x = 0
        while x < 16:
            c = row[x]
            if c == ".":
                x += 1
                continue
            run = 1
            while x + run < 16 and row[x + run] == c:
                run += 1
            rects.append(f'<rect x="{x}" y="{y}" width="{run}" height="1" fill="{W2K_INK[c]}"/>')
            x += run
    return '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">' + "".join(rects) + "</svg>\n"


def master():
    # 1024: the macOS shape at icon-grid size (824 tile, 100 margin).
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">'
        + defs()
        + '<rect x="100" y="100" width="824" height="824" rx="190" fill="url(#code-ink)"/>'
        + f'<rect x="108" y="108" width="808" height="808" rx="182" fill="none" stroke="{AGENT}" stroke-opacity="0.22" stroke-width="10"/>'
        + mark(100, 100, 824, 824)
        + "</svg>\n"
    )


STYLES = {
    "macos": macos,
    "ios": ios,
    "android": android,
    "windows": windows,
    "nextstep": nextstep,
    "omarchy": omarchy,
    "windows-2000": windows_2000,
}

if __name__ == "__main__":
    for style, make in STYLES.items():
        path = f"widgets/themes/{style}/icons/code.svg"
        with open(path, "w") as f:
            f.write(make())
        print(path)
    os.makedirs("apps/code/resources", exist_ok=True)
    with open("apps/code/resources/icon.svg", "w") as f:
        f.write(master())
    print("apps/code/resources/icon.svg")
