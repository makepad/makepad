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
| **THE SHADERS LIVE IN THE DOCUMENTS** (an AI can only make a new look if it owns the pixel math) | done | Every family has a named look hook, and 110 of 120 presets carry it inline (87 migrated this pass + 22 that already did + the new 113). Hooks ADDED this pass: heightmap (`DrawVjFxTerrain`), emitters, firefly, mountainjet, screen (a whole new shader), and tiles (its identity `fx_tint` became a real `fx_color`). Engines keep the same code as their default, so a doc without a hook still renders — old and third-party docs cannot break. Law pinned by `seed::registry_tests::every_bundled_preset_carries_its_own_shader`. Parity: full-library capture sweep before/after over the SAME 119 documents, 117/119 bit-identical and 119/119 within 1 LSB (that ±1 is the float floor — an UNMIGRATED `screen` doc shows the same); content-coupling sweep (VJFX_INPUT) over one preset per family, 22/24 bit-identical and 24/24 within 1 LSB. Cost: doc load 0.42 → 0.83 ms, a full load-and-render walk of the library 239 s → 241 s (+0.9%). The ten `screen` presets stay config-only BY DESIGN (next row) |
| `screen` family: a doc-authored FULLSCREEN pixel shader | done | `DrawVjFxScreen` (shaders.rs) + `ShaderKind::Screen` + `VjFxView::draw_screen_pass`: a screen doc that declares `shader:` gets a real clip-space pass whose whole fragment is its `fx_color(uv, content, cmix)`, with the stage chain still on top. Demo preset 113_scan_sermon — no engine geometry at all, every pixel doc-authored. OPT-IN on purpose (see below), so the ten shipped screen presets stay pixel-frozen and config-only. Two traps paid for: the pass needs its VIEWPORT published (`fx_set_pass_camera`) or it renders into a strip, and it needs its own DRAW LIST (`draw_list.begin_always`) or the instance lands in the parent's list and paints the window instead of the texture |
| A document's frame depends on the document shown before it | done | Found by the capture instrument: loading a doc does not clear the post chain's textures (they come out of the platform texture pool holding whatever the last owner left), so a feedback chain re-projected the previous effect's pixels into its first frames. Fixed structurally rather than with a clear: a stage that samples its OWN texture across frames now carries a `ColdStart` (post.rs), and while cold it does not read that texture at all — feedback binds the live frame in its place with the trail amount forced to zero, and the new `hold` stage takes a fresh grab first. `ColdStart::new()` is the only constructor and it starts cold, so a stage added later gets the safe state or does not compile. Pinned by `post::tests::cold_start_begins_cold_and_warms_exactly_once`. Verified at `VJFX_CAPTURE=1` (the grab IS the first frame after the load): `47_hyperdrive` and `222_freeze` — the latter's frame is nothing but the latched texture, so a leak would be the whole picture — are BIT-IDENTICAL when run behind two different predecessors, and a repeated run is bit-identical, so the harness is deterministic. What remains order-sensitive at a one-frame capture is TRAIL DEPTH (a frozen frame can be re-drawn a varying number of times, and each redraw is another feedback iteration): the diffs are the document's own structure, with no trace of the previous document's picture |
| Frame LATCH stage (freeze / scanline delay / strip delay) | done | `{kind: "hold", beats, trigger, mix, bands, stagger, axis}` — grabs the incoming frame on a beat slot or a rising trigger, shows it back over the live one. `bands` > 1 indexes the hold by position and releases band by band across the beat, so ONE stored frame reads as a per-line (or per-strip) delay. Presets 222 Freeze, 223 Time Slice, 224 Strip Delay |
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
| Tile motion authored by the DOCUMENT, not the engine | done | Vertex hook pair on `DrawVjFxTiles`: `fx_tile(id, grid, uv_window, t) -> (dx, dy, dz, scale)` and `fx_tile_spin(...) -> (axis.xyz, angle)`, applied on top of whatever the mode did, plus `mode: "hook"` where the engine moves nothing at all. Both hooks may fetch input0 with `sample_nearest` — which is how a doc lights its own relief from the clip's luma gradient. Presets 219 Tile Jitter (beat-quantised hashed throw), 220 Plane Grid (per-plane wrapped rotation), 221 Card Grid (staggered half-turn that always finishes inside the bar) |
| Luma relief (the picture stands off the wall) | done | `extrude:` on the tiles engine — the vertex stage fetches each tile's centre texel and pushes the tile along the plane normal by its brightness, shading it by its own height. Default 0 and gated in-shader, so every tiles doc written before it is bit-identical. Preset 218 Extrude |
| Video draped on the tunnel bore, doc-authored | done | `fx_wall(uv, content, cmix, ring) -> vec3` on `DrawVjFxTunnel` (pixel stage) owns every wall pixel at `content: 1`; its default is the stock papered bore, so the five shipped tunnels are unchanged. `fx_color` also gained `content`/`cmix` (vertex stage, the wall texel at that vertex) so rings and rails can take their tint from the picture they cross. Presets 225 Video Tunnel, 226 Video Torus |
| Scriptable fireworks choreography | done | 19_fireworks_show — beat-sequenced launches, pops, rings, sparkle field from the doc's frame fn |
| Clouds (billboard puff clusters) | done | particle mode "clouds" (cluster centers + soft sprites + wind wrap); calm preset 33 (sunset cumulus, tiltshifted) + storm preset 84 (lightning-inside-the-cloud via gated beat_pulse binding + fx_color subclass) — both verified |
| Flock / murmuration | done | `flock` engine (engines_flock.rs): CPU boids (O(N²), 0.77 ms @ 500 birds) emit banked glider tris per frame; wing flap in the VS off per-vertex amplitude+phase; goal jumps every `goal_beats`, drifting anchor keeps it streaming, predator scatters on the bar; presets 82/83 verified |
| Animated pipes (3D-pipes lattice) | done | `pipes` engine (engines_pipes.rs): self-avoiding lattice turtles + bulged elbow balls, teleport respawn; birth order on the stream, growth REPLAYED by grow/grow_beats (pop-in overshoot + hot tail); presets 96 (calm homage) / 97 (kick-lurch industrial) verified |
| Procedural city flyover | done | `city` engine (engines_city.rs): static towers/ground/sky/trails, facade uv baked in WINDOW units, per-window flicker = hash(tower, window, floor(beat)); engine-authored Lissajous cam with view-space banking (CAMERA CONTRACT); presets 93/94/95 verified |
| Video on parametric 3D geometry (cubes/spheres/tori/corridors of live video) | done | `videomesh` engine (engines_videomesh.rs): shape catalogue (box/sphere/torus/disc/cylinder/capsule/octahedron/star_prism/facets/grid/corridor) stamped per instance with ids+hashes on the stream; DOC vertex hooks fx_place/fx_axis/fx_scale choreograph instances, pixel fx_color/fx_backdrop own the look; per-instance uv windows (`uv_split`), vertex luma relief, orbit/inside/corridor rigs; tex1 bound → `decks: 2` docs are two-deck transitions (242_trans_ball). Presets 227-242 all grabbed and looked at |
| Boned/animated dancers (splash-constructed puppets) | planned | bones as uniforms (≤32 mat4 or pos+quat vec4 pairs), per-vertex bone index on the stream, binding-driven bone curves; crowd via instances |
| Stockcharts (procedural candlesticks, retro terminal) | done | `stockcharts` engine (engines_charts.rs): beat-committed OHLC (phase-wrap beat detection), live candle w/ pulse-spiked volatility, bar-armed crash cascades, MA ribbon + crosshair + scanlines; NO glyphs (no text path — axis = dash ticks); presets 98/99/100 verified |
| GPU fluid sim (float render passes) | planned | needs float render-target formats (RenderRGBAf16/32 — to verify in platform/src/texture.rs); stable-fluids advect/project ping-pong; budgeted jacobi iters |
| Stateful GPU particles (state in float sim textures, VS-fetched) | planned | same float-pass primitive; vertex-stage `sample_nearest(uv, lod)` is verified available |
| Sim-texture FIELDS consumed by mesh engines (living wind for trees) | planned | named field node in the render graph, VS-sampled by any engine — design noted in CONTRACT.md once the float-pass primitive lands |
| Raymarching family (SDF scenes, subclassable scene fn) | done | `raymarch` engine (engines_raymarch.rs): one clip-space quad + a sphere-trace pixel shader; `scene_sdf` SUBCLASS = the variant mechanism (SDF toolkit helpers, branched-NaN normal, 3-tap AO, gated 14-step soft shadow, cosine palette, one-bounce Snell glass sampling input0). Presets 85-89 verified. NOT built: the reduced-res internal pass — the render graph has no half-res scene pass today, so the budget lives in the `steps` doc param (documented per preset in MANIFEST.md) |
| Reaction-diffusion / Chladni plates | partial | Chladni = cheap heightmap pattern branch (planned); reaction-diffusion = rejected for now (a real sim engine's worth of work, fluid sim first) |
| Strange attractors (Lorenz/Aizawa) as ribbon fields | planned | `field:` param on ribbons — cheap |
| Lissajous / oscilloscope curves | planned | `path:` param on tunnel — cheap |
| Phyllotaxis bloom | planned | particle mode — cheap closed form |
| Spectrum terrain / audio-reactive geometry | done | THE AUDIO PICTURE (`audio_tex.rs`): a per-frame float texture — 256 log bins x 256 spectrogram history rows + 64 waveform rows, both rings, cursors in `audio_dim` — analysed off the SAME capture stream beat sync consumes (read-only tap, no source picker) and bound BY NAME on every engine draw, so `self.audio_fft(f, age)` / `self.audio_wave(t)` / `self.audio_env` work in any family's hook. The same analysis also finally feeds the `energy/bass/mid/high` binding signals, which were stubbed 0 since the contract was written. Twelve visualisers on it: 260-271 (bar field, radial bloom, oscilloscope ribbon, waveform tunnel, spectrum sea, level pulse rings, spectro kaleido, bass warp drape, spectrogram curtain, band lattice, harmonic petals, scope horizon) — verified live against a test tone through the loopback capture, and idle (silent rig) on every thumbnail |
| Voronoi shatter, DLA growth, IFS fractal instancing, Verlet cloth, greeble skylines, boolean skylines | rejected (this pass) | each is a full engine build; revisit after the sim-texture primitive exists |
| GPS map renderer as an effect | rejected (data-gated) | widgets/src/map needs mbtiles data + its own draw architecture (DrawVector aligned-instance law); designed as "wrap MapView in a slot pass" but not worth shipping blank — revisit with the map session |
| Shadertoy technique mining (plasma/aurora/palette cycling) | partial | cosine-palette technique used (own constants); more looks land with the raymarch family; NO verbatim ports (user's boundary) |
| Tron light-cycle arenas (grid floor, trail walls, chase cam) | done | city engine style "tron" + `trails`: beat-swept wall fronts on the street lattice (collapse ahead of the front in the VS), fine-grid floor; ridden cycles not modeled (trails + grid are the energy); preset 95_lightcycle_arena |
| Retro city flyover (wireframe/neon towers, sun-stripe horizon) | done | city engine style "retro": neon cell-edge facades, banded sun disc on the sky cylinder, glowing grid streets; preset 94_neon_grid_city |
| Endless mountain range + virtual fighter jet | done | `mountainjet` engine (engines_jet.rs): heightmap streaming technique + view-space primitive jet (banked weave, beat-pulsed afterburner as opaque shaped geometry) in one stream; the heightmap engine itself could not host a foreground mesh (its VS displaces everything by grid uv). Presets 90/91/92 = sunset alpenglow / Battlezone wire / night-vision, all verified |
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
