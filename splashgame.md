# Splash Game DSL Guide

The complete `game.*` API for games running in the Makepad Game Maker
(`examples/gamemaker`). A game is ONE splash script — `game.splash` — evaluated
live: statements run top to bottom, build the world, then drive it from
`game.on_tick`. Every clean edit hot-reloads the running world instantly; a
broken edit never replaces it (the last working world keeps running and the
error waits in `./tools/ag errors`).

This file is loaded into the game-making agent's system prompt by the app
(like `splash.md` is for UI work). Keep it in sync with the engine:
adding a verb = a match arm in `examples/gamemaker/src/game_view.rs`
`game_dispatch` + a row here.

## Key rules

- Declare `fn` helpers at the top; `let` bindings may interleave with build
  statements (`let player = game.mover(...)` after terrain is fine).
- Property syntax is `name: value` (colon); calls take `({...})` object args.
- **Hex colors containing the letter `e` need the `#x` prefix**: `#x2ecc71`,
  `#x1e1e2e`. Colors without `e` (like `#ff4444`) work with plain `#`.
- The file re-runs from the top on each edit — don't accumulate state across
  edits; rebuild it. `game.time()` restarts at 0 on every reload.
- Closures capture top-level `let` variables — that's how `on_tick` remembers
  entity ids. **Captured variables are MUTABLE and persist across ticks**:
  lap counters, cooldowns, game phases are just `let score = 0` at the top and
  `score = score + 1` inside `on_tick`.
- Math: `sin cos sqrt atan2(y,x) abs min max floor round sign clamp`, `%`,
  `lerp(a, b, t)` (scalars and vectors), constants `PI` and `TAU`, and vector
  methods `v.length()`, `v.normalized()`, `a.dot(b)`, `a.cross(b)` — steering
  AI is `(target - me).normalized() * speed`.
- Everything is procedural: colored shapes and synthesized sound. No image,
  model, or audio files — no files besides game.splash.
- ALWAYS check `./tools/ag errors` after editing. Empty = live. Error = the
  player still sees the OLD world.

## The shape of a game

```splash
let SPEED = 6.0
let JUMP = 11.0

game.sky({})
game.terrain({size: 160, cells: 257, smooth: true, water: 3.5, seed: 7,
              freq: 0.014, offset: 0.5, amp: 30, step: 0.5, min: 0.5, max: 26,
              bands: [{h: 3.6, color: #xd9c780}, {h: 13.0, color: #x5cad4c},
                      {h: 999.0, color: #xf0f5ff}],
              plaza: {r: 26, ramp: 14, h: 7}})

let player = game.mover({pos: vec3(-4, 9, 8), size: vec3(0.8, 1.6, 0.8), color: #x4a7fd6, tag: "player"})
game.part(player, {pos: vec3(-0.18, 0.55, -0.38), size: vec3(0.14, 0.14, 0.06), color: #x11131a})
game.part(player, {pos: vec3(0.18, 0.55, -0.38), size: vec3(0.14, 0.14, 0.06), color: #x11131a})
game.label(player, "You")
game.camera({third_person: player, height: 1.6, boom: 10, pitch: -0.35})

game.on_tick(|dt, input| {
    game.walk(player, input.move_x * SPEED, input.move_z * SPEED)
    if input.jump_pressed && game.on_floor(player) {
        game.jump(player, JUMP)
        game.sfx("jump")
    }
    if game.pos(player).y < -12 { game.set_pos(player, vec3(-4, 9, 8)) }
})
```

## Spawning (returns an entity id)

| call | meaning |
|---|---|
| `game.box({pos, size, color, tag, sensor, collide, body, glow, shape, rot_y})` | a solid. `sensor: true` = no collision, reports touches (goals, pickups), drawn translucent. `collide: false` = opaque DECORATION — looks solid, no physics (rotated road slabs!). `rot_y: 0.6` turns the visual (collision stays the axis box). `body: "kinematic"` = script-moved platform (set its vel; movers standing on it are carried). `glow: 2` = emissive |
| `game.mover({pos, size, color, tag, gravity, turn_rate, shape})` | a character: gravity + collides with the world. `gravity: 0` floats. Movers **auto-face where they walk** (front = -z); `turn_rate` rad/s (default 7) |
| `game.spawn({pos, vel, size, color, tag, life, hits, gravity, glow, shape})` | a projectile: auto-removed after `life` seconds; `hits: true` reports everything it touches through `on_touch` (creatures AND walls) |
| `game.part(owner, {pos, size, color, glow, rot_x/rot_y/rot_z, shape})` → part id | a visual-only shape welded to an entity IN ITS FRAME (turns and scales with it; front = -z): eyes, arms, ears, horns, hats, wheels. No collision; dies with its owner |
| `game.terrain({...})` | the whole landscape in ONE call — see Terrain below |
| `game.label(id, "Bob")` | floating outlined nametag above an entity, camera-facing. `""` removes. Extra labels: `game.label(id, "HELP!", {height: 2.4, color, size})` → label id, update via `game.label_text(lid, "...")` |

`shape:` on any of the above picks the visual: `"box"` (default), `"sphere"`
(alias `"ball"`), `"cylinder"`, `"cone"`, `"wedge"` (alias `"ramp"`). Collision
is always the `size` box — shape is looks only. Round eyes (`shape: "sphere"`),
cone horns, cylinder tree trunks, wedge ramps: use them — creatures made only
of boxes look stiff. Rendering is instanced per shape, so mixing shapes is
free.

## Terrain

```splash
game.terrain({size: 160, cells: 257, smooth: true, water: 3.5,
              seed: 7, freq: 0.014, offset: 0.5, amp: 30, step: 0.5,
              min: 0.5, max: 26, plaza: {r: 26, ramp: 14, h: 7},
              bands: [{h: 3.6, color: sand}, {h: 13, color: grass},
                      {h: 17.5, color: dirt}, {h: 21, color: stone},
                      {h: 999, color: snow}]})
```

- **`smooth: true` always for outdoor worlds** — one connected rolling-hills
  mesh, walkable slopes, collision by ground height. Without it: stepped columns.
- Engine noise (`seed/freq/offset/amp/step/min/max/plaza`) costs NO script
  budget — up to `cells: 384`. `step` = terrace size (0 = smooth), `plaza`
  flattens a disc at the origin, the `max` clamp carves plateau peaks.
- `bands` paints by height — snow above stone is what makes distant hills read
  as MOUNTAINS. Or pass `heights:` (flat row-major `z * cells + x` array) and
  `colors:` for hand-built ground; or `color:` auto-shades one color.
- `water: 3.5` adds a translucent lake sheet (a sensor tagged "water").
- `game.ground_y(x, z)` → height there; `game.ground_peak()` → vec3 of the
  highest point. Place spawns, trees, and the goal ON the terrain with these.

## Driving the game

`game.on_tick(|dt, input| ...)` runs 60×/second, fixed step. `input` fields:
`left right up down jump shoot grab reset back` (held), `jump_pressed
shoot_pressed grab_pressed reset_pressed back_pressed` (this tick only),
`axis_x axis_z` (raw −1..1), **`move_x move_z` — the axes rotated to match the
camera. ALWAYS walk with these** (raw axes only for `side: true` 2D games),
and `look_dx look_dy` — the mouse-drag delta this tick (0 unless the kid is
orbiting; chase cams use it to yield to the kid's hand). Keyboard (WASD/
arrows, Space, F shoot, G grab, R reset — kids get stuck upside-down, give
them a reset! — C back) and gamepad (stick/dpad, A jump, X shoot, B grab,
Y reset) both feed everything automatically.

| call | meaning |
|---|---|
| `game.walk(id, vx, vz)` | set horizontal velocity (vertical untouched) |
| `game.jump(id, v)` | set upward velocity (check `game.on_floor(id)`) |
| `game.on_floor(id)` | standing on something? |
| `game.pos(id)` / `game.vel(id)` | vec3 position / velocity |
| `game.set_pos(id, v)` | teleport (zeroes velocity) |
| `game.set_vel(id, v)` | set full velocity |
| `game.face(id, yaw)` / `game.yaw(id)` | override / read facing. The override is STICKY (walking doesn't revert it); `game.face(id)` with no yaw hands facing back to auto-face |
| `game.find("tag")` | array of ids with that tag |
| `game.tag(id)` / `game.distance(a, b)` | tag / distance — `a`/`b` may each be an entity id OR a vec3 point (checkpoints are positions) |
| `game.remove(id)` | despawn (parts and labels go with it) |
| `game.attach(id, owner, offset)` / `game.detach(id)` | seat-mount (vehicles, carrying) — rider faces with the owner |
| `game.attach(id, owner, {pos, mode: "ride", spin: 2})` | latch ON someone (headcrab): pinned each frame, model spins |
| `game.speed_mult(id, 0.5)` | scale an entity's walk speed engine-side (debuffs) until changed |
| `game.push(id, v)` | ADD to velocity (a shunt, a gust) — `set_vel` overwrites, `push` nudges. Movers pass through each other: to bump someone, detect overlap (`hits`/`on_touch`/`overlap_sphere`) and `push` them |
| `game.raycast(from, dir, max)` | → nil or `{hit, pos, normal, dist}`. Hits terrain (`hit` = -1), walls, creatures, decor. THE sense for wall-avoiding AI, brake-for-the-car-ahead, line of sight, aimed guns. It also hits the caster — cast from just outside your own body, or skip a hit whose id is you |
| `game.overlap_sphere(pos, r)` | → array of entity ids near a point |
| `game.ground_normal(x, z)` | → terrain surface normal (align cars to slopes) |
| `game.save("best_lap", 42.3)` / `game.load("best_lap", 999)` | persist numbers/strings across edits, reloads AND app restarts — high scores live here. Second load arg = default |
| `game.every(secs, \|\| ...)` → timer id / `game.cancel(id)` | repeating timer (game.after also returns a cancellable id now) |
| `game.after(secs, \|\| ...)` → timer id | run once, later; `game.cancel(id)` aborts it |
| `game.on_touch(\|a, b\| ...)` | a sensor overlapped a mover, or a `hits` projectile touched something. Fires EVERY overlapping tick — latch or remove |
| `game.rand()` / `game.rand_range(a, b)` | random, seeded per eval — replays stay repeatable (never bring your own RNG) |
| `game.held("left")` / `game.pressed("jump")` / `game.axis("left","right")` | input outside on_tick |
| `game.log("msg")` / `game.time()` | debug line into .agent/game.log / seconds since reload |
| `game.api()` | dump every verb + its option keys into .agent/game.log — self-lint when an option "did nothing" |

## The look

| call | meaning |
|---|---|
| `game.sky({})` | daylight gradient sky + distance fog. Call it for every outdoor game |
| `game.set_color(id, c)` / `game.glow(id_or_part, e)` | restyle / emissive energy (eyes 3–4; ramp it with AI state) |
| `game.scale(id, s)` | ease the whole model's scale (giants 1.9, sleep-curl via vec3(1, 0.6, 1)) |
| `game.move_part(part, {pos, rot_x/y/z, size, rate})` | ease a part toward a pose (arm reach: `{rot_x: -1.5}`); `rate` defaults 9/s |
| `game.beam(a, b, {size: 0.12, color, glow})` | a stretched cable/laser between two points — re-issue it every tick while it exists (grapple ropes, tethers) |
| `game.camera({third_person: id, height: 1.6, boom: 10, pitch: -0.35, fov: 70})` | THE camera for 3D exploring: drag looks, wheel zooms, slides in when hills block the view. Tag pure-decoration entities `"scenery"` so they don't pull the camera in. Also: `{follow: id, distance: 16}` orbit, `{side: true}` 2D platformer |
| `game.camera({chase: id, boom: 13, height: 2.4, pitch: -0.22, lag: 0.3, recenter: 1.2, speed_tighten: 0.15})` | **the racing camera in ONE line** — third_person's rig plus engine-side ease-behind-the-target. `lag` = ease time-constant (s); `speed_tighten` tightens it with the target's speed; the kid's drag takes over instantly and the rig resumes `recenter` s after the drag ends (wheel zoom is never fought). Angle wrapping is handled engine-side — do NOT hand-roll yaw math on top. `chase: 0` stops the easing, keeping the rig for the mouse |
| `game.set_cam_yaw(a)` / `game.set_cam_pitch(p)` / `game.set_cam_dist(d)` / `game.set_cam_fov(f)` | WRITE the camera — the same state the mouse drags. Writes stick: under a chase rig a write becomes the new camera state and easing continues from there (a scripted look-at burst just works) |
| `game.cam_yaw()` `game.cam_pitch()` `game.cam_dist()` `game.cam_fov()` `game.cam_dragging()` | read the whole camera pose (preserve the kid's wheel zoom before scripting it) |
| `game.cam_shake(0.4)` | impact shake — decays over ~half a second, stacks |
| `game.text("You win!")` | big center banner; `""` clears. Named slots: `game.text("lap", "LAP 2/3", {anchor: "top_right", color, size})` — anchors `top_left top top_right center bottom_left bottom bottom_right`; slots stack per anchor. `"hint"`/`"top"`/`"center"` keep their classic homes |
| `game.bar("speed", 0.62, {color, anchor})` | a gauge (speedometer, boost). Negative fraction removes it |
| `game.format(3.14159, 2)` | → "3.14" — lap times without hand-rolled math |
| `game.crosshair(true)` | center aiming dot (shooting games) |

Blob shadows under movers, label outlines, near-camera clipping (a creature
overlapping the lens clips open instead of filling the screen) are automatic.

House style: give every creature a face (`game.part` eyes) and a name
(`game.label`) — two lines each, do it without being asked. Build big
characters from many parts and animate them with `move_part`/`scale`/`glow`.

## Sound (all synthesized — never files)

| call | meaning |
|---|---|
| `game.sfx("jump")` | named bank: `jump shoot zap grab angry calm rescue shove board coin hurt win lose squeak roar bark moo clank whip`. Pitch: `game.sfx("bark", 1.4)` — animals sound distinct by pitch (chicken high, cow low) |
| `game.beep({freq: 440, to: 880, ms: 120, wave: "square", gain: 0.25})` | one tone; `to` glides pitch; waves: sine square saw triangle noise |
| `game.jingle("C5 E5 G5 C6", 100)` | note names at N ms/note (sharps: "F#5") |
| `game.tone({freq: 80, wave: "saw", gain: 0.15})` → tone id | a SUSTAINED tone — the car-engine primitive. Starts and keeps sounding |
| `game.tone_set(id, {freq: 80 + speed * 6})` | retune it per tick — smoothed, never retriggers |
| `game.tone_stop(id)` | fade it out. Tones also stop on every reload (no stuck hums) |

Add sounds without being asked — jumps, pickups, winning. They make it real.

## Checking your work — ALWAYS

1. `./tools/ag errors` after every edit. Empty = your edit is live.
2. Playtest: `./tools/ag test 120 tools/tapes/selftest.json` — replays a
   frame-numbered input tape, writes `.agent/sheet.png` (frames over time) and
   `.agent/probe.txt` (pos/vel of probed tags every 15 frames). **Read the
   image, read the numbers** — "the jump clears the step" should be a probe
   line you saw. Same tape = same frames, byte-identical.
3. `./tools/ag peek` — 4 screenshots of the live game + entity state, without
   interrupting the player.
4. `./tools/ag logs` — your `game.log()` lines + eval reports.

Tapes: `{"probe": ["player"], "events": [{"f":5,"press":"right"},
{"f":30,"press":"jump"},{"f":33,"release":"jump"}]}` — actions are the input
names above (`left right up down jump shoot grab`).

## Gotchas found the hard way

- Movers are ~0.8×1.6×0.8. Keep playfields within the terrain you built.
- Use tags + `game.find` for groups (coins, enemies) — like scene groups.
- `on_touch` fires every overlapping tick: latch with a bool or remove the
  sensor, or you'll play 60 win jingles a second.
- `turn_rate: 0` on a mover means "NEVER auto-face" — steer its visual with
  `game.face` yourself (cars want this). One `game.face(id, yaw)` call takes
  over facing permanently; `game.face(id)` gives it back to auto-face.
- Typos are loud now: an unknown `game.` verb FAILS the eval (the kid keeps
  the old world; the error gives the game.splash line, names the verb, and
  suggests the nearest real one); an unknown option key logs a warning to
  `.agent/game.log` and `ag errors` shows the warning count. If a thing you
  set "did nothing", check both — or `game.api()` to see the real keys.
- Errors report REAL `game.splash` line numbers (`game.splash:118:9`) — trust
  them, jump straight there.
- `game.time()` restarts at 0 on every reload — durable numbers (best laps,
  high scores) belong in `game.save`/`game.load`.
- Small, visible changes. Tune constants and add shapes; avoid big rewrites.
- Intercept AI (bodyguards): pick threats with TWO distance gates
  (threat-to-player AND threat-to-me), steer at `threat + (player-threat)
  .normalized() * 2`, act within a bonk range. The engine gives you `find`,
  `distance`, `pos` — the brains are yours.
- Weeping-angel AI: freeze when watched — `game.cam_yaw()` gives the camera
  yaw; the look direction is `(sin(yaw), -cos(yaw))` on the ground plane (the
  same rotation `move_x/move_z` use); dot it with the direction to-me and gate
  on > 0.55. Note the camera yaw is NOT an entity yaw — entities face
  `(-sin(e_yaw), -cos(e_yaw))`; the x sign differs. Don't equate the two —
  that's why chase cams belong to `camera({chase})`, not hand-rolled math.
