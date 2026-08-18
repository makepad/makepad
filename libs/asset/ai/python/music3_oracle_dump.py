# music3_oracle_dump.py - standalone MiniMax-Music3 stage dumps for the native
# port. Does NOT talk to the live :8123 service. Loads the official
# ModularPipeline from a local cache dir (same view rewrite as music3_worker.py)
# and writes numpy + wav + meta.json for a short seeded clip.
#
# Usage (169):
#   C:\ai\music3venv\Scripts\python.exe music3_oracle_dump.py \
#     --model-dir C:\ai\asset_node_cache\music\MiniMax-Music3 \
#     --out-dir C:\ai\music3_oracle\pine_5s_seed7 \
#     --seconds 5 --seed 7
import argparse
import json
import os
import shutil
import sys
import time
import wave

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")

# Official fixture (docs example lyrics/caption, shortened duration).
DEFAULT_PROMPT = (
    "Genre: acoustic pop. BPM: 96. Key: C major. Warm and intimate, "
    "building gently into the chorus. Vocals: soft female lead, close and "
    "breathy, light stacked harmonies in the chorus. Arrangement: "
    "fingerpicked guitar and soft piano; brushed drums and upright bass "
    "enter in the chorus."
)
DEFAULT_LYRICS = (
    "[verse]\n"
    "Morning light filtering through the pine\n"
    "Every quiet street is yours and mine\n"
    "[chorus]\n"
    "Softly the world begins to breathe"
)


def build_local_view(model_dir, view_dir):
    for root, _dirs, files in os.walk(model_dir):
        rel = os.path.relpath(root, model_dir)
        dst_root = os.path.join(view_dir, rel) if rel != "." else view_dir
        os.makedirs(dst_root, exist_ok=True)
        for name in files:
            src = os.path.join(root, name)
            dst = os.path.join(dst_root, name)
            if os.path.exists(dst) and os.path.getsize(dst) == os.path.getsize(src):
                continue
            if os.path.exists(dst):
                os.remove(dst)
            if name == "modular_model_index.json":
                with open(src, "r", encoding="utf-8") as handle:
                    index = json.load(handle)
                for value in index.values():
                    if isinstance(value, list) and len(value) == 3 and isinstance(value[2], dict):
                        if "pretrained_model_name_or_path" in value[2]:
                            value[2]["pretrained_model_name_or_path"] = view_dir
                with open(dst, "w", encoding="utf-8") as handle:
                    json.dump(index, handle, indent=1)
                continue
            try:
                os.link(src, dst)
            except OSError:
                shutil.copyfile(src, dst)


def to_numpy(value):
    import numpy as np
    import torch

    if isinstance(value, torch.Tensor):
        return value.detach().to(torch.float32).cpu().numpy()
    return np.asarray(value)


def write_wav_i16(path, data, sample_rate):
    import numpy as np

    pcm = (np.clip(data.T, -1.0, 1.0) * 32767.0).astype("<i2")
    with wave.open(path, "wb") as handle:
        handle.setnchannels(pcm.shape[1])
        handle.setsampwidth(2)
        handle.setframerate(int(sample_rate))
        handle.writeframes(pcm.tobytes())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--out-dir", required=True)
    parser.add_argument("--view-dir", default=None)
    parser.add_argument("--prompt", default=DEFAULT_PROMPT)
    parser.add_argument("--lyrics", default=DEFAULT_LYRICS)
    parser.add_argument("--seconds", type=float, default=5.0)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--steps", type=int, default=30)
    args = parser.parse_args()

    out_dir = os.path.abspath(args.out_dir)
    os.makedirs(out_dir, exist_ok=True)
    view_dir = args.view_dir or os.path.join(out_dir, "_model_view")
    stats = {}
    t0 = time.time()

    def save(name, value):
        import numpy as np

        arr = np.ascontiguousarray(to_numpy(value))
        np.save(os.path.join(out_dir, name + ".npy"), arr)
        stats[name] = {
            "shape": list(arr.shape),
            "dtype": str(arr.dtype),
            "absmax": float(np.abs(arr).max()) if arr.size else 0.0,
            "mean": float(arr.astype("float64").mean()) if arr.size else 0.0,
            "std": float(arr.astype("float64").std()) if arr.size else 0.0,
        }
        print("saved", name, arr.shape, "absmax", stats[name]["absmax"], flush=True)

    print("build-view", args.model_dir, "->", view_dir, flush=True)
    build_local_view(args.model_dir, view_dir)

    print("load-libs", flush=True)
    import numpy as np
    import torch
    from diffusers import ModularPipeline
    from diffusers.modular_pipelines.minimax_music3.encoders import (
        _AUDIO_CFG_TOKEN_ID,
        _AUDIO_CODE_OFFSET,
        _AUDIO_END_TOKEN_ID,
        _AR_CFG_SCALE,
        _AR_CFG_TOP_K,
        _AR_SAMPLING_TOP_K,
        _clean_caption,
        _normalize_lyrics,
    )

    print("load-components", flush=True)
    pipe = ModularPipeline.from_pretrained(view_dir)
    pipe.load_components(dtype=torch.bfloat16)
    print("to-gpu", flush=True)
    pipe.to("cuda")
    print(
        "ready sr=%s fr=%s hop=%s codebooks=%s vram=%.1fGiB"
        % (
            pipe.sampling_rate,
            pipe.frame_rate,
            pipe.latent_hop_length,
            pipe.num_codebooks,
            torch.cuda.max_memory_allocated() / (1024**3),
        ),
        flush=True,
    )

    assembled = (
        "<|im_start|><|caption_start|>"
        + _clean_caption(args.prompt)
        + "<|caption_end|><|lyrics_start|>"
        + _normalize_lyrics(args.lyrics)
        + "<|lyrics_end|><|im_end|><|audio_start|>"
    )
    with open(os.path.join(out_dir, "assembled_prompt.txt"), "w", encoding="utf-8") as handle:
        handle.write(assembled)
    tok = pipe.tokenizer
    text_ids = tok(assembled, return_tensors="pt")["input_ids"]
    uncond = text_ids.clone()
    uncond[:, 1:-2] = _AUDIO_CFG_TOKEN_ID
    text_ids_pair = torch.cat((text_ids, uncond), dim=0)
    save("text_ids", text_ids_pair.cpu().numpy().astype(np.int64))
    specials = {
        name: int(tok.convert_tokens_to_ids(name))
        for name in (
            "<|im_start|>",
            "<|im_end|>",
            "<|caption_start|>",
            "<|caption_end|>",
            "<|lyrics_start|>",
            "<|lyrics_end|>",
            "<|audio_start|>",
            "<|endoftext|>",
        )
    }

    taps = {
        "lm_calls": 0,
        "dit_calls": 0,
        "rvq_calls": 0,
        "cond_calls": 0,
        "vocoder_calls": 0,
        "semantic_codes": [],
        "rvq_codes": [],
    }

    lm_model = pipe.language_model.model
    orig_lm = lm_model.forward

    def lm_forward(*a, **kw):
        out = orig_lm(*a, **kw)
        taps["lm_calls"] += 1
        if taps["lm_calls"] == 1:
            save("lm_prefill_last_hidden", out.last_hidden_state)
            last = out.last_hidden_state[:, -1]
            logits = pipe.language_model.lm_head(last).float()
            save("lm_prefill_logits", logits)
        return out

    lm_model.forward = lm_forward

    orig_dit = pipe.transformer.forward

    def dit_forward(*a, **kw):
        out = orig_dit(*a, **kw)
        taps["dit_calls"] += 1
        sample = out[0] if isinstance(out, (tuple, list)) else getattr(out, "sample", out)
        if taps["dit_calls"] == 1:
            hidden = kw.get("hidden_states", a[0] if a else None)
            timestep = kw.get("timestep")
            cond = kw.get("encoder_hidden_states")
            if hidden is not None:
                save("dit_step0_x", hidden)
            if timestep is not None:
                save("dit_step0_t", timestep)
            if cond is not None:
                save("dit_step0_cond", cond)
            save("dit_step0_v_cond", sample)
        elif taps["dit_calls"] == 2:
            save("dit_step0_v_uncond", sample)
        return out

    pipe.transformer.forward = dit_forward

    orig_rvq = pipe.rvq_depth_decoder.forward

    def rvq_forward(*a, **kw):
        out = orig_rvq(*a, **kw)
        taps["rvq_calls"] += 1
        if taps["rvq_calls"] == 1:
            hidden = a[0] if a else kw.get("inputs_embeds")
            if hidden is not None:
                save("rvq_step0_in", hidden)
            save("rvq_step0_out", out)
        return out

    pipe.rvq_depth_decoder.forward = rvq_forward

    orig_cond = pipe.condition_encoder.forward

    def cond_forward(*a, **kw):
        out = orig_cond(*a, **kw)
        taps["cond_calls"] += 1
        if taps["cond_calls"] == 1:
            hidden = a[0] if a else kw.get("hidden_states")
            if hidden is not None:
                save("cond_enc_in", hidden)
            save("cond_enc_out", out)
        return out

    pipe.condition_encoder.forward = cond_forward

    orig_voc = pipe.vocoder.forward

    def voc_forward(*a, **kw):
        out = orig_voc(*a, **kw)
        taps["vocoder_calls"] += 1
        if taps["vocoder_calls"] == 1:
            hidden = a[0] if a else kw.get("latents")
            if hidden is not None:
                save("vocoder_in", hidden)
            save("vocoder_out", out)
        return out

    pipe.vocoder.forward = voc_forward

    # Capture sampled semantic codes from lm_head-guided first token of each
    # inner-model decode after prefill by wrapping the official sampler.
    import diffusers.modular_pipelines.minimax_music3.encoders as enc

    orig_sample = enc._sample_top_k
    sample_n = {"n": 0}

    def sample_top_k(logits, generator):
        token = orig_sample(logits, generator)
        sample_n["n"] += 1
        # Global LM samples once per frame (after CFG), then RVQ samples 7
        # residual codes. Record the first 8 sample values of frame 0 plus
        # every global-LM sample as semantic_codes when the vocab is the LM.
        if logits.shape[-1] == pipe.language_model.config.vocab_size:
            taps["semantic_codes"].append(int(token.reshape(-1)[0].item()))
        elif logits.shape[-1] == pipe.audio_vocab_size:
            taps["rvq_codes"].append(int(token.reshape(-1)[0].item()))
        if sample_n["n"] == 1:
            save("first_sample_logits", logits)
        return token

    enc._sample_top_k = sample_top_k

    print("generate seconds=%s seed=%s steps=%s" % (args.seconds, args.seed, args.steps), flush=True)
    gen_t0 = time.time()
    generator = torch.Generator("cuda").manual_seed(int(args.seed))
    audio = pipe(
        prompt=args.prompt,
        lyrics=args.lyrics,
        audio_duration=float(args.seconds),
        num_inference_steps=int(args.steps),
        generator=generator,
        output="audios",
    )[0]
    gen_s = time.time() - gen_t0

    data = to_numpy(audio)
    if data.ndim == 3:
        data = data[0]
    if data.ndim == 1:
        data = data[None, :]
    if data.shape[0] > data.shape[1]:
        data = data.T
    save("audio", data)
    wav_path = os.path.join(out_dir, "song.wav")
    write_wav_i16(wav_path, data, int(pipe.sampling_rate))

    if taps["semantic_codes"]:
        save("semantic_codes", np.asarray(taps["semantic_codes"], dtype=np.int64))
    if taps["rvq_codes"]:
        codes = np.asarray(taps["rvq_codes"], dtype=np.int64)
        save("rvq_codes_flat", codes)
        n_cb = int(pipe.num_codebooks) - 1
        if n_cb > 0 and codes.size % n_cb == 0:
            save("rvq_codes", codes.reshape(-1, n_cb))

    meta = {
        "fixture": "pine_5s_seed7",
        "prompt": args.prompt,
        "lyrics": args.lyrics,
        "assembled_prompt": assembled,
        "seconds": float(args.seconds),
        "seed": int(args.seed),
        "num_inference_steps": int(args.steps),
        "sample_rate": int(pipe.sampling_rate),
        "frame_rate": float(pipe.frame_rate),
        "latent_hop_length": int(pipe.latent_hop_length),
        "num_codebooks": int(pipe.num_codebooks),
        "audio_vocab_size": int(pipe.audio_vocab_size),
        "num_channels_latents": int(pipe.num_channels_latents),
        "special_tokens": specials,
        "audio_end_token_id": int(_AUDIO_END_TOKEN_ID),
        "audio_cfg_token_id": int(_AUDIO_CFG_TOKEN_ID),
        "audio_code_offset": int(_AUDIO_CODE_OFFSET),
        "ar_cfg_scale": float(_AR_CFG_SCALE),
        "ar_cfg_top_k": int(_AR_CFG_TOP_K),
        "ar_sampling_top_k": int(_AR_SAMPLING_TOP_K),
        "lm_vocab_size": int(pipe.language_model.config.vocab_size),
        "lm_hidden_size": int(pipe.language_model.config.hidden_size),
        "lm_num_layers": int(pipe.language_model.config.num_hidden_layers),
        "counts": {
            "lm_calls": taps["lm_calls"],
            "dit_calls": taps["dit_calls"],
            "rvq_calls": taps["rvq_calls"],
            "cond_calls": taps["cond_calls"],
            "vocoder_calls": taps["vocoder_calls"],
            "semantic_codes": len(taps["semantic_codes"]),
            "rvq_codes": len(taps["rvq_codes"]),
        },
        "wall_s": {
            "total": time.time() - t0,
            "generate": gen_s,
        },
        "peak_vram_bytes": int(torch.cuda.max_memory_allocated()),
        "stats": stats,
        "wav": wav_path,
    }
    with open(os.path.join(out_dir, "meta.json"), "w", encoding="utf-8") as handle:
        json.dump(meta, handle, indent=2)
    print("done", json.dumps({k: meta[k] for k in ("counts", "wall_s", "peak_vram_bytes", "wav")}), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
