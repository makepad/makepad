#!/usr/bin/env python3
"""FLUX.2-klein-4B image-edit oracle dumps.

Locked fixture: 512px, 4 steps, seed 7, one reference PNG, instruction
"change the sky to sunset orange". Dumps input_ids, packed ref latents,
noise, mid-step residual, decoded PNG, timing.

Run on the CUDA box only. Yield the GPU if another job holds it.

  python ref_dump_flux2_klein_edit.py --models C:\\ai\\flux2edit\\models --dumps C:\\ai\\flux2edit\\dumps
"""

from __future__ import annotations

import argparse
import json
import os
import time

import numpy as np
import torch
from PIL import Image


PROMPT = "change the sky to sunset orange"
SEED = 7
WIDTH = 512
HEIGHT = 512
STEPS = 4


def dump(dir_path, name, tensor):
    array = tensor.detach().to(torch.float32).cpu().numpy()
    path = os.path.join(dir_path, f"{name}.npy")
    np.save(path, array)
    print(f"dump {name}: {tuple(array.shape)} -> {path}", flush=True)


def make_reference(path):
    if os.path.isfile(path):
        return Image.open(path).convert("RGB")
    img = Image.new("RGB", (WIDTH, HEIGHT), (40, 80, 180))
    for y in range(HEIGHT // 2, HEIGHT):
        for x in range(WIDTH):
            img.putpixel((x, y), (30, 90, 40))
    img.save(path)
    return img


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--models", default=r"C:\ai\flux2edit\models")
    parser.add_argument("--dumps", default=r"C:\ai\flux2edit\dumps")
    parser.add_argument("--ref", default=None)
    args = parser.parse_args()
    os.makedirs(args.dumps, exist_ok=True)

    from diffusers import Flux2KleinPipeline

    device = "cuda"
    dtype = torch.bfloat16
    pipe = Flux2KleinPipeline.from_pretrained(args.models, torch_dtype=dtype).to(device)
    pipe.set_progress_bar_config(disable=False)

    ref_path = args.ref or os.path.join(args.dumps, "reference.png")
    ref = make_reference(ref_path)

    # Tokenize the same way Qwen3Embedder / Flux2KleinPipeline does.
    tokenizer = pipe.tokenizer
    messages = [{"role": "user", "content": PROMPT}]
    text = tokenizer.apply_chat_template(
        messages,
        tokenize=False,
        add_generation_prompt=True,
        enable_thinking=False,
    )
    encoded = tokenizer(
        text,
        return_tensors="pt",
        padding="max_length",
        truncation=True,
        max_length=512,
    )
    dump(args.dumps, "input_ids", encoded["input_ids"][0])
    dump(args.dumps, "attention_mask", encoded["attention_mask"][0])
    with open(os.path.join(args.dumps, "rendered_template.txt"), "w", encoding="utf-8") as f:
        f.write(text)

    generator = torch.Generator(device=device).manual_seed(SEED)

    # Warm once so the timed run is the second pass.
    _ = pipe(
        prompt=PROMPT,
        image=[ref],
        width=WIDTH,
        height=HEIGHT,
        num_inference_steps=STEPS,
        generator=torch.Generator(device=device).manual_seed(SEED),
        output_type="np",
    )
    torch.cuda.synchronize()

    # Instrument encode / noise / mid residual via hooks on the transformer.
    captured = {}

    def on_step_end(pipe_self, step_index, timestep, callback_kwargs):
        latents = callback_kwargs.get("latents")
        if latents is not None and step_index == 1:
            dump(args.dumps, "step_latent_1", latents)
        return callback_kwargs

    t0 = time.perf_counter()
    out = pipe(
        prompt=PROMPT,
        image=[ref],
        width=WIDTH,
        height=HEIGHT,
        num_inference_steps=STEPS,
        generator=generator,
        output_type="np",
        callback_on_step_end=on_step_end,
    )
    torch.cuda.synchronize()
    warm_ms = (time.perf_counter() - t0) * 1000.0

    image = (out.images[0] * 255.0).round().astype(np.uint8)
    Image.fromarray(image).save(os.path.join(args.dumps, "decoded.png"))

    # Best-effort extra dumps if the pipeline exposed them.
    if hasattr(pipe, "_flux2_oracle"):
        extra = pipe._flux2_oracle
        for key, value in extra.items():
            dump(args.dumps, key, value)

    vram = torch.cuda.max_memory_allocated() / (1024 ** 3)
    meta = {
        "prompt": PROMPT,
        "width": WIDTH,
        "height": HEIGHT,
        "steps": STEPS,
        "seed": SEED,
        "warm_ms": warm_ms,
        "peak_vram_gb": vram,
        "device": torch.cuda.get_device_name(0),
    }
    with open(os.path.join(args.dumps, "meta.json"), "w", encoding="utf-8") as f:
        json.dump(meta, f, indent=1)
    print("meta", json.dumps(meta), flush=True)


if __name__ == "__main__":
    main()
