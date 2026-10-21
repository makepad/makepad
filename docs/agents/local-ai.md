# Local AI: run Makepad Code on a local model

Purpose: from a fresh checkout and no model, end with `makepad-code`
answering on a model running on this machine. Each step: command, what
success looks like, what to do if it fails. Run the steps in order.

## Pick a profile

| Machine | Model | Notes |
|---|---|---|
| Mac, 16 GB unified memory (M1 and later) | `qwen3.5-9b` | UNVERIFIED: not run on a 16 GB Mac; quant, context and memory use not measured |
| GPU with 24 GB+ VRAM (Linux/Windows, NVIDIA) | `qwen3.8-27b` | 17.1 GB Q4_K_M + 0.9 GB vision projector; 19.2 GB VRAM resident once loaded (32768 context, q8 KV). Verified end to end on an RTX PRO 6000 (Linux). Windows UNVERIFIED |
| No usable GPU | none | use a hub on the LAN: `MAKEPAD_AI_HUB=host:port makepad-code` |

The hub only serves models that are installed; install one model and that
is the one it loads. It never picks a model you did not install.

## 0. Prerequisites

- Rust via rustup (`rustup default stable`). Check: `cargo --version`.
- Disk: ~25 GB free for the 27B (17 GB of weights on disk, measured, plus about 1 GB of build output), the 9B is UNVERIFIED (not measured).
  Check: `df -h ~`.
- Linux/Windows with NVIDIA: the CUDA toolkit, and its root in
  `MAKEPAD_CUDA_ROOT` (for example `export MAKEPAD_CUDA_ROOT=/opt/cuda`).
  Without it the build succeeds but prints
  `makepad-ai-cuda: building WITHOUT CUDA — GPU inference is unavailable`.
  Add `MAKEPAD_GGML_REQUIRE_CUDA=1` to make that an error instead.
- macOS: nothing extra (Metal).

## 1. Build

```sh
cd makepad
cargo build --release -p makepad-app-ai-hub -p makepad-code
cargo build --release -p makepad-ai-hub --features local --bin hub-install
export PATH="$PWD/target/release:$PATH"
```

Success: `target/release/makepad-app-ai-hub`, `makepad-code` and
`hub-install` exist (about 2.5 min on a 32-core Linux box with the CUDA kernels; the 23 kernels build twice, about 1 min each).
If `makepad-ai-cuda: building WITHOUT CUDA` appears on an NVIDIA machine,
set `MAKEPAD_CUDA_ROOT` (step 0) and build again.

## 2. Install the model

```sh
hub-install qwen3.8-27b --accept-license      # or qwen3.5-9b on a 16 GB Mac
```

`--accept-license` accepts the model's licence (Apache-2.0 for both). Files go
to `~/.makepad/weights` (the one place every Makepad hub on the machine reads) (`--weights-dir DIR` to change). Success: a `verified <file>` line per file, then the installed paths
(`llm-gguf:` and `mmproj:`). The 18 GB download takes from a few minutes to
over an hour depending on the link; progress lines print as it goes. A failed or
interrupted download can be re-run; finished files are kept.

## 3. Start the hub

```sh
makepad-app-ai-hub --machine &
echo $! > ~/hub.pid
```

Success: output ends with `listening on http 127.0.0.1:8765` and
`machine node: registered ~/.makepad/run/<key>.node (ttl 15m idle)`.
The machine node binds loopback only. It loads the model on the first request,
not at start, and exits by itself after 15 idle minutes
(`MAKEPAD_AI_HUB_MACHINE_TTL_MIN` changes that).

If it prints `Cannot bind http server port`, another hub already holds 8765
on this machine: use that one, or start this one with `--port 8799`
(`makepad-code` finds it through the registered node entry).

## 4. Check the model

```sh
cargo build --release -p makepad-ai-hub --bin hub-control
hub-control http://127.0.0.1:8765 get
```

Before the first request the `qwen3.8-27b` entry in `models` says
`"state":"unloaded"` and `llm.model` is `null`. That is normal. After step 5
it says `"state":"loaded"` with `resident_mb` about 19168, and
`nvidia-smi` shows about 19.7 GB more used than before. `curl
http://127.0.0.1:8765/health` also lists `models_loaded`.
(Use the port the hub printed.)

## 5. Run Makepad Code

```sh
cd ~/my-project
makepad-code -p "list the files here and say what each one is"
```

Success: a `model: qwen3.8-27b on 127.0.0.1` line, tool lines (`+ ls .`), then
an answer. The first request pays the model load. Run `makepad-code` with no
`-p` for the interactive session.

If it prints `No local AI model is installed yet` right after step 2 passed,
the hub is not running (or not on the port in its node entry): the message is
misleading in that case. Start it (step 3). Makepad Code does not start the
hub itself. The `[hub-chat] no model:` line above the error names the address
it tried.

## 6. Stop the hub

```sh
kill $(cat ~/hub.pid)
```

VRAM goes back to what it was before step 5 within seconds. A stale
`~/.makepad/run/<key>.node` file stays behind and is harmless; the next hub
start replaces it. Kill by the recorded PID only, never by process name.
