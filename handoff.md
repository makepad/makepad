# handoff — the Makepad AI Game Maker (godot-like livecoding agent)

State as of 2026-07-10. Everything below is in the working tree, **nothing committed**.
Written as the handoff for `examples/gamemaker`: a voice-driven game maker for kids
where Claude Code live-edits a single splash script and the game hot-reloads in-process
— the replacement for the Godot-backed `examples/godot` (which still exists, untouched,
as the reference implementation).

## 1. What it is

```
kid holds F1, talks (Whisper) ──► Claude Code (via makepad_ai::ClaudeCodeAgent)
        ▲                              │ edits ~/games/<name>/game.splash
        │ TTS speaks replies           ▼
Game Maker app ── chat pane │ GameView pane: splash isolate + fixed-60Hz world,
                            │ re-evals on every edit (aichat-style streaming),
                            │ broken edits never replace the running world
```

- One process. No engine restarts, no focus stealing. A clean edit is visible mid-turn
  ("the world grows while the AI talks"); a broken edit keeps the last-good world
  running and returns line-numbered errors to the agent.
- The whole game is ONE file, `game.splash`, written in a curated ~45-verb `game.*`
  DSL (deliberately smaller than GDScript — easier to prompt, verify, and keep stable).
- The agent self-verifies through `tools/ag`: screenshots, deterministic input-tape
  playtests with numeric probes, error/log round-trip.

## 2. Engine architecture (examples/gamemaker)

**Host** (`src/main.rs`, cloned from the godot example): chat/voice/TTS/session shell,
per-game state in `<game>/.gamemaker/` (chat log, Claude session id, model), project
dropdown over `~/games/*/game.splash` (`GAMEMAKER_HOME` overrides), harness re-stamped
into each game on switch (`tools/`, `CLAUDE.md` — app-owned, kid files never touched).
System prompt teaches kid-talk rules + the DSL + the always-check-errors workflow.
Permission policy: `Edit/Write(./**)` + `Bash(./tools/ag:*)` only.

**GameView** (`src/game_view.rs`, ~2k lines — splitting into modules was repeatedly
deferred, do it before it grows again):
- **Script hosting**: dedicated splash isolate (`cx.alloc_splash_vm_with_network`),
  incremental re-eval via `eval_with_append_source` parser checkpoints (the same
  machinery aichat uses to stream `runsplash` blocks), instruction limits (2M per
  eval / 500k per tick), pointer-stable ScriptMod keying. Reload triggers: 250ms mtime
  watch + immediately on the agent's Edit/Write ToolRequest completing.
- **Last-good semantics**: eval snapshots the world (entities, parts, labels, terrain,
  sky, HUD), rebuilds from scratch, rolls back wholesale on failure. Errors go to
  `.agent/game.log` + `.agent/last_error.txt` (cleared-to-empty on success BY DESIGN —
  "empty = your edit is live") via a platform addition: `ScriptVm::take_errors` +
  `captured_errors` sink (streaming evals otherwise silence errors).
- **Dispatch design**: one native `game` handle over `Rc<RefCell<GameWorld>>`; every
  `game.verb(...)` is a match arm in `game_dispatch` — synchronous mutation, no async
  widget trampoline, deterministic ordering. Adding a verb = one arm + one doc row.
- **Physics**: deliberately tiny — gravity + axis-separated AABB sweeps, kinematic
  carry, sensors, mover ground detection, terrain height-lookup collision with 0.55
  step-up. **The physics body NEVER rotates — only the visual model yaws.** This
  matches Godot exactly (their CharacterBody3D does the same); it is the reason a
  mini-engine reaches parity. box3d swap remains a marked seam (see §5).
- **Renderer**: offscreen 3D pass composited into the pane. Shaders (script-DSL,
  JIT-compiled on headless): `DrawGameCube` (instanced lit boxes + emission + fog),
  `DrawGameAlpha` (translucent pass: sensors, water, blob shadows), `DrawGameSky`
  (gradient dome), `DrawGameTerrain` (triangulated heightfield, per-vertex color).
  Near plane **1.0** (Godot's CAM_NEAR) so lens-overlapping creatures clip open
  instead of filling the screen. Billboard labels project into a 2D overlay with
  automatic 4-copy dark outlines; HUD slots + crosshair are plain overlay draws.
- **Input**: ActionMap — keyboard (WASD/arrows, Space, F=shoot, G=grab) + gamepad
  (`cx.game_input_states`: stick analog w/ deadzone, dpad, A=jump/X=shoot/B=grab,
  edge-detected) merged at read time via `PadState` (never into the key-held set, so
  devices can't cancel each other). Input snapshot per tick: held/pressed booleans,
  raw `axis_x/axis_z`, **camera-relative `move_x/move_z`** (the canonical walk input;
  the axes rotated by effective camera yaw — derived from the camera basis, and the
  fix for "controls don't match the camera": the first documented recipe rotated by
  −yaw). Tapes zero the pad and pin yaw+pitch → byte-identical replays.
- **Audio** (`src/synth.rs`): polyphonic synth (osc + percussive envelope, 24-voice
  cap) mixed into the app's audio callback under the TTS voice; mute button silences
  both. Named bank (18: jump shoot zap grab angry calm rescue shove board coin hurt
  win lose squeak roar bark moo clank whip) + `game.beep{}` + `game.jingle("C5 E5")`.
  The Godot corpus AI hand-built a synth in GDScript when it had no audio API —
  that's why this is an engine service.
- **Agent harness** (`tools/ag` + file RPC through `.agent/`): `peek` (4 live frames
  via `Cx::capture_next_frame_to_file` — a platform addition riding the studio
  screenshot pipeline — + entity state), `test N tape` (restart, frame-indexed tape
  through the ActionMap, captures + `probe.txt`, `sheet.py` contact sheet), `errors`,
  `logs`. Tape format is the Godot harness's JSON unchanged. Errors are also
  PUSHED into the agent chat (GameViewAction → auto fix wake-up on post-turn broken
  evals and idle runtime errors; 2-wake-up guard, reset by a kid message;
  GAMEMAKER_NO_AGENT=1 disables the agent for token-safe headless tests). Engine
  registration errors surface in the status bar, never in the kid chat.

**DSL surface** — the authoritative, always-in-sync docs:
- **repo-root `splashgame.md`** — THE agent-facing API contract, loaded into the
  system prompt at runtime (fs read + include_str fallback), exactly the pattern
  aichat uses for `splash.md`. Adding a verb = dispatch arm + a row here.
- `resources/template/CLAUDE.md` — per-game workflow only (ag ritual, house style,
  gotchas); points at the system prompt for the API.
- `resources/aigame-dsl.md` — developer guide (execution model, extension points).
Spawning: `box/mover/spawn/terrain/part/label`. Driving: `on_tick(dt, input)`, `walk/
jump/on_floor/pos/vel/set_pos/set_vel/face/yaw`, `find/tag/distance`, timers `after`,
`on_touch`. Look: `set_color/glow/scale/move_part/sky/camera` (orbit / `side:` /
`third_person:` with occlusion pull-in ignoring `"scenery"`). Systems: `attach`
(seat + `mode:"ride"` w/ spin)/`detach`, `speed_mult`, `beam`, `ground_y/ground_peak`,
`rand/rand_range` (seeded per eval — tape-deterministic, better than the corpus's
`randomize()`), HUD `text(slot,…)/crosshair`, labels w/ ids + `label_text`, sounds.
Terrain noise is shaped engine-side (`freq/offset/amp/step/min/max/plaza`, ≤384
cells, `bands:` height-color thresholds — snow bands are what make hills read as
mountains) so big worlds cost no script instruction budget.

## 3. How parity was reached (method matters as much as the result)

1. **Corpus first**: the AI-generated Godot game (`~/games/my-game`, grew to ~44
   creatures) was inventoried exhaustively (`aigame_port_inventory.md`), and every
   engine feature exists because the corpus used it — nothing speculative. Confirmed
   non-needs (no tweens/particles/navmesh/shaders in 2600+ generated lines) were
   deliberately NOT built.
2. **Port as acceptance test**: the game was ported to
   `resources/fixtures/sandbox3d.splash` (932 lines, 90 entities — ~2100 lines of
   GDScript; the compression is the argument the DSL is at the right altitude).
   Each port round produced findings (`aigame_port_findings.md`); findings became
   engine verbs; the fixture re-ported until clean. Gap analysis lives in
   `aigame_parity_gap.md`; the original plan in `aigame.md`.
3. **Verification discipline**: headless eval cleanliness + numeric probes against
   Godot ground truth (plaza floor exactly 7.9, walk exactly 6.0, jump apex JUMP²/2G,
   ride-debuff exactly 3.0) + determinism (two tape runs byte-identical). Pixels on
   GPU were checked by rik; pure-headless pixels are blocked on §5.5.

The fixture runs the CURRENT game: Giant DogDay guardian (intercept-charge, beam
bonk-arc), 3 headcrabs (leap→latch→speed debuff→jump to shake off), the Prototype
weeping-angel (LOS dot vs camera), Baba Chops (glow-ramp fire eyes, ram), Nightmare
Huggy (arm-reach via move_part), CatNap sleep-curl (scale), Kissy bodyguard, heal
quest, 10 farm animals (per-kind sfx pitch), trucks + passengers, grapple hand on G
(terrain yank / creature haul, beam cable), 257×257 smooth terrain at Godot's exact
constants with banded snow mountains, sky/fog/blob shadows, crosshair/hint/flash HUD.
Installed for rik as `~/games/dogday-world`.

## 4. Infrastructure built along the way (reusable beyond gamemaker)

- **Headless render/test pipeline**: `MAKEPAD=headless` builds render frames to PNG
  via CPU raster + JIT-compiled Rust shaders (`--draws=N`,
  `MAKEPAD_HEADLESS_OUT_DIR`); warm cache at `examples/splash/target`. gamemaker has
  its own `build.rs` mirroring the platform's env→cfg wiring.
- **Three JIT shader-compiler bugs fixed** (platform/script + headless preamble):
  scalar casts `u32(x)` → `as` casts; `Mat4f*Mat4f` missing in the runtime preamble;
  heterogeneous constructors splat-padding because `ShaderType::Id` args weren't
  scope-resolved. Regression stages 4k/4l in `platform/script/test/src/main.rs`.
- **`ScriptVm::take_errors` + `captured_errors` sink** (platform/script/src/vm.rs) —
  the error round-trip primitive; should be upstreamed into the `Splash` widget too
  (aichat runsplash blocks still render blank on error).
- **`Cx::capture_next_frame_to_file`** (platform/src/os/cx_shared.rs + metal path) —
  in-app window capture without the studio bridge.
- **xr on box3d**: the whole `makepad-xr` crate ported off Rapier3D (raycast vehicle
  reimplemented in `xr/src/scene/raycast_vehicle.rs`; 4 pre-existing test failures
  match the Rapier baseline exactly). box3d `body_set_mass_data` stale
  world-inverse-inertia bug found + fixed with a regression test.
- The **godot example harness** got the no-focus-steal treatment first (open -g,
  unfocusable offscreen capture window, last-good relaunch policy) — kept as-is.

## 5. Remaining work for FULL parity (ranked)

1. **Caves / overhangs** — the only dropped world feature. Heightfields can't carve.
   Options: engine `game.tunnel(a, b, r)` laying rock-slab roofs (matches the Godot
   `_carve_caves` approach — it lays roof slabs too, it does NOT boolean-carve), or
   accept boxes-as-caves authored by the AI.
2. **`game.raycast(from, dir, {mask})`** — the grapple currently probes terrain
   height only; it can't catch trees/boxes/creatures mid-flight, and ledge-probe AI
   (the corpus's no-navmesh pathing trick) can't be written. One verb unlocks both.
3. **Real shadow maps** — blob shadows ground creatures but the 21:54 Godot capture
   has directional shadows. A single-cascade sun map over the play area is enough.
4. **Camera-overlap creature fade** — near-clip 1.0 fixed the giant-polygon fill;
   inside a crowd you can still sit within a body. Fade entities whose AABB
   intersects a small camera sphere (Godot mitigates via its spring-arm feel).
5. **Headless offscreen-pass compositing** (task #9) — box3d example renders nothing
   headless; gamemaker's pane is blank in pure-headless frames (GPU capture path is
   fine). Blocks CI-grade visual verification of `ag test` sheets. Look at
   `platform/src/os/headless/event_loop.rs` pass scheduling + overlay draw lists +
   NextFrame-only apps never drawing.
6. **box3d under the verbs** — the mini-AABB physics is the deliberate seam
   (`TODO(aigame)` in game_view.rs). Swap when games need slopes-with-momentum,
   stacking, ragdolls, vehicles-with-suspension. box3d is deterministic and already
   in-tree; keep the tape guarantees.
7. **The 2D side-scroller** (`main.tscn`) was never ported — `side:` camera + boxes
   cover it in principle; port it as a second fixture to harden 2D ergonomics
   (AnimatableBody movers + Camera2D limits analogues).
8. **Engine polish debt**: terrain band colors interact with auto-shade (bands win —
   fine, but no slope shading within a band); `game.terrain` columns mode still
   spawns per-cell entities (instanced draw + height collision would retire it);
   part transforms have no rot lerp shortest-path handling; gamepad stick vertical
   sign untested on hardware (one-line flip if inverted).
9. **Perf headroom**: 90 entities × parts ≈ fine; the tick budget (500k instructions)
   fits the fixture's ~15 actors of AI — a 100-creature brawl will need either budget
   raise or engine-side steering helpers (`walk_towards`, `flee`) which the corpus's
   shared `_drive()` suggests anyway.
10. **Future (from aigame.md phase 3)**: host the scene layer on `XrNode` so the same
    game.splash runs on Quest (xr is on box3d now, and has hands/multiplayer);
    upstream the error round-trip to `Splash`; batch corpus regression harness
    (splash_preview-style: run recorded kid asks through the real CLI against the
    DSL guide, eval + tape-smoke each result).

## 6. Run / verify cheatsheet

```bash
# the app (from the makepad repo root; Whisper model resolves from CWD)
cargo run -p makepad-example-gamemaker --release
# rik's install: ~/games/dogday-world (set as .last), full fixture

# headless smoke of any game dir
MAKEPAD=headless CARGO_TARGET_DIR=examples/splash/target \
  MAKEPAD_HEADLESS_OUT_DIR=/tmp/frames GAMEMAKER_HOME=<games-root> \
  cargo run -p makepad-example-gamemaker --release -- --draws=6
# then: <game>/.agent/game.log  ("eval #N: ok, E entities"), last_error.txt empty
# AND count startup script errors: ... 2>&1 | grep -cE "^\[E\]" must be 0
# (shader/widget REGISTRATION errors print as [E] lines but do not fail the run —
#  a missed [E] once shipped a build with the terrain shader dead)

# agent-side playtest, in a game dir (app running)
./tools/ag errors | logs | peek | test 200 tools/tapes/<tape>.json
# sheet: .agent/sheet.png   probes: .agent/probe.txt   (byte-identical across runs)

# engine tests touched by this work
cargo run -p makepad-script-test --release        # shader codegen stages 4k/4l
cargo test -p makepad-box3d --release             # incl. mass-data regression
cargo test -p makepad-xr --release                # 127 pass / 4 pre-existing
```

## 7. Document index

| doc | what |
|---|---|
| `aigame.md` | the original migration plan (Godot → splash), phases + rationale |
| `aigame_port_inventory.md` | exhaustive Godot API usage of the generated corpus |
| `aigame_port_findings.md` | port findings v1+v2: API cleanups, fidelity ledgers, open gaps |
| `aigame_parity_gap.md` | the final gap matrix (current game vs engine) + fork specs |
| `examples/gamemaker/resources/aigame-dsl.md` | developer guide to the DSL + engine internals |
| `splashgame.md` (repo root) | THE agent-facing API contract, loaded into the system prompt (keep in sync!) |
| `examples/gamemaker/resources/template/CLAUDE.md` | per-game workflow stamped into each project |
| this file | orientation + what's left |
