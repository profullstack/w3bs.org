#!/usr/bin/env python3
"""Draw the W3BS DE wallpapers: the w3bs mark (public/mark.svg) as a quiet outline on ink.

    python3 de/tools/wallpaper.py de/share/wallpaper

Writes w3bs-dark.png (3840x2160) and w3bs-light.png. Needs Pillow.
"""
import math
import sys
from pathlib import Path

from PIL import Image, ImageDraw

INK = (28, 43, 38)        # a shade under --ink #23352f, so the bar and borders read on it
PAPER = (246, 244, 237)   # --paper
ORANGE = (217, 82, 49)    # --orange
W, H = 3840, 2160
SS = 3                    # supersampling for the mark


def ellipse_points(cx, cy, rx, ry, deg, n=720):
    a = math.radians(deg)
    pts = []
    for i in range(n + 1):
        t = 2 * math.pi * i / n
        x, y = rx * math.cos(t), ry * math.sin(t)
        pts.append((cx + x * math.cos(a) - y * math.sin(a), cy + x * math.sin(a) + y * math.cos(a)))
    return pts


def mark(size, stroke_rgba, dot_rgba, width):
    """The mark's lines (two crossed ellipses, the vertical, the dot), without its tile."""
    s = size * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    k = s / 64
    w = max(1, round(width * SS))
    for deg in (-30, 30):
        d.line(ellipse_points(32 * k, 32 * k, 22 * k, 11 * k, deg), fill=stroke_rgba, width=w, joint="curve")
    d.line([(32 * k, 10 * k), (32 * k, 54 * k)], fill=stroke_rgba, width=w)
    r = 5 * k
    d.ellipse([32 * k - r, 32 * k - r, 32 * k + r, 32 * k + r], fill=dot_rgba)
    return img.resize((size, size), Image.LANCZOS)


def wallpaper(bg, line, dot, path):
    img = Image.new("RGB", (W, H), bg)
    size = 1500
    m = mark(size, line, dot, 3.2 * size / 64 / 6)
    img.paste(m, ((W - size) // 2, (H - size) // 2), m)
    img.save(path, optimize=True)


def main(out):
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    wallpaper(INK, PAPER + (22,), ORANGE + (200,), out / "w3bs-dark.png")
    wallpaper(PAPER, (35, 53, 47, 26), ORANGE + (210,), out / "w3bs-light.png")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "de/share/wallpaper")
