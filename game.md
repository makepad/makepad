# game.md — Makepad Arcade (networked AI game sandbox)

**Name: Makepad Arcade. App lives in `apps/arcade`; engine crates in `libs/game/*`.
Arcade REPLACES gamemaker and the xr example apps — no compat requirement; gamemaker
survives only as the parity/tape oracle until Arcade absorbs its fixtures.**

## BUILT (2026-08-03) — M0-M6 landed on branch rik2

`libs/game/{math,sim,render,script,blocks,net,session,coedit,pkg}` + `apps/arcade`.
Deterministic math kernel (cross-arch parity goldens) · sim extracted from
game_view.rs (5545 -> 1878 lines) with box3d as the dynamics layer · forward
renderer with skinned characters (KayKit, download_assets.sh) · table-driven
102-verb script layer shared by gamemaker and Arcade · blocks (car/character/
plane/brains/race kit) — **a complete racing game is 72 lines** · host-auth LAN
multiplayer (HMAC, per-entity seq, 2400pps/20.9Mbit for 6x200) · MR/VR stage modes
(sim-untouched, proven) · voice tiers + librarian + /pair · multi-Claude co-editing
(intent log + semantic rebase + soft leases) · packaging/registry with hardened
extraction and capability-stripped isolates.
**The tape probe has stayed byte-identical through every commit.**
Remaining: M7 pretty pass (in flight), M3 part 2 (XR root widget for passthrough,
env-depth, Windows OpenXR/D3D11), device testing on Quest.

Same-room multi-device playground: kids talk, Fable builds the game, everyone in the
room joins from PC / Mac / Quest / phone. AI composes games from working building
blocks (cars, planes, characters). Games shareable online. Rendering = pretty-but-
barebones forward pass (Quest budget). Quest plays **mixed reality** (passthrough,
game anchored/scaled into the room); PC VR / desktop / mobile play **full-environment**.

## What we have (2026-08-02)

- **gamemaker** (`examples/gamemaker`): voice → Fable edits `game.splash` → live 3D
  pane, rollback-on-error streaming re-eval, agent harness `tools/ag`, synth, tape
  determinism. Physics = own mini AABB, NOT box3d. `game_view.rs` = 5545-line monolith.
- **box3d** (`libs/box3d`): deterministic rigid-body physics, bit-exact cross-arch,
  snapshot + recording, beats rapier 8/9. Consumer today: xr/, not gamemaker.
- **xr/** (~23k lines): scene framework, hands/controllers/passthrough/env-depth,
  raycast vehicle, **working LAN multiplayer** (`xr/src/net`: UDP discovery :41546 +
  UDP data + TCP sync, LZ4, shared-object batching, peer alignment). Quest ships via
  cargo-makepad `quest` variant.
- **splash VM** (`platform/script`, ~53k LOC): NaN-boxed values, compile-to-opcode
  interpreter (no JIT), real isolates (`widgets/src/widget_async.rs`) but UI-thread
  time-sliced, hard single-threaded (`!Send`). Two integration idioms today:
  xr = declarative (eval once → typed apply → Rust drives), gamemaker = imperative
  (streaming eval → `game` handle + retained closures → script drives at 60Hz).
- **Rendering**: `DrawPbr` (GGX+IBL+baked AO), `SceneSun` map-only (shiny.md T7 undone;
  5 ad-hoc sun rigs). Shadows = blob/baked geometry. Gaps: no pass-depth sampling on
  any backend (no shadow maps), GLES offscreen passes never attach depth
  (`opengl.rs:869-910` — breaks 3D pane on Android), no offscreen MSAA.
- **Voice**: `libs/voice` (Whisper + Silero VAD), `libs/converse` (QwenFilter local
  judge, SEND/SKIP), `libs/tts` (Kokoro). Gamemaker already on converse's speech leg.
- **Sharing**: none. `libs/zip_file` read-only. HTTP client/server in `platform/network`.

## Review findings that drive the design (2026-08-02 code review)

1. **Sim/render boundary nearly exists**: `GameWorld` + step/queries (~500 lines) touch
   no Cx/draw types. Real entanglements: `ScriptObjectRef` callbacks inside sim state
   (blocks `Clone`/snapshot), camera state split widget↔world with a request mailbox,
   `render_rev` bumped from 12 dispatch sites, synth mutex called from sim reset.
2. **Multiplayer surface = exactly the singletons**: one camera rig, one input set
   (`poll_gamepad` keeps only the most-active pad!), one HUD, one 2D audio mixer, one
   rng, one save file. No engine "player" exists (tag convention) — good.
   Tightest knot: `input.move_x/move_z` rotated by *the* `cam_yaw` — per-player
   movement requires per-player camera yaw in the sim.
3. **Determinism verdict**: engine tick is same-binary deterministic (tapes byte-equal)
   but f32 libm (`sin/cos/atan2` in sim path) + script-side hashmap order kill
   cross-arch lockstep. box3d is bit-exact cross-arch; the sim above it isn't.
   → **host-authoritative state sync is the only correct v1.** Lockstep shelved.
4. **Mid-game join impossible today**: engine heap is enumerable/snapshottable, but
   causal game state (actors array, `driving`, `won`) lives in the script heap with
   no serializer. Host-auth sync of engine tier + "join anytime, causal state stays
   host-side" resolves this cleanly.
5. **Script call cost**: every call allocs a scope object; ~100s of ns per native call;
   84-arm linear verb dispatch; options-objects do dozens of hash lookups. Budget
   ~2ms/frame script. → per-entity-per-tick script calls are out; behaviors must run
   engine-side, script configures them.
6. **Splash VM bugs that bite a long-lived game**: isolate heaps are NEVER GC'd
   (per-tick args/input objects leak forever), timers aren't VM-scoped (cross-isolate
   cancel), instruction limits are per-call not per-tick, streaming parser swallows
   the tail statement (gamemaker appends `"\n;"`), checkpoint identity abuses the
   widget's heap address, stale `let`s survive re-eval.
7. **Blocks layer validated**: 36-40% of the 939-line fixture is prefab-absorbable
   (95 lines part-geometry, 60 spawn factories, 80 wander/chase brains, 37 vehicle
   mount/drive, 65 grapple). Irreducible core = distinct AI + spawn tables = data.
8. **AABB-isms are API contracts**: movers never collide with each other, yaw never
   rotates collision, axis-separated x/z/y sweeps, 0.55 step-up, attach = position
   pin. A box3d swap must preserve these via a character-controller layer, not drop-in.
9. **Perf debt**: O(n) entity lookup (O(n²) script loops), statics cloned every tick,
   box-mode terrain up to 147k entities, unenforced sorted-id invariant under
   binary-searching render code, blocking file I/O on the tick path.

## Audit before adoption

Much of the foundation was built with earlier/weaker AIs; this session's review alone
found latent defects in "working" code (unenforced sorted-Vec invariant, snapshot
id-collision leak, single-pad input, cross-isolate timer cancel, disabled free path).
**Rule: no inherited component is promoted into `libs/game/*` without an adversarial
audit + tests encoding its invariants.** Verdicts: promote / rework / rewrite.

| component | trust | audit focus before adoption |
|---|---|---|
| `libs/box3d` | high (test suite, determinism harness, benchmarks) | only the surfaces we lean on: snapshot/recording API, heightfield collider, kinematic char-controller interplay |
| `game_view.rs` sim semantics | audited 2026-08-02 | defect list above IS the M0 work list; add invariant tests as extraction proceeds |
| splash VM | audited 2026-08-02 | GC/determinism fixes get regression tests in `platform/script/test`; re-audit eval/streaming path after body-identity fix |
| `xr/src/net` (protocol v11) | **AUDITED 2026-08-02: transport promote-with-fixes, authority model REWRITE** | see §Net audit below — 5 P0 security/DoS fixes gate promotion |
| `xr/src/scene/xr_physics` (3126 l) + worker | unaudited | probably NOT promoted — game_sim owns box3d directly; mine it for sub-step/worker patterns only |
| `raycast_vehicle` | unaudited | physics correctness, tuning surface, determinism (libm use?), feel test before it becomes game.car |
| `DrawPbr` (2954 l, uniform feature gates) | unaudited | per-pixel cost on Quest GPU, dead features, gate granularity — may want a slimmer forward shader for game_render |
| `libs/zip_file`, `http_server` | unaudited | they face the network + untrusted archives: fuzz the zip parser, bounds/limits on the server, path-traversal on extract |
| `libs/converse`/`voice`/`tts` | fresh (2026-07) | mic-test gap noted in memory; latency budget on host while hosting a game |

**Untrusted games are untrusted code.** Online-shared `game.splash` runs in our VM:
shared-game isolates must be capability-stripped (no `std.run`, no net, no fs beyond
the game dir, instruction + heap budgets) and the script-std surface audited for
escapes before the registry ships (M6 gate).

## Engine design

**Thesis: script orchestrates, engine simulates.** Fable writes a thin splash program:
world layout, prefab spawns (data), rules, and a small `on_tick` for the custom 20%.
Everything hot — physics, vehicle/character/AI behaviors, replication, rendering —
runs engine-side in Rust at 60Hz. This merges the two splash idioms: declarative
prefab configuration (xr-style) + imperative orchestration (gamemaker-style) over one
world.

### Crates — `libs/game/*`

- **`game_math`** — deterministic f32 kernel: minimax-poly sin/cos/atan2/exp/pow,
  pure Rust, zero platform-libm calls (IEEE only guarantees + − × ÷ sqrt bit-exact;
  transcendentals differ per OS/arch — box3d avoided libm, the sim above it didn't).
  Mandatory in game_sim; patched into script math builtins for game isolates.
- **`game_sim`** — World, entities (sorted-id Vec + enforced invariant, binary-search
  lookup — shipped M0r), terrain, fixed 60Hz tick, **box3d as the dynamics layer
  (hybrid, decided at M1a)**: statics/kinematics mirrored into a box3d world, NEW
  `body:"rigid"` entities get full box3d dynamics (stacking, impulses, vehicle
  raycasts), movers KEEP the verbatim kinematic sweep — it IS the character
  controller, Godot-parity-proven, tape byte-parity holds. Queries, events. No Cx, no draw, no ScriptObjectRef: script callbacks become u32
  handle slots resolved by the host. `#[derive(Clone)]` World → snapshot ring, replay,
  session save. Every field tagged with a replication tier (below).
  **Bit-deterministic cross-arch** (game_math + seeded rng + no hash-order iteration):
  buys replays, desync checksums in the tier-1 stream, host migration when the host
  leaves, and reopens lockstep later — netcode stays host-auth v1 regardless.
- **`game_blocks`** — engine-side behavior components configured from script, run in
  Rust: car (raycast vehicle), plane (arcade flight), character (kinematic controller,
  step-up 0.55), wander/chase/flee brains, turret, projectile, pickup, checkpoint +
  race/lap logic, spawnpoint, score. Each bundles collision, camera rig preset, input
  map (kbd/pad/touch/XR), sfx hooks, replication semantics.
  **Characters are skinned meshes, not box-parts** (beyond gamemaker's cubes):
  clip playback + blending driven by controller state (idle↔walk↔run by speed,
  jump/fall; yaw stays procedural). Anim state = Derived tier (recomputed from
  velocity/state, free over the network); explicit emotes = shared events. Stock
  rigged library ships with Arcade: a few body types on ONE shared rig + swappable
  skins, so the AI recolors/retextures cheaply; downloaded/generated rigged glTF
  slots into the same path (auto-rigging arbitrary meshes is hard — stock-rig+skins
  until rigged gen-AI is real). Props/vehicles may stay primitive-built.
  Stock sourcing (verified 2026-08-02, all CC0, all ship glTF, all low-poly):
  **KayKit Adventurers** (4 rigged chars, ONE rig, ~75 anims — the M1 starter pack;
  github.com/KayKit-Game-Assets) + KayKit Skeletons; **Kenney Animated Characters**
  (rigged + swappable skins — matches the one-rig+skins plan); **Quaternius** packs
  as expansion. CC0 → vendorable into repo/registry without license friction.
- **`game_script`** — the `game.*` binding layer: table-driven verb dispatch (lut, not
  84-arm chain), the hardened eval-host patterns extracted from gamemaker (streaming
  checkpoint + tail-statement guard + captured_errors + world rollback), proper body
  identity (no address abuse), per-tick script budget.
- **`game_render`** — forward pass over a `WorldView` snapshot: sky/terrain/opaque/
  alpha slab batching (dirty-set derived from sim change-log, not 12 manual bumps),
  SceneSun, blob shadows, HUD, labels. Per-view: N cameras (split-screen, XR stereo,
  spectator) over one world. **Skinned-mesh path** (NEW — the xr glTF loader is
  static-only, verified 2026-08-02: no skin/joint/animation code in xr/src/obj or
  draw/): glTF skins + animation-clip parsing, GPU skinning in the forward shader
  (bone palette ≤64 uniforms, 4 weights/vertex — no compute, Quest-friendly).
- **`game_net`** — transport HARVESTED from `xr/src/net` (~1100 lines: LZ4 frame
  codec, dual UDP/TCP worker skeleton, poll budgets, connect tie-break, MTU batching,
  peer/lifecycle types); authority model REBUILT host-auth. See §Net audit.

### Net audit verdict (2026-08-02, read-only adversarial pass)

Transport = promote-with-fixes. **Authority model = rewrite**: it's peer-symmetric
(pairwise takeover triangle), and `XrTakeoverAccept` is unauthenticated+unvalidated —
any LAN host seizes every object with one packet. Host-auth inverts the trust
direction: client→host intent, host→client authoritative state, drop per-object
`authority`, sender check becomes `sender == host_id`.

P0 (gate promotion — all currently unbounded/unauthenticated):
1. `connect_timeout` is BLOCKING on the worker thread → 10k spoofed Hellos = ~20min
   stall AND a 20min hang in Drop. Nonblocking connect / bounded in-flight queue.
2. Cap peers (~16), pending connections + handshake deadline, pending controls —
   today all unbounded (fd exhaustion, 40GiB read_bufs, quadratic re-scan spiral).
3. **No authentication anywhere.** Pre-shared lobby key + HMAC per datagram/frame
   closes: 1-packet permanent peer-silencing (seq-window poison), 1-packet kick
   (unverified Leave), 1-packet traffic hijack (`touch_peer` overwrites peer addr
   from packet source), authority theft, clock-pong amplification.
4. Validate authority transfers (or reduce to host-auth check).
5. Budget the UDP receive loops (TCP path has budgets, UDP doesn't → livelock).

P1 (before 6-player 60Hz ships): per-entity seq (one global seq means a single
reordered datagram drops 12 entities); reset seq state on rejoin (stable player ids
break the accidental protection process-random node ids give today); write_buf
backpressure; incremental batch sizing (currently O(n²) re-serialization);
tick-snapshot discipline (apply order is arrival order — no logical tick exists).
Bandwidth reality check: 200 entities × 5 clients ≈ 5100 pps / 74 Mbit up — fine
wired, marginal on Quest WiFi → delta encoding + interest management needed, not a
bigger batch cap. P2: `String::de_bin` panics on invalid UTF-8 (micro_serde — lands
the moment anyone adds a player-name field), non-finite float validation, late-joiner
replay rebuilt as ordered snapshot, fuzz the LZ4 decompressor.
Cutting the XR alignment-descriptor payload lets the frame cap drop 4MiB→256KiB,
which retires the decompression-amplification finding for free.

Shells on top: gamemaker app (authoring: chat + agent + editor + game pane), player
app (desktop/mobile flat), XR shell (MR stage / VR). All three embed the same crates.

### Multiplayer model

- **Host-authoritative, 60Hz, LAN.** Target: 6 players + ~200 moving bodies (box3d
  laughs at this; the wire fits existing xr batching). Host = the PC/Mac that owns
  the agent + mic; Quest/phones join. Bots = host-side players without a device.
- **Input packet per player per tick**: axes, buttons, cam_yaw (+look deltas).
  `move_x/move_z` computed host-side from the packet's cam_yaw — resolves the
  input⊗camera knot; the camera rig itself stays client-local presentation.
- **Players are first-class**: `game.on_join(fn(p))` / `game.on_leave`, `p.input`,
  `p.camera({...})` (replicated command to that client's rig), `p.hud.text/bar`,
  `p.attach(id)` for possession. Global camera/HUD verbs alias to "all players" for
  single-player back-compat. Spawnpoints assign ownership.
- **Replication tiers** (field-level, engine-enforced):
  1. *Shared*: entity pos/vel/half/body/tag/life, blocks state that gates gameplay,
     score, HUD content, spawn/remove/sfx events. Host→clients, quantized, batched.
  2. *Derived*: facing yaw, part animation, scale/glow easing, blob shadows, attach
     visual offsets — recomputed client-side from shared state + tick. Never sent.
  3. *Local*: particles/effects, beams-as-fx, camera rig, audio mix, MR stage
     transform. Per-device budget; a Quest runs leaner particles than a PC, no desync.
  The verb docs mark each verb's tier so Fable can't hang gameplay off local state.
- **Join anytime**: client downloads the game package (http), receives engine-world
  snapshot + tier-1 stream. Causal script state stays host-side by design (finding 4).
- Hot-reload during a session = host-only eval; clients just see tier-1 state jump.
  box3d determinism still buys host-side replay/rewind for the tape harness.

### Presentation modes (one sim, three projections)

- **MR stage (Quest default)**: passthrough on, world anchored into the room via the
  existing peer-alignment path, scale-to-stage (miniature race on the floor),
  sky/fog/horizon suppressed, shadow-catcher quad on the real floor, env-depth
  occlusion where available. Manifest declares footprint + preferred scale.
- **VR full-scale**: PC VR (no passthrough): full environment — sky, terrain, fog,
  first-person. Linux OpenXR works today; **Windows VR = new**: `XR_KHR_D3D11_enable`
  graphics binding reusing the existing openxr session/input/anchor code (move it out
  of `os/linux/`), D3D11 swapchain images, stereo pass wiring. Desktop runtimes ship
  their own loader. Smallest-surface new-platform job we have.
- **Flat**: desktop/mobile, the current pane; touch = virtual sticks (new).
- Asymmetric same-room play falls out: phone drives full-screen, Quest watches the
  same race as a diorama on the carpet.

### Splash VM work (platform/script, prerequisite fixes)

- **GC campaign** (heaps of isolated VMs are never collected today):
  eager reclaim of host-created per-tick transients (args/input objects; fix the
  `DISABLED: RootObject already freed` suspect path first), isolate VMs in a budgeted
  round-robin GC, full collections only at natural pauses (eval boundaries, round
  transitions — never mid-play), sweep only dirty categories, drop routine
  `shrink_to_fit`, periodic reclaim of dead isolates (today: only on next alloc).
- **Determinism**: script math builtins route to game_math in game isolates;
  `std.random` per-isolate + seedable (today: one process-global stream seeded from
  wall clock); object iteration in insertion order (also fixes the O(n²) `.nth()`
  walk — the map half needs an insertion index).
- VM-scoped timers (cross-isolate cancel bug).
- Per-tick cumulative instruction budget API (today: per-call).
- Public body-identity API for streaming eval (kill the address-as-column hack).
- Nice-to-have: stale-`let` cleanup on re-eval.

### DSL evolution

- Keep: camera-relative movement, visual-only yaw (network gift), blob shadows,
  seeded rng, attach/ride, step-up policy. Fix: the −Z yaw-sign trap (normalize,
  document once); de-Godot the terrain doc constants (keep as *a* preset).
- Add: `game.player(...)` family, prefab verbs (`game.car/plane/character/...`),
  behavior components (`wander/chase/patrol`), `game.particles` + `sfx_at`
  (positional, tier-3), mover-vs-mover touch events, structured save (object-valued,
  session checkpoint), `game.action` runtime input registration.
- `splashgame.md` v2 = the single canonical doc, tier-annotated, loaded into Fable's
  system prompt; local Qwen only ever reads manifests.

## AI tiers (unchanged)

- **Fable (cloud)**: creates/edits games — the only codegen. Existing ClaudeCodeAgent +
  `ag` harness + error push-loop carry over.
- **Agent backend is modular** — makepad_ai already ships claude_code / claude /
  acp / openai / gemini backends; Arcade codes against the Agent trait, never a
  provider. The game-editing protocol is a minimal tool surface (read source /
  submit transaction / read errors) implementable by ANY tool-calling model.
  Per-platform reality: PCs can run agent CLIs (Claude Code etc. — CLI executes its
  own tool loop); Quest/mobile have no CLI → direct HTTP APIs, Arcade executes the
  tool calls itself. Same protocol either way, so a standalone Quest (no PC in the
  room) still creates games. Provider/model/key = per-device config; default small
  edits to cheaper models.
- **BYO API key on direct-API devices**: users connect their own key. Settings flow
  per device: provider picker + key entry — typing keys on Quest is misery, so offer
  QR/pair-from-phone entry. Keys live in the platform keystore (Android Keystore /
  iOS Keychain), are never replicated, never enter game packages or logs. No key =
  device is join/play-only — and in a room with a host, creation requests can still
  route through the host's agent, so BYO-key is only needed for standalone creation.
  Entry path for keyboard-less devices = **LAN paste**: the target device (headset/
  phone) serves a self-contained `/pair` page on the existing http server; headset
  shows "open http://<ip>/pair on the computer with your key" + a 4-digit confirm
  code; paste → POST → keystore. Arcade already assumes a working LAN (multiplayer
  runs on it), so this covers every room Arcade works in. Plaintext-on-trusted-LAN
  is the accepted tradeoff (confirm code prevents pasting to the wrong device).
  NEVER a third-party QR/paste website — that's how keys leak (decided: no QR path
  at all; LAN paste covers every room Arcade works in).
- **Local Qwen** (`libs/converse` QwenFilter): talk-gate (mic → VAD → Whisper → judge →
  only real requests reach Fable) + librarian (load games by description, tweak
  manifest-declared knobs). No local codegen.
- v1: host owns mic + agent; Kokoro TTS out. Endgame: every player's device runs its
  own mic chain + Claude (below).
- **Per-platform capability fallback**: the local models (Whisper/Silero/Qwen via
  makepad-ggml) don't have compute backends everywhere. Where they do (macOS metal,
  …): full voice chain. Where they don't (Quest/phones/web today): plain chatbox
  piped straight to Fable — typing IS the talk-gate, no VAD/judge needed. Detect at
  startup, same ConversePipeline surface either way.

## Collaborative editing (multi-Claude)

Endgame: every player's machine runs a Claude editing the same game, live. Not a
CRDT — text CRDTs guarantee convergence, not correctness (two AIs restructuring
overlapping code char-merge into valid-looking nonsense), and they solve a problem we
don't have (no authority, keystroke-rate concurrency). Our edits are chunky
transactions seconds apart, and the host is already authoritative. Design:

- **Intent log, host-serialized.** A player's Claude submits a transaction:
  `{intent prompt, base generation, diff}`. Host orders them; source = append-only
  generation history.
- **Merge**: disjoint hunks → 3-way auto-merge. Overlapping → semantic rebase: the
  submitting Claude gets "base moved to generation N, re-apply your intent" and
  re-derives its diff — the same AI that wrote the change resolves it (reuses the
  existing error push-loop channel).
- **Soft region leases**: a Claude declares what it's touching ("vehicles"); host
  leases the region; other Claudes queue or pick different work. Splitting a game
  into a few files (world/actors/rules) keeps leases coarse and conflicts rare.
- Accepted transaction = generation N+1 → host evals (last-good rollback as today) →
  eval/runtime errors push back to the proposer, not the room.
- Clients never need source at runtime (host-auth sim), so sync is Claude↔host only.

## Generated assets (later, design for it now)

Games eventually get AI-generated images and 3D models (generative models or
downloads). Not a milestone yet, but the format and engine leave the door open:

- **No big external assets in git** (user rule): stock packs fetch via
  `download_assets.sh` (pinned URLs + sha256, idempotent, dir gitignored); tests
  skip-with-hint when absent; tiny self-made fixtures (<50KB) may be committed.
- **Assets are content-addressed blobs** in the game dir (`assets/<hash>.<ext>`),
  referenced by hash from splash/manifest. Immutable → no merge problem in
  multi-Claude editing, dedup across games, lazy transfer on join (hash check →
  fetch missing), LRU cache on device.
- **Types**: images (textures, billboards/decals, skybox — png/jpg, Quest budget:
  size caps + mip/compress on import), 3D models (glTF — xr has a loader in
  `xr/src/obj/gltf`; audit before promotion, like everything else), later splats.
- **DSL**: `game.image` (billboard/decal/texture-on-shape), `game.model(hash)` on
  entities — collision stays the AABB/prefab shape, model is visual like `shape:` today.
- **Acquisition paths**: (a) Fable requests generation via a provider-pluggable
  makepad_ai tool → blob lands in assets/; (b) download from the registry/asset
  libraries; (c) local gen models when viable. All three land the same blob format.
- **Renderer**: textured entities are new (everything is flat-colored shapes today) —
  slot a textured variant into the forward pass when this lands; keep tier rules
  (assets are content, not state — nothing about them is per-tick).
- Untrusted like everything else: image decoders + glTF parser face downloaded
  content → same fuzz/bounds bar as the zip path.

## Game format + sharing

- Game dir = `game.splash` + `manifest.toml` (name, description, knobs, players
  min/max, MR footprint/scale, thumbnail) + `assets/` (content-addressed, above).
- Package = zip (add writer to `libs/zip_file`).
- LAN: host serves package over `platform/network/http_server`; joiners auto-download.
- Online: dumb HTTP registry (Cloudflare, map-bake precedent) — upload zip + manifest,
  browse/search in-app; Qwen searches local + cached manifests offline.

## Testing strategy (headless-first)

Every crate testable without a window; rendering is the only exception and even that
has a headless path. The Cx-free deterministic sim is what makes this possible:
**two full game instances can run in one test process with a virtual clock.**

- `game_math`: golden-value tables + cross-arch bit-parity fixtures (mac arm64 / x86 /
  quest binary in CI) — the determinism claim IS a test, or it rots.
- `game_sim`: pure crate — invariant tests for every contract (movers-don't-collide,
  0.55 step-up, attach pin, id/generation reuse), tape-replay fixtures asserting a
  per-tick world-state hash, fuzz the command stream (verbs with hostile args).
- `game_blocks`: scenario tests as data — spawn car + scripted inputs → assert lap
  time window, never-flips, stays-on-heightfield. Numeric, no rendering.
- `game_script`: verb table → golden world states; streaming-eval regression suite
  (tail-statement, checkpoint identity, rollback completeness — snapshot leaks get
  tests as they're fixed); per-tick budget enforcement.
- `game_net`: in-process loopback transport with seeded loss/reorder/dup injection;
  host + N client sims in one process → converge, mid-join reconstructs, desync
  checksum fires on intentional divergence. Protocol golden-bytes tests.
- VM fixes: regression tests in `platform/script/test` (isolate GC actually collects —
  heap-flat-after-N-ticks assertion, timer scoping, budget accounting).
- `game_render`: logic split from GPU — slab packing/dirty-set/culling as pure unit
  tests (winding-test precedent in game_view.rs); pixel truth via the existing
  MAKEPAD=headless render-to-png suites (`examples/*/tests` pattern), goldens per
  backend where they differ.
- Integration: the `ag` harness pattern (peek/test/errors) extends to multi-instance —
  scripted 4-player sessions asserted headless; MR/VR shells get render goldens only
  where sim tests can't reach.

## Milestones

- **M0 — extraction**: `game_sim` + `game_render` split out (callback handle slots,
  camera ownership, dirty-set rendering, slotmap entities, I/O off the tick path),
  `game_math` kernel in. VM fixes: GC campaign + per-tick budget. Gate: gamemaker app
  green, tape fixture byte-identical **cross-arch** (mac ↔ quest binary), `sandbox3d`
  perf ≥ today, 1-hour soak with flat heap.
- **M1 — box3d + blocks**: solver swap behind the character-controller contracts;
  car/character/plane + brains as engine components; racing fixture. Gate: sandbox3d
  parity (diffs documented), 6 cars + 100 movers at 60Hz on M-class + Quest spike.
- **M2 — same-room multiplayer**: xr/net audit → `game_net` (promote/rework verdict),
  players API, host+join flat (PC hosts, Mac + phone join), LAN package download.
  Gate: 4-player race, 2 devices + 2 bots, mid-race join, loss/reorder soak test.
- **M3 — XR shells**: Quest MR stage (anchor+scale, shadow catcher, alignment) + VR
  full-scale; Windows OpenXR/D3D11. Gate: Quest joins the M2 race at 72Hz in
  passthrough; same build full-scale on PC VR.
- **M4 — voice + AI**: converse pipeline on host, Qwen librarian, Fable authoring
  in-session. Gate: hands-free "make a racing game" → 4 devices playing it.
- **M5 — multi-Claude co-editing**: intent log + transactions + leases + semantic
  rebase; per-device mic chains. Gate: two machines' Claudes edit one running game
  concurrently, conflicting edits included, no lost intent.
- **M6 — sharing**: zip writer (+ fuzzed reader), registry, in-app browser,
  capability-stripped isolate for downloaded games. Gate: game built on one machine,
  browsed + hosted from another house; hostile-archive + script-escape test suite green.
- **M7 — pretty pass**: SceneSun/T7 unification, projected shadow geometry, particles
  (tier-3), positional audio, Quest perf soak → entity budget doc in Fable's prompt.

## Risks / open

- game_sim extraction touches every dispatch arm — tape fixture is the safety net
  (same-binary until game_math lands, cross-arch after); keep the old path compiling
  until M1 gates pass.
- game_math last-bit parity needs a cross-arch CI fixture early (mac arm64 vs x86 vs
  quest) — determinism claims without the harness rot instantly.
- Quest: stereo 72Hz × a desktop-tuned renderer — spike early (M1 gate, not M3).
- Character-controller-on-box3d parity (movers-don't-collide, step-up feel) is the
  riskiest behavioral change; consider keeping mini-AABB as a per-game compat flag.
- Windows OpenXR: D3D11 binding is new code on a 5.3k-line sys layer — budget real
  bring-up time; Vulkan fallback exists on paper only.
- Touch controls are new; phones are gimped without them.
- No pass-depth sampling anywhere → shadow maps stay off the table; look = blob +
  projected geometry + baked AO (fits Quest anyway).
- Script-heap opacity is load-bearing for join-anytime; if a game hides gameplay in
  script vars against the tier docs, late joiners see ghosts — lint via `ag`.
- iOS/tvOS: flat only; no visionOS.
