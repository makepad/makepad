# VJ Effects — the working contract

This file is the coordination contract for agents working on the VJ effect
renderstack in parallel. Read it top to bottom before touching anything.

## THE FIRST THING TO KNOW: a document carries its own shader

**A `.splash` effect document is a complete, forkable unit. The pixel math
it runs is IN THE FILE.** Open `09_synthwave.splash` and the neon-grid
terrain shader is there, in the document, as source you can edit; change a
line and the effect looks different. That is the point of this design:
these documents are authored by AIs, and an author who can only re-tune
`amp: 0.6` can only ever produce a variation. An author who owns the
fragment code can produce a NEW LOOK.

So the law for every shipped preset (pinned by
`seed::registry_tests::every_bundled_preset_carries_its_own_shader`):

- every preset carries a `shader:` block subclassing its engine's draw
  shader, with the family's look function written out inline;
- several presets of one family each carry their own copy of the same
  default — **self-containedness beats DRY here.** Do not factor them back
  together; each file has to read as the whole effect;
- the ENGINE still holds the same code as its default, so a document that
  declares no hook (an old doc, a third-party one, a quick sketch) renders
  exactly as before. The hook is an override, never a requirement.

The engine keeps what is STRUCTURAL — geometry, motion, the camera, which
texture is sampled where, the content-coupling plumbing and the beat. The
document owns THE LOOK.

## Build state (update when it changes)

- `cargo build --release -p makepad-vj --example effect_gallery` and the
  `makepad-vj` app — GREEN, `cargo test -p makepad-vj --example
  effect_gallery` green (65 tests: expr, lsys, engines, seed/registry +
  the sibling engines' tests).
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

    // SELF-DESCRIBING DIALS: name the levers that mean something to THIS
    // effect, so a host can label real knobs ("SYNC", "DRIVE") instead of
    // dead "P1/P2" dials. The VJ shows a FIXED THREE dials per slot mapped
    // to p0..p2 (fixed so MIDI mappings survive effect swaps); a param the
    // doc does not declare shows dimmed and inert. p3 is the RESERVED
    // lever: the transition slot drives it with triangle(program_mix), so
    // bind transition intensity to p3 and do not declare a dial on it.
    // `default` is only the knob's resting position — an untouched knob
    // never overrides the doc's own binding. A doc with no block inherits
    // its ENGINE's default set (doc.rs `engine_default_dials`; empty for
    // engines whose stock shader reads no user params). AI-generated
    // effects should always declare their dials.
    dials: [
        {name: "SWEEP", bind: "p0", default: 0.5},
        {name: "DRIVE", bind: "p1", default: 0.3}
    ]

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

    // THE LOOK — a SUBCLASS of the engine's draw shader (never a plain
    // object — that would replace the whole shader def and is rejected).
    // Every shipped preset carries this block; see "The shader hooks"
    // below for the family's signature and inputs:
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
are 0 until the host feeds `set_signals`), and `p0 p1 p2 p3` — the
RESOLVED user params for this frame (a touched host dial wins, else the
doc's own `p0:` binding), so ANY binding can route a dial:
`sway: "0.8 * (0.25 + 1.5*p0)"`. A p-param's OWN binding sees the other
p's as 0 — no cycles by construction. This is THE dial-routing idiom:
pick the expression so the dial's declared `default` reproduces the
stock value exactly (mid-knob multipliers like `(0.2 + 1.6*pN)` = 1.0
at pN 0.5), and set the doc's `pN:` constant to that default. The
emitters `frame:` tick sees the same values as `fx.p0..fx.p3`.
Constants `pi`, `tau`.
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
- `self.col_a/col_b/col_c/col_bg`, `self.fog` = (density, glow, CONTENT
  MIX, 0)

### The shader hooks — one look function per family

Every family exposes a named entry point the document replaces. The style
is the same everywhere: **`fx_color(t, attr, …) -> vec4`**, where `t` is
the family's primary look parameter and `attr` its raw vertex channels
(the per-engine table further down says what each channel holds). The
remaining arguments come in two shapes, decided by where the family's
content coupling lives:

- **`(normal: vec3, wpos: vec3)`** — the engine folds the content sample
  in AROUND the hook (a different shader stage, or a structural backdrop).
  Return whatever you like: the coupling keeps working.
- **`(content: vec4, cmix: float)`** — the classic look and the content
  sample compose in the SAME expression, so both live in the hook.
  `content` is the input0 texel the engine sampled for this fragment,
  `cmix` the pre-gated strength (`self.fog.z`, **0 whenever no real
  channel video is bound** — see Content coupling below). A rewritten look
  that ignores `cmix` simply has no coupling; it can never leak the
  fallback pattern, because the host gates the strength, not the hook.

| family (engine keys) | shader type | hooks — stage, signature |
|---|---|---|
| lsystem, grass, metaballs | `DrawVjFxMesh` | vertex: `fx_displace(pos, normal, attr) -> vec3`, `fx_color(t = arc/height 0..1, attr, normal, wpos)` |
| …with `wind_field:` | `DrawVjFxMeshField` | same two, same meaning (the wind comes from a sim field instead of the analytic sway) |
| heightmap | `DrawVjFxTerrain` | pixel: `fx_color(t = height 0..1, attr = (slope shade, grid line, grid uv.x, grid uv.y), content = drape texel, cmix)` |
| ribbons | `DrawVjFxRibbon` | vertex: `fx_color(t = trail age 0..1, attr, normal = tangent, wpos)` |
| tunnel | `DrawVjFxTunnel` | vertex: `fx_color(t = along the bore 0..1, attr, normal = inward, wpos)` |
| particles | `DrawVjFxParticles` | vertex: `fx_center(mode, id, dir, t01, cyc, r0, r1) -> vec3` (the MOTION program), `fx_color(t = life 0..1, attr, dir, wpos)`; pixel: `fx_sprite(uv, tint)` |
| emitters | `DrawVjFxEmitter` | vertex: `fx_color(t = life 0..1, attr = (id, seconds alive, ignition rnd, spread rnd), content, cmix)`; pixel: `fx_sprite(uv, tint)` |
| firefly | `DrawVjFxFirefly` | vertex: `fx_color(t, attr, content, cmix)` — ONE hook for both kinds, `attr.y < 1.5` a grass blade (t = along), `>= 2` a fly (t = blink brightness); pixel: `fx_sprite(uv, tint)` |
| harmonograph | `DrawVjFxHarmono` | vertex: `fx_color(t = curve position 0..1, attr, normal, wpos)` |
| domino | `DrawVjFxDomino` | vertex: `fx_color(t = topple ease 0..1, attr, normal, wpos)` |
| forge | `DrawVjFxForge` | vertex: `fx_color(t = flight heat 0..1, attr, normal, wpos)` |
| copperbars | `DrawVjFxCopper` | pixel: `fx_color(t = bar gradient axis 0..1, attr, normal, wpos)` |
| tiles | `DrawVjFxTiles` | pixel: `fx_color(t = mode highlight drive, attr = (tile shade, edge 0..1, stagger rnd, tumble rnd), content = this tile's texel, cmix)` |
| flock | `DrawVjFxFlock` | vertex: `fx_color(t = speed 0..1, attr, normal = banked up, wpos)` |
| mountainjet | `DrawVjFxJet` | pixel: `fx_color(t = element look parameter, attr = (class 0 land / 1 jet / 2 burner, same parameter, secondary, distance), content = drape texel, cmix)`, plus the hull-only `fx_jet_color(shade, part, edge)` |
| city | `DrawVjFxCity` | vertex: `fx_color(t = height 0..1, attr = (class, id, hash, height/phase), normal, wpos)` |
| pipes | `DrawVjFxPipes` | vertex: `fx_color(t = birth order 0..1, attr, normal, wpos)` |
| stockcharts | `DrawVjFxCharts` | vertex: `fx_color(t = candle age 0..1, attr = (element class 0..6, age, up/down, move size), normal, wpos)` |
| simswarm | `DrawVjFxSimSwarmDraw` | vertex: `fx_color(t = age 0..1, attr = (id, speed, rnd, state age), normal, wpos)`; pixel: `fx_sprite(uv, tint)` |
| fluid | `DrawVjFxFluidView` | pixel: `fx_shade(uv, dye, flow, base) -> vec4` — the ink over the (optionally warped) base |
| raymarch | `DrawVjFxRaymarch` | pixel: `scene_sdf(p) -> vec2` (THE scene — distance + material), `fx_palette(t) -> vec3` |
| transition | `DrawVjFxDuo` | pixel: `trans(uv, t) -> vec4` — the whole two-deck fragment |
| screen | `DrawVjFxScreen` | pixel: `fx_color(uv, content = input0 at uv, cmix) -> vec4` — THE WHOLE FRAME. OPT-IN: a screen doc with no `shader:` block keeps the classic path (input0 straight into the stage chain, no scene pass at all), because routing it through a blit would resample the input before the chain saw it. Declare the block and the pass runs — the cheapest way there is to author an effect from nothing (113_scan_sermon) |

Rules that hold for every hook:

- **A hook binds to ONE stage.** A fn the vertex path calls cannot also be
  called from `pixel` — the generated Metal signature mismatches and the
  whole shader dies. The table says which stage each hook runs in; write
  code for that stage.
- Hooks are methods: `self.time_beat`, `self.sig`, `self.user`,
  `self.anim`, `self.shape`, `self.flow`, `self.col_a/b/c/bg`, `self.fog`
  and the family's varyings (`self.v_*`) are all in scope, as are the
  shader's own helpers (`self.hash1`, `self.vnoise`, …).
- The block must SUBCLASS (`shader: draw.DrawVjFxMesh { … }`). A plain
  `{ … }` replaces the whole definition, leaves it without a vertex fn,
  and is rejected at parse with a warning in the widget status.
- Every distinct document body evaluates in its own content-addressed
  module, so 119 documents with 119 inline shaders is 119 shader modules
  compiled lazily on first draw — that is the design, not a leak. Measured
  cost of the whole migration: document load 0.42 → 0.83 ms each; a full
  load-and-render walk of the 119-document library 239 s → 241 s (+0.9%).

### Authoring a whole effect as pure document

The shortest path to a NEW look is: pick the family whose geometry and
motion you want, copy the preset closest to it, and rewrite its `fx_color`.
Nothing else is required — no Rust, no rebuild.

```text
// SONAR RINGS — a terrain flythrough that stops being neon-synthwave and
// becomes a depth sounder: concentric rings sweeping out of the horizon.
{
    name: "Sonar Rings"
    engine: "heightmap"          // geometry + motion come from the engine
    seed: 3
    size: 60.0  cells: 120  amp: 3.2  scroll: 2.4  ridged: 0.2
    beat_pulse: 0.4  fog: 0.045
    color_bg: #x00120e  color_a: #x0affc0  color_b: #x006a5a
    color_c: #xd8fff4
    dials: [{name: "SWEEP", bind: "p0", default: 0.5}]
    p0: 0.5
    stages: [{kind: "bloom", threshold: 0.4, strength: 1.3, levels: 3}]

    shader: draw.DrawVjFxTerrain {
        // t = height 0..1, attr = (slope shade, grid line, uv.x, uv.y),
        // content = the input0 texel on the land, cmix = its strength.
        fx_color: fn(t: float, attr: vec4, content: vec4, cmix: float) -> vec4 {
            // Range rings: distance from the sounder, swept by the beat.
            let d = length(vec2(attr.z - 0.5, attr.w - 0.5)) * 2.0
            let ring = fract(d * 9.0 - self.time_beat.y * 0.5)
            let edge = pow(1.0 - min(ring, 1.0 - ring) * 2.0, 12.0)
            // The land itself is a dark wash; the rings carry the picture.
            let bed = self.col_b.xyz * (0.10 + 0.35 * attr.x) * (0.4 + t)
            let lit = self.col_a.xyz * edge * (1.0 + self.time_beat.w * 1.4)
                + self.col_c.xyz * edge * t * 0.5
            // Content: the clip washes the bed, the rings stay the effect.
            let paint = bed.mix(content.xyz * (0.3 + 0.7 * attr.x), clamp(cmix * 1.2, 0.0, 1.0))
            return vec4(paint + lit, 1.0)
        }
    }
}
```

Drop that in `apps/vj/resources/effects/`, register it in `seed.rs`, and
`VJFX_DOC=<name> ./target/release/examples/effect_gallery` shows it. The
same file is what the store publishes and what the next author forks.

### Verifying a look change (the capture instrument)

The gallery renders deterministically on demand, which is how the shader
migration proved it changed no pixel:

- `VJFX_CAPTURE=<frames>` (or `<frames>@<dt>`) — after each document load
  the widget advances by a FIXED step for exactly that many frames, resets
  the beat with the document, then freezes. Whatever is on screen is a
  pure function of the document.
- `VJFX_SWEEP=<dir>` — walk every document, write one PNG each, quit.
  Existing PNGs are skipped, so an interrupted sweep resumes.
- `VJFX_ONLY=a,b,c` — restrict the sweep to matching document names.
- `VJFX_DIR=<dir>` — read documents from somewhere else (how one binary
  timed the pre- and post-migration document sets).
- `VJFX_INPUT=<image>` — bind real content on input 0; the coupling
  verify lever. Grab standalone AND with content.

The sweep runs a REAL windowed gallery — the GPU path, ~4 minutes for the
whole library. (A headless build renders the same documents on the CPU
rasterizer with JIT-compiled shaders; it is far too slow for a 120-document
sweep. Do not reach for it here.) The run self-terminates when the sweep
finishes; if you drive an instance by hand, `/gq` it when you are done —
never leave a window on the user's screen.

Two sweeps of two builds compare pixel for pixel. The floor is ±1 LSB:
feedback stages accumulate in float and a run can differ from itself by
one unit in the last place. **A document's frame also depends on which
document was shown before it** (the post chain keeps its textures across
loads), so compare sweeps taken in the SAME order.

### Content coupling (every engine PLAYS the video, it does not tint with it)

When a real channel video is bound to input 0 (the VJ's effect-pass
mode), EVERY engine family folds it into its look. **THE BAR: at the
default strength a viewer must INSTANTLY SEE THE VIDEO PLAYING in the
effect — a picture, not a tint.** Judge it at arm's length on a
with-content grab; "the palette shifted" is a fail. The effect's
identity stays primary (its geometry, motion and beat still own the
frame), but the clip has to be legible in it. The shared plumbing
(pinned names):

- Doc key **`content`** (animatable, 0..1, default 0.75): the coupling
  strength. It reaches every fx shader as **`self.fog.z`**, PRE-GATED by
  the host to 0.0 whenever input0 holds no real content — the animated
  fallback pattern and `field:` inputs gate it off, so a coupling can
  never leak the test pattern and `fog.z == 0` is BY LAW exactly the
  classic standalone look. Mix your classic term toward your content term
  by `self.fog.z`; tune so 0.75 reads plainly and 1.0 is video-dominant.
  (The first pass shipped 0.5 with ~0.35-0.65 family gains and was
  rejected live as "I can barely make out the video".)
- Uniform **`has_content: uniform(0.0)`** (declared on every fx family
  shader): the raw gate — 1.0 real content, 0.0 fallback/field — for
  BEHAVIORAL switches (engines like tiles/terrain/particles-image that
  deliberately render the fallback keep doing so; new couplings that
  want real-only behavior read this).
- Texture **`tex0`**: input0 is bound to texture slot 0 on every engine
  draw automatically (view.rs `draw_engine!`), EXCEPT the sim consumers
  — `DrawVjFxMeshField` (wind_tex) and `DrawVjFxSimSwarmDraw` (state_tex)
  own slot 0, their `tex0` is declared second and fed on slot 1 — and
  the duo transition engine (deck textures, never the fallback). Vertex
  stage sampling: `sample_nearest(uv, 0.0)`.
- Presets: a doc may re-bind `content` (`content: "0.3 + 0.6*p2"`) or
  declare a dial on a p-param routed into it — but never break an
  existing 3-dial set for it; a bare `content:` key without a dial is
  fine. Docs that omit the key get the 0.75 default.

**STRUCTURE BEATS GAIN.** Two shapes of coupling, chosen by what the
family actually puts on screen:

- **Drape / project / mirror** — families that fill the frame with
  surface (terrain, mountainjet's range, tunnel bore, city facades and
  streets, raymarch materials, tiles, metaball glass, pipe and copper and
  forge metal). The video becomes the surface: it REPLACES the base
  colour under the family's own lighting rather than tinting it, because
  a tint on a dark ramp is a mood, not a picture. Pick the mapping that
  glues it to the geometry — scrolled grid uv for terrain, per-tower
  facade uv for the city, planar-by-dominant-normal for the marcher,
  screen uv nudged by the normal for anything that reads as metal or
  glass (a mirror-direction env map squeezes a whole frame into a few
  degrees of normal and always reads as a smear).
- **The shared CONTENT BACKDROP** — families whose classic look is
  BRIGHT SPARSE GEOMETRY over a near-black field (particles, emitters,
  ribbons, plants, flies, pen, dominoes, pipes, shards, bars, swarms).
  No gain on a few thousand thin triangles can carry a picture, so the
  picture goes BEHIND them: `DrawVjFxBackdrop` (shaders.rs) draws one
  clip-space quad at the far plane, writing no depth, before the engine
  draw. Per-family dim lives in ONE table — `VjFxView::backdrop_level`
  (view.rs); 0 opts a family out and the host then skips the draw
  entirely, which is what keeps `content: 0` bit-exact. Flock and
  stockcharts predate it and keep their own in-mesh backdrop quads.
  The two shapes compose: a backdrop family should ALSO take the video
  into its geometry, so the effect paints the clip instead of floating
  over it.

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
| flock | bird id | speed01 (2.0 = the content-backdrop quad, clip-space, screen uv) | hue | FLAP AMPLITUDE at this vertex (0 spine, wingspan at tips) | (along-body 0..1, flap phase hash) | banked UP vector (the flap axis) |
| city | tower/trail id | CLASS: 0 tower 1 ground 2 sky 3 trail | tower hash / trail hue | tower height / trail phase | tower: facade uv in WINDOW units; ground: world xz; sky: (azimuth01, h01); trail: (arc01, h01) | face normal |
| pipes | pipe id | birth order 0..1 (THE growth axis) | pipe hue | local radius (balls bulged) | (around01, along/elevation) | radial outward |
| stockcharts | element class 0..6 (body/wick/grid/crosshair/MA/tick/content-backdrop) | candle age 0..1 | up/down (crosshair: axis) | move size 0..1 | quad-local uv | +Z |
| raymarch | corner idx | 0 | 0 | 0 | screen uv (0,0 = top-left) | +Z (pos = CLIP-SPACE corner; the pixel shader is the whole effect) |
| mountainjet (terrain) | checker cell | 0 | vertex hash | 0 | grid uv | +Y |
| mountainjet (jet hull) | 0 | 2.0 + part hash | face shade tint | 0 | face uv (edge-wire material) | LOCAL face normal (pos = LOCAL jet coords, nose = -z) |
| mountainjet (burner) | 0 | 4.0 + flicker seed | flicker seed | along plume 0..1 | (across, along) | +Z |

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
`raymarch` (fullscreen SDF marcher; the scene is a `scene_sdf` shader
SUBCLASS — engines_raymarch.rs documents the contract + SDF toolkit),
`mountainjet` (endless range + view-space fighter jet, three looks,
engines_jet.rs),
`screen` (no mesh — input0 straight into the stage chain: the fullscreen
effect family; a doc that declares a `shader:` block gets a real
clip-space pass through its own `fx_color` instead, which is the whole
effect — see 113_scan_sermon). Engine keys: see the module docs in
`mod.rs` (kept current) and `engines.rs`.

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
  rect with composite on), feed a few beats, grab frames. The VJ-side
  cache is fx_thumbs.rs. TRANSITION docs sheet the COMPLETE sweep:
  frame k of N renders with `p3 = k/(N-1)` (frame-indexed — first frame
  pure deck A, last pure deck B), never a wall-clock capture window.
- Offscreen heartbeat idiom (status quo): every offscreen host (fx slots,
  thumbs, mesh/splat/flow) lives as a 4x4 widget in the always-drawn top
  bar — its draw_walk both orchestrates the child pass AND issues the
  sample draw that IS the pass dependency. The 4x4s stack in ONE overlay
  slot under a bar-colored cover so no sampled pixel shows. The cleaner
  design — no parked widgets, passes driven per frame via
  `Cx::repaint_pass` from the pump — first requires lifting the hosts'
  pass orchestration out of draw_walk (they only run when walked); until
  that refactor, the cover-quad idiom is the law.
- Host param overrides (the VJ's EFFECT-SLOT knobs, fx_slot.rs):
  `set_user_override([Option<f32>; 4])` pins any of `p0..p3` over the
  document's binding (`None` = the doc's value stays in charge), and
  `set_speed_scale(f32)` multiplies the document's own clock. TRANSITION
  slot convention: two-input (`engine: "transition"`) docs get `p3` = the
  crossfader position itself, and the doc-declared `engage:` profile
  ("triangle" default, "ramp" for overlay/key docs that stay applied at
  the B end) decides how the host rides them. Input0-only docs keep the
  premix path: the host drives `p3` with `triangle(program_mix)` (0 at
  the fader ends, 1 mid-fade), so a transition-suited document can bind its
  intensity to `p3`. Transition-suited presets carry the `transition`
  catalog tag (seed.rs `TRANSITION_PRESETS`/`TRANSITION_TAG`).

## Preset conventions

- Directory: `apps/vj/resources/effects/*.splash`. Naming:
  `NN_snake_name.splash` (NN orders the gallery; name is the identity).
- Every preset: distinct look (rules/geometry/bindings, not a recolor),
  comments that TEACH the pattern it demonstrates, `#x` colors, and at
  least one beat-aware element.
- **Every preset carries its own `shader:` block** — the family's look
  function written out inline, even when it is the family default and even
  when its neighbour carries the same code. That is what makes the file a
  forkable unit (see the top of this file). The ten shipped `screen`
  presets are the one exception: that family's classic path has no scene
  pass to carry (its look IS the stage list). A screen doc that DOES
  declare a shader gets a real fullscreen pass — 113_scan_sermon.
- Verify before calling done: `VJFX_DOC=<name> ./target/release/examples/
  effect_gallery --remote`, grab via `curl :PORT/g`, LOOK at the PNG.
  Cycle: `/t?t=n` (next), `/t?t=p` (prev), `/t?t=g<name>` (jump). Close
  with `/gq`. Never leave an instance running. `VJFX_INPUT=<image path>`
  binds a real image as channel content — when touching a content
  coupling, grab the family standalone AND with content (the standalone
  grab must stay its classic self).

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
- A shader helper fn binds to ONE stage: a fn the vertex path calls
  (directly or transitively) cannot also be called from `pixel` — the
  generated Metal signature mismatches and the whole shader dies. Inline
  the math on the second stage instead (engines_jet.rs grain).
- Wind/growth displacement must be a function of CONTINUOUS per-vertex
  data (rest position, arc length) or connected geometry tears.
- **Endless-terrain scroll sign.** The heightmap grid (and the jet's twin
  of it) runs `uv.y` 0 at `z = -size/2` and 1 at `z = +size/2`, and the
  camera sits at `+z` looking down `-z` — so `uv.y` INCREASES toward the
  viewer. Sampling the field at `uv + scroll` walks features to
  DECREASING `uv.y` as scroll grows: away from the camera, which reads as
  flying BACKWARDS (it shipped that way and was reported). The scroll is
  NEGATIVE; grid lines and the content drape must carry the same sign or
  the land and its markings slide against each other.
- Content couplings that BRIGHTEN (papered tunnel walls, drapes, mirror
  reflections) run under the family's bloom/glow stages: a coupling tuned
  to full level on the raw pass blooms the whole frame to white. Keep
  the transferred level under 1 and check a preset that ships bloom.
