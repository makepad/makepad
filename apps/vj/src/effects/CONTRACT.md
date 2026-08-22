# VJ Effects — the working contract

This file is the coordination contract for agents working on the VJ effect
renderstack in parallel. Read it top to bottom before touching anything.

## Build state (update when it changes)

- `cargo build --release -p makepad-vj --example effect_gallery` and the
  `makepad-vj` app — GREEN, `cargo test -p makepad-vj --example
  effect_gallery` green (expr, lsys, engines, seed tests + the sibling
  engines' tests).
- Verified visually via the gallery + remote bridge: particles (burst /
  tunnel / vortex / rain / galaxy / image / clouds / phyllo), lsystem,
  metaballs, heightmap, ribbons (curl / lorenz / aizawa), tunnel (knot /
  lissajous, seam-free), grass, emitters (scripted fireworks), screen +
  warp/bloom/feedback/tiltshift stages; sibling engines firefly /
  harmonograph / domino verified by their builder. 72 presets shipped
  (resources/effects + MANIFEST.md), all compiled into the binary by
  `seed.rs` and seeded publish-if-absent into the local store when the VJ
  connects (`AssetKind::VjEffect`, alias `vjfx/<name>`). Known rough edges
  are tracked in IDEAS.md.

## Architecture in one paragraph

A **vj effect** is a normal makepad widget (`VjFxView`) configured by a
**splash document** (makepad-script, evaluated at LOAD time only). The
document picks one mesh-generating **engine** written in Rust, parameterises
it (including little programs: L-system rules, per-frame emitter scripts),
declares an ordered list of **render stages** (bloom/blur/feedback/tiltshift
/warp — the multipass graph), binds any animatable parameter to **music
signals** via compiled expression strings, and may **subclass the engine's
draw shader** to override hook functions. Engines encode everything they
know onto the **vertex stream** once; the **vertex shader** animates from
those attributes plus time/beat uniforms every frame. Per-frame CPU work is:
signal build + binding eval (nanoseconds), engine regen (no-op for static
engines), and — for the emitters engine only — a bounded splash tick.

## File map and ownership

| file | contents | parallel policy |
|---|---|---|
| `apps/vj/src/effects/mod.rs` | module root, `script_mod` registration chain, contract doc | SHARED — additive edits only (add your `script_mod` call + `pub mod` line) |
| `mesh.rs` | `FxMesh` buffer, vertex layout, rng | FROZEN — extend only with new push helpers |
| `expr.rs` | binding expressions + `Signals` | FROZEN — add functions/signals additively |
| `doc.rs` | document evaluation, `EffectDoc`, `Reader` helpers, `StageCfg` | SHARED — add your engine's parse arm + config keys |
| `engines.rs` | all engines + `Engine` wrapper | SHARED today; NEW ENGINES GO IN THEIR OWN FILE (`engines_<name>.rs`) with a small arm added to the `Engine` enum/match blocks |
| `lsys.rs` | L-system compiler + turtle | owned by the core session |
| `shaders.rs` | all draw shaders + registration | SHARED — new engines add their own shader in their own file's `script_mod!` and register it from `mod.rs`; do NOT grow the existing shaders (silent shader size budget!) |
| `post.rs` | the stage-chain runtime | owned by the core session; new stage kinds = coordinate first |
| `view.rs` | the widget: clocks, signals, passes, draw dispatch | owned by the core session; new engines need one `ShaderKind` arm — keep it a 5-line diff |
| `apps/vj/resources/effects/*.splash` | THE PRESET LIBRARY | OPEN — add freely, follow the naming + doc conventions below |
| `apps/vj/examples/effect_gallery.rs` | the preview/verify rig | owned by the core session |

Registration: `effects::script_mod(vm)` (mod.rs) calls each submodule's
`script_mod`. The vj app calls it in `AppMain::script_mod` (main.rs); the
gallery example does the same. A new engine = new file + `pub mod` +
one `script_mod` call + `Engine` enum arm + doc.rs parse arm + `ShaderKind`
dispatch arm.

## The effect document

A `.splash` file: makepad-script whose **last expression is one object**.
`let` bindings above it are fine. Evaluated once at load with this prelude
already in scope: `mod.std.*`, `mod.pod.*` (type names `float`, `vec2`,
`vec3`, `vec4`), `mod.math.*`, `mod.shader.*` (`instance`, `uniform`),
and `draw` (= `mod.draw`, for shader subclassing).

Reading is forgiving: missing key = default, wrong type = default + warning
(surfaced in the widget status), unknown keys ignored. Colors are literals —
**always write `#x` hex** (`#x40f0ff`); rely on nothing after a bare `#`.

```text
{
    name: "Neon Growth"
    engine: "lsystem"       // REQUIRED — see the engine list
    seed: 7

    // shared animation — every one of these ACCEPTS A NUMBER OR A BINDING
    // STRING (see Signals below) unless marked const:
    speed: 1.0              // const: master time multiplier
    beat_pulse: 0.5         // 0..2, how hard the beat pumps scale/glow
    beat_rate: 1.0          // const: pulses per beat
    bar_beats: 4            // const: beats per bar for the `bar` signal
    sway: 0.4               // wind amplitude (mesh engines)
    sway_freq: 0.9
    twist: 0.0
    fog: 0.045
    glow: 1.0
    grow: "loop"            // off | loop | pingpong (u_growth sweep)
    grow_beats: 8           // const
    p0: 0.0  p1: 0.0  p2: 0.0  p3: 0.0   // free params -> self.user in shaders

    // palette
    color_bg: #x05060f   color_a: #x40f0ff
    color_b: #xff40a0    color_c: #xffffff

    // camera (auto-framed when omitted; tunnel flies its own path)
    cam_dist: 8.0  cam_height: 2.5  cam_orbit: 0.12  cam_fov: 50.0

    // texture input 0 — the "extra slot". In the VJ's effect-pass mode the
    // channel's main content lands here; when nothing is bound the runtime
    // substitutes a built-in ANIMATED test pattern, so every texture effect
    // always renders something. "test" documents the intent.
    input0: "test"

    // ordered render stages, max 4 (numeric params are bindable):
    stages: [
        {kind: "bloom", threshold: 0.5, strength: 1.4, levels: 3},
        {kind: "feedback", amount: "0.7 + pulse * 0.25", zoom: 1.012,
         rotate: 0.004, dim: 0.97},
        {kind: "tiltshift", focus: 0.55, width: 0.22, levels: 3},
        {kind: "kaleido", p1: 0.7, p2: "0.2 + pulse"}
    ]

    // shader hooks: a SUBCLASS of the engine's draw shader (never a plain
    // object — that would replace the whole shader def and is rejected):
    shader: draw.DrawVjFxMesh {
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            return vec4(fract(t * 3.0), 0.5, 1.0 - t, 1.0)
        }
    }

    // per-frame tick (emitters engine only; bounded — see below):
    frame: fn(fx) { ... return [ {spawn objects} ] }
}
```

### Signals and binding expressions

Any animatable parameter (and warp/feedback/bloom stage numbers) may be a
STRING: a tiny expression compiled once at load, evaluated per frame.

Signals: `time dt beat phase bar bpm pulse energy bass mid high`
(`beat` = continuous beat position; `phase` = 0..1 at `beat_rate`; `bar` =
0..1 over `bar_beats`; `pulse` = eased envelope `(1-phase)^3`; audio ones
are 0 until the host feeds `set_signals`). Constants `pi`, `tau`.
Functions: `sin cos abs floor fract sqrt tri saw env` (1-arg),
`min max pow step` (2), `clamp mix` (3). Grammar: `+ - * /`, unary `-`,
parentheses. Examples: `"0.3 + 0.5*env(phase)"`, `"sin(bar*tau)*0.02"`,
`"mix(0.2, 1.0, energy)"`.

### The standard shader signal block

EVERY fx draw shader carries these instance fields — hook code and
subclasses can always read them, no ceremony:

- `self.time_beat` = (time, beat, phase, pulse)
- `self.sig` = (bar, bpm, energy, dt)
- `self.user` = (p0, p1, p2, p3) — the document's bound user params
- `self.anim` = (sway, sway_freq, growth, twist)
- `self.shape`, `self.flow` — engine-specific (see engines.rs)
- `self.col_a/col_b/col_c/col_bg`, `self.fog` = (density, glow, texmix, 0)

### Vertex attribute conventions (the CubeVertex layout, 12 floats)

`geom_pos`(3) `geom_id`(a_id) `geom_normal`(3) `geom_pad`(a_aux)
`geom_uv`(2) `geom_tail_pad_0`(a_r0) `geom_tail_pad_1`(a_r1).

| engine | a_id | a_aux | a_r0 | a_r1 | uv | normal |
|---|---|---|---|---|---|---|
| lsystem | branch depth | arc from root 0..1 | hue | tube radius | (ring angle, branch hash) | radial |
| particles | particle id | spawn phase | rnd | rnd | corner | direction seed |
| metaballs | dominant blob | height 0..1 | blob hue | dist to blob | (polar angle, field) | -∇field |
| heightmap | checker cell | radial 0..1 | vertex hash | 0 | grid uv | +Y |
| ribbons | ribbon idx | trail age | hue | side ±1 | (speed01, age) | tangent |
| tunnel | around 0..1 | along 0..1 | ring hash | tube radius | (around, along) | inward radial |
| emitters | particle id | 0 | rnd | rnd | corner | direction seed |
| firefly (blades) | row 0..2 | along 0..1 | hue | half width | (side, phase hash) | lateral dir |
| firefly (flies) | fly id | 2.0 + height class | blink phase | blink rate rnd | corner | ANCHOR POSITION |
| harmonograph | strand idx | curve t 0..1 | strand rnd | side ±1 | (t, strand01) | strand seed vec |
| domino | branch id | arc index (dominoes) | tile hash | yaw (rad) | GROUND PIVOT (x, z) | LOCAL face normal (pos = LOCAL corner) |
| tiles | tile index (grid coords + uv window derive from it) | radial 0..1 from plane centre | rnd (stagger/shade) | rnd (tumble) | corner 0/1 (tile-local uv) | SHATTER FLIGHT VECTOR (unit dir × flight distance, baked; pos = rest centre) |
| flock | bird id | speed01 | hue | FLAP AMPLITUDE at this vertex (0 spine, wingspan at tips) | (along-body 0..1, flap phase hash) | banked UP vector (the flap axis) |
| city | tower/trail id | CLASS: 0 tower 1 ground 2 sky 3 trail | tower hash / trail hue | tower height / trail phase | tower: facade uv in WINDOW units; ground: world xz; sky: (azimuth01, h01); trail: (arc01, h01) | face normal |
| pipes | pipe id | birth order 0..1 (THE growth axis) | pipe hue | local radius (balls bulged) | (around01, along/elevation) | radial outward |
| stockcharts | element class 0..5 (body/wick/grid/crosshair/MA/tick) | candle age 0..1 | up/down (crosshair: axis) | move size 0..1 | quad-local uv | +Z |

Encode MORE data when your engine knows more — spare channels are the
vertex shader's raw material. Document any new channel here.

### Engines (today)

`particles` (modes burst/fountain/tunnel/vortex/rain/galaxy/image),
`lsystem`, `metaballs`, `heightmap`, `ribbons`, `tunnel`, `emitters`
(script-driven, below), `firefly` (synced meadow, engines_firefly.rs),
`harmonograph` (pendulum ribbon, engines_harmonograph.rs), `domino`
(topple run, engines_domino.rs), `tiles` (input texture as a tile grid:
wave/shatter/conveyor/spiral, engines_tiles.rs), `flock` (boid
murmuration of gliders, engines_flock.rs), `city` (flyover:
night/retro/tron styles + light-cycle trails, engines_city.rs), `pipes`
(the 3D-pipes lattice, growth replayed on the beat, engines_pipes.rs),
`stockcharts` (beat-clocked candlestick terminal, engines_charts.rs),
`screen` (no mesh — input0 straight into the stage chain: the fullscreen
effect family). Engine keys: see the module docs in `mod.rs` (kept
current) and `engines.rs`.

### The emitters engine + `frame:` tick

The document's `frame: fn(fx)` runs once per frame INSIDE the script VM,
bounded by a 200k-instruction limit (overrun = loud status clip, never a
stall). `fx` = `{time, dt, beat, phase, bar, pulse, energy, emitters}`.
It returns an ARRAY OF SPAWN OBJECTS (or nil); each spawns one emitter:
`{kind: "burst"|"jet"|"sparkle"|"ring", pos: vec3, vel: vec3, life, speed,
size, gravity, stagger, fraction, spread, color, color2, seed, slot}`.
Re-spawning an existing `slot` MOVES that emitter (script-animated
persistent emitters). Emitters cap at 192; each is ONE draw instance whose
particles are stateless vertex-shader work. The tick touches emitters only
— never individual particles, never per-vertex loops.

### Hosting modes + thumbnails

- Primary: `VjFxView` with `composite: false`; host composites
  `output_texture()` (re-fetch per frame — feedback ping-pongs identity).
- Effect-pass: host renders channel content to a texture,
  `set_input_texture(0, tex)`; the effect's output replaces the channel.
- Beat: `set_beat(beat_pos, bpm)` per frame + `set_signals([energy,bass,
  mid,high])`; free-runs at `set_bpm` otherwise.
- Thumbnails (lazy, animated — VJ-side): host a hidden `VjFxView` in slot
  mode (pass renders at SLOT_PASS; a host can wrap it in a small widget
  rect with composite on), feed a few beats, grab frames. The runtime hook
  is exactly the slot-mode API above; the VJ-side cache is follow-up work.

## Preset conventions

- Directory: `apps/vj/resources/effects/*.splash`. Naming:
  `NN_snake_name.splash` (NN orders the gallery; name is the identity).
- Every preset: distinct look (rules/geometry/bindings, not a recolor),
  comments that TEACH the pattern it demonstrates, `#x` colors, and at
  least one beat-aware element.
- Verify before calling done: `VJFX_DOC=<name> ./target/release/examples/
  effect_gallery --remote`, grab via `curl :PORT/g`, LOOK at the PNG.
  Cycle: `/t?t=n` (next), `/t?t=p` (prev), `/t?t=g<name>` (jump). Close
  with `/gq`. Never leave an instance running.

## Rules that bit us already (do not relearn)

- Instance fields are vec4/f32 only, AFTER `#[deref] draw_vars`.
- The shader compiler has a silent size budget — keep shaders modest; new
  looks go in NEW shaders, not new branches of old ones.
- `shader:` hooks MUST subclass (`draw.DrawVjFxMesh{...}`); a plain object
  replaces the def and used to abort the whole app (now rejected at parse).
- Frame path is panic-free BY LAW: clamp/log/skip, never unwrap on doc
  data. A panic in the macOS timer callback aborts the entire VJ.
- Buffer sizes stay constant per frame (pad to high water) — a changed
  byte length reallocates the GPU buffer every frame.
- Vertex-stage texture fetch: `sample_nearest(uv, 0.0)`.
- Wind/growth displacement must be a function of CONTINUOUS per-vertex
  data (rest position, arc length) or connected geometry tears.
