# rwbench: real-world workloads for Relax's code quality

The same Rust source runs under `rustc --release` and under `makepad run`. Every workload is a headless
program over the real library crates (real call depth, generics, cross-crate inlining) on fixed
in-repo inputs; no UI, GPU or window. The micro-kernels live in `../bench_rt`.

| workload | what runs |
|---|---|
| `text_shape_raster` | rustybuzz shaping, ttf-parser outlines, ab_glyph_rasterizer coverage (the sequence `draw/src/text` uses) over `README.md` + `AGENTS.md` |
| `inflate_real` | makepad-fast-inflate on a font, source text, markdown |
| `zip_read` | makepad-zip-file reader over an archive built in setup from repo files |
| `png_decode` | makepad-zune-png (the platform's decoder) on three repo PNGs |
| `brotli_decode` | vendored brotli decoder on streams made in setup from repo files |
| `maps_tile_decode` | MbtilesReader (SQLite b-tree, brotli codec) + MVT protobuf/geometry decode of a generated 196-tile z14 archive (the repo carries no tile set) |
| `layout_widgets` | a 120-row widget tree (View/Label/Button/Chip) laid out and drawn into a draw list by the real `Cx2d` turtle code, 12 pass widths; headless `Cx`, nothing submitted to a GPU |
| `rust_tokenize` | makepad-rust-tokenizer over a large Rust source file |
| `markdown_parse` | pulldown-cmark over two in-repo markdown documents |
| `html_parse` | makepad-html over `libs/piano_model/piano-explained.html` |
| `llm_host` | the LLM host path of one CUDA decode token with no GPU and no weight data (`makepad-ai-llm::host_bench`, makepad commit `e3fc4c003a`): a scaled-down qwen35 GGUF (64 blocks, real 248,320 vocab) written in setup, mapped as a session maps it; per iteration one decode-graph build plus 256 tokens of layout validation, input writes, readback shaping of a 1 MB logits vector, collapse and greedy sampling. `RW_LLM_GGUF=<path>` runs a real GGUF. The CUDA graph planner (`plan_raw_graph`) is not covered: it compiles only with the CUDA kernels built |

Not here: an Amp program (there is no `apps/commercial/amp` in the tree), whisper. Build with `MAKEPAD_GGML_NO_CUDA=1` (the submit scripts set it) so the llm crate's CUDA store does not run nvcc.

## Runtime port status

The fixed `layout_widgets` scene now constructs View/Label/Button/Chip from the compiled typed
styles. Its row count, text, ordering, measured rows and pass widths are unchanged.
The parser workloads use the Rust tokenizer, markdown parser and HTML parser directly.
The audio score workload still uses the score document composer.

### makepad relax `a3b0786428`

`render_kernels` already uses native Rust functions at this revision; its old compute compiler
is gone. `makepad-ai-llm::host_bench::HostDecodeBench` is still exported. The benchmark does not
replace either library with benchmark-local implementations.

`layout_widgets` uses `Flow::right()` for the current data-carrying `Right` variant: top row
alignment and no row wrapping, matching the former `Flow::Right`. It retains all 120 rows,
the same two labels/button/chip per row, text, child order, row alignment, twelve widths and
rectangle checksum. No per-iteration work is removed. The historical table below predates
the typed widget migration and is not a baseline for this implementation; compare compiler
builds on the same makepad revision and fixed input tree.

Process failures or checksum mismatches make the runner exit nonzero.

## Output

`relax-rwbench <workload>...` prints per workload
`| name steady_ms=.. tts_main_ms=.. tts_proc_ms=.. iters=.. first_ms=.. <checksum>`.
`steady_ms` is the median of the last iterations after warm-up; `tts_*` is the time until the
per-iteration time is within 10% of steady (3-iteration running median): `tts_main` from entering
`main`, `tts_proc` from process start (`RW_WALL0=<epoch ns>`, set by `run.py`, so it includes Relax's
compile). Setup (reading inputs, building archives) counts towards time-to-steady. Env: `RW_MS`
(iteration budget per process, default 3000), `RW_MIN_ITERS` (20), `RW_ROOT` (tree holding the inputs).

## Run

```
python3 run.py <target dir> <name=makepad binary>...
```
builds the rustc side (or uses `NATIVE=<binary>`) and runs every workload (`WORKLOADS="a b"` picks some) under rustc and
under each Relax binary, `ROUNDS` (default 4) interleaved rounds: each round runs every workload once, rustc first, then
the binaries in alternating order (base, new / new, base / ...). Per name it prints, per workload, the median over rounds
of the steady ms with the [min..max] spread, the Relax/rustc ratio (median of the per-round ratios, so drift hits both
sides) with its spread, time-to-steady, O3 coverage and whether all checksums agree. With several names a final A/B
table gives new/base per workload (median of per-round ratios, spread) and the geomean. The first name is the base; the
same binary under two names measures the noise floor. `RW_MS` (4000) is the per-process iteration budget, `MAKEPAD_RUN_STD`
defaults to `../std`, `RUN_PREFIX` defaults to `nice -n 10` (`none` on Windows). Timing belongs on the quiet machines
through the bench queue:

* x64 (`win` box): `local/agent_state/fast-rust/codegen2/rwtime_submit.sh <tag> <name=fastrust-commercial>...`
  (names sharing a key share one Relax build; `BENCH_KEY=<key>` takes the rwbench tree from another commit)
* arm64 (mini): `.../codegen2/rwtime_a64_submit.sh <name=fastrust-commercial>...`

Both pack `src_tar.py` trees plus the commercial `engine/` and `stage/libs/` dirs, build Relax and rwbench on the box and
print these tables. The crate resolves offline from `vendor/` (its own `[patch.crates-io]` mirrors the root manifest's).

## Known results

Node 165 (x86-64 Linux, shared, NOT a quiet machine; frozen `relax-t19`):

| workload | rustc steady ms | Relax steady ms | Relax/rustc | rustc time-to-steady ms | Relax time-to-steady ms | Relax first iter ms | Relax O3 coverage | checksums equal |
|---|---:|---:|---:|---:|---:|---:|---|---|
| text_shape_raster | 8.035 | 9.623 | 1.20 | 36 | 2750 | 278.7 | O3: 300 hot, 300 installed | yes |
| inflate_real | 4.728 | 7.115 | 1.50 | 79 | 1426 | 27.3 | O3: 26 hot, 26 installed | yes |
| zip_read | 5.074 | 7.378 | 1.45 | 102 | 1431 | 26.5 | O3: 62 hot, 62 installed | yes |
| png_decode | 11.996 | 64.425 | 5.37 | 5 | 1432 | 129.4 | O3: 57 hot, 57 installed | yes |
| brotli_decode | 7.097 | 11.865 | 1.67 | 482 | 2526 | 74.9 | O3: 113 hot, 114 installed | yes |
| maps_tile_decode | 19.523 | 19.173 | 0.98 | 1650 | 1846 | 141.4 | O3: 301 hot, 302 installed | yes |
| layout_widgets | 29.792 | 85.201 | 2.86 | 40 | 4303 | 868.3 | O3: 1104 hot, 116 installed | yes |

Quiet-machine results (x64 win box, arm64 mini) are posted by the bench queue under local/agent_state/fast-rust/bench/results/.
