# Bundled preset manifest

One row per shipped effect document (name, engine, one-line character).
Regenerate the table when the library changes; the seeding module
(src/effects/seed.rs) compiles every row into the vj binary.

**Every document here carries its own shader.** The family's look function
is written out inline in the file, so a preset is a complete forkable unit
an author can rewrite into a new look rather than only re-tune (see
CONTRACT.md, "a document carries its own shader"). The ten `screen` presets
are the exception: that family has no scene pass, its look IS the stage
list — 113_scan_sermon shows what a `screen` document with its own
fullscreen shader looks like.

| file | title | engine | character |
|---|---|---|---|
| 01_fireworks | Fireworks | particles/burst | staggered burst shells, stateless GPU particles. |
| 02_particle_tunnel | Warp Tunnel | particles/tunnel | rings of particles streaming past the camera, |
| 03_vortex_swarm | Vortex Swarm | particles/vortex | a rising helix of embers, tighter at the waist. |
| 04_galaxy | Galaxy | particles/galaxy | a slow three-armed spiral disc; inner stars orbit faster |
| 05_rain | Neon Rain | particles/rain | stretched streak sprites falling through the volume, a |
| 06_neon_growth | Neon Growth | lsystem | the classic bracketed L-system plant, growing in over |
| 07_coral_ring | Coral Ring | lsystem | five 3D L-system shrubs in a ring, pitch-and-roll rules so |
| 08_liquid_metal | Liquid Metal | metaballs | marching-tetrahedra metaballs, blob radii swelling on the |
| 09_synthwave | Synthwave Grid | heightmap | the retro terrain flythrough. The grid mesh never |
| 10_canyon_flight | Canyon Flight | heightmap | the same terrain engine with ridged folding: fbm folds |
| 11_ribbon_storm | Ribbon Storm | ribbons | flow-field ribbon trails orbiting a torus band, feedback |
| 12_wormhole | Wormhole | tunnel | a (3,2) torus-knot tube flown from the inside; neon rings |
| 13_pixel_dissolve | Pixel Dissolve | particles/image | the texture-input demo: every particle carries one |
| 14_reactive_relief | Reactive Relief | heightmap | the second texture-input demo: the terrain's height IS |
| 15_acid_bloom | Acid Bloom | lsystem | the shader-hook demo: an L-system dome whose look comes |
| 16_kaleido_video | Kaleido Video | screen | the fullscreen family: no mesh at all. The channel's |
| 17_video_trails | Video Trails | screen | feedback zoom + chroma split over the live content, both |
| 18_video_tiltshift | Video Tiltshift | screen | the miniature look as an effect pass: pyramid blur |
| 19_fireworks_show | Fireworks Show | emitters | the programmable-emitters engine. The `frame:` function |
| 20_pulse_wormhole | Pulse Wormhole | tunnel | the binding layer at work on a mesh engine: glow and |
| 21_winter_oak_sway | Winter Oak | lsystem | a bare oak silhouette in cold light. Pattern taught here: |
| 22_fern_unfurl | Fern Unfurl | lsystem | the classic asymmetric fern rule, breathing open and |
| 23_thunder_veins | Thunder Veins | lsystem | lightning as an L-system: huge angle jitter turns the |
| 24_crystal_spire | Crystal Spire | lsystem | rigid hexagonal growth: 60-degree angles + rolls, zero |
| 25_dragon_coil | Dragon Coil | lsystem | the dragon-curve rewriting system drawn as a glowing |
| 26_seaweed_ballet | Seaweed Ballet | lsystem | kelp columns rolling in a slow current. Pattern taught: |
| 27_bamboo_ring | Bamboo Ring | lsystem | eight upright canes around the camera. Pattern taught: |
| 28_neon_thicket | Neon Thicket | lsystem | hard-techno lsystem: boxy 90-degree branches, strobing |
| 29_ember_storm | Ember Storm | particles/burst | burst particles in fire colors with heavy fallout and a |
| 30_starfield_warp | Starfield Warp | particles/tunnel | the jump-to-lightspeed: tunnel-mode particles with |
| 31_plankton_drift | Plankton Drift | particles/vortex | calm ambient: a slow teal vortex of tiny motes, half |
| 32_solar_fountain | Solar Fountain | particles/fountain | a molten geyser: fountain particles in solar colors, |
| 33_cumulus_drift | Cumulus Drift | particles/clouds | the calm-cloud preset: puff clusters in sunset light, |
| 34_sunflower_pulse | Sunflower Pulse | particles/phyllo | phyllotaxis: golden-angle floret spiral breathing on |
| 35_code_rain | Code Rain | particles/rain | terminal-green streaks with a glitch slice on the beat. |
| 36_lava_lamp | Lava Lamp | metaballs | the slow one: two-tone blobs rolling around each other, |
| 37_mercury_beat | Mercury Beat | metaballs | hard metal: fast chrome-white blobs SLAMMING wider on |
| 38_alpine_dawn | Alpine Dawn | heightmap | the endless mountain range: ridged fbm peaks streaming |
| 39_night_ridge | Night Ridge | heightmap | the night-vision flythrough: mono-green ridged terrain, |
| 40_ocean_swell | Ocean Swell | heightmap | low rolling water: smooth (unridged) noise, small height, |
| 41_lorenz_storm | Lorenz Storm | ribbons/lorenz | ribbons riding the genuine Lorenz attractor ODEs: the |
| 42_aizawa_bloom | Aizawa Bloom | ribbons/aizawa | the rounder strange attractor: a glowing mushroom-dome of |
| 43_silk_veil | Silk Veil | ribbons | a few WIDE slow ribbons in pastel light, tilt-shifted: |
| 44_laser_scribble | Laser Scribble | ribbons | thin fast acid-green trails tearing through frame, |
| 45_lissajous_scope | Lissajous Scope | tunnel | flying through an oscilloscope figure: the tunnel's |
| 46_candy_torus | Candy Torus | tunnel | a plain ring (p=1, q=0 degenerates the knot into a torus) |
| 47_hyperdrive | Hyperdrive | tunnel | a (5,2) knot flown FAST, ice-blue, radial blur hammering |
| 48_mirror_hall | Mirror Hall | screen | fullscreen family: the live content folded against itself |
| 49_vhs_breakup | VHS Breakup | screen | the degraded-tape look: glitch slices + chroma bleed + |
| 50_mosaic_pump | Mosaic Pump | screen | pixelation whose BLOCK SIZE snaps on the beat (p2 bound |
| 51_uv_maelstrom | UV Maelstrom | screen | the screen-space tunnel warp + a swirl on top: the |
| 52_spectral_smear | Spectral Smear | screen | heavy chroma displacement whose ANGLE sweeps with the |
| 53_meteor_shower | Meteor Shower | emitters | scripted emitters: jets streak DOWN across the sky on |
| 54_beat_salvo | Beat Salvo | emitters | hard techno emitters: a burst SLAMS on every beat, |
| 55_dissolve_tilt | Dissolve Tilt | particles/image | the hybrid crown jewel: the live video becomes a field |
| 56_video_shatter | Video Shatter | particles/image | the live content as particles + a glitch pass: the |
| 57_luma_canyon | Luma Canyon | heightmap | the live content EXTRUDED: terrain height is 70% the |
| 58_kaleido_bloom_feed | Kaleido Bloom Feed | screen | the required combined chain over live content: |
| 59_zoom_echo | Zoom Echo | screen | the infinite-zoom feedback corridor: strong zoom feedback |
| 60_metaball_kaleido | Metaball Kaleido | metaballs | a MESH engine under a fullscreen warp: liquid blobs |
| 61_golden_meadow | Golden Meadow | grass | the nature loop: seven thousand grass blades in late |
| 62_biolume_field | Biolume Field | grass | the dark meadow that PULSES on the kick: near-black |
| 63_wheat_wind | Wheat Wind | grass | tall pale stalks leaning hard in a strong wind, warm |
| 64_june_field | June Field | firefly | a dark summer meadow of fireflies pulled into sync by the |
| 65_one_pulse | One Pulse | firefly | the firefly field driven almost fully into sync: fourteen |
| 66_vigil | Vigil | firefly | the same firefly engine wearing a sacred mood: flies become |
| 67_ink_loom | Ink Loom | harmonograph | a Victorian harmonograph drawing pale ink in the dark: one |
| 68_laser_loom | Laser Loom | harmonograph | three chorus strands of the harmonograph at hue offsets, |
| 69_pendulum_cathedral | Pendulum Cathedral | harmonograph | the camera stands INSIDE a huge, slowly morphing |
| 70_monastery_spiral | Monastery Spiral | domino | seven hundred ivory dominoes on an Archimedean |
| 71_snare_garden | Snare Garden | domino | a branching domino tree: the trunk topples up the middle |
| 72_serpent_strobe | Serpent Strobe | domino | hard techno dominoes: two thousand black tiles on a |
| 79_tile_lagoon | Tile Lagoon | tiles/wave | the live content as a mosaic raft on a slow swell; tiles |
| 80_bar_shatter | Bar Shatter | tiles/shatter | the picture explodes on the downbeat and reassembles |
| 81_conveyor_wall | Conveyor Wall | tiles/conveyor | endless belt: alternate rows stream the image opposite |
| 82_dusk_murmuration | Dusk Murmuration | flock | starling silhouettes on a glowing dusk sky; the goal |
| 83_confetti_swarm | Confetti Swarm | flock | additive neon gliders; a predator dives through the swarm |
| 84_storm_cell | Storm Cell | particles/clouds | dark cumulus with lightning INSIDE on the kick (gated |
| 85_pillar_sanctum | Pillar Sanctum | raymarch | infinite hypostyle hall flown down one lane; columns twist |
| 86_fractal_descent | Fractal Descent | raymarch | kaleidoscopic-IFS fractal orbited in the dark, fold angle |
| 87_molten_glass | Molten Glass | raymarch | smooth-min metaball puddle, radii pumping on the beat; the |
| 88_corridor_rush | Corridor Rush | raymarch | endless run of rooms; bulkhead doorways yawn open on the |
| 89_beat_lens | Beat Lens | raymarch/optics | glass blobs refract input0 via real Snell rays, bend on |
| 90_ridge_runner | Ridge Runner | mountainjet | alpenglow range streaming under a banking fighter jet, |
| 91_vector_patrol | Vector Patrol | mountainjet/wire | Battlezone vector terrain, jet as a hot green outline |
| 92_night_strike | Night Strike | mountainjet/nv | night-vision sortie: phosphor ramp + sensor grain, burner |
| 93_midnight_metropolis | Midnight Metropolis | city/night | realistic night flyover: window grids re-roll per beat, |
| 94_neon_grid_city | Neon Grid City | city/retro | wireframe-neon towers, striped synthwave sun, hot-pink |
| 95_lightcycle_arena | Lightcycle Arena | city/tron | fine-grid floor + ten light-cycle trail walls sweeping |
| 96_screensaver_pipes | Screensaver Pipes | pipes | the classic lattice homage: glossy pipes grow in over 24 |
| 97_pressure_manifold | Pressure Manifold | pipes | hard-techno pipes: the growth front LURCHES on the kick |
| 98_bull_terminal | Bull Terminal | stockcharts | green-on-black phosphor tape: one candle per beat, beat- |
| 99_amber_exchange | Amber Exchange | stockcharts | after-hours amber terminal drifting in 3D, two candles |
| 100_flash_crash | Flash Crash | stockcharts | the red panic: bar-armed crash cascades + a pulse-driven |
| 113_scan_sermon | Scan Sermon | screen/doc-shader | THE PURE-DOCUMENT EFFECT: no engine geometry at all, a
| 218_extrude | Extrude | tiles/relief | the picture stands off the wall: each tile pushed by its own |
| 219_tile_jitter | Tile Jitter | tiles/hook | beat-quantised hashed throw and spin that settles before the |
| 220_plane_grid | Plane Grid | tiles/hook | a wall of big planes, each turning on its own hashed axis at |
| 221_card_grid | Card Grid | tiles/hook | a rack of cards; once a bar a staggered wave of half-turns |
| 222_freeze | Freeze | screen/hold | the frame stops dead and re-grabs every two beats; GRAB is a |
| 223_time_slice | Time Slice | screen/hold | scanline bands each on their own delay, the boundary raking |
| 224_strip_delay | Strip Delay | screen/hold | vertical strips catching up in hashed order — the picture |
| 225_video_tunnel | Video Tunnel | tunnel/drape | the clip IS the bore wall, mirrored around the seam, beat |
| 226_video_torus | Video Torus | tunnel/drape | the same drape on a plain ring; the clip comes round again |
| 260_spectrum_bar_field | Spectrum Bar Field | screen/audio | THE AUDIO REFERENCE: log-frequency bars from audio_fft,
| 261_radial_spectrum_bloom | Radial Spectrum Bloom | screen/audio | the bar field bent into a circle; the ghost rings behind
| 262_oscilloscope_ribbon | Oscilloscope Ribbon | screen/audio | audio_wave as a lit ribbon; the trail copies read the
| 263_waveform_tunnel | Waveform Tunnel | screen/audio | radius IS time: rings of older audio flying out past the
| 264_spectrum_sea | Spectrum Sea | screen/audio | the spectrogram HISTORY as a lit landscape — frequency
| 265_level_pulse_rings | Level Pulse Rings | screen/audio | every kick of the last five seconds still travelling; the
| 266_spectro_kaleido | Spectro Kaleido | screen/audio | the audio texture read on BOTH axes and folded into a
| 267_bass_warp_drape | Bass Warp Drape | screen/audio | THE CONTENT ONE: live deck video pumped by the low end,
| 268_spectrogram_curtain | Spectrogram Curtain | screen/audio | the analysis shown almost raw — the waterfall it is,
| 269_band_lattice | Band Lattice | screen/audio | a wall of level lamps: across = band, down = how long
| 270_harmonic_petals | Harmonic Petals | screen/audio | a flower whose outline IS the spectrum, with four older
| 271_scope_horizon | Scope Horizon | screen/audio | the waveform as a coastline with its own reflection,
| 227_spin_cube | Spin Cube | videomesh/box | the live video wrapped onto one big tumbling cube, edges flashing on the beat. |
| 228_inside_cube | Inside Cube | videomesh/box | the camera INSIDE a video room: six walls play the clip, seams glow in tempo. |
| 229_cube_grid | Cube Grid | videomesh/box | a 6x6 wall of cubes, each carrying its cell of the picture; a staggered flip wave runs once a bar. |
| 230_video_box | Video Box | videomesh/box | five video-faced boxes on a slow carousel, each tumbling at its own hashed rate. |
| 231_mirror_ball | Mirror Ball | videomesh/sphere | the clip quantised into facet tiles on a revolving ball, glitter re-rolled on quantised time. |
| 232_video_torus_3d | Video Torus 3D | videomesh/torus | the clip wrapped around a fat revolving ring, a hot highlight chasing it once per beat. |
| 233_star_prism | Star Prism | videomesh/star_prism | a five-point star slab: both faces play the clip, the side band runs beat-pumped neon. |
| 234_octa_star | Octa Star | videomesh/octahedron | eight flat video facets meeting at hard edges, per-face palette tints, nose-over wobble. |
| 235_octa_ring | Octa Ring | videomesh/octahedron | ten small video octahedra on a carousel, tumbling on hashed axes. |
| 236_corridor | Corridor | videomesh/corridor | endless flight down a square video duct; a light-line sweeps each segment in beat time. |
| 237_maze_run | Maze Run | videomesh/corridor | the corridor with every segment rolled a hashed quarter-turn — a twisting funhouse run. |
| 238_blimp | Blimp | videomesh/capsule | a fat video airship cruising a bounded figure, clip wrapped end to end. |
| 239_sphere_relief | Sphere Relief | videomesh/sphere | the clip EXTRUDED off a globe: bright passages grow mountains (vertex luma relief), PUMP gains it. |
| 240_beam_fan | Beam Fan | videomesh/grid | twelve thin video beams fanning through one centre, each carrying its band of the clip. |
| 241_slat_depth | Slat Depth | videomesh/grid | the picture sliced into sixteen slats that swim in depth on a travelling wave, image kept readable. |
| 242_trans_ball | Ball | videomesh/sphere | TWO-DECK transition: the outgoing picture curls into a ball and rolls away while deck B dissolves in underneath. |
