#!/usr/bin/env python
"""motion_hymotion.py <in.glb> <out.glb> — the motion domain's box script.

Rigged GLB + params sidecar in, animated GLB with NAMED in-place clips out.
Two proven stages from the motion campaign (local/agent_state/motion-123.md):

1. HY-Motion text-to-motion (C:\\ai\\HY-Motion-1.0, THIS interpreter =
   venv_hymotion): one prompt line per requested clip ("text#frames#id" in
   a txt file under --input_text_dir), local_infer.py --disable_rewrite
   --disable_duration_est --num_seeds 1 -> per-task SMPL-H NPZ
   (0000000N_000.npz, N = 1-based prompt-line order; Rh/trans/poses156).
   Stock local_infer.py exposes no seed argument.  This wrapper seeds its
   imported `random` module before runpy executes the stock entry point, so
   the sidecar seed deterministically controls its generated seed list.
2. Direction-based retarget (venv_unirig python + bpy, retarget_multi.py —
   the campaign's retarget.py extended with the --in-place strip): NEVER
   the global-delta transfer (double-applies the rest pose). Writes the
   final GLB with one named NLA-track animation per clip.

Params sidecar <in.glb>.json (motion_backend.rs MotionParamsJson):
  {"prompt": style hint, "clips": ["idle","walk","jump"], "seed": N,
   "fps": 30, "in_place": true}

Env knobs:
  HYMOTION_DIR      repo dir (default C:\\ai\\HY-Motion-1.0)
  HYMOTION_MODEL    ckpt dir under the repo (default ckpts/tencent/
                    HY-Motion-1.0 — the FULL 1B model, per user directive:
                    never downsize for co-residency; motion is hosted on the
                    96GB box)
  UNIRIG_PYTHON     bpy-capable python (default C:\\ai\\venv_unirig\\
                    Scripts\\python.exe)
  RETARGET_SCRIPT   retarget entry (default C:\\ai\\retarget_multi.py)
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile


def progress(frac, stage):
    # bpy-as-module can leave the console stdout handle invalid on Windows
    # after its subprocess runs (observed: errno 22 on the FINAL print, after
    # a fully successful export). Progress is advisory — never let it turn a
    # produced artifact into a failed job.
    try:
        print("@P %.3f %s" % (frac, stage), flush=True)
    except OSError:
        pass


# Per-clip motion prompts and frame budgets at 30fps.  HY-Motion's official
# prompting contract says to describe limb/torso ACTION and explicitly does
# not support visual/subject attributes.  Feeding the image prompt (clothes,
# colors, species) into every clip made locomotion needlessly prompt-fragile;
# appearance belongs to TRELLIS/SkinTokens and motion retargets afterwards.
CLIP_RECIPES = {
    "idle": ("A person stands in a relaxed neutral idle pose, with subtle breathing, feet apart and arms resting at their sides", 120),
    "walk": ("A person walks forward naturally at a steady pace, upright, with alternating arm swing and feet kept apart", 120),
    "jump": ("A person bends both knees, jumps straight up once, and lands balanced on both feet", 100),
    "run": ("A person runs forward naturally with alternating arm swing", 120),
}


def run_streamed(cmd, cwd, tag, frac_lo, frac_hi):
    """Run a child, stream output, walk the fraction lo->hi on output."""
    print("%s: run %r (cwd %s)" % (tag, cmd, cwd), flush=True)
    child = subprocess.Popen(
        cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, bufsize=1,
    )
    n = 0
    for line in child.stdout:
        n += 1
        if n % 20 == 0:
            frac = min(frac_hi, frac_lo + (frac_hi - frac_lo) * n / 400.0)
            progress(frac, "%s: working" % tag)
        print("%s| %s" % (tag, line.rstrip("\r\n")), flush=True)
    return child.wait()


def main():
    if len(sys.argv) != 3:
        print("usage: motion_hymotion.py <in.glb> <out.glb>", flush=True)
        return 2
    in_glb = os.path.abspath(sys.argv[1])
    out_glb = os.path.abspath(sys.argv[2])
    repo = os.environ.get("HYMOTION_DIR", r"C:\ai\HY-Motion-1.0")
    model = os.environ.get("HYMOTION_MODEL", "ckpts/tencent/HY-Motion-1.0")
    unirig_py = os.environ.get(
        "UNIRIG_PYTHON", r"C:\ai\venv_unirig\Scripts\python.exe"
    )
    retarget = os.environ.get("RETARGET_SCRIPT", r"C:\ai\retarget_multi.py")

    params = {}
    try:
        with open(in_glb + ".json", "r", encoding="utf-8") as f:
            params = json.load(f)
    except OSError:
        pass
    hint = (params.get("prompt") or "a person").strip() or "a person"
    clips = params.get("clips") or ["idle", "walk", "jump"]
    seed = int(params.get("seed") or 42)
    in_place = bool(params.get("in_place", True))
    print("motion: hint %r clips %r seed %d in_place %r"
          % (hint, clips, seed, in_place), flush=True)

    work = tempfile.mkdtemp(prefix="motion_", dir=os.path.dirname(in_glb))
    try:
        # ---- stage 1: HY-Motion clips -----------------------------------
        # One txt file (line = "prompt#frames#id") in its own directory:
        # local_infer.py scans --input_text_dir for txt/json files.
        prompt_dir = os.path.join(work, "prompts")
        os.makedirs(prompt_dir, exist_ok=True)
        with open(os.path.join(prompt_dir, "job.txt"), "w", encoding="utf-8") as f:
            for i, clip in enumerate(clips):
                recipe, frames = CLIP_RECIPES.get(
                    clip, ("A person performs this action: " + clip, 120)
                )
                f.write("%s#%d#%d\n" % (recipe, frames, i))
        out_dir = os.path.join(work, "npz")
        os.makedirs(out_dir, exist_ok=True)
        progress(0.05, "hy-motion: loading (text encoder + dit)")
        code = run_streamed(
            [
                # local_infer.generate_random_seeds uses Python's module-level
                # random generator.  Seed it in-process, then execute the
                # unmodified official CLI with its normal argv contract.
                sys.executable, "-c",
                "import random,runpy; random.seed(%d); "
                "runpy.run_path('local_infer.py', run_name='__main__')" % seed,
                "--model_path", model,
                "--input_text_dir", prompt_dir,
                "--output_dir", out_dir,
                "--disable_rewrite", "--disable_duration_est",
                "--num_seeds", "1",
            ],
            repo, "hym", 0.05, 0.55,
        )
        if code != 0:
            print("motion: local_infer.py exit %d" % code, flush=True)
            return 1
        # Collect one NPZ per clip, in prompt order (outputs are
        # 0000000N_000.npz under the output dir, possibly nested).
        npzs = []
        for root, _dirs, files in os.walk(out_dir):
            for name in sorted(files):
                if name.endswith(".npz"):
                    npzs.append(os.path.join(root, name))
        npzs.sort()
        if len(npzs) < len(clips):
            print("motion: expected %d npz, found %d (%r)"
                  % (len(clips), len(npzs), npzs), flush=True)
            return 1
        progress(0.60, "retarget: %d clips onto rig" % len(clips))

        # ---- stage 2: retarget onto the input rig -----------------------
        # retarget_multi.py CLI = the campaign retarget.py's ("clip=npz"
        # positional pairs) + the --in-place flag.
        cmd = [unirig_py, retarget, in_glb, out_glb]
        for clip, npz in zip(clips, npzs):
            cmd.append("%s=%s" % (clip, npz))
        if in_place:
            cmd.append("--in-place")
        code = run_streamed(cmd, os.path.dirname(retarget) or ".",
                            "retarget", 0.60, 0.92)
        if code != 0:
            # bpy-as-module is known to crash with 0xC0000005 during
            # interpreter TEARDOWN, after a fully successful export
            # (observed on the box the first standalone run). The output
            # contract below is the real gate — log and fall through.
            print("motion: retarget exit %d (checking output anyway — "
                  "bpy teardown crashes are benign)" % code, flush=True)

        # Output contract: a GLB with skins AND animations.
        try:
            with open(out_glb, "rb") as f:
                head = f.read(96 * 1024 * 1024)
        except OSError as e:
            print("motion: output missing (retarget exit %d): %s"
                  % (code, e), flush=True)
            return 1
        if head[:4] != b"glTF" or b'"skins"' not in head \
                or b'"animations"' not in head:
            print("motion: output is not an animated rigged GLB", flush=True)
            return 1
        progress(0.98, "motion: done")
        return 0
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
