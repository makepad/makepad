# yue2_oracle_dump.py - reference-stage dumps for the native YuE2 port.
#
# Dev tool only (like music3_oracle_dump.py): it runs the official yue2
# package once, in an isolated venv on the CUDA dev node, and writes raw
# little-endian tensors + meta.json that the native port compares against
# (`yue2_render --check <dir>`). Nothing in the product imports or runs it.
#
# Usage (node 165):
#   ~/ref/yue2-venv/bin/python yue2_oracle_dump.py \
#     --model-dir ~/.makepad/weights/music/YuE2-3B \
#     --vae-dir ~/.makepad/weights/music/YuE2-Vae \
#     --fixture ~/.makepad/weights/music/YuE2-3B/examples/tonight-awake.json \
#     --out-dir ~/ref/yue2_oracle/tonight-awake
#
# Files (all little-endian, shapes in meta.json):
#   prefix_abc.i32          ABC-stage prefix ids (tokenizer check)
#   tok_<name>.i32          tokenizer fixtures (style, lyrics, abc text)
#   abc_prefill_logits.f32  [vocab] last-position logits of the ABC prefix
#   greedy_abc.i32          temperature-0 ABC ids (eager, no CUDA graph)
#   abc.i32 / abc.txt       sampled ABC ids + decoded score (release path)
#   semantic.i32            sampled codec ids (0..32768, offset removed)
#   tf_abc_logits.f32       [n, 151649] teacher-forced logits, ABC phase rows 0..ABC_END
#   tf_sem_logits.f32       [n, 32769] teacher-forced logits, rows MUSIC_END..codec end
#   nar_noise.f32 / nar_v0.f32 / nar_vmid.f32   chunk-0 noise, velocity at t=1, at the first midpoint
#   latents.f32             [frames, 64] final ODE state (release path)
#   audio.f32 / audio.wav   [samples, 2] VAE decode of latents.f32
import argparse
import json
import os
import struct
import sys
import time
import wave

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")

TF_ABC_ROWS = 64
TF_SEM_ROWS = 256


def save(out_dir, name, array, meta):
    import numpy as np

    array = np.ascontiguousarray(array)
    if array.dtype.kind == "f":
        array = array.astype("<f4")
    else:
        array = array.astype("<i4")
    array.tofile(os.path.join(out_dir, name))
    meta["files"][name] = {"shape": list(array.shape), "dtype": str(array.dtype)}


def write_wav_i16(path, audio, sample_rate):
    import numpy as np

    pcm = (np.clip(audio, -1.0, 1.0) * 32767.0).astype("<i2")
    with wave.open(path, "wb") as handle:
        handle.setnchannels(pcm.shape[1])
        handle.setsampwidth(2)
        handle.setframerate(sample_rate)
        handle.writeframes(pcm.tobytes())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--vae-dir", required=True)
    parser.add_argument("--fixture", required=True, help="json with style, lyrics, seed, cot")
    parser.add_argument("--out-dir", required=True)
    parser.add_argument("--max-semantic", type=int, default=None,
                        help="cap semantic tokens (short oracle); default = release 9000")
    args = parser.parse_args()

    import numpy as np
    import torch
    from yue2 import YuE2Pipeline
    from yue2.protocol import (ABC_END, CODEC_OFFSET, CODEC_SIZE, EOD, MUSIC_END,
                               MUSIC_START, Sampling, token_prefixes)
    from yue2.sampling import generate_tokens
    from yue2.nar import CachedNAR, song_chunks
    from yue2.pipeline import SemanticResult

    os.makedirs(args.out_dir, exist_ok=True)
    fixture = json.load(open(args.fixture, encoding="utf-8"))
    meta = {"fixture": fixture, "files": {}, "timing": {},
            "torch": torch.__version__, "cuda": torch.version.cuda,
            "device": torch.cuda.get_device_name(0)}

    pipe = YuE2Pipeline(args.model_dir, args.vae_dir, device="cuda", progress=False)
    tok = pipe.tokenizer
    request = pipe._request(fixture["style"], fixture["lyrics"],
                            cot=fixture.get("cot", "full"), seed=int(fixture["seed"]))

    # Tokenizer fixtures.
    prefix = token_prefixes(request, tok)
    save(args.out_dir, "prefix_abc.i32", np.array(prefix), meta)
    save(args.out_dir, "tok_style.i32", np.array(tok.encode(fixture["style"])), meta)
    save(args.out_dir, "tok_lyrics.i32", np.array(tok.encode(fixture["lyrics"])), meta)

    model = pipe._load_model()

    # ABC prefill logits (eager forward, last position).
    with torch.inference_mode():
        out = model(torch.tensor([prefix], device="cuda"), use_cache=False, logits_to_keep=1)
        save(args.out_dir, "abc_prefill_logits.f32", out.logits[0, -1].float().cpu().numpy(), meta)

    # Greedy ABC (eager, no graph): exact-token gate independent of RNG.
    greedy = Sampling(0.0, 1.0, 1, 1.0, 1, 0, 128)
    ids, _, _ = generate_tokens(model, prefix, greedy, request.seed, "abc", use_cuda_graph=False)
    save(args.out_dir, "greedy_abc.i32", np.array(ids), meta)

    # Release path: plan + semantic with the shipped sampling (CUDA graph).
    torch.cuda.synchronize()
    t0 = time.perf_counter()
    plan = pipe.plan(request=request)
    torch.cuda.synchronize()
    meta["timing"]["abc_seconds"] = time.perf_counter() - t0
    meta["timing"]["abc"] = plan.timing
    save(args.out_dir, "abc.i32", np.array(plan.abc_ids), meta)
    with open(os.path.join(args.out_dir, "abc.txt"), "w", encoding="utf-8") as handle:
        handle.write(plan.abc or "")
    save(args.out_dir, "tok_abc.i32", np.array(tok.encode(plan.abc or "")), meta)
    save(args.out_dir, "prefix_sem.i32", np.array(plan.prefix), meta)

    sampling = None
    if args.max_semantic:
        from dataclasses import replace
        sampling = replace(pipe.generation_config.semantic, max_tokens=args.max_semantic,
                           min_tokens=min(200, args.max_semantic))
    t0 = time.perf_counter()
    semantic = pipe.generate_semantic(plan, sampling=sampling)
    torch.cuda.synchronize()
    meta["timing"]["semantic_seconds"] = time.perf_counter() - t0
    meta["timing"]["semantic"] = semantic.timing
    meta["semantic_truncated"] = semantic.truncated
    save(args.out_dir, "semantic.i32", np.array(semantic.tokens), meta)

    # Teacher-forced logits over the sampled sequence (eager, one forward).
    with torch.inference_mode():
        seq = list(prefix) + list(plan.abc_ids[:TF_ABC_ROWS])
        logits = model(torch.tensor([seq], device="cuda"), use_cache=False).logits[0]
        rows = logits[len(prefix) - 1: len(prefix) - 1 + min(TF_ABC_ROWS, len(plan.abc_ids)), : ABC_END + 1]
        save(args.out_dir, "tf_abc_logits.f32", rows.float().cpu().numpy(), meta)
        sem_ids = [t + CODEC_OFFSET for t in semantic.tokens[:TF_SEM_ROWS]]
        seq = list(plan.prefix) + sem_ids
        logits = model(torch.tensor([seq], device="cuda"), use_cache=False).logits[0]
        start = len(plan.prefix) - 1
        rows = logits[start: start + len(sem_ids), MUSIC_END: CODEC_OFFSET + CODEC_SIZE]
        save(args.out_dir, "tf_sem_logits.f32", rows.float().cpu().numpy(), meta)
        del logits, rows

    # NAR probes on chunk 0, then the release synthesis.
    chunks = song_chunks(plan.prefix, semantic.tokens, request.seed)
    meta["nar_chunks"] = [[len(c.ar_tokens), int(c.noise.shape[0])] for c in chunks]
    engine = CachedNAR(model, chunks[0])
    with torch.inference_mode():
        state = chunks[0].noise.to(device="cuda", dtype=torch.bfloat16)
        steps = pipe.generation_config.ode_steps
        dt = 1.0 / steps
        raw = torch.logit(torch.tensor(1.0, dtype=torch.float64)).clamp(-20, 20).item()
        v0 = engine.velocity(state, raw)
        mid = state - v0 * (dt / 2)
        raw_mid = torch.logit(torch.tensor(1.0 - dt / 2, dtype=torch.float64)).clamp(-20, 20).item()
        vmid = engine.velocity(mid, raw_mid)
        save(args.out_dir, "nar_noise.f32", chunks[0].noise.numpy(), meta)
        save(args.out_dir, "nar_v0.f32", v0.float().cpu().numpy(), meta)
        save(args.out_dir, "nar_vmid.f32", vmid.float().cpu().numpy(), meta)
        meta["nar_raw_t"] = [raw, raw_mid]
    engine.close()
    del engine

    t0 = time.perf_counter()
    latents = pipe.synthesize(semantic)
    torch.cuda.synchronize()
    meta["timing"]["nar_seconds"] = time.perf_counter() - t0
    save(args.out_dir, "latents.f32", latents, meta)

    t0 = time.perf_counter()
    audio = pipe.decode(latents)
    meta["timing"]["vae_seconds"] = time.perf_counter() - t0
    save(args.out_dir, "audio.f32", audio, meta)
    write_wav_i16(os.path.join(args.out_dir, "audio.wav"), audio, 48000)
    meta["audio_seconds"] = audio.shape[0] / 48000.0
    meta["peak_vram_gib"] = torch.cuda.max_memory_allocated() / 2**30

    with open(os.path.join(args.out_dir, "meta.json"), "w", encoding="utf-8") as handle:
        json.dump(meta, handle, indent=1, ensure_ascii=False)
    print("ORACLE-DONE", args.out_dir, json.dumps(meta["timing"], default=str)[:400])


if __name__ == "__main__":
    sys.exit(main())
