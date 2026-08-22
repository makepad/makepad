//! # VJ effects — mesh-generating, splash-configured, beat-driven
//!
//! A **vj effect** is a normal makepad component ([`view::VjFxView`]) whose
//! entire configuration surface is a splash document (a makepad-script
//! source evaluated at load time) plus a shader-hook API. The document names
//! one mesh-generating ENGINE, sets its parameters (including little
//! programs like L-system rules), declares an optional multi-stage render
//! chain, and may override the shader hooks. The effect asset of the future
//! is exactly such a document — AI-authorable text.
//!
//! Architecture laws (why it is shaped this way):
//! * **Splash is configuration, never the frame loop.** Documents are
//!   evaluated once at load; a frame is Rust engine update (usually a
//!   no-op) + uniform writes + GPU draw.
//! * **Data rides the vertex stream.** Engines encode what they know onto
//!   per-vertex attributes (particle id/seed, branch depth, birth order,
//!   trail age, tube radius) once; the vertex shader animates from those
//!   attributes plus time/beat uniforms every frame. Mesh regeneration is
//!   the slow path (metaballs, ribbons — capacity-stable recycled buffers);
//!   attribute-driven shader animation is the fast path (particles,
//!   l-system, heightmap, tunnel upload their geometry exactly once).
//! * **Little interpreters run at load.** The L-system text compiles to a
//!   bytecode op stream and a turtle emits the mesh — at (re)build time,
//!   never per frame.
//!
//! ---------------------------------------------------------------------------
//! # THE EFFECT DOCUMENT CONTRACT (what an LLM writes)
//!
//! A document is a `.splash` file: makepad-script whose **last expression is
//! one flat object**. `let` bindings and expressions above it are fine.
//! Missing keys default; wrong-typed keys default with a warning; unknown
//! keys are ignored. Colors are literals like `#x40f0ff` (always use the
//! `#x` prefix). Everything is flat and named — no nesting except `stages`
//! and `shader`.
//!
//! ```text
//! {
//!     name: "Neon Growth"          // display name
//!     engine: "lsystem"            // REQUIRED: particles | lsystem |
//!                                  // metaballs | heightmap | ribbons | tunnel
//!     seed: 7                      // any integer; reroll for variation
//!
//!     // ---- shared animation (all engines) ----
//!     speed: 1.0                   // master time multiplier
//!     beat_pulse: 0.5              // 0..2 how hard the beat pumps scale/glow
//!     beat_rate: 1.0               // pulses per beat (2 = eighths)
//!     sway: 0.4                    // vertex sway amplitude (mesh engines)
//!     sway_freq: 0.9               // sway oscillations /sec
//!     twist: 0.0                   // static twist around Y by height
//!     fog: 0.045                   // distance fade density (0 = none)
//!     glow: 1.0                    // emissive gain
//!     grow: "loop"                 // off | loop | pingpong — sweeps the
//!                                  // growth front over birth order
//!     grow_beats: 8                // beats per growth sweep
//!
//!     // ---- palette ----
//!     color_bg: #x05060f           // clear/fog color
//!     color_a: #x40f0ff            // primary
//!     color_b: #xff40a0            // secondary (gradient end)
//!     color_c: #xffffff            // accent / beat flash
//!
//!     // ---- camera (auto-framed when omitted) ----
//!     cam_dist: 8.0                // orbit distance
//!     cam_height: 2.5              // eye height
//!     cam_orbit: 0.12              // radians/sec orbit (0 = locked)
//!     cam_fov: 50.0
//!
//!     // ---- texture input (the "extra slot") ----
//!     input0: "test"               // "test" = gallery test pattern; in the
//!                                  // VJ a channel's main content lands here
//!
//!     // ---- multi-stage render chain (ordered, max 4) ----
//!     stages: [
//!         {kind: "bloom" threshold: 0.5 strength: 1.4 levels: 3}
//!         {kind: "feedback" amount: 0.86 zoom: 1.012 rotate: 0.004 dim: 0.97}
//!         {kind: "blur" levels: 2}
//!     ]
//!
//!     // ---- shader hooks (optional, advanced) ----
//!     // Fn overrides compiled into the engine's draw shader at load.
//!     shader: {
//!         fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
//!             return vec4(fract(t * 3.0), 0.5, 1.0 - t, 1.0)
//!         }
//!     }
//! }
//! ```
//!
//! ## Engine parameter reference
//!
//! ### `engine: "particles"` — stateless GPU particles
//! The CPU uploads one quad per particle ONCE; the vertex shader re-derives
//! every position per frame from (id, seeds, time, beat). Keys:
//! * `mode`: `burst` (staggered firework shells) | `fountain` | `tunnel`
//!   (rings streaming past the camera) | `vortex` (rising helix swarm) |
//!   `rain` | `galaxy` (spiral disc) | `image` (particles carry input0's
//!   pixels and dissolve on the beat — needs `input0`)
//! * `count` (4000, ≤30000), `spread` (6.0, world extent), `size` (0.10)
//! * `rate` (0.4 respawn cycles/sec), `gravity` (1.0), `swirl` (1.0),
//!   `stretch` (1.0, sprite elongation for rain/streaks)
//! * hooks: `fx_center(mode, id, dir, t01, cyc, r0, r1) -> vec3` (motion
//!   program!), `fx_sprite(uv, tint) -> vec4`, `fx_color`
//!
//! ### `engine: "lsystem"` — L-system growth (the program-parameterised one)
//! * `axiom`: start string, e.g. `"X"`
//! * `rules`: list of `"SYMBOL=REPLACEMENT"`, e.g.
//!   `["X=F[+X][-X]FX" "F=FF"]`. Alphabet: `F` draw, `f` move, `+ -` yaw,
//!   `& ^` pitch, `/ \\` roll, `[ ]` push/pop branch, `!` shrink radius,
//!   `'` shift hue; other letters are structure symbols.
//! * `iterations` (5, ≤12 — expansion stops at a 60k-segment budget),
//!   `angle` (25.7°), `angle_jitter` (0), `step` (0.16), `radius` (0.045),
//!   `radius_decay` (0.82 per `!`), `sides` (5, tube cross-section 3..8),
//!   `copies` (1, plants arranged in a ring)
//! * Growth animation: set `grow: "loop"` + `grow_beats` — branch depth
//!   sways, segments appear in birth order. hooks: `fx_displace`, `fx_color`
//!   (t = birth order, attr = (depth, birth, hue, radius)).
//!
//! ### `engine: "metaballs"` — marching-tetrahedra iso-surface
//! Regenerated on the CPU every frame (the honest slow path; see perf notes
//! in the module). Blob orbits are closed-form.
//! * `blobs` (6, ≤12), `grid` (30, ≤48 cells/axis), `extent` (3.0),
//!   `blob_radius` (0.95), `orbit` (1.5), `orbit_speed` (0.7),
//!   `beat_swell` (0.35 — radii pump on the beat)
//!
//! ### `engine: "heightmap"` — terrain flythrough, displaced in the shader
//! Static grid; height = 3-octave fbm scrolled by time (or input0
//! luminance), synthwave neon-grid shading.
//! * `res` (110, ≤220), `size` (30), `height` (2.4), `noise_scale` (0.16),
//!   `scroll` (2.2 world-units/sec), `ridged` (0..1 canyon shaping),
//!   `tex_displace` (0..1 blend to input0-driven height — needs `input0`)
//!
//! ### `engine: "ribbons"` — flow-field ribbon trails
//! CPU steps ribbon heads through an analytic curl field; strips rebuilt per
//! frame (small, capacity-stable), expanded view-facing in the shader.
//! * `ribbons` (28, ≤96), `trail` (56 points, ≤160), `width` (0.10),
//!   `flow_speed` (2.0), `swirl` (1.6), `bound` (5.0)
//!
//! ### `engine: "tunnel"` — torus-knot tube flown from inside
//! Static tube; camera flies the knot; neon rings sweep with the BEAT.
//! * `knot_p` (2), `knot_q` (3), `major` (6 path radius), `tube` (1.35),
//!   `rings` (720), `sides` (22), `fly` (3.2 laps/min), `bands` (90 ring
//!   count along the tube)
//!
//! ### `engine: "firefly"` — a meadow of fireflies synced by the music
//! Static grass + fly sprites; blink phase = mix(intrinsic clock, beat
//! phase, sync) — quiet = chaos, loud = one heartbeat.
//! * `flies` (700), `blades` (6000), `area` (8), `fly_height` (1.7),
//!   `fly_size` (0.10), `sync` (0.35 base), `blink_rate` (0.5/s),
//!   `blink_sharp` (7), `wander` (0.55), `moon` (0.35), `grass_height`
//!   (0.8), `clump` (0.5)
//! * bindings: `p0` ADDS to sync (the emotional arc), `p1` scales wander
//! * hooks: `fx_sprite` (see engines_firefly.rs)
//!
//! ### `engine: "harmonograph"` — damped pendulum ribbon, morphing per bars
//! Position is pure vertex-shader math; a new figure is drawn from a hash
//! every `morph_beats` beats and eased in; `grow: "loop"` draws pen-first.
//! * `segments` (1600), `strands` (1..6), `freq_x`/`freq_y`/`freq_z`
//!   (2/3/1 — the figure family), `damping` (0.45), `detune` (0.08),
//!   `turns` (5), `width` (0.07), `morph_beats` (16)
//! * bindings: `p0` scales detune, `p1` pumps width, `p2` adds z-depth,
//!   `p3` spins hue; hooks: `fx_color` (see engines_harmonograph.rs)
//!
//! ### `engine: "domino"` — toppling run, front = beat * per_beat
//! Static boxes along a generated path; the topple front travels at
//! exactly `per_beat` dominoes per beat, `resurrect` sweeps it back.
//! * `layout` (spiral/serpent/tree), `count` (900), `per_beat` (4),
//!   `spacing` (0.30), `tile_h`/`tile_w`/`tile_t` (0.62/0.34/0.085),
//!   `branches` (5), `jitter` (0.12), `resurrect` (1), `pause_beats` (2),
//!   `flash` (1.2)
//! * bindings: `p0` nudges the front (beats), `p1` adds impact flash,
//!   `p2` adds anticipation glow; hooks: `fx_color` (engines_domino.rs)
//!
//! ### `engine: "forge"` — kick-launched shard pile on a drum membrane
//! Static shards + membrane disc; every pulse re-hashes stateless ballistic
//! launches (`v·t − ½g·t²` from phase/bpm/seeds) and rings the membrane.
//! * `shards` (2000, ≤6000), `radius` (4), `impulse` (7), `gravity` (42 —
//!   high gravity = HIGH jumps: launches land before the next hit, so
//!   reachable height is g·T²/8), `spin` (1), `membrane_wave` (0.5),
//!   `shard_size` (0.16), `scatter` (0.55), `falloff` (0.55), `pile`
//!   (0.55), `auto_pump` (1 — constant launch floor for the free-running
//!   clock; 0 = silence-still with real audio), `glint` (1)
//! * bindings: `p0` = impulse gain (THE binding — `"0.6 + 2.6*bass"`),
//!   `p1` adds membrane wave, `p2` boosts glints (hats);
//!   hooks: `fx_color` (t = flight heat — see engines_forge.rs)
//!
//! ### `engine: "copperbars"` — rasterbar slabs with beat choreography
//! Static full-width boxes; the VS runs a per-bar choreography (`mode`:
//! sine / pile / scissor / curtain), crossfading to `mode_b` by `p3`.
//! * `bars` (24, 4..64), `mode`/`mode_b`, `width` (15), `span` (6.5),
//!   `thickness` (0.42), `depth` (1.2), `amplitude` (1.6), `weave` (1.2),
//!   `metal` (3 gradient hardness), `drop` (7 pile drop height)
//! * bindings: `p0` = amplitude gain, `p1` = thickness pump
//!   (`"0.4 + 0.9*env(phase)"`), `p3` = mode crossfade 0..1;
//!   hooks: `fx_color` (the bar gradient fn — see engines_copper.rs)
//!
//! ### `engine: "tiles"` — the input image shattered into a tile grid
//! One textured quad per tile; each tile carries its uv window + grid
//! coords + seeds on the stream and the vertex shader runs an endless
//! motion program. The pixel stage samples input0 (animated dummy when
//! nothing is bound). The engine flies its own gently swaying front-on
//! camera (doc cam keys ignored, tunnel-style).
//! * `mode`: `wave` (traveling swell, tiles tilt with the slope) |
//!   `shatter` (BAR-SYNCED explode + pixel-perfect reassembly) |
//!   `conveyor` (endless belt, alternate rows opposite ways, edge rolls) |
//!   `spiral` (differential whirlpool with a beat funnel)
//! * `grid` (24, 4..64 per side), `spread` (7 plane width), `aspect` (1.0),
//!   `gap` (0.06 grout), `amp` (0.5), `freq` (1.0 — wave freq / conveyor
//!   speed / spiral spin), `spin` (1.0 shatter tumble), `scatter` (1.2
//!   shatter flight distance)
//! * bindings: `p0` ADDS shatter drive (strobe the explosion), `p1` scales
//!   wave amp, `p2` adds grout glow; hooks: `fx_tint(c, attr, flash)`
//!
//! ### `engine: "flock"` — boid murmuration of oriented gliders
//! CPU boids (O(N²), regen-per-frame family) emit banked wing/fin
//! triangles; the wing FLAP runs in the vertex shader off per-vertex flap
//! amplitude + phase. The goal point jumps every `goal_beats` beats (the
//! flock swings with the music); an optional predator dives through the
//! flock on every bar.
//! * `birds` (320, ≤600), `size` (0.14), `flight_speed` (2.4), `flap`
//!   (3.0/s), `bound` (6.0), `spacing` (0.45), `vision` (1.6),
//!   `goal_beats` (2), `predator` (0..1 bar-scatter), `additive` (0 dusk
//!   silhouettes .. 1 neon), `bank` (1.0 roll-into-turns)
//! * hooks: `fx_color` (t = speed01 — see engines_flock.rs)
//!
//! ### `engine: "grass"` — waving meadow (thousands of blades)
//! * `blades` (7000, ≤20000), `area` (9 half-extent), `height` (0.85),
//!   `width` (0.035), `clump` (0..1 clustering). Gusts travel: the wind is
//!   the same continuous field the L-systems sway in.
//!
//! ### `engine: "emitters"` — scriptable emitters (programmable fireworks)
//! * `particles` (768 sheet size), `size`, `gravity`, plus the per-frame
//!   `frame: fn(fx) { ... return [spawns] }` tick — see CONTRACT.md for the
//!   spawn-object keys, budget, and the slot-respawn = move rule.
//!
//! ### `engine: "raymarch"` — fullscreen SDF raymarcher, subclassable scene
//! One quad; the pixel shader sphere-traces a distance field. THE variant
//! mechanism is the `scene_sdf` shader-hook subclass (see
//! engines_raymarch.rs for the contract + the SDF toolkit helpers).
//! * `steps` (64, 16..120 — the march budget/cost dial), `max_dist` (40),
//!   `cam` (orbit/fly/dolly), `cam_speed` (0.25), `cam_dist` (7),
//!   `cam_height` (2.2), `shadow` (1 — 0 skips the soft-shadow march)
//! * shared `twist` bends the whole field; `p0`-`p2` = free scene levers,
//!   `p3` = glass IOR bend; hooks: `scene_sdf`, `fx_palette`
//! * a scene material < 0 marks GLASS: the ray refracts once (Snell) and
//!   samples input0 — the optics family (needs `input0`)
//!
//! ### `engine: "mountainjet"` — endless mountains + a foreground fighter jet
//! Terrain grid (heightmap streaming technique) + a primitive-built jet in
//! VIEW space (banked weaving turns, beat-pulsed afterburner) in one static
//! stream (engines_jet.rs).
//! * terrain: `res` (120), `size` (34), `height` (4.6), `noise_scale`
//!   (0.17), `scroll` (3.2), `ridged` (0.7), `cells` (40)
//! * `look`: `solid` (alpenglow) | `wire` (Battlezone vector) |
//!   `nightvision` (mono ramp + grain)
//! * jet: `jet_size` (1.0), `weave` (1.0)
//! * bindings: `p0` ADDS afterburner, `p1` scales weave rate, `p2` adds
//!   glow; hook: `fx_jet_color`
//!
//! ### `engine: "city"` — procedural city flyover, banking camera
//! Static towers + ground + sky + (tron) trail walls; windows are a PIXEL
//! grid of the facade uv, per-window flicker = hash(tower, window,
//! floor(beat)); the camera drifts a Lissajous over the blocks and BANKS
//! (view-space roll — see engines_city.rs CAMERA CONTRACT).
//! * `style`: `night` | `retro` (wireframe-neon + sun-stripe horizon) |
//!   `tron` (grid floor + beat-swept light trails)
//! * `blocks` (8, ≤14), `block` (6 pitch), `street` (0.34), `towers`
//!   (240 cap), `max_h` (10), `win` (0.30), `density` (0.55 lit),
//!   `flicker` (0.12 re-rolled/beat), `trails` (0, ≤16), `trail_beats`
//!   (8), `wall_h` (1.3), `alt` (auto), `fly` (1.0), `bank` (1.0)
//! * bindings: `p0` ADDS window density (city wakes with energy), `p1` =
//!   sun/trail-head gain, `p2` = beacon/edge gain; hook: `fx_color`
//!
//! ### `engine: "pipes"` — the 3D-pipes lattice, growth replayed in tempo
//! Self-avoiding turtle walks with elbow balls; stuck pipes teleport (the
//! screensaver respawn). Birth order rides the stream; `grow: "loop"` +
//! `grow_beats` replays the build on the beat clock — segments POP in with
//! an overshoot bulge, the newest length burns white-hot, the loop is the
//! respawn (engines_pipes.rs).
//! * `pipes` (6, ≤16), `bound` (6 cells, ≤10), `cell` (0.55), `radius`
//!   (0.16), `sides` (10), `steps` (900, ≤2600), `turn_chance` (0.35),
//!   `pop` (0.4 overshoot), `hot` (2.5 %-of-run hot tail)
//! * bindings: `p0` nudges the growth front (kick lurch), `p1` = hot-tail
//!   gain, `p2` = specular gain; hook: `fx_color`
//!
//! ### `engine: "stockcharts"` — candlestick terminal (regen family)
//! Random-walk OHLC on the CPU; candles COMMIT at `per_beat` per beat
//! (beats detected from the phase wrap), the live candle ticks with
//! beat-spiked volatility, cascades slam multi-beat crashes. NO text/
//! numbers (no glyph path in the fx stack — ticks are dash quads).
//! * `candles` (96, ≤400), `per_beat` (1), `vol` (0.9 %/step), `drift`
//!   (0), `spike` (1.2), `cascade` (0..1 chance/bar), `bar` (4),
//!   `width`/`height` (14/7), `body_w` (0.62), `ma` (12, 0 = off),
//!   `grid_x` (4), `grid_y` (7), `scan` (14 scanlines, 0 = off)
//! * bindings: `p0` = brightness, `p1` = crosshair glow, `p2` = panic red
//!   wash 0..1; hook: `fx_color` (engines_charts.rs)
//!
//! ### `engine: "screen"` — fullscreen family (no mesh)
//! input0 goes straight into the stage chain; the warp stages (`kaleido`,
//! `mirror`, `chroma`, `pixelate`, `swirl`, `ripple`, `glitch`,
//! `posterize`, `radial_blur`, `warp_tunnel`) plus `blur`/`bloom`/
//! `tiltshift`/`feedback` do the work.
//!
//! ## Binding expressions (per-frame music sync)
//! Most numeric animation keys and all numeric stage parameters accept a
//! STRING expression evaluated every frame against the music signals
//! (`time dt beat phase bar bpm pulse energy bass mid high`; fns
//! `sin cos abs floor fract sqrt tri saw env min max pow step clamp mix`).
//! Full grammar + the standard shader signal block: `CONTRACT.md`.
//!
//! ## Hosting modes
//! * **Primary**: the effect IS a channel's content — `VjFxView` in the
//!   slot, `composite: false`, host composites `output_texture()`.
//! * **Effect-pass**: the channel's main content renders to a texture and
//!   arrives as `input0`; the effect's output replaces the channel output
//!   (`image` particles and `tex_displace` heightmap are built for this).
//!   With nothing bound, a built-in ANIMATED test pattern stands in — a
//!   texture effect always renders something.
//! ---------------------------------------------------------------------------

pub mod doc;
pub mod engines;
pub mod engines_charts;
pub mod engines_city;
pub mod engines_copper;
pub mod engines_domino;
pub mod engines_firefly;
pub mod engines_flock;
pub mod engines_forge;
pub mod engines_harmonograph;
pub mod engines_jet;
pub mod engines_pipes;
pub mod engines_duo;
pub mod engines_raymarch;
pub mod engines_simfx;
pub mod engines_tiles;
pub mod expr;
pub mod lsys;
pub mod mesh;
pub mod post;
pub mod seed;
pub mod shaders;
pub mod sim;
pub mod view;

pub use doc::EffectDoc;
pub use view::VjFxView;
use makepad_widgets::*;

/// Register the fx draw shaders + the widget. Call from the app's
/// registration chain after `makepad_widgets::script_mod`.
pub fn script_mod(vm: &mut ScriptVm) {
    shaders::script_mod(vm);
    engines_firefly::script_mod(vm);
    engines_harmonograph::script_mod(vm);
    engines_domino::script_mod(vm);
    engines_forge::script_mod(vm);
    engines_copper::script_mod(vm);
    engines_tiles::script_mod(vm);
    engines_flock::script_mod(vm);
    engines_duo::script_mod(vm);
    engines_raymarch::script_mod(vm);
    engines_jet::script_mod(vm);
    engines_city::script_mod(vm);
    engines_pipes::script_mod(vm);
    engines_charts::script_mod(vm);
    sim::script_mod(vm);
    view::script_mod(vm);
}
