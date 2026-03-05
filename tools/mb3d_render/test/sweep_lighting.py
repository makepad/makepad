import os
import subprocess
from PIL import Image
import math

def calculate_metrics(img1_path, img2_path):
    try:
        img1 = Image.open(img1_path).convert('RGB')
        img2 = Image.open(img2_path).convert('RGB')
    except Exception as e:
        return float('inf'), float('inf'), 0.0

    if img1.size != img2.size:
        img2 = img2.resize(img1.size, Image.Resampling.LANCZOS)

    data1 = list(img1.getdata())
    data2 = list(img2.getdata())

    mse = sum(sum((a - b) ** 2 for a, b in zip(p1, p2)) for p1, p2 in zip(data1, data2)) / (len(data1) * 3.0 * 255.0 * 255.0)
    mae = sum(sum(abs(a - b) for a, b in zip(p1, p2)) for p1, p2 in zip(data1, data2)) / (len(data1) * 3.0 * 255.0)

    # Luminance correlation
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

target_img = "cathedral.jpg.webp"
output_img = "cathedral_test.png"

best_corr = -1
best_params = {}

def run_test(params):
    env = os.environ.copy()
    for k, v in params.items():
        env[k] = str(v)
    
    cmd = ["cargo", "run", "--release", "--quiet", "--", "../../local/mb3d/cathedral.m3p", output_img]
    subprocess.run(cmd, env=env, cwd="tools/mb3d_render", stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    
    mse, mae, corr = calculate_metrics(target_img, "tools/mb3d_render/" + output_img)
    return mse, mae, corr

# We will test a few parameter combinations to see what improves correlation
tests = [
    {"AO_STRENGTH": 0.8, "AO_BASE_MUL": 0.3, "SHADOW_STRENGTH": 0.8, "TONE_GAMMA": 0.5},
    {"AO_STRENGTH": 1.0, "AO_BASE_MUL": 0.2, "SHADOW_STRENGTH": 0.9, "TONE_GAMMA": 0.6},
    {"AO_STRENGTH": 1.2, "AO_BASE_MUL": 0.1, "SHADOW_STRENGTH": 1.0, "TONE_GAMMA": 0.7},
]

for t in tests:
    mse, mae, corr = run_test(t)
    print(f"Params: {t} -> MSE: {mse:.4f}, MAE: {mae:.4f}, Corr: {corr:.4f}")

