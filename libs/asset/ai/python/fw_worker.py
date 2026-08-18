# fw_worker.py - persistent FlashWorld job worker for the makepad-ai-content
# `world` domain backend (world_backend.rs). Line protocol:
#   stdin : one JSON object per line: {"prompt": str, "image_path": str|absent,
#           "seed": int, "out_dir": str} or {"exit": true}
#   stdout: events prefixed "@EV " (everything else is ignored by the parent):
#           {"ev":"stage","stage":name[,"k":i,"n":total]}
#           {"ev":"ready"}                       after model load
#           {"ev":"done","ply":path}             job finished
#           {"ev":"error","message":text}        load/job failed (worker lives on
#                                                after job errors)
# Runs with cwd = the FlashWorld repo clone so `cli`/`app` import; the repo's
# app.py is patched box-side to emit @EV load-stage events from
# GenerationSystem.__init__ (patch_fw2.py).
import argparse
import copy
import functools
import json
import os
import sys


def ev(**kw):
    sys.stdout.write("@EV " + json.dumps(kw) + "\n")
    sys.stdout.flush()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--ckpt", required=True)
    parser.add_argument("--cameras", required=True)
    args = parser.parse_args()

    ev(ev="stage", stage="boot")
    sys.path.insert(0, os.getcwd())

    with open(args.cameras, "r") as f:
        preset = json.load(f)
    n_frame, image_height, image_width = preset["resolution"]

    ev(ev="stage", stage="load-libs")
    import torch  # noqa: E402
    from PIL import Image  # noqa: E402

    # cli imports app (GenerationSystem) and gsplat; module level only defines.
    from cli import process_generation_request  # noqa: E402
    from app import GenerationSystem  # noqa: E402

    try:
        system = GenerationSystem(ckpt_path=args.ckpt, device=torch.device("cuda"))
    except Exception as e:  # noqa: BLE001
        import traceback

        traceback.print_exc()
        ev(ev="error", message="load failed: %s" % str(e)[:400])
        return 1

    # Denoise progress: the DiT runs exactly len(denoising_steps) forwards per
    # job (3 feedback steps + 1 final). functools.wraps keeps the signature
    # visible to any inspect-based callers (the H3 lesson).
    n_steps = len(system.denoising_steps)
    state = {"k": 0}
    orig_forward = system.transformer.forward

    @functools.wraps(orig_forward)
    def counted_forward(*a, **kw):
        state["k"] += 1
        k = min(state["k"], n_steps)
        ev(ev="stage", stage="denoise %d/%d" % (k, n_steps), k=k, n=n_steps)
        return orig_forward(*a, **kw)

    system.transformer.forward = counted_forward

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
        state["k"] = 0
        try:
            out_dir = job["out_dir"]
            os.makedirs(out_dir, exist_ok=True)

            image_path = job.get("image_path")
            if image_path:
                # Normalize to the preset's exact canvas so the embedded
                # camera intrinsics pass through cli.py's crop/rescale
                # unchanged (scale == 1 path).
                img = Image.open(image_path).convert("RGB")
                w, h = img.size
                target_aspect = image_width / image_height
                if w / h > target_aspect:
                    new_w = int(round(h * target_aspect))
                    x0 = (w - new_w) // 2
                    img = img.crop((x0, 0, x0 + new_w, h))
                else:
                    new_h = int(round(w / target_aspect))
                    y0 = (h - new_h) // 2
                    img = img.crop((0, y0, w, y0 + new_h))
                img = img.resize((image_width, image_height), Image.LANCZOS)
                norm_path = os.path.join(out_dir, "input_704x480.png")
                img.save(norm_path)
                image_path = norm_path

            data = {
                "text_prompt": job.get("prompt", ""),
                "resolution": preset["resolution"],
                "image_index": preset["image_index"],
                # process_generation_request mutates camera intrinsics in
                # place when an image is supplied - deep-copy per job.
                "cameras": copy.deepcopy(preset["cameras"]),
            }
            if image_path:
                data["image_prompt"] = image_path

            torch.manual_seed(int(job.get("seed", 0)))
            ev(ev="stage", stage="generate")
            result = process_generation_request(
                data, system, out_dir, video=False, spz=False, ply=True
            )
            if isinstance(result, dict) and result.get("error"):
                ev(ev="error", message=str(result["error"])[:400])
                continue
            ev(ev="stage", stage="export")
            ply = os.path.join(out_dir, "gaussians.ply")
            if not os.path.isfile(ply):
                ev(ev="error", message="no gaussians.ply produced")
                continue
            ev(ev="done", ply=ply)
        except Exception as e:  # noqa: BLE001
            import traceback

            traceback.print_exc()
            ev(ev="error", message=str(e)[:400])
    return 0


if __name__ == "__main__":
    sys.exit(main())
