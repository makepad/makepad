# makepad-box3d

A pure-Rust port of [Box3D](https://github.com/erincatto/box3d) by Erin Catto
(MIT). No external crates, std only.

## Ported revision

| | |
|---|---|
| Upstream | https://github.com/erincatto/box3d |
| Commit | `29bf523ce7bc4590aba9f17c9db791cdc5c4397e` |
| Tag/describe | `v0.1.0-2-g29bf523` ("Fixes 02 (#31)", 2026-07-02) |
| Local checkout | `<repo>/box3d/` (vendored at the same commit) |
| Ported | 2026-07-03/04 |

## How to sync with upstream

The port is a mechanical 1:1 translation: **one Rust module per C file**, same
function names (`b3` prefix stripped, snake_case), same struct fields
(snake_case), same algorithm structure and float operation order. See
`PORTING.md` for the full conventions table (ids, `Vec` for `b3Array`,
`Arc<T>` for user-owned geometry, closures for callback+context pairs, etc.).

To pull in upstream changes:

1. `cd box3d && git fetch && git log 29bf523ce..origin/main -- src/ include/`
2. For each changed C file, diff it and apply the same change to the
   same-named module in `libs/box3d/src/`. The function order inside each
   module follows the C file, so hunks usually land in the same place.
3. If a test changed, mirror it in `libs/box3d/tests/` (same file names).
4. `cargo test -p makepad-box3d` must stay green (147 tests as of the initial
   port).
5. Update the commit hash in this README.

### Where things live

- `include/box3d/*.h` types → `src/types.rs`, `src/id.rs`, `src/constants.rs`,
  `src/math_functions.rs` (public math + math_functions.c)
- `src/<file>.c` → `src/<file>.rs` (including all 8 joint files, solver,
  contact_solver, physics_world, etc.)
- `src/math_internal.h` → `src/math_internal.rs`; `src/core.h/.c` →
  `src/core.rs`; `src/simd.h/.c` → `src/simd.rs`
- `shared/utils.h` (test RNG) → `src/test_utils.rs`
- `test/test_<x>.c` → `tests/test_<x>.rs` (`ENSURE` → `ensure!`,
  `ENSURE_SMALL` → `ensure_small!`, exported from the crate root)
- `benchmark/main.c` + `shared/benchmarks.c` → `examples/benchmark.rs`
  (`cargo run --release -p makepad-box3d --example benchmark`)
- World snapshots: `world_snapshot.c` → `src/world_snapshot.rs`, on top of the
  recording layer in `src/recording.rs` (byte buffer, LE writers, geometry
  registry, capture hooks in every mutator, `world_start/stop_recording`,
  `hash_world_state`) and `src/recording_replay.rs` (bounds-checked readers,
  registry + tag-table loading, the op dispatch table, `Player` with keyframe
  scrubbing/seek/restart and the per-frame query store, `validate_replay`).
  The snapshot/recording byte formats are port-specific (own magic/version +
  struct-layout hash), not C-file-compatible; the logical record structure
  mirrors C so hunks land in the same places. The guarantee is bit-identical
  continuation after restore and hash-exact replay at every recorded step
  (verified in `tests/test_snapshot.rs`, `tests/test_recording_capture.rs`,
  `tests/test_recording.rs`).

## Cargo features

- `disable-simd` — C `BOX3D_DISABLE_SIMD`: scalar math instead of SSE2/NEON.
  Default builds use SSE2 on x86_64 and NEON on aarch64 (contact solver wide
  ops; `V32` stays scalar on ARM exactly like C). All three paths are
  bit-identical: the determinism ragdoll hash matches across NEON, SSE2 and
  scalar (`tests/test_simd.rs` asserts per-op bit equality).
- `double-precision` — C `BOX3D_DOUBLE_PRECISION` (large world mode): `Pos`
  becomes `{f64, f64, f64}` and `WorldTransform` gets a double translation
  with a float quaternion. All crossings go through the boundary functions in
  `math_functions.rs`, mirroring the C header. Enables the far-from-origin
  test halves (6 extra tests). Snapshot images record the precision mode and
  reject cross-mode loads.

## Performance vs C (Apple Silicon, release + fat LTO, 2026-07-04)

C compiled `clang -O3`, upstream benchmark scenarios (`examples/benchmark.rs`,
`-w=<workers>`). Single worker: Rust is 1.04–1.56× slower (geomean ≈ 1.3×);
the residual gap sits in collide/constraints codegen (bounds checks, no
`restrict` aliasing). At 8 workers Rust scales 3.4–5.7× over its own serial
times (large_pyramid 1676→366 ms, washer 30.6→8.7 s, junkyard 25.2→6.7 s) and
lands 1.2–2.3× behind C at 8 workers (geomean ≈ 1.6× — parallel scaling
efficiency is the current tuning frontier). Rust at 8 workers beats
single-threaded C by 2.4–4.8× on heavy scenes.

## Intentional differences from C (keep these in mind when diffing)

**Not ported** (skip these when syncing, or port them then):
- Debug draw (`b3World_Draw`, `b3DebugDraw`, draw fns in joints/shapes) and
  dump/save/load debug helpers (`b3Dump*`, `b3DynamicTree_Save/Load`). This
  also excludes the player's draw-only surface: debug-shape callbacks
  (`b3RecPlayer_SetDebugShapeCallbacks`) and `b3RecPlayer_DrawFrameQueries`
  (the query info accessors ARE ported). The recording/replay op stream and
  player are otherwise fully ported (see above); `recording_ops.inl`'s
  X-macro table became the `RecOp` enum + the dispatch match in
  `recording_replay.rs`
- Compound byte serialization (`b3ConvertCompoundToBytes`/`BytesToCompound`)
  — compounds serialize through the snapshot geometry registry instead
- Threading IS ported (scheduler.c/parallel_for.c + the solver's atomic
  block-claiming stage machinery, sync primitives in sync.rs): set
  WorldDef.worker_count > 1. Results are bit-identical at any worker count
  (the determinism test asserts the same hash at 1/2/4 workers). External
  task-system callbacks (enqueueTask/finishTask) are not ported — built-in
  scheduler only. Pre-solve/custom-filter callbacks force the affected pass
  to run serially (Box<dyn FnMut> is not Sync)
- The global world registry: `World` is an owned struct, every API function
  takes `world` explicitly (`b3Body_GetPosition(id)` →
  `body_get_position(&world, id)`)

**Representation changes:**
- `b3Array(T)` → `Vec<T>`; the arena/block allocators are bookkeeping shims
  (`arena_allocator.rs`), call sites use plain `Vec`s
- Blob geometry (hull/mesh/height field/compound trailing arrays + byte
  offsets) → plain `Vec` fields; `version`/`byteCount` fields don't exist, so
  tests asserting them skip those lines
- User-owned `const b3HullData*` etc. → `Arc<HullData>`; `b3DestroyHull` is
  Arc drop. The world hull database keeps C's explicit per-shape refcount
- C unions → both-fields structs (`ContactCache`, contact convex/mesh) or
  enums (`ShapeGeometry`, `JointUnion`, `ChildShapeGeom`)
- Solver pointers → indices: per-color constraint pointers became
  (start, count) ranges into StepContext-owned arrays; see the layout contract
  at the top of `contact_solver.rs` and the StepContext redesign note at the
  top of `solver.rs` (awake-set states/sims are `mem::take`n into the context
  during solve — any new C code that reads body data through the world during
  the solve stages needs the `Option<&StepContext>` dual-path pattern, see
  `joint.rs::reaction_body_transform`)

**Numerical/determinism notes:**
- The port preserves float operation order, and the deterministic
  `b3Atan2`/`b3ComputeCosSin` are digit-for-digit. The simulation is
  self-deterministic AND cross-architecture deterministic: the ragdoll
  determinism hash is identical on NEON, SSE2 (Rosetta-verified) and scalar
  builds (sleep step 269 matches the C float build exactly). It is NOT
  bit-identical to C builds: `remainderf` is implemented via f64
  (`math_functions.rs::remainder_f32`), and geometry content hashes are
  computed over a canonical little-endian serialization instead of raw struct
  bytes, so hash VALUES differ from C. The two precision modes hash
  differently, like C's per-mode EXPECTED_HASH
- `qsort.h` call sites use `sort_unstable_by`; ordering of exactly-equal keys
  may differ from C (self-consistent)
- Upstream quirks preserved on purpose (flagged with comments, don't "fix"
  when syncing): the scalar `scatter_bodies` does not apply per-axis lock
  flags (matches C's `B3_SIMD_NONE` path; C's SSE2/NEON path does),
  `get_wheel_joint_force` sums `lowerSuspensionLimit` instead of
  `lowerSuspensionImpulse`, spine_02 inherits spine_01's name in the human
  scenario

**Test suite:** `cargo test -p makepad-box3d` (179 tests; 185 with
`--features double-precision`). Also run `--features disable-simd` and the
feature combinations when syncing. `test_recording.c` is ported as
`tests/test_recording.rs` minus its two debug-draw-callback subtests
(DebugShapeCallbacks, KeyframeHandleReuse — debug draw is not ported); it adds
port-specific worker-count-invariance round trips (record at 4 workers, replay
at 1 and 4). `test_determinism.rs` asserts run-to-run equality instead of the
C `EXPECTED_HASH` constant. `tests/test_smoke.rs`, `tests/test_simd.rs`,
`tests/test_snapshot.rs` and `tests/test_recording_capture.rs` are
port-specific (not from C).
