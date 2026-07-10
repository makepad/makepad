# The aigame DSL — authoring guide

The Game Maker's games are single `game.splash` files evaluated live inside the
app by `GameView` (examples/gamemaker/src/game_view.rs). This is the canonical
guide to the script surface; the per-game `resources/template/CLAUDE.md` is the
agent-facing copy stamped into every game project (keep the two in sync — the
template is what the AI actually reads).

## Execution model

- The file is a **program**, not a widget tree: statements run top to bottom
  inside a dedicated splash isolate, with a native `game` handle injected.
- Every edit re-evaluates the WHOLE file incrementally (`eval_with_append_source`
  parser checkpoints — the same machinery aichat uses to stream `runsplash`
  blocks). Evaluation rebuilds the world from scratch; a clean eval swaps in
  live, a failed eval is rolled back and the previous world keeps running
  ("last good"). Errors land in `.agent/last_error.txt` + `.agent/game.log`.
- After the build, the engine ticks at a fixed 60Hz: script `on_tick` closures
  first, then timers, then physics (gravity + axis-separated AABB sweeps),
  then sensor overlap events.

## Vocabulary

See the template CLAUDE.md for the full API table the AI is taught. Summary:

- **Spawn**: `game.box({pos, size, color, tag, sensor, body})`,
  `game.mover({pos, size, color, tag, gravity})` → entity id (number).
  Bodies: static (default), `"kinematic"` (script-moved platform, carries
  riders), mover (gravity + collision).
- **Drive**: `game.on_tick(|dt, input| ...)`; `input` = `{left,right,up,down,
  jump,jump_pressed,shoot,shoot_pressed,grab,grab_pressed,axis_x,axis_z,
  move_x,move_z}` snapshot (no cross-boundary calls in the hot path).
  `move_x/move_z` are the axes rotated by the effective camera yaw (0 for
  `side:` cameras) — the canonical walk input. Keyboard (F=shoot, G=grab) and
  gamepad (stick analog + dpad, A=jump, X=shoot, B=grab, merged at read time
  via `PadState`, tape runs zero it) both feed everything. Terrain noise is
  shapeable engine-side (`freq/offset/step/min/max/plaza`, cells ≤ 384) with
  `ground_y(x,z)`/`ground_peak()` queries; labels draw with automatic dark
  outlines. Verbs: `walk`, `jump`, `on_floor`, `pos`, `vel`, `set_pos`,
  `set_vel`, `set_color`, `remove`, `find(tag)`, `tag(id)`, `distance(a,b)`,
  `held/pressed/axis`, `gravity`, `camera({follow,distance,side,target})`,
  `text(hud)`, `after(secs, fn)`, `on_touch(fn)`, `log(msg)`, `time()`,
  `rand()`/`rand_range(a,b)` (xorshift seeded per eval → tape-deterministic,
  unlike the Godot corpus's randomize()), `cam_yaw()` (orbit yaw for
  camera-relative movement), `attach(id,owner,offset)`/`detach(id)` (vehicle
  seats/carrying — pinned post-integration, physics skipped). Input adds
  `shoot`/`shoot_pressed` (F). All five were added by the sandbox3d port —
  see aigame_port_findings.md for the rationale trail.
- **Sound** (src/synth.rs, a polyphonic synth mixed into the app's audio
  callback — the Godot corpus AI hand-built one of these in GDScript, so it's
  an engine service now): `game.sfx(name, pitch?)` with a named bank (jump
  shoot zap grab angry calm rescue shove board coin hurt win lose squeak roar
  bark moo clank whip — the last four match the game's sfx.gd menagerie),
  `game.beep({freq, to, ms, wave, gain})` (waves: sine/square/saw/triangle/
  noise, `to` = pitch glide), `game.jingle("C5 E5 G5 C6", ms_per_note)`.
  Voices are capped at 24 and mute with the app's mute button; headless runs
  have no audio output, so tapes are unaffected.
- **Projectiles**: `game.spawn({pos, vel, life, hits, ...})` — a mover with a
  lifetime (auto-removed) and, with `hits: true`, contact reporting through
  `on_touch`: overlaps against movers/kinematics (movers pass through each
  other, so overlap is the hit) plus whatever solid the sweep stopped it
  against (`Entity::hit_wall`). Touch events fire every overlapping tick.
- **Terrain**: `game.terrain({size, cells, heights, colors, color, base,
  seed, amp})` — engine-built column heightfield (one Static entity per cell;
  row-major `heights`, parallel `colors` or auto-shaded `color`, built-in
  terraced value noise fallback). An instanced-draw + height-lookup collision
  variant is a deferred optimization; columns are plain entities today.
- **Decoration**: `game.part(owner, {pos, size, color})` visual-only child
  boxes (drawn at owner + offset, pruned with their owner) and
  `game.label(id, text)` billboard nametags (projected to the 2D overlay each
  frame — always camera-facing, never occluded, like Godot's Label3D with
  no_depth_test).
- **The look** (Fork A of the parity plan, aigame_parity_gap.md): entities
  carry a visual-only model transform — auto-face yaw for movers (Godot's
  `_drive()` turn-rate clamp; physics AABB never rotates), eased `scale`
  (`game.scale`), per-entity/part emission (`glow:` opt + `game.glow`).
  Parts are owner-local (rotate/scale with the model, front −z), have ids,
  and ease toward `game.move_part` pose targets (rate/sec, default 9) — the
  arm-reach/ear-wiggle primitive. `game.sky({...})` = gradient dome (drawn as
  an 800u cube around the camera) + exponential distance fog fed to every
  shader. `game.terrain({smooth: true, water: h})` = one triangulated
  heightfield mesh (PbrVertex geometry, flat per-tri normals, per-tri avg
  colors) with height-lookup collision in the sweeps (CLIMB 0.55 step-up,
  cliffs block sideways) — no column entities — plus a translucent water
  sensor slab. Blob shadows render under every free mover (alpha quad at
  ground height, fading over 8u). Shaders: DrawGameCube/Alpha/Sky/Terrain in
  game_view.rs's script_mod, all verified against the headless JIT.
- **Systems** (Fork B of the parity plan): `game.attach` grows an options form
  `{pos, mode: "ride", spin}` — "seat" (default) pins + faces with the owner,
  "ride" latches a spinning rider (headcrab); `game.speed_mult(id, f)` scales
  walk velocities engine-side (the debuffed script never knows).
  `game.beam(from, to, {size, color, glow})` = immediate-mode stretched box
  (cleared at every tick start; re-issue from on_tick — grapple cables).
  `game.camera({third_person: id, height, boom, pitch})` = the Godot player
  rig: pivot at entity+height, drag orbits (pitch clamped [-1.2, 0.25]), boom
  marches in past terrain/solids (entities tagged "scenery" ignored), and
  `move_x/move_z` stay camera-relative. HUD: `game.text(slot, msg, {color,
  size})` slots center/top/hint + `game.crosshair(bool)` (a DrawColor dot);
  labels are LabelDefs now — multiple per entity, `{height, color, size}`
  opts, `game.label_text(lid, ...)` updates. Tape runs pin BOTH camera angles
  (third-person: yaw 0/pitch −0.35; orbit: the widget defaults), so captures
  are deterministic.
- **Racing batch** (features.md, my-game-5, 2026-07-10): writable camera
  (`set_cam_yaw/pitch/dist/fov` + `cam_pitch/dist/fov/dragging` readers +
  `input.look_dx/dy` — script writes go through cam_yaw/pitch_request so the
  mouse and script share ONE authoritative rig; tapes pin everything),
  `cam_shake` (tick-hashed offset, never the world rng — pixels wobble,
  simulation doesn't), unknown VERBS are captured errors (eval fails,
  last-good holds) and unknown OPTION keys log warnings (warn_unknown_keys
  allow-lists per verb), `game.time()` resets per eval (snapshot-restored on
  rollback), `rot_y`/`collide:false` on spawnables (visual yaw renders for
  statics via the slab; decor = opaque + non-colliding, still raycast-hittable
  and camera-blocking), `raycast`/`overlap_sphere`/`ground_normal` (step-march
  vs terrain + AABBs, face normals from deepest axis), `push` (velocity add),
  `save`/`load` (SaveVal map → .gamemaker/save.json, 1s debounced flush,
  survives eval/reset by design), sustained `tone`/`tone_set`/`tone_stop`
  (synth.rs Tone voices, 30ms param smoothing, killed on reset_content),
  HUD named slots with 7 anchors + `bar` gauges + `format`, inputs `reset`
  (R/pad-Y) + `back` (C), `every`/`cancel` repeating timers (after returns an
  id too), `distance` takes ids or points.
- Adding a verb = one match arm in `game_dispatch` + a row in the template
  CLAUDE.md. The dispatcher mutates a shared `Rc<RefCell<GameWorld>>`
  synchronously — no async widget trampoline — so world-building finishes
  before eval returns and ordering is deterministic.

## The harness (`tools/ag`)

File RPC through `<game>/.agent/`, answered by the GameView's tick:

| verb | request | answer |
|---|---|---|
| `ag peek` | `peek_request` | `live/f000N.png` ×4 (via `Cx::capture_next_frame_to_file`) + `live/state.txt` + `live/done` |
| `ag test N tape` | `test_request` (JSON) | game restarts, tape replays through the ActionMap, `cap/fNNNNNN.png` every K frames + `probe.txt`, then `test_done` |
| `ag errors` / `ag logs` | — | `last_error.txt` / tail `game.log` |

`sheet.py` (shared with the godot example) tiles captures into one labelled
contact sheet so the agent reads a single image.

**Errors are also PUSHED to the agent**, not only polled: `GameView` dispatches
`GameViewAction::{EvalOk, EvalFailed, RuntimeError}` (every class the isolate
produces — parse, runtime, pod, and shader-compiler errors all ride the same
captured-error sink with file:line intact). The app wakes the agent with a
SYSTEM prompt (visible in the chat as a "⚠ game error" bubble) when a turn
ends with the eval still broken, or when the running game hits a runtime error
while no turn is active (debounced per eval-generation + error hash). Guard:
at most 2 consecutive wake-ups without a kid message — then the status line
says "The game hit a snag — ask me to try again!" and the loop waits for a
human. `GAMEMAKER_NO_AGENT=1` disables the agent entirely (headless tests
must never spend tokens). Engine-level registration errors (the app's own
script VM) are deliberately NOT injected — the game AI can't fix those; they
re-log to the console and surface in the status bar.

Input tapes are frame-indexed `{"f":N,"press":"jump"}` JSON — the same format
the Godot harness used; actions are the ActionMap names (`left right up down
jump`). Keyboard and gamepad are disabled while a tape runs, the camera yaw is
pinned to 0 (camera-relative `move_x/move_z` must not depend on where the kid
left the orbit), and the world is re-evaluated first — so runs are repeatable.

## Renderer

Batching model (see the `PERF:` block at `draw_scene` in game_view.rs):

- **Shapes**: every entity/part carries a `shape:` — box (default), sphere,
  cylinder, cone, wedge — a shared unit geometry spanning [-0.5, 0.5], scaled
  by the instance's `cube_size`. Physics stays the AABB `size` box. Rendering
  is **one draw call per shape per pass** (opaque + alpha), instanced via
  `begin_many_instances` (no per-instance area bookkeeping); empty batches are
  skipped. Windings are outward (the opaque pass backface-culls) — enforced by
  the `shape_windings_face_outward` unit test.
- **Static slab**: instance data for static entities and settled parts of
  static owners is packed once into per-shape `Vec<f32>` slabs and re-emitted
  with a single `extend_from_slice` per frame. The slabs are keyed by
  `GameWorld::render_rev`; every mutation that changes what static content
  looks like must call `mark_render_dirty()` (spawn/remove/restyle/scale/
  glow/move_part on statics, sky/fog changes, world reset, and a static
  owner's part-animation settling). Forget the bump and the edit won't show.
- **Dynamics** (movers, kinematics, their parts, beams, blob shadows) repack
  every frame; part easing skips settled parts (`anim_active`), and dynamic
  part owners are resolved once per frame via binary search on the
  spawn-ordered entity ids — never per shape.
- Measured on a 2000-static + 50 movers ×10 parts stress scene (headless CPU):
  draw_scene 1117µs → **352µs** (3.2×), with correct visuals byte-identical
  tape probes.

## Known gaps (extension points)

- `TODO(aigame)` markers in game_view.rs: box3d physics behind the same verbs
  (the xr port has landed; the mini-AABB world is still in place).
- `capture_next_frame_to_file` grabs the whole window (chat + game). Good
  enough for the agent; a pane-only capture needs a texture readback API for
  child passes.
- Headless runs render frames to `MAKEPAD_HEADLESS_OUT_DIR` on their own; the
  capture-to-file path is GPU-only (metal completion handler).
