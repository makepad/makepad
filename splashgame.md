# Splash Game DSL Guide

The complete `game.*` API for games running in Makepad Arcade (`apps/sandbox`;
also `examples/gamemaker`, which shares the same engine and is being retired).
A game is ONE splash script — `game.splash` — evaluated
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
- Arrays: `let xs = []`, then **`xs.push(v)`** (a METHOD — there is no free
  `push(xs, v)` function; calling one is an error that stops the game),
  `xs.len()`, `xs[i]`, and `for x in xs { }`. `game.find("tag")` and
  `game.overlap_sphere(pos, r)` hand you arrays to walk the same way.
- **Use the stock library.** ~4,700 CC0 models ship with Arcade — houses,
  vehicles, trees, rocks, furniture, dungeon and road tiles, rigged
  characters. A scene built from bare `game.box` primitives looks cheap; the
  same scene with real models looks like a game. See **The stock library**.
  Sound is synthesized; the only file a game itself owns is `game.splash`.
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
| `game.box({pos, size, color, tag, sensor, collide, hidden, body, glow, shape, rot_y, density, friction, restitution})` | a solid. `sensor: true` = no collision, reports touches (goals, pickups), drawn translucent. Add `hidden: true` for an invisible trigger volume that still reports touches. `collide: false` = opaque DECORATION — looks solid, no physics (rotated road slabs!). `rot_y: 0.6` turns the visual (collision stays the axis box). `body: "kinematic"` = script-moved platform (set its vel; movers standing on it are carried). `body: "rigid"` = REAL physics (box3d): stacks, tumbles, rotates, bounces — crates, balls, dominoes. Rigids collide with statics/kinematics/each other, NOT with movers; `density`/`friction`/`restitution` tune the material; `shape:"sphere"` rigids roll (collider radius = half width). Rigid state is shared-tier (networked); `push` gives a real impulse. `glow: 2` = emissive |
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
- `water: 3.5` adds a translucent lake sheet (a sensor tagged "water") —
  flat, legacy. For waves, buoyancy, boats and swimming use `game.water`
  (see **Water** below); the flat sheet stays exactly as it always was.
- `game.ground_y(x, z)` → height there; `game.ground_peak()` → vec3 of the
  highest point. Place spawns, trees, and the goal ON the terrain with these.

### Editable terrain (dig, build)

```splash
game.terrain_volume({min: vec3(-20, -6, -20), max: vec3(20, 12, 20),
                     mode: "smooth"})            // Astroneer-style sculpting
game.terrain_volume({min: vec3(30, -2, -10), max: vec3(50, 12, 10),
                     mode: "blocky"})            // Minecraft-style cubes
game.dig(vec3(0, 0, 0), {r: 3})                  // carve a crater
game.dig(hit.pos, {r: 2, mode: "fill", material: 3})   // raise rock
game.set_block(vec3(35, 1, 0), 2)                // place one cube (0 removes)
```

| verb | what |
|---|---|
| `game.terrain_volume({min, max, mode, cell, palette})` | → volume index. Declares an editable voxel region layered over the terrain. `mode: "smooth"` sculpts (tunnels, overhangs, caves); `"blocky"` reads cells as cubes with greedy-meshed faces. `cell` (default 0.5) is fixed by the FIRST volume; `palette: [colors]` maps materials 1.. (0 = air). Outside every volume the terrain stays untouched heightfield |
| `game.dig(pos, {r, mode, material})` | brush one edit: `"carve"` removes, `"fill"` adds `material`, `"flatten"` levels to the brush centre's height inside radius `r`. Aim with `game.raycast` from the camera and dig at `hit.pos` |
| `game.set_block(pos, material)` | set the single cell containing `pos`; material 0 removes. The Minecraft verb — loop it for walls and towers |

- Edits are **ops in host tick order**: replicated to every device, late
  joiners receive the edited chunks, and edits SURVIVE script reload (they
  are player state, like `game.save`). Re-declare your volumes on every
  eval — the edits reappear inside them.
- Everything collides for free: cars drive through dug tunnels, walkers
  stand on built towers, projectiles and `game.raycast` see edited ground
  (reported as terrain, `hit` = -1).
- Digging is budgeted (a few chunks remesh per tick) — a giant brush
  finishes over a few frames, never a hitch.

## Driving the game

`game.on_tick(|dt, input| ...)` runs 60×/second, fixed step. `input` fields:
`left right up down jump shoot grab reset back` (held), `jump_pressed
shoot_pressed grab_pressed reset_pressed back_pressed` (this tick only),
`axis_x axis_z` (raw −1..1), **`move_x move_z` — the axes rotated to match the
camera. ALWAYS walk with these** (raw axes only for `side: true` 2D games),
and `look_dx look_dy` — the camera-look delta this tick (0 unless the kid is
mouse-orbiting or on the gamepad's RIGHT stick; chase cams use it to yield to
the kid's hand). Keyboard (WASD/arrows, Space, F shoot, G grab, R reset —
kids get stuck upside-down, give them a reset! — C back) and gamepad (left
stick/dpad move, RIGHT stick rotates the camera like the mouse, A jump,
X shoot, B grab, Y reset) both feed everything automatically — never write
your own camera-from-stick code.

**When you add an ability, always give it a gamepad path too**: bind it to
one of the named actions above (`jump shoot grab back reset` all have pad
buttons) instead of inventing keyboard-only triggers, so the kid on a
controller is never locked out of something you built.

| call | meaning |
|---|---|
| `game.walk(id, vx, vz)` | set horizontal velocity (vertical untouched) |
| `game.jump(id, v)` | set upward velocity (check `game.on_floor(id)`) |
| `game.on_floor(id)` | standing on something? |
| `game.pos(id)` / `game.vel(id)` | vec3 position / velocity |
| `game.set_pos(id, v)` | teleport (zeroes velocity) |
| `game.set_vel(id, v)` | set full velocity |
| `game.face(id, yaw)` / `game.yaw(id)` | override / read facing. `game.yaw` reports the live heading of a rotating rigid vehicle, not merely its spawn yaw. The override is STICKY (walking doesn't revert it); `game.face(id)` with no yaw hands facing back to auto-face |
| `game.find("tag")` | array of ids with that tag |
| `game.tag(id)` / `game.distance(a, b)` | tag / distance — `a`/`b` may each be an entity id OR a vec3 point (checkpoints are positions) |
| `game.remove(id, {after})` | Despawn now (parts and labels go with it), or retain the same replicated entity for `after` seconds before removal. A dead delayed body immediately becomes nonblocking/non-targetable but keeps its health + appearance identity so every pane can finish the skinned death pose. |
| `game.attach(id, owner, offset)` / `game.detach(id)` | seat-mount (vehicles, carrying) — rider faces with the owner |
| `game.attach(id, owner, {pos, mode: "ride", spin: 2})` | latch ON someone (headcrab): pinned each frame, model spins |
| `game.speed_mult(id, 0.5)` | scale an entity's walk speed engine-side (debuffs) until changed |
| `game.push(id, v)` | ADD to velocity (a shunt, a gust) — `set_vel` overwrites, `push` nudges. Movers pass through each other: to bump someone, detect overlap (`hits`/`on_touch`/`overlap_sphere`) and `push` them. On a `body:"rigid"` entity this is a true mass-scaled impulse (same Δv feel; wakes the body) |
| `game.raycast(from, dir, max)` | → nil or `{hit, pos, normal, dist, material}`. An EXACT physics ray (true surface normals, exact distances): hits terrain (`hit` = -1, per-cell `material`), walls, rigids, creatures (their capsule — hitscan sees a strafing player) and decor. THE sense for wall-avoiding AI, brake-for-the-car-ahead, line of sight, aimed guns. A ray that STARTS inside a body (your own eye ray) does not report that body — no self-hit skipping needed anymore |
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

## The stock library — ~4,700 CC0 models

Arcade ships Kenney's low-poly library: houses, vehicles, trees, rocks, walls,
furniture, food, weapons, road and dungeon tiles, and rigged characters.
**Reach for these before `game.box`.** A scene of coloured primitives reads as
a prototype; the same layout with real models reads as a game.

| call | meaning |
|---|---|
| `game.find_model("pine tree", {count: 4})` | search → **a LIST of DISTINCT model ids**. Options: `count`, `spread` (`mixed` default / `kinds` / `variants`), `seed`, `kind` (`model`/`sound`), `rigged: true`, `max_size` |
| `game.find_palette("village", 7)` | → `{pack, group: [ids], ...}` — a MATCHED set from ONE art pack |
| `game.model(id, {pos, yaw, scale, collide, tag})` | place one. Ids come from `find_model` — never invent one; a wrong id reports near-misses and places nothing |
| `game.kits()` | → `[{pack, tiles, tile_size, roles}]` — the packs whose tiles snap together |
| `game.cast()` | → `[{joints, members, states}]` — rigged characters, grouped by shared rig (one animation set drives every member of a group) |

### The three rules that decide whether it looks good

1. **Never place result #1 five times.** This is the single most common way to
   make a scene look cheap. `find_model` returns a list precisely so you can
   walk it — a village wants five different houses, a wood wants several
   species and several variants of each.
2. **One art pack per region.** Spreading across the whole library gives a
   suburban house beside a hex-tile house beside a sci-fi house. Use
   `find_palette` for a region, or keep to one `pack`.
3. **Generate layouts; don't hand-place a hundred things.** `game.town`,
   `game.dungeon` and `game.road_network` lay real tiles with correct corners
   and junctions, which is tedious and error-prone by hand.

```
// WRONG — one house, five times
let h = game.find_model("house")[0]
for i in 0..5 { game.model(h, {pos: vec3(i * 8.0, 0.0, 0.0)}) }

// RIGHT — five different houses, all from one pack, all facing the street
let houses = game.find_model("suburban house", {count: 5, seed: 3})
for i in 0..5 {
    game.model(houses[i], {pos: vec3(i * 8.0 - 16.0, 0.0, -10.0), yaw: 0.0})
}
```

## Building a place (composition)

Tiles from a kit snap onto a grid; the generators pick corners, junctions and
dead ends from how the layout actually connects, so you never name a piece.

| call | meaning |
|---|---|
| `game.road_network({kit, paths: [[vec3, vec3, ...], ...], seed})` | polylines → a road. Two paths that cross give a crossroad, one that tees gives a T — automatically |
| `game.town({roads_kit, buildings_kit, props_kit, extent: 24, block: 4, density: 0.8, seed})` | a street grid with buildings FRONTING the streets |
| `game.dungeon({kit, extent: 32, min_room: 5, depth: 4, seed})` | rooms + corridors, every room reachable. Returns `{tiles, entrance, exit}` — spawn the player at `entrance` |

All three return `{tiles}` (a count) and take `collide` (default on for towns
and dungeons, off for roads so cars drive over them). All are
**seed-deterministic**: the same seed gives the same level every load.

`modular-dungeon-kit`, `modular-cave-kit` and `modular-space-kit` share one
role vocabulary — the same `game.dungeon` call gives a crypt, a cavern or a
space station purely by swapping `kit`.

```
// A small place: a road, a village along it, and a dungeon to find.
let d = game.dungeon({kit: "kenney/modular-dungeon-kit", extent: 24, seed: 5})
game.town({
    roads_kit: "kenney/city-kit-roads",
    buildings_kit: "kenney/city-kit-suburban",
    extent: 20, block: 5, density: 0.7, seed: 5,
})
let trees = game.find_model("pine tree", {count: 4, seed: 5})
for i in 0..24 {
    let a = i * 0.26
    game.model(trees[i % 4], {pos: vec3(cos(a) * 34.0, 0.0, sin(a) * 34.0)})
}
let hero = game.mover({pos: d.entrance, size: vec3(0.6, 1.7, 0.6)})
game.camera({third_person: hero, boom: 9, pitch: -0.3})
```

## Light and weather

| call | meaning |
|---|---|
| `game.sun({time_of_day: 8.5})` | one sun for the whole scene: 0..24 local hours. Morning and evening are warm and throw long shadows, noon is white and short |
| `game.sun({dir: vec3(0.3, 0.9, 0.2), color: vec3(1.0, 0.9, 0.7)})` | or aim it yourself. `ambient` lifts the shadow side, `shadow_alpha` (0..1) sets how dark cast shadows draw |

Objects cast real shadows that stretch and swing as the sun moves — you get
them for free, there is nothing to turn on.

## Particles (device-local — never affects the game)

| call | meaning |
|---|---|
| `game.particles(id, {kind: "smoke", rate: 20})` → emitter | a continuous emitter that FOLLOWS an entity: exhaust, fire, a dust trail |
| `game.particles(vec3(x,y,z), {kind: "dust"})` → emitter | or pinned to a spot |
| `game.burst(vec3(x,y,z), {kind: "spark", count: 16})` | a one-shot puff: impacts, pickups, explosions |
| `game.particles_stop(emitter)` | stop one emitter |

Kinds: `spark` (fast, falls), `smoke` (rises, grows), `dust` (drifts, settles),
`trail` (fades in place). Tune with `life size color spread speed gravity`.

Particles are **cosmetic only**. They never collide, never touch game state,
and each device draws its own — so never make a rule depend on one. A phone may
draw fewer than a PC in the same game, and that is fine by design.

## Sound (all synthesized — never files)

| call | meaning |
|---|---|
| `game.sfx("jump")` | named bank: `jump shoot zap grab angry calm rescue shove board coin hurt win lose squeak roar bark moo clank whip`. Pitch: `game.sfx("bark", 1.4)` — animals sound distinct by pitch (chicken high, cow low) |
| `game.beep({freq: 440, to: 880, ms: 120, wave: "square", gain: 0.25})` | one tone; `to` glides pitch; waves: sine square saw triangle noise |
| `game.jingle("C5 E5 G5 C6", 100)` | note names at N ms/note (sharps: "F#5") |
| `game.tone({freq: 80, wave: "saw", gain: 0.15})` → tone id | a SUSTAINED tone — the car-engine primitive. Starts and keeps sounding |
| `game.tone_set(id, {freq: 80 + speed * 6})` | retune it per tick — smoothed, never retriggers |
| `game.tone_stop(id)` | fade it out. Tones also stop on every reload (no stuck hums) |
| `game.sfx_at(vec3(x,y,z), "clank", {range: 40})` | same bank, but POSITIONED: quieter with distance, panned left/right by where it is relative to the camera. Use it for anything with a place — impacts, engines, other players |

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
- **Reserved words cannot be bound.** `let me = game.car(...)` is a PARSE
  ERROR (the game keeps running the old world; `ag errors` shows the line).
  The same guard covers `self`, `scope`, `nil`, `true`/`false` and all
  statement keywords (`for`, `loop`, `match`, ...), in every binding
  position: `let`/`var`, `fn`/closure argument names, `for` variables, and
  destructuring patterns. Name the player `hero` or `player1`. Reading `me`
  is still fine — only binding it errors. (`{me: 1}` as an object KEY is
  data, not a binding, and stays legal.)
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

## Building blocks

High-level prefabs. **The engine runs these at 60Hz** — you call the verb once
to create and configure, then the block drives itself. Don't reimplement their
behaviour in `on_tick`; steer them with `game.drive` or let them run.

All block state is **Shared tier**: laps, scores, positions and control intent
replicate to other players. Animation blending is Derived (recomputed from
velocity), so it costs nothing over the network.

| verb | what you get |
|---|---|
| `game.car({pos, color, player: true, top_speed, accel, grip, seats})` | A driveable raycast vehicle on a rigid chassis: suspension, grip, arcade steering, and it will not roll over. Returns the entity id. |
| `game.character({pos, color, player: true, model, speed, jump, view: "third"})` | A walker on the mover sweep (0.55 step-up, jumps, camera-relative movement) with idle↔walk↔run blending driven by its own speed. Platformer options below apply here and on `player_character`. |
| `game.avatar(entity, {model: "skeleton_minion"})` | Give any existing entity a shared semantic model identity. This is appearance only: it adds no movement, camera, input, health, or combat behavior. Repeating it replaces the model instead of stacking; the identity reaches LAN peers and late joiners. `character` and `player_character` register their `model` automatically (or the stock `character/default` fallback). |
| platformer options (all opt-in, on `character`/`player_character`) | `double_jump: true` — one extra mid-air jump, refilled by any footing. `dash: 14` — dash speed u/s (tap the run key / shoot button; ground or air, air dashes carry level); `dash_time`/`dash_cooldown` tune it. `wall_jump: 12` — jump again off any wall you're pressing into (kicks up and away). `ledge_grab: true` — falling past a box's top edge while pushing into it hangs; jump pulls up, pushing away drops. `slope_limit: 45` — DEGREES; steeper ground can't be stood on or climbed, you slide off. `slope_speed: 0.8` — uphill slower, downhill faster. `collider: "capsule"` — real wall SLIDING on angled walls (box3d capsule) instead of the axis stop. |
| `game.racecar({pos, color, model, player: true, roll_balance, stability_assist, grip, grip_front, grip_rear, power, lsd, brake_bias, drive_front, drag})` | The SIM tier: TMeasy tires with per-wheel load from the suspension, forces at the contact points (it dives, rolls and CAN flip — `stability_assist: 1` buys the arcade manners back), a real engine/gearbox/diff (launch wheelspin, shifts you can hear), per-surface grip, and motion-rig telemetry (OutSim/OutGauge via `SANDBOX_MOTION=host:port`) for whoever sits in it. Same seats, camera, autodrive and race kit as `game.car`. |
| `game.tune(car, {roll_balance, stability_assist, grip, grip_front, grip_rear, power, lsd, brake_bias, drive_front, drag})` | Live setup on a racecar — absent keys keep their value, so `game.tune(car, {roll_balance: 0.6})` is a one-knob edit. More front roll stiffness = understeer, more rear = oversteer. Arcade cars have no setup; tuning one logs why nothing happened. |
| `game.character({pos, color, player: true, model, speed, jump, view: "third"})` | A walker on the mover sweep (0.55 step-up, jumps, camera-relative movement) with idle↔walk↔run blending driven by its own speed. |
| `game.plane({pos, color, player: true, thrust, lift_speed})` | Arcade flight: lift from airspeed, auto-level, weathervane stability. Holding pitch loops; it cannot stall or spin. |
| `game.plane({model: "sim", ...})` | The flight SIM tier: a whole-aircraft stability-derivative model. It trims hands-off at cruise (`cm0`/`cm_alpha` set the trim angle), stalls past `alpha_stall` (`stall_blend` is the forgiveness — wide is a gentle mush, narrow a sharp break), shows adverse yaw when you roll without rudder, floats in ground effect, and feeds the motion rig from real forces at the pilot seat. `lift_speed` is THE speed knob (lift = weight there); `thrust_weight` sets climb; authority/damping pairs `cm_de`/`cm_q`, `cl_da`/`cl_p`, `cn_dr`/`cn_r` set pitch/roll/yaw feel. Give it `friction: 0.05` — low contact friction is the undercarriage. Boarding, cameras and `game.speed` are shared with the arcade tier. |
| `game.trim(plane, {any coefficient, lift_speed, seat})` | Live setup on a sim plane, mid-flight — absent keys keep their value, so `game.trim(p, {cm_de: 0.5, cm_q: -1.2})` turns a trainer into an aerobat in one line. Arcade planes have no coefficients; trimming one logs why nothing happened. |
| `game.flightdata(plane)` | → `{airspeed, altitude, alpha, beta, throttle, stalled}` — the instrument feed for `game.bar`/`game.text` HUDs (α/β radians; `stalled` 0..1; zeros for arcade planes, which have no such thing). |
| `game.drive(id, {steer, throttle, brake, handbrake, pitch, roll, yaw, move_x, move_z, jump})` | Feed control intent to any block — this is how script-driven or AI opponents are controlled. `yaw` is the aircraft rudder (−1 left … +1 right); on planes `steer` is also read as rudder, which is what a gamepad's bumpers and the keyboard's Z/C fill for a seated pilot. |
| `game.autodrive(car, {points: [vec3, ...], pace})` | Hands a car a racing line to follow. `pace` is 0..1 of its top speed. |
| `game.boat({pos, color, player: true, thrust, top_speed, rudder, grip, seats, density, balance, slope_force, standing_rider})` | A buoyant rigid with prop thrust and a water-speed-scaled rudder, floated by the engine's hull probes (needs a `game.water` volume under it). Board it with the same button as a car and race it over checkpoint gates on the water. `slope_force` adds wave-face push without changing the rider pose; `standing_rider: true` explicitly places a mounted avatar's feet on the deck. Defaults float (`density: 0.35`); crank density past the water's (1.0) and it sinks — deliberately. |
| `game.surfboard({pos, color, player: true, density, balance, standing_rider})` | A small buoyant rigid the wave face pushes: on the front of a set wave it accelerates down the slope, so carving = steering along the face. Board it like a boat and opt into the surfing pose with `standing_rider: true` (the default remains seated). `balance` (0..1) is the stay-upright assist — the mini-game knob. |
| `game.drive(id, {steer, throttle, brake, handbrake, pitch, roll, move_x, move_z, jump})` | Feed control intent to any block — this is how script-driven or AI opponents are controlled. |
| `game.autodrive(car, {points: [vec3, ...], pace})` | Hands a car OR a boat a racing line to follow. `pace` is 0..1 of its top speed. |
| `game.speed(id)` | Forward speed for a car, airspeed for a plane, planar speed otherwise. |

### Brains

Attach an AI to any mover. Re-issuing a brain on the same entity replaces it.

All brains **navigate**: when the straight line to their goal is blocked by
walls, props or water, they route around it on the engine's walkability grid
(A* + string-pulling — a chase brain finds the doorway, a patrol crosses a
courtyard maze). No option to set; with a clear line they steer exactly as
they always did.

| verb | behaviour |
|---|---|
| `game.wander(id, {home, range, speed, pause})` | Amble to random points near home, pausing between trips. |
| `game.chase(id, {tag, range, catch, speed})` | Hunt the nearest entity carrying `tag`. `game.caught(id)` returns who it reached this tick. |
| `game.patrol(id, {points: [vec3, ...], speed})` | Walk a fixed route, looping forever. **Do not pass `loop:`** — `loop` is a reserved word and using it as an option key hangs the script until the instruction limit kills the eval, so the game never starts. Looping is the default; ping-pong is unavailable until the engine renames that key. |

### Race kit

| verb | behaviour |
|---|---|
| `game.spawnpoint({pos, yaw})` | Declares a start slot; returns its index. |
| `game.checkpoint({pos, size})` | Declares a gate. Gates are numbered in declaration order and **must be crossed in order** — cutting the course scores nothing. |
| `game.place(id, slot)` | Puts an entity on a start slot and enters it in the race. |
| `game.race({laps})` | (Re)starts lap tracking. Call it again to restart a race. |
| `game.standings()` | `[{entity, lap, checkpoint, finished, score}]`, leader first. |
| `game.lap(id)` / `game.rank(id)` / `game.finished(id)` | Per-racer progress. |
| `game.score(id, points)` / `game.score_of(id)` | Scoring for non-racing games too. |

A complete 4-car race is ~60 lines — see
`examples/gamemaker/resources/fixtures/racing.splash`.

### Water — waves, buoyancy, boats, swimming, surf

One verb declares a region of water with a deterministic wave surface; the
engine does the rest per tick. Rigids in the volume get hull-probe buoyancy —
**float or sink is decided by `density` alone** (water is 1.0; drive the race
car into the bay and find out). Characters over deep water swim
automatically: they tread at the surface, move at `swim_speed` (default 55%
of walk speed), hop out on jump at the surface and stroke upward underwater;
leaving the water restores normal movement exactly.

```splash
game.water({min: vec3(-80, -6, -80), max: vec3(80, 0, 80),
            waves: [{dir: vec3(1, 0, 0.3), amp: 0.4, len: 14},
                    {dir: vec3(-0.4, 0, 1), amp: 0.2, len: 7}],
            current: vec3(0.3, 0, 0)})
game.surf_spot({pos: vec3(0, 0, 40), dir: vec3(1, 0, 0), period: 12, amp: 1.1})
```

| verb | behaviour |
|---|---|
| `game.water({min, max, waves, current, density, color})` | A water box whose surface (`max.y`) carries up to 8 wave components (`{dir, amp, len, speed, phase, group}`; `speed` defaults to the deep-water dispersion for `len`). Also spawns a hidden sensor tagged "water" spanning the volume, so `on_touch` fires for anything IN the water. Returns the volume index. |
| `game.surf_spot({pos, dir, period, amp, len})` | Adds a repeating set-wave train to the volume at `pos`: a big crest arrives every `period` seconds and travels along `dir` — the wave a `game.surfboard` rides shoreward. |

The wave surface is simulated CPU-side and drawn by the same sum — what you
float on is exactly what you see. Boat races are the ordinary race kit with
checkpoint gates placed over the water.

### Health kit (the damage pipeline)

One pipeline for ALL harm — guns, melee, hazards, vehicle impacts route their
damage through these verbs, so cross-game hits (a plane strafing a car) need no
extra wiring. Health is **Shared tier**: every device renders the same hp.

| verb | behaviour |
|---|---|
| `game.health(id, {max: 100, team: 1})` | Register an entity in the damage pipeline. Unregistered entities are invulnerable. Re-calling is the RESPAWN: full heal, death cleared. `team` is a tag (0 = none) — the engine never enforces friendly fire; read `game.team_of` and decide. |
| `game.damage(id, n, {from: attacker})` | → hp left. Hitting a corpse does nothing (one kill = one death). `from` attributes the kill for `on_death`. |
| `game.heal(id, n)` | → hp, clamped at max. The dead stay dead — revival is `game.health` again. |
| `game.hp(id)` | → hp (0 = dead or unregistered) — feed `game.bar` for a health bar. |
| `game.team_of(id)` | → team tag. |
| `game.on_death(\|id, from\| ...)` | Fires once per kill, after the tick that emptied the hp. What death means is your call: player prefabs respawn through their shared life kit; for disposable actors prefer `game.remove(id, {after: 2})` so the authoritative death pose remains visible before despawn. |

### FPS kit (aim, guns, monsters)

The crosshair IS the gun. **The engine steps everything per-tick** — the aim
ray, fire rate, clip, reserve, reload, the shot itself — and every point of
damage goes through the health kit above, so a gun kill and a monster bite
fire the same `on_death`. You configure a loadout once and react to hits.

| verb | behaviour |
|---|---|
| `game.aim(player?)` | → `{pos, dir}` — the eye-through-crosshair ray of that player's rig (default: you). Carries the camera's full yaw AND pitch, so `game.raycast(a.pos, a.dir, 100)` is a manual hitscan for grapples, paint, telekinesis. Nil for a player with no rig. |
| `game.gun(owner, {rate: 6, spread: 0.01, damage: 12, clip: 8, ammo: 64, reload_time: 0.9, kind: "rifle"})` | → gun id. A hitscan firearm on `owner`: fires while the owning player holds the **shoot** button, spaced by `rate`/s; `clip` empties into an automatic `reload_time` reload fed from `ammo` (absent or -1 = infinite). Shots trace the exact ray — a strafing player's capsule is hittable — and land as `game.damage(target, damage, {from: owner})`. `spread` (radians) cones the aim; `auto: false` = one shot per press; `recoil` kicks YOUR camera only (device-local); `color` tints the tracer. The owner's FIRST gun arms itself. |
| `game.gun(owner, {projectile: true, speed: 26, gravity: 0.35, damage: 40, rate: 1.2, kind: "rocket"})` | The same verb lobs REAL projectiles instead: a glowing CCD shell at `speed` m/s that drops with `gravity` (× world gravity — arcs stay honest if gravity changes), damages what it strikes through the same pipeline, and despawns. `range/speed` is its flight time. |
| `game.give(entity, gun)` | Hand a gun to an entity and make it their ACTIVE weapon — the weapon switch and the pickup are the same verb. One active gun per owner; holstered guns keep reloading. |
| `game.ammo(gun)` | → `{clip, reserve, reloading}` (reserve -1 = infinite) — feed `game.text`/`game.bar` for the ammo HUD. Replicated: a client HUD shows host truth. |
| `game.on_hit(\|target, from, gun\| ...)` | A shot landed on an entity (terrain hits stay silent). Sparks, hitmarkers, aggro — the damage is already applied. |
| `game.monster(id, {targets: "player", range: 30, attack_range: 1.8, damage: 10, rate: 1, speed: 4.5, stagger: 0.35})` | Make any mover a Quake-style hunter: it chases the nearest `targets`-tagged entity — **through corridors**, on the nav grid, like every brain — bites in `attack_range` through `game.damage` at `rate`/s, freezes for `stagger` seconds whenever something hurts it, and goes inert when the health kit declares it dead (your `on_death` decides what a corpse means). Give it `game.health` or it is invulnerable. |

Guns fire for whichever player's rig is carrying them — local, remote or
bot — and a seated player's carried gun goes quiet (the seat owns the shoot
button). The smallest working shooter is
`apps/sandbox/resources/games/arena/game.splash` — corridor walls, a rifle,
a rocket launcher and a den of monsters that hunt you through the gaps.

### Ball game kit (Rocket League / pong / soccer)

One call builds the whole match: a rigid sphere ball (it really rolls and
bounces — cars, characters and thrown crates all strike it through ordinary
physics), goal boxes, team scores, kickoff resets. **The engine runs the
match**; you configure and react.

| verb | behaviour |
|---|---|
| `game.ballgame({ball: {r: 1.1, restitution: 0.7, gravity_scale: 1, pos, color}, goals: [{pos, size, team, color}, ...], bounds: {min, max}, teams: 2, hit_boost: 1.6, reset_delay: 2.5})` | → ball entity id. The ball entering a goal box scores **for that goal's `team`** — the goal you SHOOT AT carries your team number (bare goals take declaration order). Optional `bounds` returns an out-of-play ball directly to kickoff without awarding a point. Goal boxes are hidden rule volumes; draw whatever posts and nets suit the game. `hit_boost` is the feel knob: 1.0 = raw physics, 1.5+ = arcade punch scaled by how hard the striker drove in. `gravity_scale < 1` floats the ball (pong/volley). After a goal the ball is out of play for `reset_delay` seconds, then re-centres itself. Re-calling replaces the whole pitch. |
| `game.on_goal(\|team, scorer\| ...)` | A goal landed: `team` scored, `scorer` is the last entity that struck the ball (0 = it rolled in untouched). Jingle here; the kickoff reset is already scheduled. |
| `game.team_score(team)` | → points — the score text is one `game.text` line in `on_tick`. |
| `game.ball()` | → the ball's entity id (0 = no ball game) — aim cameras or AI at it. |
| `game.kickoff()` | Re-centre the ball dead still, right now (period starts, manual restarts). |

Strikers need NO declaration: anything that hits the ball — a `game.car`, a
walking character, a kinematic platform — is a striker and gets scorer credit.
A complete car-football match is ~50 lines — see
`apps/sandbox/resources/games/rocket/game.splash`.

### Fighting kit (3D fighter — duels, frame data)

Moves are DATA and **the engine steps the fight at 60Hz**: frame data
(startup/active/recovery are TICK counts — the tick is the frame), hit
volumes tested against the opponent's high/mid/low hurtbox bands, guard,
counter-hits, juggles, knockdown, rounds and the clock all run in Rust.
The RPS is engine law: **strike beats throw, throw beats guard, guard
blocks strikes** — high guard stops highs+mids, low guard (guard + stick
down) stops lows+mids, so sweeps must be blocked low and overheads
standing. Throw rides the existing grab action; damage rides `game.health`
(register your fighters or `game.duel` will at 100); a knockdown is the
same ground state a car impact causes. Players punch/kick/guard on the
J/K/L keys or pad ○/△/L1.

| verb | behaviour |
|---|---|
| `game.fighter(id, {moves: {jab: {input: "punch", startup: 3, active: 2, recovery: 8, damage: 5, stun: 12, block_stun: 6, volume: {offset: vec3(0.35, 1.4, -0.7), r: 0.25}}, sweep: {input: "down+kick", damage: 12, hits: "low", knockdown: true}, launcher: {input: "up+punch", damage: 10, launch: 7.5}}})` | Register a moveset on any mover. `input`: `"punch"` \| `"kick"` \| `"grab"`, with an optional `"down+"`/`"up+"` RAW-stick gate. A move first connects exactly `startup` ticks after the press. `volume` is a sphere in the attacker's frame (+x right, +y up from the FEET, front at −z); omit it and one is derived from `hits` at the attacker's own scale. `hits` (`"high"|"mid"|"low"`, default mid) decides which guard stops it. `launch` is vertical m/s — the victim juggles until landing, then takes a hard knockdown. A moveset with no grab move gets a built-in throw. Re-calling replaces the moveset. |
| `game.duel(a, b, {rounds: 2, timer: 60})` | → duel id. Locks both fighters' facing to each other (movement turns opponent-relative under the duel camera — sidestep with the stick), mounts the duel camera for any player driving one of them, and runs the match: KO or time-out scores a round, fighters reset to their marks, first to `rounds` wins. `timer` is seconds per round (0 = untimed). |
| `game.duel_state(duel)` | → `{a, b, round, rounds, score_a, score_b, fighting, over, winner, time_left}` — the round card and clock are two `game.text` lines in `on_tick`. |
| `game.fighter_state(id)` | → `{phase, current, tick}` — phase is one word (`idle`, `startup`, `active`, `recovery`, `hitstun`, `blockstun`, `launched`, `down`, `guard`), `current` the live move's name. Poll it in `on_tick` to drive `game.move_part` choreography on box-built fighters — fists forward in startup/active, back in recovery. |
| `game.on_hit(\|attacker, victim, move, damage\| ...)` | A strike or throw connected (counter-hits carry bonus stun automatically). Sparks, shake and sound go here — the damage is already applied. |
| `game.on_block(\|attacker, victim, move\| ...)` | A strike was guarded: chip nothing, both slide a little. |
| `game.on_ko(\|loser, winner\| ...)` | A duelist's hp hit zero. The round is already scored and the reset scheduled — this is for the K.O. banner and the jingle. |
| `game.drive(id, {punch: true, kick: "down", guard: "low", throw: true})` | Script a fighter's attacks (an AI rival): `punch`/`kick` press this tick — pass `"down"`/`"up"` to satisfy a gated move — `guard` holds (`true`/`"high"`/`"low"`, `false` releases), `throw` presses the grab. Walk with the same call's `move_x/move_z`. |

The smallest working duel is `apps/sandbox/resources/games/dojo/game.splash`
— one ring, three moves, an AI rival on a beat, two rounds, ~120 lines.

### Unit kit (RTS-lite)

Commandable armies over the same movers everything else uses. **The engine
does every per-tick job** — pathfinding on the walkability grid, one flow
field per group order (a hundred units share one solve), formation slots so
squads settle into a block instead of piling onto a point, arrival events.
You state goals and react; per-tick steering never crosses the script
boundary.

| verb | behaviour |
|---|---|
| `game.unit(id, {team: 1, speed: 3.5, arrive: 0.8, spacing: 1.4})` | Make a mover commandable. `team` gates who may order it; `spacing` is the formation pitch. Re-calling updates the config. |
| `game.order(units, {kind: "move", target: vec3(0, 0, 10)})` | → order id. `units`: one id, `[ids]`, or `nil` = the current selection. `kind`: `"move"` \| `"attack"` \| `"follow"` \| `"hold"`. `target`: a `vec3` spot, or an entity id — attack/follow keep tracking it as it walks (move snapshots it). A new order replaces the units' old one; `hold` stops them where they stand. |
| `game.select({from: vec3, to: vec3, team: 1})` | Replace the selection with `team`'s units inside the world-space rect (any corner order); omit `team` for all. → count. |
| `game.select({x0, y0, x1, y1, aspect, team})` | The same, from SCREEN fractions (0,0 = top-left): each corner casts an exact pick ray from the orbit camera onto the ground, then the world rect test runs between the two ground points. This is the mouse drag-box — pass the drag corners. |
| `game.select({ids: [unit_a, unit_b], team: 1})` | Select an explicit stable roster, independent of where its members moved. Invalid/removed/wrong-team ids are ignored. Use this for named squads. → count. |
| `game.selected()` | → `[unit ids]` — feed it straight to `game.order(nil, ...)` or read it for HUD counts. |
| `game.on_arrive(\|unit, target\| ...)` | A unit settled into its formation slot (fires once per arrival; `target` = the tracked entity or 0). |
| `game.commander(player, team)` | Which team a player may command. **Host-enforced**: a client's order arrives over the reliable channel and any unit outside the sender's team is dropped — you cannot move the other side's army. |

Units, brains and characters are all movers: run a `game.chase` monster into
a battle line, or `game.health` the whole army and let the damage pipeline
score the fight. The smallest working battlefield is
`apps/sandbox/resources/games/war/game.splash` — a chokepoint wall, twelve
units, select-and-order on timers.

## Players (multiplayer)

Every world has at least one player: id `0`, this device. A hosted room adds one
per connected client, plus any bots the game creates. Blocks take `player:` to
say who drives them.

| verb | what it does |
|---|---|
| `game.players()` | all player ids, local first — **Shared** |
| `game.player_name(p)` | display name — **Shared** |
| `game.player_entity(p)` | the entity this player drives, 0 if none — **Shared** |
| `game.player_input(p)` | that player's input object (same shape `on_tick` gets) |
| `game.bot(name)` | add a host-side player with no device — **Shared** |
| `game.on_join(fn(p))` | someone joined the room |
| `game.on_leave(fn(p))` | someone left; their body is freed for you |

Movement stays camera-relative per player: `move_x`/`move_z` are rotated by *that
player's* camera yaw, which travels inside their input packet. A client's camera
is presentation only and never replicates.

**Replication tiers.** The host simulates; clients receive. What crosses the wire
is decided per field, not per entity:

- **Shared** — position, velocity, size, body kind, tag, score, lap progress.
  Host to clients, every tick.
- **Derived** — facing yaw, walk-cycle blending, part animation, scale and glow
  easing, blob shadows. Never sent: a client recomputes these from Shared state,
  which is why 200 moving props cost nothing to rotate.
- **Local** — camera, audio, particles. This device's business alone.

Gameplay must not depend on Local state, or late joiners will see a different
game from everyone else.
