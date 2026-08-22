# VJ effects — idea ledger

Every idea from the user's directives, mapped to its status. Statuses:
**done** (shipped + visually verified), **partial** (shipped, listed gap),
**planned** (designed, not built — with the design note), **rejected**
(with reason). This file is the audit surface — keep it current.

## Architecture asks

| idea | status | notes |
|---|---|---|
| Rust mesh generators, splash as configuration (never the frame loop) | done | doc eval at load; frame = uniforms + regen; measured costs in the final report |
| Parameterise vertex/pixel shaders from the doc (codegen/subclass) | done | `shader: draw.DrawVjFxMesh{ fx_color: fn()... }` subclass contract; recompiles at load; verified (15_acid_bloom) |
| Shader SUBCLASSING as the variant mechanism | done | subclass-or-rejected validation at parse (a plain `{}` used to abort the app) |
| Encode data on the vertex stream, animate in the vertex shader | done | per-engine attribute tables in CONTRACT.md; all engines |
| Little bytecode interpreters (L-system rules → bytecode at load) | done | lsys.rs compiler + turtle |
| Beat/music signals into EVERY shader automatically | done | `time_beat`/`sig`/`user` instance blocks on all fx shaders |
| Splash-configurable per-frame BINDINGS to beat curves | done | expr.rs compiled expressions; any animatable + stage params |
| Per-frame splash TICK at emitter granularity ("like our game engine") | done | emitters engine `frame:` fn, 200k-instruction budget, spawn-list return, slot-respawn = move |
| 1–2 texture inputs, primary + effect-channel modes | partial | input0 wired end-to-end (effect-pass mode incl.); input1 not yet plumbed |
| Multi-stage render graph in the doc (flat ordered list) | done | stages: feedback/bloom/blur/tiltshift + 10 warp modes |
| Animated fallback/dummy input texture (first-class runtime) | done | beat-aware procedural pattern auto-bound when input0 empty |
| Lazy ANIMATED thumbnails rendered by the VJ | partial | seeded rows carry real (non-flat) placeholder JPEGs; the runtime hook (slot-mode offscreen render + `output_texture()`) exists and is documented in CONTRACT.md — the VJ grid's lazy render-and-replace pass is the remaining integration |
| `vjeffect` asset kind + presets published to the store | done | `AssetKind::VjEffect` (canon 14, wire "vjeffect") additive across data/client/chat/store/importer/asset-ui; startup seeding in main.rs (publish-if-absent by alias `vjfx/<name>`, detached worker, empty-store first launch = full library as real rows) |
| ~100 preset library | partial | 72 shipped + every one LOOKED AT (contact-sheet sweep); CONTRACT.md enables parallel preset agents; manifest in resources/effects/MANIFEST.md |
| Empty-store first launch renders everything | done (by design + gallery) | no preset references store content; texture inputs fall back to the built-in animated pattern; the gallery run IS the no-store render test |

## Engine families

| idea | status | notes |
|---|---|---|
| Particles: fireworks bursts | done | shell-phase-quantized stateless GPU bursts (01) |
| Particles: flow tunnels | done | mode "tunnel" + feedback warp (02) |
| Particles: vortex swarms | done | braided-strand vortex (03) |
| Particles: galaxy / starfield | done | differential-rotation spiral (04); starfield = tunnel-mode preset territory |
| Particles: rain/spark curtains | done | streak sprites (05) |
| Image dissolve (video → particles) | done | mode "image", texel-carrying particles (13) |
| L-systems (growth, connected branches, depth sway) | done | arc-continuous wind (the user's "little trick"), branch depth+index on the stream, growth front; presets 06/07/15 |
| L-system preset spread (oak/fern/vine/coral/lightning/crystal/dragon…) | partial | 3 shipped; batch of preset docs next (engine supports all of them today) |
| Metaballs / iso-surfaces | done | marching tetrahedra, per-frame CPU (08); ~2.4 ms at grid 32 |
| Heightmap terrain flythrough | done | synthwave + ridged canyon (09/10); input-luma displacement (14) |
| Ribbons / flow fields | done | 11; speed01 on the vertex stream |
| Supershape/torus-knot tunnels | done | parallel-transport tube, beat rings (12/20) |
| Fullscreen effect family (kaleido/chroma/glitch/radial blur/etc.) | done | `screen` engine + warp stages (16/17); also stack on any mesh engine |
| Multipass texture chains (blur/bloom/tiltshift/feedback) | done | pyramid honors the bicubic/tent re-home law; 18 |
| Video→particles→tiltshift hybrid | planned | all pieces exist (image particles + tiltshift stage) — preset to write; luma-driven size/lift needs a 3-line shader addition |
| Tile-wave of the input image (textured-quad particles) | done | `tiles` engine (engines_tiles.rs): per-tile uv window derived from a_id, baked flight vectors, own shader sampling input0; modes wave / bar-synced shatter (blast on the downbeat, holds reassembled) / conveyor / spiral; presets 79/80/81 verified |
| Scriptable fireworks choreography | done | 19_fireworks_show — beat-sequenced launches, pops, rings, sparkle field from the doc's frame fn |
| Clouds (billboard puff clusters) | done | particle mode "clouds" (cluster centers + soft sprites + wind wrap); calm preset 33 (sunset cumulus, tiltshifted) + storm preset 84 (lightning-inside-the-cloud via gated beat_pulse binding + fx_color subclass) — both verified |
| Flock / murmuration | done | `flock` engine (engines_flock.rs): CPU boids (O(N²), 0.77 ms @ 500 birds) emit banked glider tris per frame; wing flap in the VS off per-vertex amplitude+phase; goal jumps every `goal_beats`, drifting anchor keeps it streaming, predator scatters on the bar; presets 82/83 verified |
| Animated pipes (3D-pipes lattice) | done | `pipes` engine (engines_pipes.rs): self-avoiding lattice turtles + bulged elbow balls, teleport respawn; birth order on the stream, growth REPLAYED by grow/grow_beats (pop-in overshoot + hot tail); presets 96 (calm homage) / 97 (kick-lurch industrial) verified |
| Procedural city flyover | done | `city` engine (engines_city.rs): static towers/ground/sky/trails, facade uv baked in WINDOW units, per-window flicker = hash(tower, window, floor(beat)); engine-authored Lissajous cam with view-space banking (CAMERA CONTRACT); presets 93/94/95 verified |
| Boned/animated dancers (splash-constructed puppets) | planned | bones as uniforms (≤32 mat4 or pos+quat vec4 pairs), per-vertex bone index on the stream, binding-driven bone curves; crowd via instances |
| Stockcharts (procedural candlesticks, retro terminal) | done | `stockcharts` engine (engines_charts.rs): beat-committed OHLC (phase-wrap beat detection), live candle w/ pulse-spiked volatility, bar-armed crash cascades, MA ribbon + crosshair + scanlines; NO glyphs (no text path — axis = dash ticks); presets 98/99/100 verified |
| GPU fluid sim (float render passes) | planned | needs float render-target formats (RenderRGBAf16/32 — to verify in platform/src/texture.rs); stable-fluids advect/project ping-pong; budgeted jacobi iters |
| Stateful GPU particles (state in float sim textures, VS-fetched) | planned | same float-pass primitive; vertex-stage `sample_nearest(uv, lod)` is verified available |
| Sim-texture FIELDS consumed by mesh engines (living wind for trees) | planned | named field node in the render graph, VS-sampled by any engine — design noted in CONTRACT.md once the float-pass primitive lands |
| Raymarching family (SDF scenes, subclassable scene fn) | planned | base marcher shader + `scene_sdf` hook; reduced-res internal pass + tent upscale; clean-room technique only |
| Reaction-diffusion / Chladni plates | partial | Chladni = cheap heightmap pattern branch (planned); reaction-diffusion = rejected for now (a real sim engine's worth of work, fluid sim first) |
| Strange attractors (Lorenz/Aizawa) as ribbon fields | planned | `field:` param on ribbons — cheap |
| Lissajous / oscilloscope curves | planned | `path:` param on tunnel — cheap |
| Phyllotaxis bloom | planned | particle mode — cheap closed form |
| Spectrum terrain / audio-reactive geometry | partial | signal namespace carries energy/bass/mid/high (stubbed 0 until VJ feeds them); spectrum-shaped engines wait on real audio wiring |
| Voronoi shatter, DLA growth, IFS fractal instancing, Verlet cloth, greeble skylines, boolean skylines | rejected (this pass) | each is a full engine build; revisit after the sim-texture primitive exists |
| GPS map renderer as an effect | rejected (data-gated) | widgets/src/map needs mbtiles data + its own draw architecture (DrawVector aligned-instance law); designed as "wrap MapView in a slot pass" but not worth shipping blank — revisit with the map session |
| Shadertoy technique mining (plasma/aurora/palette cycling) | partial | cosine-palette technique used (own constants); more looks land with the raymarch family; NO verbatim ports (user's boundary) |
| Tron light-cycle arenas (grid floor, trail walls, chase cam) | done | city engine style "tron" + `trails`: beat-swept wall fronts on the street lattice (collapse ahead of the front in the VS), fine-grid floor; ridden cycles not modeled (trails + grid are the energy); preset 95_lightcycle_arena |
| Retro city flyover (wireframe/neon towers, sun-stripe horizon) | done | city engine style "retro": neon cell-edge facades, banded sun disc on the sky cylinder, glowing grid streets; preset 94_neon_grid_city |
| Endless mountain range + virtual fighter jet | planned | heightmap already scrolls endlessly; jet = primitive-built foreground mesh, banked turns, afterburner pulse; wireframe/sunset/night-vision variants |
| Feedback-driven particle class (prev-frame texture as particle input) | planned | folds into the sim-texture family |

## Expansion builds (local/agent_state/vjfx-ideas/IDEAS-EXPANSION.md)

| idea | status | notes |
|---|---|---|
| #14 Firefly Synchrony | done | `firefly` engine (engines_firefly.rs): grass + fly sprites in one stream, blink = mix(intrinsic, beat phase, sync); presets 64/65/66 verified (chaos→one-heartbeat arc via p0 binding) |
| #24 Harmonograph Loom | done | `harmonograph` engine (engines_harmonograph.rs): curve = pure VS math, tempo-locked figure morph per `morph_beats`, growth draws pen-first; presets 67/68/69 verified; damping-envelope taper prevents center blowout |
| #47 Domino Liturgy | done | `domino` engine (engines_domino.rs): static boxes on spiral/serpent/tree paths, front = beat·per_beat, triangle-wave resurrection, beat-scaled impact comet; presets 70/71/72 verified; branch gate-bits simplified to arc-continuation (branches topple in step past their junction) |

## Preset moods (for the 100-preset target)

calm ambient / organic / geometric / retro / hard techno strobe — each
engine family should ship presets in at least two moods. Tracked in
resources/effects/MANIFEST.md as the library grows.
