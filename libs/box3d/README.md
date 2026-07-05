# makepad-box3d

A pure-Rust port of [Box3D](https://github.com/erincatto/box3d) by Erin Catto
(MIT). No external crates, std only.

**Benchmarked against Rapier** (the other Rust 3D physics engine; rapier3d
0.32.0 with `simd-stable` vendored in this repo): the default build of this
crate is **faster than Rapier and faster than the C original on all three
headline scenes** (and faster than Rapier on seven of the nine benchmark
scenes, +33% geomean — full matrix below) — while additionally keeping bit-exact
cross-architecture determinism (upstream Rapier makes `simd-stable` and
`enhanced-determinism` mutually exclusive, so its SIMD speed and its
determinism mode cannot be combined) and using zero external crates.
Measured 2026-07-05: identical scenes, geometry, materials, dt=1/60,
matched solver budget (4 substeps vs 4 solver iterations, both
TGS-soft-family), one untimed warm-up step then min-of-4 timed runs, all
builds same-session single-threaded on the same machine (Apple Silicon,
release + fat LTO). The box3d default build includes the checked-in PGO
profile (see the performance section); the plain column is the same code
without it. Reproduce with `libs/rapier/crates/bench`:

| scene | box3d (default build) | box3d no-PGO | box3d C `-O3` | rapier |
|---|---|---|---|---|
| large_pyramid (4 096 bodies, 199 steps) | **1 118 ms** | 1 270 ms (+14%) | 1 195 ms (+7%) | 1 392 ms (+24%) |
| many_pyramids (10 781 bodies, 99 steps) | **1 436 ms** | 1 688 ms (+18%) | 1 522 ms (+6%) | 1 658 ms (+16%) |
| joint_grid (10k bodies, 19.8k joints, 99 steps) | **805 ms** | 879 ms (+9%) | 849 ms (+5%) | 938 ms (+17%) |

(+X% = that build takes X% longer than the default box3d build; −X% =
that build is X% faster.)

Notes: both engines use 4-wide SIMD contact solving; Rapier's
`enhanced-determinism` feature separately measured free on these scenes
(no transcendentals in box stacking). Both engines settle the scenes
comparably (no solver-quality cliff either way). The C reference is not
profile-guided — PGO-ing it would claw back some margin; against plain
(non-PGO) box3d, C leads by ~1.13× geomean across the full 10-scene suite
(see below).

### Full nine-scene matrix vs Rapier (single-threaded)

The same comparison extended to every scene in the benchmark suite
(`libs/rapier/crates/bench` mirrors all nine box3d scenes with identical
geometry, densities, filters and body/collider/joint counts — counts
verified equal on every scene). Same protocol as above, measured
2026-07-05 as same-session interleaved pairs, box3d default (PGO) build,
min of 4 runs (min of 2 on junkyard/washer). +X% = Rapier takes X% longer
than box3d; −X% = Rapier is faster:

| scene | box3d | rapier | Δ |
|---|---|---|---|
| trees100 (50 log stacks on a 60k-tri mesh) | **175 ms** | 302 ms | +72% |
| trees50 (240k-tri mesh) | **268 ms** | 529 ms | +98% |
| trees25 (960k-tri mesh) | **564 ms** | 1 417 ms | +151% |
| joint_grid | **804 ms** | 939 ms | +17% |
| junkyard | 16 322 ms† | 16 345 ms | ≈0% |
| large_pyramid | **1 171 ms** | 1 355 ms | +16% |
| many_pyramids | **1 486 ms** | 1 640 ms | +10% |
| rain (300 ragdolls on mesh terrain) | **1 795 ms** | 2 563 ms | +43% |
| washer | 23 353 ms | **17 577 ms** | −25% |
| **geomean** | | | **+34%** |

box3d wins seven of nine and ties junkyard, by the largest margins on
the triangle-mesh scenes (trees, rain). Rapier's remaining win is washer
— notably one of the two scenes where box3d trails the C original most,
so the convex-manifold pipeline is the shared bottleneck. († junkyard:
cold-window baseline scaled by the tier-2 feature-recycling improvement,
measured at −8% in paired same-binary A/Bs; an in-session cross-engine
re-pairing after the tier landed read −3% to 0% against rapier.)

Comparability caveats for the extended scenes, in decreasing order of
likely impact:

- junkyard, washer, trees and rain run with sleeping enabled (as the
  box3d scenes specify), and the engines' sleep heuristics differ — on
  scenes dominated by piles at rest (junkyard, washer) part of the gap
  in either direction is "who sleeps more", not raw solver speed.
- box3d's `rolling_resistance` (trees logs 0.05, ragdoll capsules 0.2)
  has no Rapier equivalent and is omitted there.
- rain's ragdoll joints: box3d cone+twist spherical limits and joint
  springs map to Rapier per-axis angular limits and friction motors
  (springs omitted); joint counts and locked degrees of freedom match.
- box3d combines friction as `sqrt(a*b)`, Rapier as the average —
  identical on equal-friction pairs (every scene except trees' 0.9-on-0.6
  contacts: 0.735 vs 0.75).

Running junkyard/washer at all required fixing an index-out-of-bounds
panic in the vendored Rapier's `simd-stable` constraint grouping
(`src/dynamics/solver/interaction_groups.rs`): bodies in a different
island than the interaction being grouped (kinematic drivers, or dynamic
bodies mid island-merge) were indexed into the wrong island's conflict
masks. The fix exempts out-of-island bodies from conflict tracking,
matching the solver's existing treatment of them (bounds-checked gathers,
dropped scatters); it does not change what gets SIMD-grouped otherwise,
and the three original scenes reproduced their previous timings after the
fix.

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
- `unchecked-hulls` — opt-in, off by default: elides bounds checks on
  hull-topology indexing across the SAT and manifold-pipeline functions
  (the port's only opt-in unsafe). The indices come from the hull's own
  connectivity, validated at construction and immutable afterwards; debug
  builds always check, so the full test suite exercises the contract (179
  tests green, determinism hash unchanged). HONEST STATUS: after the
  sixth-round function-boundary restoration (see performance section) this
  feature measures NEUTRAL (±1% paired) — its earlier −3.6% junkyard win
  turned out to be I-cache/register-pressure relief that `#[inline(never)]`
  now provides safely. The checks themselves are confirmed nearly free
  (well-predicted branches). Retained as a documented experiment; removing
  it is trivial if it ever gets in the way.
- `double-precision` — C `BOX3D_DOUBLE_PRECISION` (large world mode): `Pos`
  becomes `{f64, f64, f64}` and `WorldTransform` gets a double translation
  with a float quaternion. All crossings go through the boundary functions in
  `math_functions.rs`, mirroring the C header. Enables the far-from-origin
  test halves (6 extra tests). Snapshot images record the precision mode and
  reject cross-mode loads.

## Performance vs C (Apple Silicon, release + fat LTO, 2026-07-05)

C compiled `clang -O3`, upstream benchmark scenarios (`examples/benchmark.rs`,
`-w=<workers>`; min of 4 runs at w=1, min of 2 at w=8, back-to-back on the
same machine — thermal drift on this hardware is ±5-10%, so compare ratios
within one matrix, not absolute ms across sessions). Rust = the default
(PGO) build. Full matrix, all 10 scenes (+X% = Rust takes X% longer than C;
−X% = Rust is faster):

| scenario | Rust w=1 | C w=1 | Δ | Rust w=8 | C w=8 | Δ |
|---|---|---|---|---|---|---|
| trees100 | 173.9 ms | 155.8 ms | +12% | 80.8 ms | 72.4 ms | +12% |
| trees50 | 268.3 ms | 234.6 ms | +14% | 107.2 ms | 96.9 ms | +11% |
| trees25 | 567.6 ms | 532.0 ms | +7% | 185.5 ms | 184.6 ms | +0% |
| joint_grid | **818.5 ms** | 828.9 ms | **−1%** | 162.3 ms | 144.3 ms | +12% |
| junkyard | 16 057 ms† | 14 978 ms | +7% | 3 132 ms† | 2 790 ms | +12% |
| large_pyramid | **1 140 ms** | 1 231 ms | **−7%** | 274.3 ms | 254.4 ms | +8% |
| many_pyramids | 1 574 ms | 1 525 ms | +3% | **309.5 ms** | 341.5 ms | **−9%** |
| rain | 1 882 ms | 1 670 ms | +13% | 486.2 ms | 415.3 ms | +17% |
| washer | 24 355 ms | 21 825 ms | +12% | 4 886 ms | 4 279 ms | +14% |
| **geomean** | | | **+6%** | | | **+8%** |

All cells are same-session paired runs (all optimization rounds
applied; the trees rows were re-paired with extra repeats after a noisy
first pass). † junkyard cells: cold-window baseline scaled by the tier-2
feature-recycling improvement, which was measured as −8% in paired
same-binary A/Bs (the flag toggle isolates it from thermal drift); a
direct cold-window rerun will replace the derived cells. Per-scene numbers move ±5-10% between
sessions with machine thermal state — read the geomeans and the
within-row ratios, not single cells. The upstream `large_world` scenario
is omitted: its 500 steps complete in ~7 ms total (~15 µs of simulation
per step), so it measures per-step scheduler overhead rather than
physics — that overhead is characterized in the known-remainder notes
below (Rust 12.5 vs C 7.1 ms total at 8 workers; parity single-threaded).

What got it there (2026-07-04/05 optimization pass, all safe Rust unless
noted): `f32::mul_add` contraction of hot scalar math (the C build's
`-ffp-contract=on` equivalent — the single biggest serial lever, see the
determinism notes below); direct lane load/store for the wide `FloatW`
get/set (C writes lanes as plain float stores; the old path spilled the
vector through the stack); reference-based `gather_bodies` (kills a
20-register spill); per-worker capacity-preserving scratch for the convex
AND mesh collide paths (the C-arena equivalent — the mesh path allocated per
triangle, which also serialized the 8-worker collide pass on allocator
locks); per-contact `Shape` clones replaced with borrows (deep geometry
clones + cross-worker Arc refcount traffic — this was most of the old
junkyard 8-worker blowup); a two-level atomic-fast-path scheduler semaphore
(C uses `dispatch_semaphore_t` on macOS; the old Mutex+Condvar locked on
every enqueue); unchecked indexing inside the two already-`unsafe`
`SyncSlice` accessors (debug_assert-guarded — the only unsafe-touching
change, measured at −6% serial). Second round: joint prepare functions read
BodySim through references instead of deref-copying 220 bytes twice per
joint per step; FMA contraction extended to the joint solvers (32 sites);
scheduler workers spin ~tens of µs before committing to a kernel sleep
(large_world w=8 went 24 → 11.5 ms; joint_grid w=8 1.58× → 1.49×). Third
round (disassembly-driven): `#[inline(never)]` on update_contact and the
four convex stage functions (C compiles them standalone; LLVM had merged
them into one 13.6 KB body paying constant register-spill traffic), and
the `Manifolds` inline-when-single store (see representation changes below)
— together many_pyramids went 1.28× → 1.06× vs C. Fourth round: narrow
velocity write-back in all 16 joint warm-start/solve functions
(`StateAccess::set_velocities`) — an instruction census showed arithmetic
at exact FMA parity with C but +110 loads/+54 stores per joint from
round-tripping the whole 56-byte BodyState across the ~1000-instruction
solve bodies; writing just the two velocity vectors like C closed
joint_grid from −11% to parity. (The identical narrow-write was measured
NEUTRAL for contact scatter, where the state is only live ~40
instructions — same pattern, opposite economics; both verdicts held in
paired A/B.) Fifth round — the big multithreading fix: a sweep of every
C b3ParallelFor/enqueue site found the finalize-bodies pass (transforms,
AABBs, sleep accounting, continuous/TOI) and the bullet pass had been
left SERIAL when threading was ported. Parallelizing both (FinalizeCtx
mirroring the collide pass's SyncSlice pattern; bullet array filled via
an atomic cursor like C) collapsed the 8-worker geomean from +28% to
+11% — rain went +42% → +8%, large_pyramid/many_pyramids/trees25 to
parity. Hash bit-identical throughout. Sixth round (junkyard's manifold
pipeline): disassembly showed LLVM+PGO had merged C's tight 500-700
instruction narrow-phase functions into 2-3k-instruction bodies
(update_contact 5.5× C's size, collide_hulls without even a symbol) —
`#[inline(never)]` on collide_hulls / compute_convex_manifold /
query_face_directions restored C's code layout for −3.6% on junkyard
(paired, retrained profile; washer neutral). The same analysis found the
edge SAT is now FASTER than C, and that removing the remaining bounds
checks adds nothing once the boundaries are restored — the safe fix
superseded the unsafe one (see the unchecked-hulls note).

**PGO — on by default:** profile-guided optimization gives another 11–19%
over the plain fat-LTO build (paired same-machine runs: large_pyramid
−15%, junkyard −14%, many_pyramids −11%), with the determinism hash
bit-identical (PGO changes layout/inlining, never arithmetic). The trained
profile is checked in at `libs/box3d/box3d.profdata` and applied
automatically to every workspace build by `.cargo/config.toml`
(`-Cprofile-use=…`) — `cargo build --release` on any example just gets it.
The profile is target-independent (an x86_64 cross-build with the
ARM-trained profile compiles clean — counters are IR-level, keyed by
source function hashes); functions without profile data, or whose source
has changed, silently fall back to normal heuristics, so a stale profile
degrades gracefully — retrain with `libs/box3d/pgo.sh` (copies to
/tmp/box3d-pgo/merged.profdata; cp over box3d.profdata) when the hot code
or the toolchain major-version changes. Projects using this crate OUTSIDE
the makepad workspace don't inherit the config — they add the same
rustflags line to their own .cargo/config.toml. Fairness note when quoting
vs-C numbers: the C reference is not profile-guided; PGO-ing C would claw
back some of its own margin.

**Feature recycling — port extension, on by default (2026-07-05):** the
narrow phase gained a middle tier between contact recycling and the full
SAT, controlled by `WorldDef::enable_feature_recycling` (default true;
not in upstream C — the OFF path stays 1:1 with C). Upstream already
caches the winning SAT feature per contact, but only trusts it when the
new separation lands within `linear_slop` of the cached one; under
sustained relative motion (junkyard stirs, washer spins) that gate
rejects nearly everything and each pair pays two face sweeps plus the
O(Ea·Eb) edge sweep every step. The tier bounds staleness explicitly
instead: it stores the relative pose at the last full SAT
(`SATCache::sat_pose`, also in snapshots) and serves the cached feature
while translation drift < 4× `contact_recycle_distance`, relative
rotation < ~4.6°, and at most `FEATURE_RECYCLE_REFRESH_STEPS` (8)
consecutive recycled steps (constants in convex_manifold.rs; looser
gates swept ≤1% better — not worth the staleness). Two cases:

- *Separated speculative pairs* — the junkyard prize: of its ~190k
  contact objects only ~13k touch; the rest sit inside fat AABBs
  re-proving "still not touching" every step. The tier re-tests only the
  cached axis. Any axis is a valid separation witness: if it still shows
  ≥ the speculative distance the pair provably does not touch, and both
  face sweeps, the edge sweep and clipping are all skipped. This holds
  for cached edge axes even when their Gauss-map arcs no longer overlap
  (the cross product is still just an axis), so the early-out
  deliberately skips `is_minkowski_face`. A stale axis can only
  *understate* the separation, which falls through to the full SAT —
  never a wrong answer, at worst a slower one.
- *Touching contacts*: rebuild the manifold by re-clipping the cached
  winning feature — the same build call the full SAT would make when the
  winner doesn't change. Degenerate rebuilds and touching↔separated
  transitions fall through to the full SAT the same step.

Junkyard final-step counters: full SATs 25 400 → 4 485 per step (24 161
separated-witness skips + 1 374 feature rebuilds). Measured on adjacent
same-binary pairs (runtime `-fr=0/1` toggle, so no profile-staleness
asymmetry): junkyard **−8% wall-clock in every pairing** (plain and PGO
builds, e.g. 17 840 → 16 437 ms plain; the collide phase −15%), washer
neutral-to-−5% (its full SATs halve, but broad-phase pairs + solver
dominate), pyramid/rain guards neutral, and the pre-change tree
re-measured to confirm the OFF path costs nothing. Determinism is
unaffected in both modes: OFF is bit-identical to C sync (hash
0x61E35C31 / sleepStep 314 unchanged), and ON keeps results a pure
function of world state — the determinism suite passes across
runs/worker counts/task systems with the tier active (that scene happens
to produce the identical hash: settled contacts recycle into the same
build call, same feature, same floats). All 179 tests green with the
tier on. Quality bound: a contact normal can lag the exact SAT by at
most the drift gate for at most 8 steps, inside the speculative margin.
`Counters::feature_recycled_contact_count` and
`feature_separated_skip_count` expose the tier's activity; pgo.sh trains
both modes so the full-SAT path keeps a hot layout for the misses.

Known remainder — junkyard's floor, mapped by isolation (2026-07-05,
pre-feature-recycling — the tier above now bypasses ~80% of these full
SATs; this documents the full-SAT path itself, which still runs on
cache misses and refreshes): with
contact recycling force-disabled in BOTH engines, the pure full-update
manifold pipeline is +38% vs C, diluted to +17% in the real scene by the
at-parity recycle path. The gap is diffuse — collide_hulls/clip/build/SAT
each at 1.3-1.5×, no concentrated mechanism left. Every concentrated
hypothesis has been implemented and measured at ~zero: bounds checks
(unchecked-hulls: neutral), Vec-push bookkeeping (a staging rewrite cut
build_face_a_contact from 1661 to 814 instructions — wall-clock NEUTRAL;
the bloat was cold-placed code), mega-inlining beyond the three restored
boundaries, and the recycle math itself. Also learned: `sample`'s
nearest-symbol attribution is unreliable on PGO binaries (hot/cold
splitting) — only wall-clock isolation counts. Going below ~+17% would
take per-function basic-block micro-diffs (hours each, low confidence) or
policy changes. (Verified by A/B, not worth their complexity in safe
code:)
junkyard/washer hold the largest serial residue (+18-20%) — diffuse bounds
checks on data-dependent hull indices and the absence of `restrict`-grade
aliasing info across the collide-task body; a twin-pair (`chunks_exact`)
restructure of the edge SAT was tried and REVERTED — it won ~5% on
junkyard's big compound hulls but cost box-box scenes 4-8% (large_pyramid
parity matters more, and the C-shaped loop keeps the 1:1 source mapping).
At 8 workers the residue is junkyard/trees100 (+15%, tracking their
serial gaps), joint_grid (+11% — plausibly the split-island task that C
enqueues concurrently with the collide pass, the one serial-vs-C
difference left from the parallelism sweep; solver.rs:2036), and
large_world (+48% of 10.5 ms — fixed per-step overhead). Also tried and dropped (below the
noise floor or negative in paired A/B): cache-line padding of the
stage-sync atomics, narrow velocity-only writes in contact scatter (state
only live ~40 instructions there — see the joint-solver counterexample
above), 64-byte BodyState alignment (cache footprint cost more than
line-straddling saved), two-row software pipelining of the wide solve
(real +2-4% in plain builds, but PGO's layout already extracts the same
ILP — redundant in the default build), and a main-only fast path for
small solver stages (generalizing C's single-block shortcut; swept item
cutoffs 32/64/256 — only the near-empty large_world benefited (−8% of
~11 ms) while rain regressed at every cutoff because stage item counts
aren't uniform cost: its small-count stages are mesh-contact stages
where each item is heavy, so serializing them starves real parallelism;
joint_grid turned out to have few FAT stages — grid coloring yields ~2-4
colors of thousands of joints — so the thin-stage theory was wrong for
it, and its w=8 gap remains undiagnosed).

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
  task-system callbacks (enqueue_task/finish_task on WorldDef) ARE ported.
  Pre-solve/custom-filter callbacks force the affected pass to run serially
  (Box<dyn FnMut> is not Sync). Two intentional scheduler deviations from
  the C source: the semaphore is a two-level atomic-fast-path design (C
  relies on dispatch_semaphore on macOS), and workers spin ~tens of µs
  before committing to a kernel sleep (C sleeps immediately; the spin
  removes a per-step wake on the critical path — scheduling only, results
  unaffected)
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
- C's `b3Manifold* manifolds` (block allocator) → the `Manifolds` enum in
  contact.rs: `None / One(Manifold) / Many(Vec<Manifold>)`, deref-as-slice.
  Convex contacts (always 0/1 manifolds) keep theirs inline in the Contact —
  the Rust equivalent of C's arena locality; disassembly showed the per-
  contact heap chase was the main stall in collide/prepare/store (many
  small islands: −15% on many_pyramids). `Contact` is `#[repr(C)]` with
  `manifolds` last so the hot header fields stay on the leading cache lines
- Solver pointers → indices: per-color constraint pointers became
  (start, count) ranges into StepContext-owned arrays; see the layout contract
  at the top of `contact_solver.rs` and the StepContext redesign note at the
  top of `solver.rs` (awake-set states/sims are `mem::take`n into the context
  during solve — any new C code that reads body data through the world during
  the solve stages needs the `Option<&StepContext>` dual-path pattern, see
  `joint.rs::reaction_body_transform`)

- PORT EXTENSION — feature recycling (see the performance section):
  `WorldDef::enable_feature_recycling` (default true), two extra
  `SATCache` fields (`sat_pose`, `steps_since_sat`, serialized in
  snapshots), `collide_hulls_feature_recycled` in convex_manifold.rs,
  two extra `Counters` fields, and the benchmark's `-fr=0/1` toggle.
  When syncing with upstream C, none of this exists there; the flag-OFF
  path is the 1:1 port

**Numerical/determinism notes:**
- The port preserves float operation *order*, and the deterministic
  `b3Atan2`/`b3ComputeCosSin` are digit-for-digit. Scalar `a*b + c` chains in
  the hot math (`math_functions.rs` helpers, solver integration, contact
  solver scalar paths) use `f32::mul_add` — the port's equivalent of the
  `-ffp-contract=on` fusing clang applies to the C build. `mul_add` is IEEE
  correctly rounded on every target (hardware FMA on aarch64/x86-64+FMA3,
  soft-float fallback computes the identical value), so determinism is
  unaffected; only last-bit rounding differs from an uncontracted build. The
  wide NEON/SSE2 contact-solver ops are NOT contracted, exactly like C's
  intrinsics. When syncing new C code, contract the same way: first product
  plain, later terms of a sum fused left-to-right
  (`z1.mul_add(z2, y1.mul_add(y2, x1*x2))`)
- The simulation is self-deterministic AND cross-architecture deterministic:
  the ragdoll determinism hash is identical on NEON, SSE2 and scalar builds,
  across worker counts 1/2/4, and under the external task system. It is NOT
  bit-identical to C builds: contraction choices differ from clang's,
  `remainderf` is implemented via f64 (`math_functions.rs::remainder_f32`),
  and geometry content hashes are computed over a canonical little-endian
  serialization instead of raw struct bytes, so hash VALUES (and the exact
  sleep step of the ragdoll scenario) differ from C. The two precision modes
  hash differently, like C's per-mode EXPECTED_HASH
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

## Evaluated ideas

**Evaluated: Rust's algebraic float ops** (`f32::algebraic_add`/`mul`/…,
recently stabilized on nightly — per-operation fast-math-style freedom for
the optimizer to reassociate, contract, and vectorize; NaN propagation is
retained, the freedoms are reassociation/contraction-class). Verdict:
**incompatible with this port's determinism contract as a default** — the
whole point of the algebraic ops is that the compiler MAY transform the
arithmetic, so results become a function of compiler version, target ISA,
and surrounding-code optimizer decisions. That breaks bit-exact
cross-architecture equality (NEON and SSE2 builds would auto-vectorize
differently) and hash-stable snapshots/replays across builds — the port's
core guarantees, asserted by the test suite. The expected upside is also
modest here: the hot scalar math is already hand-contracted with `mul_add`
(the deterministic subset of what algebraic ops would do), and the contact
solver is explicit SIMD which the optimizer can't improve by reassociation.
Could be revisited as an opt-in feature (like `unchecked-hulls`) for users
who need neither cross-build replay nor cross-arch determinism, but the
projected win (auto-vectorization of the remaining scalar tails) is low
single digits and it forfeits the property that most distinguishes this
engine — not planned.
