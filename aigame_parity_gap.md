# Full-parity gap: current ~/games/my-game vs the aigame engine

Goal (rik, 2026-07-09 late): run the little Godot game — CURRENT state — fully in
splash/gamemaker. Evidence: 21:54 `gd shot` sheet + a full close-read of all 17 actor
scripts. The game now spawns **~44 creatures + 2 vehicles**: Giant DogDay guardian
(23-box model, 1.9x scale, intercept-charges nightmares), 3 headcrabs (leap + latch to
the player's head, 0.5x speed debuff, shake off by jumping), the Prototype
(weeping-angel: only moves unwatched, LOS dot 0.55 vs camera forward), Baba Chops ram
(27 boxes, fire eyes with proximity-ramped emission), Nightmare Huggy (arm-reach lerp),
4 nightmare critters, 10 farm animals (per-kind synth calls w/ pitch), Huggy pack,
Kissy-as-bodyguard, grapple hand (yank player to walls / haul creatures, stretched
cable), 64-chunk welded smooth terrain (256x256 cells, 0.5 terracing, caves, alpha
water at y=3.5), ProceduralSky, shadowed sun, crosshair + hint + 4 colored flash HUD.

**Key architectural relief:** Godot NEVER rotates the physics body — only the visual
`Model` child yaws (atan2 facing + turn-rate clamp). Our unrotated-AABB physics is the
same design; the gap is visual/animation, not physics.

Axis trap for the re-port: main3d.gd heightfield is x-major (`i*(CELLS+1)+j`), our
game.terrain is z-major row-major — transpose or the goal lands on the wrong peak.
(The Godot-side "_heights@62" parse error is a stale Godot cache: identifier doesn't
exist on disk; file is internally consistent.)

## Engine work — Fork A "the look" (game_view render/world)

| # | Item | Spec from the game |
|---|---|---|
| A1 | Per-entity model yaw | auto-face velocity w/ turn-rate clamp (default movers), `game.face(id, yaw)` / `turn_rate` override; physics AABB unchanged |
| A2 | Owner-local parts | part offsets rotate with owner yaw; fronts at −z convention |
| A3 | Part animation | `game.move_part(part, {pos, rot_x, rot_z…})` engine-lerped (t≈dt*9, Godot's constant); `game.scale(id, v)` model scale (CatNap curl 1/0.6/1, DogDay 1.9x); headcrab spin |
| A4 | Emission | glow parts (eyes 3–4, runtime ramp 1.5→5, bolt/beacon); `glow:` spawn opt + `game.glow(id, e)` |
| A5 | Sky + ambient + fog | gradient top (0.32,0.58,0.9) → horizon (0.75,0.87,0.96); distance fade |
| A6 | Blob shadows | dark ground quad per mover at height-lookup y (real shadow maps out of scope) |
| A7 | Smooth terrain | triangulated heightfield mesh, flat per-tri normals, per-vertex height colors (SAND≤3.6/GRASS≤13/DIRT≤17.5/STONE≤21/SNOW), height-lookup collision; translucent water sheet (0.25,0.55,0.85,0.6) |

## Engine work — Fork B "systems"

| # | Item | Spec |
|---|---|---|
| B1 | Ride attach + debuff | `attach(..., {mode:"ride", spin})` per-frame head-follow; `game.speed_mult(id, f)`; shake-off = script (vel.y > 5 or timeout → detach) |
| B2 | Grapple support | `game.beam(a, b, {size,color})` stretched transient box (cable); haul = attach + detach-with-pop (exists); yank = set_vel (exists) |
| B3 | Camera | third-person low rig: pivot height 1.6, boom 10, pitch clamp [−1.2, 0.25], occlusion pull-in vs terrain/boxes (`max(1, hit−0.5)`), ignore tagged scenery; optional mouse-look |
| B4 | HUD slots | crosshair toggle, hint line (top-left), 4 independent colored center banners: `game.text(slot, msg, {color, size})`; billboard labels: multiple per entity, color/size/height opts |
| B5 | Synth voices | bark, moo, clank, whip (+ per-call pitch already works) |

## Script-side patterns (no engine work — fixture implements with existing APIs)
group-AI intercept (tags + find + dual distance gates), LOS gate (cam_yaw dot), duck
damage dispatch (tag → handler table in script), stun/stagger contract, animal AI.

## Sequence
Fork A → Fork B (same files, sequential) → Fork C re-port of the CURRENT game as
fixtures/sandbox3d.splash v2 (acceptance test) → GPU visual check by rik.
