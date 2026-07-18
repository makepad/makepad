# aigame — moving the AI Game Maker from Godot to Makepad

Plan for replacing the Godot backend of the kids' Game Maker (`examples/godot`) with a
small, AI-generatable game engine running **inside makepad itself**, built from three
things we already have: the **splash script VM** (hot-reloadable, per-isolate), the
**box3d** physics engine, and the **xr** 3D scene framework. Written 2026-07-09.

---

## 1. What we have today (the Godot version)

**Where games go:** `~/games/<name>/` (override with `GAMEMAKER_HOME`), one Godot 4.7
project per game. The app owns `tools/` (agent harness) and `CLAUDE.md`; the kid's game
is `project.godot` + `scenes/*.tscn` + `scripts/*.gd`. Per-game app state (chat log,
Claude session id, model choice) lives in `<game>/.gamemaker/`.

**The loop:** kid holds F1 and talks (Whisper) or types → transcript goes to Claude Code
(via `makepad_ai::ClaudeCodeAgent`, tool policy locked to Read/Glob/Grep/Edit/Write +
`tools/gd` only) → Claude edits GDScript/tscn files → the app keeps the kid on the
last-good running game and relaunches it only when the turn completes.

**What the AI actually builds** (from the real `~/games/my-game`, ~2600 lines generated):

- Everything is **procedural colored boxes** — no image/model/sound assets at all
  (enforced by the system prompt). 2D: `StaticBody2D`+`ColorRect` ground segments,
  platforms, moving platforms, walls, a goal flag. 3D: cube ground/stairs/towers/trees,
  a gold goal block, box-people.
- **Character controllers**: `CharacterBody2D/3D` + gravity + jump + `move_and_slide`,
  respawn on fall, mount/dismount vehicles.
- **Behaviors**: per-frame `_physics_process(delta)` steering (villagers chase, soldiers
  patrol), group queries (`get_nodes_in_group("vehicle")`), distance checks, spawn/despawn,
  timers (`create_timer`), win/lose conditions.
- **HUD**: `Label`s toggled visible ("You win!", "Caught!").
- **Input** through named actions only (`ui_left/ui_right/ui_accept`) so keyboard, input
  tapes, and gamepad (AgentEye binds the A button) all work through one vocabulary.

This is the complete capability envelope a kids' game engine needs. It is small.

## 2. How the agent remote-operates the game and looks at pictures

Three mechanisms, all file-based (Claude only has `tools/gd` as a shell surface):

| Verb | Mechanism | What the AI gets |
|---|---|---|
| `gd peek` | Drops `.agent/peek_request`; the **AgentEye autoload** in the *live* game polls (250ms), grabs 4 viewport screenshots over ~1.2s via `get_viewport().get_texture().get_image()`, writes player pos/vel/`is_on_floor()` | `.agent/sheet.png` + `state.txt` — sees what the kid sees, zero interruption |
| `gd shot <scene> [frames] [tape]` | Second Godot instance boots `tools/harness.tscn`; harness loads the target scene, **replays a JSON input tape by frame number** (`{"f":30,"press":"ui_accept"}` → `Input.action_press`), prints `[probe]` pos/vel/floor lines every 15 frames; recorded with `--write-movie --fixed-fps 60` (deterministic: same tape ⇒ same frames). Since 2026-07-09 it launches via `open -g` + an unfocusable offscreen window so it never steals the kid's focus | contact sheet + numeric probe log — "the jump feels floaty" becomes a number |
| `gd errors` | Greps the run log for `SCRIPT ERROR` / `ERROR:` | error text |

**The pictures:** `tools/sheet.py` tiles N evenly-spaced frames into one labelled contact
sheet (`.agent/sheet.png`) so the agent reads *one* image showing motion over time instead
of 120 frames.

**Weaknesses inherent to the Godot backend** (what the migration removes):

1. **Separate process.** Applying `.tscn` changes needs a full game restart (state loss,
   focus management, pid babysitting, zombie processes). We just spent a day making this
   tolerable; in-process it disappears entirely.
2. **Capture needs a real (hidden) window** — Godot's `--headless` crashes with
   `--write-movie`. Makepad has a true headless CPU renderer.
3. **Opaque runtime.** The only introspection is print statements the harness happened to
   include; errors come from log-grepping.
4. **Two unfamiliar languages** (GDScript + tscn) and a giant API surface the model can
   misuse. A curated DSL of ~25 constructs is easier to prompt and to verify.

## 3. Building blocks in makepad (surveyed 2026-07-09)

### 3.1 Splash script VM — the hosting/hot-reload story is already built

- `widgets/src/splash.rs`: the `Splash` widget evaluates DSL **strings** in a dedicated
  isolate VM (`cx.alloc_splash_vm_with_network`), with instruction limits
  (`with_instruction_limit(200_000, …)`), and re-evaluates **incrementally** via
  `eval_with_append_source` parser checkpoints — this is how aichat streams a growing
  `runsplash` block into a live widget. Per-isolate `let`/`fn` state persists across events.
- The DSL is a real language: `let`/`fn`, templates, `for`, `if/else`, closures,
  `on_click`-style handlers, struct arrays, `promise()/.await()`, HTTP. `splash.md` is the
  authoring manual the AI already follows; `examples/splash_preview/` is an offline
  generate-and-verify corpus harness (drives the `claude` CLI, evals every generated app,
  flags empty widget trees).
- **Gaps for games:** no script-facing frame tick or timers, no keyboard events routed
  into isolates, and eval errors are *not* fed back (a broken block renders blank; errors
  only reach stderr via `ScriptVm::drain_errors`). All three are core workstreams below.

### 3.2 box3d — the physics core (`libs/box3d`, pure Rust port of Erin Catto's Box3D)

- Shapes: sphere, capsule, convex hull (`make_box_hull`), triangle mesh, heightfield,
  compound. Bodies: static/kinematic/dynamic. Full joint set. Sensors with begin/end
  touch **events**, contact/hit events, ray/shape casts, explosions, wind.
- A **kinematic character controller** (`mover.rs`, `world_collide_mover`,
  `world_cast_mover`) — exactly what the player/NPC vocabulary needs.
- **Bit-exact deterministic** across architectures and worker counts, faster than Rapier
  on 8/9 benchmark scenes, with a built-in **snapshot + record/replay** substrate
  (hash-exact). This upgrades the whole verify story: same tape ⇒ same *simulation*, not
  just same frames.
- Consumer API is flat free functions (`create_world`, `create_body`,
  `create_hull_shape`, `world_step`, `body_get_transform`). **Rust-only today — zero
  script bindings.** That binding layer is the single biggest work item.
- `examples/box3d/src/main.rs` already shows the renderer we need: instanced lit
  boxes/spheres (`DrawPhysMesh` script-shader, per-instance color+transform), an
  `XrCamera` orbit, `NextFrame`-driven stepping. ~600 lines, self-contained.

### 3.3 makepad-xr — the 3D scene layer (`xr/`, crate `makepad-xr`)

- `XrNode` scene graph (pos/rot/scale, physics body kind, children), object library
  (`Cube`, `IcoSphere`, `Gltf`, `FractalTree`, splats), behaviors in Rust (`Tank`, `Car`,
  `Shooter`), 2D-UI-on-a-plane (`XrView` — a free HUD system), PBR-ish shading.
- **Worlds are authored in the script DSL and hot-reload** (`XrNode` handles
  `apply.is_reload()`; `on_render` closures rebuild geometry live) — proof that
  "AI edits script → live 3D scene updates" already works in this codebase.
- Desktop fallback: orbit camera + **gamepad** gameplay (`cx.game_input_states()`).
  Quest: hands, passthrough, depth-scanned colliders, multiplayer (`xr/net`).
- Caveats: physics is **Rapier3D** (not box3d), **no audio**, **no keyboard gameplay
  input**, and behaviors are compiled Rust (only the scene layer is scriptable).

### 3.4 Remote-operate substrate we already proved this session

- **Headless renderer**: `MAKEPAD=headless` builds render real frames to PNG on CPU with
  JIT-compiled shaders (`--draws=N`, `MAKEPAD_HEADLESS_OUT_DIR`). We used it today to
  pixel-verify a widget fix. This replaces the hidden-window capture instance outright.
- **makepad_test** (`libs/makepad_test`): selector-driven UI automation (click/fill/
  wait_text) against a headless app instance, with failure screenshots — the skeleton of
  an input-tape runner.
- **Studio hub bridge**: `WidgetTreeDump` / `Click` / `Screenshot` RPC into a running
  app — the skeleton of `peek` without file polling.

## 4. The aigame engine — design

**Principle: keep the exact product shape** (voice → Claude edits files → kid keeps
playing until the AI is happy → instant apply), swap the engine underneath. The Game
Maker app shell (chat, Whisper, TTS, sessions, model picker, relaunch policy) is reused
as-is; only "relaunch Godot" becomes "hot-swap the game script".

### 4.1 Architecture

```
~/games/<name>/game.splash          ← the file(s) Claude edits (Read/Edit/Write, same as now)
        │  (file watch)
GameMaker app ─── GameHost widget   ← owns a splash isolate + a box3d World + fixed 60Hz tick
        │             │
        │             ├─ shadow-eval on change: new isolate, eval, 1 smoke tick
        │             │    ├─ clean  → swap in (kid sees change in <1s, mid-play)
        │             │    └─ errors → keep last-good running, errors go to the agent
        │             ├─ renderer: instanced boxes/spheres/capsules (from examples/box3d)
        │             ├─ HUD: plain makepad widgets overlaid (or XrView in 3D)
        │             └─ ActionMap: keyboard + gamepad + tape → named actions
        └─ agent harness (tools/ag): test / peek / errors — see §5
```

- **One process.** The game is a widget in the Game Maker window (optionally poppable
  into its own window later). No pids, no focus stealing, no restart.
- **"It works" state, upgraded:** today the kid keeps the old *process*; here the
  last-good *isolate + world* keep running while the new source shadow-evals. A turn that
  ends broken never even flickers the kid's game — and the AI gets the error text
  immediately instead of a blank screen (fixes the Splash blank-on-error gap).
- **Fixed timestep** (1/60, 4 substeps, like the box3d example) for determinism; render
  interpolation optional later.

### 4.2 The script surface the AI writes (curated, not raw bindings)

Bind a small **game vocabulary** into the isolate rather than exposing raw box3d — a
~25-construct API is easier to prompt, to sandbox, and to keep stable. Sketch (syntax
illustrative, follows splash rules):

```splash
// game.splash — everything the AI edits lives here
let SPEED = 240.0
let JUMP = 520.0

fn build_world() {
    game.gravity(vec3(0, -30, 0))
    game.box{pos: vec3(0, -1, 0) size: vec3(120, 2, 8) color: #x3a8f4a}       // ground
    for i in 0..6 {
        game.box{pos: vec3(10 + i * 8, i * 2, 0) size: vec3(4, 1, 4) color: #x8a6a3a}
    }
    game.box{pos: vec3(58, 13, 0) size: vec3(2, 2, 2) color: #xf5c13a tag: "goal" sensor: true}
}

player := game.mover{pos: vec3(0, 2, 0) size: vec3(1, 2, 1) color: #x4466aa lock_z: true}

npc := game.mover{pos: vec3(30, 2, 0) size: vec3(1, 2, 1) color: #xaa4444 lock_z: true
    on_tick: |dt| { self.walk_towards(player.pos(), 3.0) }
}

player.on_tick: |dt| {
    self.walk(input.axis("left", "right") * SPEED * dt)
    if input.pressed("jump") && self.on_floor() { self.jump(JUMP) }
    if self.pos().y < -20 { self.teleport(vec3(0, 2, 0)) }
}

game.on_touch: |a, b| {
    if a.tag() == "goal" || b.tag() == "goal" { ui.hud_win.set_visible(true) }
}

game.camera.follow(player, side_2d: true)
```

Vocabulary checklist, derived 1:1 from what the Godot corpus actually used:

| Corpus need (Godot) | aigame construct | Backed by |
|---|---|---|
| ColorRect/box world building | `game.box/sphere/capsule{...}` (static/dynamic/kinematic, color, tag, sensor) | box3d shapes + instanced `DrawPhysMesh` |
| CharacterBody + move_and_slide | `game.mover{...}` + `walk/jump/on_floor/teleport` | box3d `mover.rs` character controller |
| `_physics_process(delta)` | `on_tick: \|dt\| {...}` per entity + `game.on_tick` | fixed-step pump into isolate |
| groups / `get_nodes_in_group` | `tag:` + `game.find("tag")`, `e.distance_to(x)` | engine-side registry |
| Area2D / goal triggers | `sensor: true` + `game.on_touch` | box3d sensor events |
| moving platforms | kinematic body + `on_tick` setting velocity | box3d kinematic |
| vehicles/mount | `attach/detach` (weld joint or parent) | box3d joints |
| HUD labels | plain widgets over the viewport (`ui.hud_win`…) | makepad widgets (free) |
| respawn / timers | `teleport`, `game.after(secs, \|\| {...})` | engine timer wheel |
| input actions | `input.pressed("jump")`, `input.axis(..)` | ActionMap (§4.3) |
| 2D platformer | `lock_z: true` + `side_2d` camera | box3d motion locks / parallel joint — **verify which; fallback: post-step plane clamp** |
| win/lose sounds | `game.beep{...}` synth SFX | `cx.audio_output` (already used for TTS) |

### 4.3 Input: one ActionMap for keyboard, gamepad, and tapes

Mirror the Godot design that made tapes+controllers free: script code only ever sees
named actions (`left/right/jump/up`). The engine maps arrow keys/WASD + gamepad
(`cx.game_input_states()`, as xr's Tank does) + **tape events** onto the same names.
Makepad has full keyboard events (`Event::KeyDown`/`KeyCode`) — xr just never wired them;
we wire them in the GameHost, not in xr.

### 4.4 Renderer choice

**Phase 1: lift `examples/box3d`'s renderer** (instanced lit unit-cube/sphere +
`XrCamera`, ~200 lines) into the GameHost. It draws exactly the corpus art style.
**Later: converge with xr** — adopt `XrNode`/`XrView` for scene+HUD and port xr's physics
from Rapier to box3d (justified independently: box3d is faster on 8/9 scenes and
deterministic; one physics engine in the tree instead of two). That convergence buys
Quest/hands/multiplayer for the *same game scripts* — a kid's game playable in VR — but
it is explicitly not on the critical path.

## 5. The agent harness on makepad (remote-operate, tier by tier)

Same three verbs, better substrate. `tools/ag` (or a `gd`-compatible shim so the prompt
barely changes):

| Verb | Godot today | aigame |
|---|---|---|
| `ag test [frames] [tape]` | hidden Godot instance, --write-movie, probe prints | **headless run of the same GameHost** (`MAKEPAD=headless`): eval `game.splash`, feed the tape into the ActionMap, `world_step` N frames, render PNGs on CPU, emit probe lines (engine reads pos/vel/on_floor directly — no print statements needed). box3d determinism ⇒ bit-exact repeatability, stronger than Godot's fixed-fps movie |
| `ag peek` | file-RPC into live game (AgentEye), viewport screenshots | in-process: the app screenshots its own game pass texture + dumps entity state on request (file trigger kept for CLI compat, or a local socket). Kid keeps playing, same as now |
| `ag errors` | grep run logs | **drain the script VM error queue** — precise parse/runtime errors with line numbers, returned as text. Also auto-attached to the turn when a shadow-eval fails, so the AI often self-corrects *without* running anything |
| pictures | `sheet.py` contact sheet | keep `sheet.py` verbatim (it's engine-agnostic: dir of PNGs → one labelled sheet) |
| tapes | JSON `{"f":N,"press":"ui_accept"}` | same format, actions renamed; probe list = tags |

Bonus unlocked by box3d: `ag test --record` / snapshot scrubbing — the AI can capture a
deterministic recording once and re-probe it at different frames without re-running.

## 6. Changes to the Game Maker app (small)

- `play_game()`/`relaunch_if_pending()` → `GameHost::reload(path)` (shadow-eval + swap).
  The turn-completion policy from 2026-07-09 stays: kid keeps last-good until the AI is
  happy — it just gets cheaper (no process restart, sub-second apply).
- System prompt + per-game `CLAUDE.md`: rewritten against the aigame DSL; ship a new
  **`aigame-dsl.md`** authoring guide (the `splash.md` equivalent for games: the
  vocabulary table, tick/input/tape rules, the `#x` hex rule, worked platformer example).
- Template: `~/games/<name>/game.splash` starter + `tools/ag` + tapes. `refresh_harness`
  (added today) already re-stamps tools on project switch.
- Permission policy shrinks: Claude gets `Edit/Write(./**)` + `Bash(./tools/ag:*)` only.

## 7. Phases and milestones

**Phase 0 — proof of loop (the risky part, do first)**
Script-bind the minimum box3d surface (world/step, box+sphere bodies, transforms) into a
splash isolate; GameHost widget with fixed tick, keyboard ActionMap, `on_tick` dispatch
into script closures; lift the box3d example renderer.
*Milestone:* a ~100-line `game.splash` platformer (locked z) runs at 60fps and hot-reloads
on file save without dropping world state of the running instance until swap.
*Verify here:* per-tick script-call overhead with ~20 entities × 60Hz under instruction
limits; box3d 2D locking mechanism; headless JIT compiles `DrawPhysMesh` (we fixed the
scalar-cast JIT bug today — same risk class).

**Phase 1 — the corpus vocabulary**
Mover controller, sensors → `on_touch`, tags/queries, spawn/despawn, timers, camera
follow (side-2D + third-person), HUD overlay widgets, gamepad, synth SFX (`game.beep`).
*Milestone:* hand-port `~/games/my-game` (both the 2D level and the 3D chase sandbox) to
`game.splash` — the real generated corpus is the acceptance test for vocabulary
completeness.

**Phase 2 — the agent loop**
Headless `ag test` (tape → frames → probe → sheet), `ag peek`, VM-error round-trip +
shadow-eval, Game Maker integration, new prompt + `aigame-dsl.md`, template swap.
*Milestone:* end-to-end kid session: "make him jump higher" → edit → self-test headless →
turn completes → game hot-swaps mid-play; zero focus steal; broken edits never reach the
kid and come back to the AI as line-numbered errors.
*Regression harness:* splash_preview-style batch corpus — a set of recorded kid requests
run through the real `claude` CLI against `aigame-dsl.md`, each result eval-checked and
tape-smoke-tested headlessly.

**Phase 3 — convergence and reach**
Port `makepad-xr` physics Rapier→box3d; host aigame scenes on `XrNode`/`XrView`; the same
`game.splash` then runs on Quest (hands/passthrough) and inherits xr multiplayer.
Optional: box3d record/replay scrubbing in the harness; state-preserving hot reload via
world snapshots.

## 8. Open questions / decision log

1. **Curated `game.*` API vs raw box3d bindings** — recommended: curated (smaller prompt,
   stable across engine refactors, sandboxable). Raw bindings can come later for power use.
2. **2D story** — one engine (3D + locked axis + orthographic-ish side camera), not a
   second 2D engine. Needs the motion-lock verification in Phase 0.
3. **Where behaviors live** — corpus says script-side `on_tick` closures suffice (steering
   is ~10 lines); compiled-Rust behaviors (xr's Tank pattern) stay an escape hatch for
   things script is too slow for.
4. **Splash-in-chat vs GameHost** — games do NOT run as `runsplash` chat blocks; the
   GameHost is a dedicated widget with its own isolate, tick, and input focus. Chat
   blocks stay for the AI showing UI snippets.
5. **Error feedback for aichat generally** — the VM-error round-trip built for aigame
   (drain_errors → agent) should be upstreamed to the `Splash` widget too; blank-on-error
   hurts every runsplash use case.
