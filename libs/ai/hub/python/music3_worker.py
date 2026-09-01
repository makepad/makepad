# music3_worker.py - persistent MiniMax-Music3 job worker for the
# makepad-ai-content `music` domain backend (music3_backend.rs). Line protocol
# (same shape as fw_worker.py):
#   stdin : one JSON object per line: {"prompt": str, "lyrics": str,
#           "duration_s": float, "seed": int, "out_wav": str} or {"exit": true}
#   stdout: events prefixed "@EV " (everything else is ignored by the parent):
#           {"ev":"stage","stage":name[,"k":i,"n":total]}
#           {"ev":"ready"}                       after model load
#           {"ev":"done","wav":path}             job finished
#           {"ev":"error","message":text}        load/job failed (worker lives
#                                                on after job errors)
#
# Runtime: the official diffusers ModularPipeline integration
# (MiniMaxAI/MiniMax-Music3 modular_model_index.json; diffusers PR #14456,
# pinned venv commit dafe3733fcfdbf3c48915fe77be3aef65b5d6a2d per the model
# card). Weights come from the service cache (registry-managed download), so
# the worker runs fully hub-offline against --model-dir.
#
# The recorded component specs in modular_model_index.json point at the hub
# repo id ("MiniMaxAI/MiniMax-Music3"); to guarantee offline loads we build a
# hardlinked view of the model dir with those specs rewritten to the local
# view path, then from_pretrained() the view.
import argparse
import json
import os
import shutil
import sys

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")


def ev(**kw):
    sys.stdout.write("@EV " + json.dumps(kw) + "\n")
    sys.stdout.flush()


def build_local_view(model_dir, view_dir):
    """Hardlink (fallback copy) the cached model into view_dir, rewriting the
    modular_model_index.json component sources to the view path so every
    component loads from local disk regardless of how diffusers resolves the
    recorded hub repo id."""
    for root, _dirs, files in os.walk(model_dir):
        rel = os.path.relpath(root, model_dir)
        dst_root = os.path.join(view_dir, rel) if rel != "." else view_dir
        os.makedirs(dst_root, exist_ok=True)
        for name in files:
            src = os.path.join(root, name)
            dst = os.path.join(dst_root, name)
            if os.path.exists(dst):
                if os.path.getsize(dst) == os.path.getsize(src):
                    continue
                os.remove(dst)
            if name == "modular_model_index.json":
                with open(src, "r", encoding="utf-8") as f:
                    index = json.load(f)
                for value in index.values():
                    if isinstance(value, list) and len(value) == 3 and isinstance(value[2], dict):
                        if "pretrained_model_name_or_path" in value[2]:
                            value[2]["pretrained_model_name_or_path"] = view_dir
                with open(dst, "w", encoding="utf-8") as f:
                    json.dump(index, f, indent=1)
                continue
            try:
                os.link(src, dst)
            except OSError:
                shutil.copyfile(src, dst)


def to_audio_array(audio):
    """Normalize the pipeline output (torch tensor OR numpy array, any of
    (T,), (ch, T), (T, ch), (batch, ch, T)) to float32 (channels, samples)."""
    try:
        import torch

        if isinstance(audio, torch.Tensor):
            audio = audio.detach().float().cpu().numpy()
    except ImportError:
        pass
    import numpy as np

    data = np.asarray(audio, dtype=np.float32)
    if data.ndim == 3:
        data = data[0]
    if data.ndim == 1:
        data = data[None, :]
    # Channels are the small axis (stereo); samples the long one.
    if data.shape[0] > data.shape[1]:
        data = data.T
    return data


def write_wav_i16(path, data, sample_rate):
    """data: float32 array shaped (channels, samples) in [-1, 1]."""
    import wave

    import numpy as np

    pcm = (np.clip(data.T, -1.0, 1.0) * 32767.0).astype("<i2")
    with wave.open(path, "wb") as w:
        w.setnchannels(pcm.shape[1])
        w.setsampwidth(2)
        w.setframerate(int(sample_rate))
        w.writeframes(pcm.tobytes())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--view-dir", required=True)
    args = parser.parse_args()

    ev(ev="stage", stage="boot")
    ev(ev="stage", stage="build-view")
    build_local_view(args.model_dir, args.view_dir)

    ev(ev="stage", stage="load-libs")
    import torch  # noqa: E402
    from diffusers import ModularPipeline  # noqa: E402

    try:
        ev(ev="stage", stage="load-components")
        offload = os.environ.get("MAKEPAD_MUSIC3_OFFLOAD") == "1"
        if offload:
            # The model card's low-VRAM path: auto CPU offload keeps peak
            # under ~22 GB at the cost of per-job PCIe restreaming.
            from diffusers import ComponentsManager

            manager = ComponentsManager()
            manager.enable_auto_cpu_offload(device="cuda")
            pipe = ModularPipeline.from_pretrained(
                args.view_dir, components_manager=manager
            )
            pipe.load_components(dtype=torch.bfloat16)
        else:
            pipe = ModularPipeline.from_pretrained(args.view_dir)
            pipe.load_components(dtype=torch.bfloat16)
            ev(ev="stage", stage="to-gpu")
            pipe.to("cuda")
    except Exception as e:  # noqa: BLE001
        import traceback

        traceback.print_exc()
        ev(ev="error", message="load failed: %s" % str(e)[:400])
        return 1

    # Progress: the long pole is the 8B global LM generating audio frames at
    # 25 frames/s of song. MiniMaxMusic3SemanticGenerationStep deliberately
    # calls `language_model.model(...)`, not the outer Qwen3ForCausalLM
    # wrapper, so the hook MUST sit on that inner model. Hooking the wrapper
    # looks plausible but produces no events and leaves clients parked at the
    # initial "generate" fraction for most of a song.
    #
    # The flow tail invokes the transformer twice per scheduler step (CFG),
    # and the vocoder once per audio chunk. Their totals are derived lazily
    # from the number of semantic forwards actually completed, so early EOS
    # songs still report useful, bounded work progress. These are completed
    # model forwards, not a wall-clock animation.
    state = {
        "lm": 0,
        "lm_n": 0,
        "dit": 0,
        "dit_n": 0,
        "vocoder": 0,
        "vocoder_n": 0,
    }

    def chunk_count_from_semantic_forwards():
        # One inner-model call is prompt prefill. With a full-length result the
        # remaining calls equal the emitted frames; early EOS can make this
        # estimate one frame high, which does not affect a chunk except exactly
        # on a 100-frame boundary and is safely clamped by the Rust consumer.
        frames = max(1, min(state["lm_n"], state["lm"] - 1))
        return 1 if frames <= 200 else len(range(0, frames - 100, 100))

    def hook_forward(module, stage_name, every, n_key=None):
        orig = module.forward

        def counted(*a, **kw):
            result = orig(*a, **kw)
            key = stage_name
            state[key] += 1
            if key == "dit" and state["dit_n"] == 0:
                chunks = chunk_count_from_semantic_forwards()
                # Official pipeline default: 30 flow steps, two CFG forwards.
                state["dit_n"] = chunks * 30 * 2
                state["vocoder_n"] = chunks
            if state[key] % every == 0:
                n = state.get(n_key, 0) if n_key else 0
                ev(
                    ev="stage",
                    stage="%s %d/%d" % (key, state[key], n) if n else key,
                    k=state[key],
                    n=n if n else None,
                )
            return result

        module.forward = counted
        return orig

    try:
        lm = getattr(pipe, "language_model", None)
        if lm is not None:
            # The pinned official semantic block bypasses Qwen3ForCausalLM's
            # forward and calls its `.model` core directly.
            hook_forward(getattr(lm, "model", lm), "lm", 5, n_key="lm_n")
        dit = getattr(pipe, "transformer", None)
        if dit is not None:
            hook_forward(dit, "dit", 5, n_key="dit_n")
        vocoder = getattr(pipe, "vocoder", None)
        if vocoder is not None:
            hook_forward(vocoder, "vocoder", 1, n_key="vocoder_n")
    except Exception:  # noqa: BLE001
        pass

    ev(ev="ready")

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            job = json.loads(line)
        except ValueError:
            ev(ev="error", message="bad job line")
            continue
        if job.get("exit"):
            break
        state["lm"] = 0
        state["dit"] = 0
        state["dit_n"] = 0
        state["vocoder"] = 0
        state["vocoder_n"] = 0
        try:
            duration = float(job.get("duration_s", 60.0))
            prompt = str(job.get("prompt", "")).strip()
            lyrics = str(job.get("lyrics", "")).strip() or "[Instrumental]"
            if not prompt:
                raise ValueError("music description must not be empty")
            # The released checkpoint is 25 Hz, but use the loaded component's
            # authoritative frame rate so progress survives compatible model
            # revisions without changing the wire protocol.
            state["lm_n"] = max(1, int(duration * float(pipe.frame_rate)))
            out_wav = job["out_wav"]
            os.makedirs(os.path.dirname(out_wav), exist_ok=True)

            ev(ev="stage", stage="generate")
            generator = torch.Generator("cuda").manual_seed(int(job.get("seed", 0)))
            audio = pipe(
                prompt=prompt,
                lyrics=lyrics,
                audio_duration=duration,
                generator=generator,
                output="audios",
            )[0]

            data = to_audio_array(audio)
            # Vocoder config pins 44100; pipe.sampling_rate wins when present.
            sample_rate = int(getattr(pipe, "sampling_rate", 44100) or 44100)
            ev(
                ev="stage",
                stage="write sr=%d ch=%d samples=%d"
                % (sample_rate, data.shape[0], data.shape[1]),
            )
            write_wav_i16(out_wav, data, sample_rate)
            if not os.path.isfile(out_wav):
                ev(ev="error", message="no wav produced")
                continue
            ev(ev="done", wav=out_wav)
        except Exception as e:  # noqa: BLE001
            import traceback

            traceback.print_exc()
            ev(ev="error", message=str(e)[:400])
    return 0


if __name__ == "__main__":
    sys.exit(main())
