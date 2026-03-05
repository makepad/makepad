import os
from PIL import Image
import math

def calculate_metrics(img1_path, img2_path):
    try:
        img1 = Image.open(img1_path).convert('RGB')
        img2 = Image.open(img2_path).convert('RGB')
    except Exception as e:
        print(f"Error: {e}")
        return float('inf'), float('inf'), 0.0

    if img1.size != img2.size:
        img2 = img2.resize(img1.size, Image.Resampling.LANCZOS)

    data1 = list(img1.getdata())
    data2 = list(img2.getdata())

    mse = sum(sum((a - b) ** 2 for a, b in zip(p1, p2)) for p1, p2 in zip(data1, data2)) / (len(data1) * 3.0 * 255.0 * 255.0)
    mae = sum(sum(abs(a - b) for a, b in zip(p1, p2)) for p1, p2 in zip(data1, data2)) / (len(data1) * 3.0 * 255.0)

    lum1 = [0.299*r + 0.587*g + 0.114*b for r, g, b in data1]
    lum2 = [0.299*r + 0.587*g + 0.114*b for r, g, b in data2]

    mean1 = sum(lum1) / len(lum1)
    mean2 = sum(lum2) / len(lum2)

    var1 = sum((l - mean1)**2 for l in lum1)
    var2 = sum((l - mean2)**2 for l in lum2)

    if var1 == 0 or var2 == 0:
        corr = 0.0
    else:
        cov = sum((l1 - mean1)*(l2 - mean2) for l1, l2 in zip(lum1, lum2))
        corr = cov / math.sqrt(var1 * var2)

    return mse, mae, corr

import sys

if len(sys.argv) >= 3:
    img1_path = sys.argv[1]
    img2_path = sys.argv[2]
else:
    img1_path = "../../cathedral.jpg.webp"
    img2_path = "cathedral_test.png"

mse, mae, corr = calculate_metrics(img1_path, img2_path)
print(f"MSE: {mse:.4f}, MAE: {mae:.4f}, Corr: {corr:.4f}")
