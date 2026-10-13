#!/usr/bin/env python3
"""Draws the camera-translate test picture (a Chinese menu on paper) to menu.png.

  python3 apps/translator/resources/fixtures/make_menu.py

Needs Pillow and a CJK font (macOS: STHeiti). The picture is a fixture for the
camera path; its lines match reader::FixtureReader's boxes.
"""
from PIL import Image, ImageDraw, ImageFont
import pathlib, random

W, H = 810, 1080
img = Image.new("RGB", (W, H), (236, 224, 190))
d = ImageDraw.Draw(img)
random.seed(7)
for _ in range(2500):
    x, y = random.randrange(W), random.randrange(H)
    d.point((x, y), fill=(220 + random.randrange(12), 207 + random.randrange(12), 172 + random.randrange(12)))
font = lambda s: ImageFont.truetype("/System/Library/Fonts/STHeiti Medium.ttc", s)
rows = [("今日菜单", 0.26, 0.10, 64), ("宫保鸡丁  38元", 0.12, 0.26, 44), ("麻婆豆腐  26元", 0.12, 0.38, 44),
        ("红烧肉  58元", 0.12, 0.50, 44), ("酸辣汤  18元", 0.12, 0.62, 44), ("米饭 免费", 0.12, 0.74, 44)]
for text, x, y, size in rows:
    d.text((x * W, y * H), text, font=font(size), fill=(40, 28, 24))
d.rectangle((30, 30, W - 30, H - 30), outline=(150, 40, 30), width=6)
img.save(pathlib.Path(__file__).with_name("menu.png"))
print("wrote menu.png")
