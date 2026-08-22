# Bundled preset manifest

One row per shipped effect document (name, engine, one-line character).
Regenerate the table when the library changes; the seeding module
(src/effects/seed.rs) compiles every row into the vj binary.

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
