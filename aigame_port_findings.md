# aigame port findings — my-game 3D sandbox → game.splash

Result of test-porting the AI-generated Godot game (`~/games/my-game`, the active
3D sandbox: ~2100 lines across main3d + 9 actor scripts) onto the gamemaker
engine as one `game.splash`. Fixture:
`examples/gamemaker/resources/fixtures/sandbox3d.splash` (~370 lines — 5.7×
denser than the GDScript, mostly because the engine owns spawning/physics/SFX).
Written 2026-07-09. Companion checklist: `aigame_port_inventory.md`.

## Verification

- Whole world evaluates clean first-load: **1036 entities** (961 terrain columns,
  water sheets, 14 trees, goal + beacon, 19-creature cast, 2 trucks), zero eval
  or tick errors at the 500k/tick instruction limit.
- Tape run (`ag test`, 200 frames): spawn→fall→land at exactly ground+half;
  camera-relative walk at exactly SPEED 6.0; jump arc peaks +2.25 = JUMP²/2G to
  the decimal; wander AI steers the cast; respawn works. Probe numbers, not hopes.
- Mount/drive/dismount verified end-to-end *by accident*: the first fixture spawn
  was 2.3 units from a truck and the player silently auto-mounted at tick 1 —
  seat height, top speed, and jump-to-dismount all matched the Godot behavior
  before I even tried to test them. (Fixed by matching the original's player
  transform (-8, 11, 8).)
- NOT verified visually (headless pane compositing still pending, task #9):
  colors, camera feel, HUD text rendering. Not exercised behaviorally: nightmare
  wake/pounce and the win touch (code-verified only; both use the same verbs the
  probes exercised).

## Engine bugs the port flushed out (both fixed)

1. **Sensors were solid.** `step_world` filtered collision candidates by body
   kind only — `sensor: true` boxes (goals, my water sheets, the beacon) blocked
   movement, contradicting the documented contract. One-line filter fix. The
   starter game never noticed because its goal floats at jump apex.
2. **(Earlier, same session) optional positional args trapped** — probing
   `args[1]` on a 1-arg call failed the whole eval. Every optional arg would
   have hit this.

A port whose only failures were engine bugs, not script bugs, is a good sign
for the DSL's authorability.

## The six predicted cleanups — verdict from actually porting

| # | Prediction (aigame_port_inventory.md) | Verdict |
|---|---|---|
| 1 | First-class SFX bank + synth | **Confirmed hard.** The corpus had invented `squeak` and `roar` beyond the bank we shipped — the bank grew twice in one day. Named-bank + beep/jingle is the right shape; expect the bank to keep growing from corpus usage. |
| 2 | Tags + broadcast/duck-dispatch | **Half-confirmed.** `find`/`tag`/`distance` + script-side state objects covered everything the port needed — cross-actor "messages" (enrage/zap/heal) became plain field writes on shared script objects, which is *better* than Godot's group+has_method bus. A native broadcast API is NOT needed while one script owns all actors. It becomes needed only if games ever split across isolates. **Deferred, deliberately.** |
| 3 | Script raycast | **Dodged, honestly.** Ledge-probe AI was replaced by heightmap lookup (`h_at`) because terrain heights are script data anyway. Fine for this game; a shooter that needs line-of-sight will force `game.raycast`. Keep on the roadmap, don't build speculatively. |
| 4 | Camera-relative helper | **Confirmed, split in two.** The missing primitive wasn't a movement helper — it was `game.cam_yaw()` (the orbit camera's yaw was engine-private). With yaw readable, two lines of cos/sin in script do the rest; documented as a pattern instead of an API. |
| 5 | `attach/detach` (seats/carrying) | **Confirmed emphatically.** Replaced ~100 lines of per-actor teleport-following + collision toggling in GDScript with 2 calls. Vehicles, passengers, and the dismount-pop all fell out. |
| 6 | Mover platform carry | Already in the engine (kinematic floor_id carry); the port didn't stress it (no moving platforms in the 3D sandbox — they're in the 2D game). |

## New findings (not predicted)

1. **No RNG in the script language** — the single most-used Godot facility
   (randf/randi everywhere in wander AI) simply didn't exist. Added
   `game.rand()`/`game.rand_range(a,b)`, xorshift **seeded per eval**: wander AI
   now replays identically under input tapes, which Godot's `randomize()`
   corpus could never do. Determinism became a feature of the port.
2. **No noise either** — the terrain wants value noise. A 12-line script
   `hash/smooth/noise2` worked fine at 31×31. At the original's 256×256 it
   wouldn't (65k columns ≈ 3M+ instructions, and 65k entities would swamp both
   the O(statics×movers) physics and the instanced renderer). The Godot game
   itself had to weld chunks — scale is an *engine* concern in any engine.
   → Roadmap: `game.terrain({cells, cell, heights})` native heightfield
   (box3d has heightfield shapes waiting) + greedy column merging.
3. **`shoot` was a hardcoded gap** — the ActionMap knew jump but nothing else;
   the corpus registers custom actions at runtime (mouse+F+gamepad X). Added F →
   `shoot`/`shoot_pressed`. Real fix on the roadmap: `game.action("dash", "KeyQ")`
   runtime action registration, like the corpus does in Godot.
4. **Projectiles work but are clunky**: a bolt = gravity-0 mover + set_vel +
   script-side life/hit bookkeeping in a `retain` closure. Fine at 5 bolts;
   a bullet-hell would want `game.spawn_projectile({vel, life, on_hit})` with
   engine-side lifetime. Mover-vs-mover hits also aren't reported by `on_touch`
   (sensor×mover only) — the port used distance checks; real overlap events for
   movers is a small, worthwhile addition.
5. **One HUD line is enough** — the original's 5 labels + colors + cancel-token
   flashes collapsed into `game.text` + a 6-line script `flash()` with a
   generation counter. Colored/positioned HUD text can wait.
6. **Multi-part models are the visible fidelity loss.** Every creature is one
   colored box; the originals have legs/shirt/head/eyes/smile built from 6–12
   cubes, billboard nametags, emissive eyes. The single biggest visual upgrade
   per line of API: `game.part(id, {offset, size, color})` decorative child
   boxes (no physics), plus `game.label(id, text)` nametags. This is also what
   made the Godot corpus *charming* — worth prioritizing over new mechanics.
7. **Script ergonomics held up.** Struct-arrays of actor state + one shared
   steering function expressed 9 GDScript classes in ~120 lines. Field mutation
   through array elements, closures capturing top-level `let`s, and `retain`
   with side effects all just worked. The `0.0 - x` workaround for (possibly
   fine) unary minus should be tested and, if broken, fixed — it's the ugliest
   thing in the fixture.

## Fidelity ledger (what the port drops vs the original)

- **Terrain at 1/8 resolution** (31×31×4u columns vs 256×256×0.625u welded
  chunks): terraces are chunkier, no caves (cave-carving needs the fine grid to
  read as tunnels), no smooth triangulated slopes, no vertex-color blending.
- **Retired cast stays retired** (Huggy, Robot, soldiers — the original has them
  commented out too), so Kissy's enrage/defend/flee arcs are dormant here as
  there; her follow behavior is live.
- Single-box creatures, no nametags, no arm-reach animation, no emissive glow,
  no procedural sky (fixed background), water is an opaque thin sheet (alpha
  untested in the cube renderer), no camera-blocking ray (camera can clip
  through hills), no mouse-capture (orbit-drag instead), mini-variants are
  recolored critters.
- Behavior approximations: injured-Kissy heals into a follower (original swaps
  to a full Kissy with her own arc); nightmare glow-eyes become a body-color
  swap; critter squeak probabilities eyeballed.

## Bottom line

The port took one authoring pass, found two engine bugs and five API gaps, and
every gap closed with a small primitive rather than a framework. The engine's
current vocabulary + script-side state objects genuinely cover the corpus's
*behavioral* range; the visible gap is decoration (parts/labels), and the
structural ceiling is terrain scale — both have clear, small next steps.

## v2 re-port (2026-07-09 late) — the full current game

`fixtures/sandbox3d.splash` v2 ports the game's CURRENT state (~3600 GDScript lines,
44 creatures + 2 trucks) onto the upgraded engine: verified headless — eval clean at
90 entities, tape run writes test_done/captures/probes, plaza floor exactly 7.9 and
walk exactly 6.0 (Godot ground truth), and two identical runs produce byte-identical
probes. The tape also exercises the full truck mount/drive/dismount chain.

### Fidelity ledger

**Exact:** every spawn-table name/position/size/color/speed; all behavior constants —
Giant DogDay guardian (follow 34/stand 9.5, charge 8.4 to an intercept point, bonk 5 →
stagger+knockback+bark, golden beam flash), Kissy bodyguard (gates 18/34, intercept,
shove 2.2), headcrabs (chase 26, leap at 6 [up 8 fwd 7, cd 1.8], latch 1.5 → ride-attach
(0,1.95,0) spin 2 + speed_mult 0.5, shake-off on vel.y>5 or 4s, self-stun 2.5),
Prototype weeping-angel (LOS dot 0.55 in 34, creep 70, eye glow 1.5/4.5), Baba Chops
(charge 14 @7.2, ram 2.4, fire-eye glow 2→5 ramp), Nightmare Huggy (hunt 90, arm-reach
via move_part inside 30), nightmare critters (creep 30, nip 1.3 bounce), CatNap
(sleep-curl scale 1/0.6/1, wake 9, catch 1.9), farm animals (shy 4.5/flee 4.4, calls
4–14s at Godot's per-kind pitches), trucks/passengers, terraced terrain recipe
(12.5+noise, snap 0.5, plaza 26/14/7, water 3.5, height-color bands), goal on the true
peak + glowing beacon, third-person camera (1.6/10/−0.35) + crosshair + hint, zap
dispatch on all threat kinds, HUD flash tokens, win latch, full synth bank.

**Approximated:** terrain colors are single-color auto-shade (Godot has height bands
sand/grass/dirt/stone/snow — a 257² script colors array would blow the eval budget;
engine follow-up: band colors as terrain options);
models simplified but silhouette-faithful (DogDay 21 parts incl. the 8-box sunbeam
collar vs 23; Baba 15 vs 27 — horns straight not curled); animal voices = moo/squeak
at Godot's pitch tables (no dedicated oink/baa/cluck recipes); only Nightmare Huggy's
arms animate (pack arms static); zap stun uniform 4.5s (Godot 4–5.5 per kind).

**Dropped:** cave tunnels only. (x-major→z-major heightfield transpose handled;
goal verified on the true peak via `game.ground_peak()`.)

### Engine gaps: found by the port, then closed engine-side (same day)

1. **CLOSED — eval budget vs terrain scale.** `game.terrain` now runs its noise
   engine-side (`freq`/`offset`/`step`/`min`/`max`/`plaza`, cells up to 384), plus
   `game.ground_y(x, z)` and `game.ground_peak()`. The fixture builds the full
   Godot-scale 257×257 world (256 cells @0.625u) with zero script instruction cost,
   and all spawn/tree/goal heights come from ground queries.
2. **CLOSED — second input action.** `grab` exists (keyboard G, gamepad B, tape
   action) with `grab`/`grab_pressed` snapshot fields. The grapple hand is ported:
   32 u/s flight to range 16 on a `game.beam` cable, terrain hit → player yank
   (22 u/s + 6.5 up), creature hit → ride-attach haul, held 1.2s, set down at the
   player's feet with a pop. Approximations (memo): solid-hit detection is
   terrain-height only (tree/box bodies don't catch the cable — needs a script
   raycast/box-probe verb); the hand vanishes at max range instead of retracting.
3. **CLOSED — label outlines.** Engine draws a 4-copy dark outline behind every
   billboard label; nothing needed in script.
4. **OPEN — height-carving.** Caves need holes/overhangs in the heightfield (or
   rock-slab CSG); the smooth mesh is single-valued.
5. **OPEN (new, minor) — terrain band colors.** Height-band coloring
   (sand/grass/dirt/stone/snow) as engine-side terrain options, since script-built
   color arrays don't scale past ~100² vertices.
6. **OPEN (new, minor) — solid-probe verb.** A `game.raycast`/box-probe would let
   the grapple (and ledge-AI patterns from the corpus) hit boxes, not just terrain.

Verification after the engine-noise/grapple update: eval clean at 90 entities,
zero JIT failures, last_error empty, tape (walk/jump/shoot/grab) → test_done +
captures + probes with plaza floor exactly 7.9 and walk exactly 6.0, two runs
byte-identical.
