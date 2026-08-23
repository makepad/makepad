//! Bundled preset seeding: the built-in effect library ships INSIDE the vj
//! binary (compiled-in via `include_str!`) and is seeded into the LOCAL
//! asset store on demand — publish-if-absent, keyed by alias, so a relaunch
//! never duplicates and never clobbers a user's newer revision under the
//! same alias (a re-published user edit is a newer head; seeding sees the
//! alias exists and leaves it alone).
//!
//! Flow (idempotent, run on a worker after the store session is up):
//!   for each bundled preset:
//!     alias = "vjfx/<name>"
//!     if client.resolve_alias(alias) succeeds -> skip (present, maybe edited)
//!     else publish_bundle(kind = VjEffect, files = [Source/Text bytes],
//!                         thumbnail = procedural placeholder JPEG,
//!                         rights = generated_cc0, alias, tags)
//!
//! Thumbnails are deliberately modest placeholders (a per-preset colored
//! pattern, never flat black): the VJ replaces them with lazily rendered
//! ANIMATED thumbnails from the effect runtime (see CONTRACT.md).

use makepad_asset_client::{
    AssetClient, ClientError, PublishBundle, PublishBundleFile, PublishRights, PublishThumbnail,
};
use makepad_asset_data::{AssetKind, DeviceTier, FileRole, MediaType, ThumbnailMedia};

/// Every bundled effect document, name + splash source.
pub fn bundled_presets() -> &'static [(&'static str, &'static str)] {
    &[
    ("01_fireworks", include_str!("../../resources/effects/01_fireworks.splash")),
    ("02_particle_tunnel", include_str!("../../resources/effects/02_particle_tunnel.splash")),
    ("03_vortex_swarm", include_str!("../../resources/effects/03_vortex_swarm.splash")),
    ("04_galaxy", include_str!("../../resources/effects/04_galaxy.splash")),
    ("05_rain", include_str!("../../resources/effects/05_rain.splash")),
    ("06_neon_growth", include_str!("../../resources/effects/06_neon_growth.splash")),
    ("07_coral_ring", include_str!("../../resources/effects/07_coral_ring.splash")),
    ("08_liquid_metal", include_str!("../../resources/effects/08_liquid_metal.splash")),
    ("09_synthwave", include_str!("../../resources/effects/09_synthwave.splash")),
    ("10_canyon_flight", include_str!("../../resources/effects/10_canyon_flight.splash")),
    ("11_ribbon_storm", include_str!("../../resources/effects/11_ribbon_storm.splash")),
    ("12_wormhole", include_str!("../../resources/effects/12_wormhole.splash")),
    ("13_pixel_dissolve", include_str!("../../resources/effects/13_pixel_dissolve.splash")),
    ("14_reactive_relief", include_str!("../../resources/effects/14_reactive_relief.splash")),
    ("15_acid_bloom", include_str!("../../resources/effects/15_acid_bloom.splash")),
    ("16_kaleido_video", include_str!("../../resources/effects/16_kaleido_video.splash")),
    ("17_video_trails", include_str!("../../resources/effects/17_video_trails.splash")),
    ("18_video_tiltshift", include_str!("../../resources/effects/18_video_tiltshift.splash")),
    ("19_fireworks_show", include_str!("../../resources/effects/19_fireworks_show.splash")),
    ("20_pulse_wormhole", include_str!("../../resources/effects/20_pulse_wormhole.splash")),
    ("21_winter_oak_sway", include_str!("../../resources/effects/21_winter_oak_sway.splash")),
    ("22_fern_unfurl", include_str!("../../resources/effects/22_fern_unfurl.splash")),
    ("23_thunder_veins", include_str!("../../resources/effects/23_thunder_veins.splash")),
    ("24_crystal_spire", include_str!("../../resources/effects/24_crystal_spire.splash")),
    ("25_dragon_coil", include_str!("../../resources/effects/25_dragon_coil.splash")),
    ("26_seaweed_ballet", include_str!("../../resources/effects/26_seaweed_ballet.splash")),
    ("27_bamboo_ring", include_str!("../../resources/effects/27_bamboo_ring.splash")),
    ("28_neon_thicket", include_str!("../../resources/effects/28_neon_thicket.splash")),
    ("29_ember_storm", include_str!("../../resources/effects/29_ember_storm.splash")),
    ("30_starfield_warp", include_str!("../../resources/effects/30_starfield_warp.splash")),
    ("31_plankton_drift", include_str!("../../resources/effects/31_plankton_drift.splash")),
    ("32_solar_fountain", include_str!("../../resources/effects/32_solar_fountain.splash")),
    ("33_cumulus_drift", include_str!("../../resources/effects/33_cumulus_drift.splash")),
    ("34_sunflower_pulse", include_str!("../../resources/effects/34_sunflower_pulse.splash")),
    ("35_code_rain", include_str!("../../resources/effects/35_code_rain.splash")),
    ("36_lava_lamp", include_str!("../../resources/effects/36_lava_lamp.splash")),
    ("37_mercury_beat", include_str!("../../resources/effects/37_mercury_beat.splash")),
    ("38_alpine_dawn", include_str!("../../resources/effects/38_alpine_dawn.splash")),
    ("39_night_ridge", include_str!("../../resources/effects/39_night_ridge.splash")),
    ("40_ocean_swell", include_str!("../../resources/effects/40_ocean_swell.splash")),
    ("41_lorenz_storm", include_str!("../../resources/effects/41_lorenz_storm.splash")),
    ("42_aizawa_bloom", include_str!("../../resources/effects/42_aizawa_bloom.splash")),
    ("43_silk_veil", include_str!("../../resources/effects/43_silk_veil.splash")),
    ("44_laser_scribble", include_str!("../../resources/effects/44_laser_scribble.splash")),
    ("45_lissajous_scope", include_str!("../../resources/effects/45_lissajous_scope.splash")),
    ("46_candy_torus", include_str!("../../resources/effects/46_candy_torus.splash")),
    ("47_hyperdrive", include_str!("../../resources/effects/47_hyperdrive.splash")),
    ("48_mirror_hall", include_str!("../../resources/effects/48_mirror_hall.splash")),
    ("49_vhs_breakup", include_str!("../../resources/effects/49_vhs_breakup.splash")),
    ("50_mosaic_pump", include_str!("../../resources/effects/50_mosaic_pump.splash")),
    ("51_uv_maelstrom", include_str!("../../resources/effects/51_uv_maelstrom.splash")),
    ("52_spectral_smear", include_str!("../../resources/effects/52_spectral_smear.splash")),
    ("53_meteor_shower", include_str!("../../resources/effects/53_meteor_shower.splash")),
    ("54_beat_salvo", include_str!("../../resources/effects/54_beat_salvo.splash")),
    ("55_dissolve_tilt", include_str!("../../resources/effects/55_dissolve_tilt.splash")),
    ("56_video_shatter", include_str!("../../resources/effects/56_video_shatter.splash")),
    ("57_luma_canyon", include_str!("../../resources/effects/57_luma_canyon.splash")),
    ("58_kaleido_bloom_feed", include_str!("../../resources/effects/58_kaleido_bloom_feed.splash")),
    ("59_zoom_echo", include_str!("../../resources/effects/59_zoom_echo.splash")),
    ("60_metaball_kaleido", include_str!("../../resources/effects/60_metaball_kaleido.splash")),
    ("61_golden_meadow", include_str!("../../resources/effects/61_golden_meadow.splash")),
    ("62_biolume_field", include_str!("../../resources/effects/62_biolume_field.splash")),
    ("63_wheat_wind", include_str!("../../resources/effects/63_wheat_wind.splash")),
    ("64_june_field", include_str!("../../resources/effects/64_june_field.splash")),
    ("65_one_pulse", include_str!("../../resources/effects/65_one_pulse.splash")),
    ("66_vigil", include_str!("../../resources/effects/66_vigil.splash")),
    ("67_ink_loom", include_str!("../../resources/effects/67_ink_loom.splash")),
    ("68_laser_loom", include_str!("../../resources/effects/68_laser_loom.splash")),
    ("69_pendulum_cathedral", include_str!("../../resources/effects/69_pendulum_cathedral.splash")),
    ("70_monastery_spiral", include_str!("../../resources/effects/70_monastery_spiral.splash")),
    ("71_snare_garden", include_str!("../../resources/effects/71_snare_garden.splash")),
    ("72_serpent_strobe", include_str!("../../resources/effects/72_serpent_strobe.splash")),
        ("73_kick_forge", include_str!("../../resources/effects/73_kick_forge.splash")),
    ("74_ember_pit", include_str!("../../resources/effects/74_ember_pit.splash")),
    ("75_coin_fountain", include_str!("../../resources/effects/75_coin_fountain.splash")),
    ("76_copper_reef", include_str!("../../resources/effects/76_copper_reef.splash")),
    ("77_guillotine", include_str!("../../resources/effects/77_guillotine.splash")),
    ("78_candy_stack", include_str!("../../resources/effects/78_candy_stack.splash")),
("79_tile_lagoon", include_str!("../../resources/effects/79_tile_lagoon.splash")),
    ("80_bar_shatter", include_str!("../../resources/effects/80_bar_shatter.splash")),
    ("81_conveyor_wall", include_str!("../../resources/effects/81_conveyor_wall.splash")),
    ("82_dusk_murmuration", include_str!("../../resources/effects/82_dusk_murmuration.splash")),
    ("83_confetti_swarm", include_str!("../../resources/effects/83_confetti_swarm.splash")),
    ("84_storm_cell", include_str!("../../resources/effects/84_storm_cell.splash")),
    ("85_pillar_sanctum", include_str!("../../resources/effects/85_pillar_sanctum.splash")),
    ("86_fractal_descent", include_str!("../../resources/effects/86_fractal_descent.splash")),
    ("87_molten_glass", include_str!("../../resources/effects/87_molten_glass.splash")),
    ("88_corridor_rush", include_str!("../../resources/effects/88_corridor_rush.splash")),
    ("89_beat_lens", include_str!("../../resources/effects/89_beat_lens.splash")),
        ("90_meadow_gale", include_str!("../../resources/effects/90_meadow_gale.splash")),
("90_ridge_runner", include_str!("../../resources/effects/90_ridge_runner.splash")),
        ("91_gale_grove", include_str!("../../resources/effects/91_gale_grove.splash")),
("91_vector_patrol", include_str!("../../resources/effects/91_vector_patrol.splash")),
        ("92_murmur_swarm", include_str!("../../resources/effects/92_murmur_swarm.splash")),
("92_night_strike", include_str!("../../resources/effects/92_night_strike.splash")),
        ("93_attractor_storm", include_str!("../../resources/effects/93_attractor_storm.splash")),
("93_midnight_metropolis", include_str!("../../resources/effects/93_midnight_metropolis.splash")),
        ("94_ink_drop", include_str!("../../resources/effects/94_ink_drop.splash")),
("94_neon_grid_city", include_str!("../../resources/effects/94_neon_grid_city.splash")),
        ("95_dye_wash", include_str!("../../resources/effects/95_dye_wash.splash")),
("95_lightcycle_arena", include_str!("../../resources/effects/95_lightcycle_arena.splash")),
        ("96_dye_relief", include_str!("../../resources/effects/96_dye_relief.splash")),
("96_screensaver_pipes", include_str!("../../resources/effects/96_screensaver_pipes.splash")),
    ("97_pressure_manifold", include_str!("../../resources/effects/97_pressure_manifold.splash")),
    ("98_bull_terminal", include_str!("../../resources/effects/98_bull_terminal.splash")),
    ("99_amber_exchange", include_str!("../../resources/effects/99_amber_exchange.splash")),
    ("100_flash_crash", include_str!("../../resources/effects/100_flash_crash.splash")),
    ("101_trans_dissolve", include_str!("../../resources/effects/101_trans_dissolve.splash")),
    ("102_trans_wipe", include_str!("../../resources/effects/102_trans_wipe.splash")),
    ("103_trans_wipe_v", include_str!("../../resources/effects/103_trans_wipe_v.splash")),
    ("104_trans_iris", include_str!("../../resources/effects/104_trans_iris.splash")),
    ("105_trans_push", include_str!("../../resources/effects/105_trans_push.splash")),
    ("106_trans_blinds", include_str!("../../resources/effects/106_trans_blinds.splash")),
    ("107_trans_lumakey", include_str!("../../resources/effects/107_trans_lumakey.splash")),
    ("108_trans_chromakey", include_str!("../../resources/effects/108_trans_chromakey.splash")),
    ("109_trans_additive", include_str!("../../resources/effects/109_trans_additive.splash")),
    ("110_trans_negative", include_str!("../../resources/effects/110_trans_negative.splash")),
    ("111_trans_pip", include_str!("../../resources/effects/111_trans_pip.splash")),
    ("112_trans_screenmix", include_str!("../../resources/effects/112_trans_screenmix.splash")),
    ("113_scan_sermon", include_str!("../../resources/effects/113_scan_sermon.splash")),
    ("114_trans_barn_doors_h", include_str!("../../resources/effects/114_trans_barn_doors_h.splash")),
    ("115_trans_barn_doors_v", include_str!("../../resources/effects/115_trans_barn_doors_v.splash")),
    ("116_trans_doors_h", include_str!("../../resources/effects/116_trans_doors_h.splash")),
    ("117_trans_doors_v", include_str!("../../resources/effects/117_trans_doors_v.splash")),
    ("118_trans_slat_sweep_u", include_str!("../../resources/effects/118_trans_slat_sweep_u.splash")),
    ("119_trans_slat_sweep_d", include_str!("../../resources/effects/119_trans_slat_sweep_d.splash")),
    ("120_trans_slat_sweep_l", include_str!("../../resources/effects/120_trans_slat_sweep_l.splash")),
    ("121_trans_slat_sweep_r", include_str!("../../resources/effects/121_trans_slat_sweep_r.splash")),
    ("122_trans_bar_curtain_h", include_str!("../../resources/effects/122_trans_bar_curtain_h.splash")),
    ("123_trans_bar_curtain_v", include_str!("../../resources/effects/123_trans_bar_curtain_v.splash")),
    ("124_trans_jaws_h", include_str!("../../resources/effects/124_trans_jaws_h.splash")),
    ("125_trans_jaws_v", include_str!("../../resources/effects/125_trans_jaws_v.splash")),
    ("126_trans_brickwork", include_str!("../../resources/effects/126_trans_brickwork.splash")),
    ("127_trans_checker_tiles", include_str!("../../resources/effects/127_trans_checker_tiles.splash")),
    ("128_trans_checker_bricks", include_str!("../../resources/effects/128_trans_checker_bricks.splash")),
    ("129_trans_checker_sweep", include_str!("../../resources/effects/129_trans_checker_sweep.splash")),
    ("130_trans_ring_bloom", include_str!("../../resources/effects/130_trans_ring_bloom.splash")),
    ("131_trans_clock_sweep", include_str!("../../resources/effects/131_trans_clock_sweep.splash")),
    ("132_trans_honeycomb", include_str!("../../resources/effects/132_trans_honeycomb.splash")),
    ("133_trans_hex_field", include_str!("../../resources/effects/133_trans_hex_field.splash")),
    ("134_trans_square_field", include_str!("../../resources/effects/134_trans_square_field.splash")),
    ("135_trans_tri_field", include_str!("../../resources/effects/135_trans_tri_field.splash")),
    ("136_trans_rect_field", include_str!("../../resources/effects/136_trans_rect_field.splash")),
    ("137_trans_box_iris", include_str!("../../resources/effects/137_trans_box_iris.splash")),
    ("138_trans_grain_dissolve", include_str!("../../resources/effects/138_trans_grain_dissolve.splash")),
    ("139_trans_pinwheel", include_str!("../../resources/effects/139_trans_pinwheel.splash")),
    ("140_trans_ripple_wipe", include_str!("../../resources/effects/140_trans_ripple_wipe.splash")),
    ("141_trans_panel_wipe", include_str!("../../resources/effects/141_trans_panel_wipe.splash")),
    ("142_trans_press_wipe", include_str!("../../resources/effects/142_trans_press_wipe.splash")),
    ("143_trans_roto_wipe", include_str!("../../resources/effects/143_trans_roto_wipe.splash")),
    ("144_trans_shift_wipe", include_str!("../../resources/effects/144_trans_shift_wipe.splash")),
    ("145_trans_multiply", include_str!("../../resources/effects/145_trans_multiply.splash")),
    ("146_trans_multiply_invert", include_str!("../../resources/effects/146_trans_multiply_invert.splash")),
    ("147_trans_add_invert", include_str!("../../resources/effects/147_trans_add_invert.splash")),
    ("148_trans_hard_multiply", include_str!("../../resources/effects/148_trans_hard_multiply.splash")),
    ("149_trans_dither_fade", include_str!("../../resources/effects/149_trans_dither_fade.splash")),
    ("150_trans_dither_multiply", include_str!("../../resources/effects/150_trans_dither_multiply.splash")),
    ("151_trans_add_fade", include_str!("../../resources/effects/151_trans_add_fade.splash")),
    ("152_trans_colour_key", include_str!("../../resources/effects/152_trans_colour_key.splash")),
    ("153_trans_tint_blend", include_str!("../../resources/effects/153_trans_tint_blend.splash")),
    ("154_trans_overlay", include_str!("../../resources/effects/154_trans_overlay.splash")),
    ("155_trans_alpha_layer", include_str!("../../resources/effects/155_trans_alpha_layer.splash")),
    ("156_pixel_drift", include_str!("../../resources/effects/156_pixel_drift.splash")),
    ("157_pixel_burst", include_str!("../../resources/effects/157_pixel_burst.splash")),
    ("158_pixel_walls", include_str!("../../resources/effects/158_pixel_walls.splash")),
    ("159_pixel_helix", include_str!("../../resources/effects/159_pixel_helix.splash")),
    ("160_pixel_swell", include_str!("../../resources/effects/160_pixel_swell.splash")),
    ("161_pixel_orbit", include_str!("../../resources/effects/161_pixel_orbit.splash")),
    ("162_pixel_jitter", include_str!("../../resources/effects/162_pixel_jitter.splash")),
    ("163_pixel_hop", include_str!("../../resources/effects/163_pixel_hop.splash")),
    ("164_stretch", include_str!("../../resources/effects/164_stretch.splash")),
    ("165_edge_stretch", include_str!("../../resources/effects/165_edge_stretch.splash")),
    ("166_bend", include_str!("../../resources/effects/166_bend.splash")),
    ("167_rotozoom", include_str!("../../resources/effects/167_rotozoom.splash")),
    ("168_roto_bounce", include_str!("../../resources/effects/168_roto_bounce.splash")),
    ("169_roto_slide", include_str!("../../resources/effects/169_roto_slide.splash")),
    ("170_pan_scale", include_str!("../../resources/effects/170_pan_scale.splash")),
    ("171_crop", include_str!("../../resources/effects/171_crop.splash")),
    ("172_multi_crop", include_str!("../../resources/effects/172_multi_crop.splash")),
    ("173_tile", include_str!("../../resources/effects/173_tile.splash")),
    ("174_nested_zoom", include_str!("../../resources/effects/174_nested_zoom.splash")),
    ("175_layer_stack", include_str!("../../resources/effects/175_layer_stack.splash")),
    ("176_split", include_str!("../../resources/effects/176_split.splash")),
    ("177_split_screen", include_str!("../../resources/effects/177_split_screen.splash")),
    ("178_edge_clone", include_str!("../../resources/effects/178_edge_clone.splash")),
    ("179_strip", include_str!("../../resources/effects/179_strip.splash")),
    ("180_slat_field", include_str!("../../resources/effects/180_slat_field.splash")),
    ("181_shuffle_h", include_str!("../../resources/effects/181_shuffle_h.splash")),
    ("182_shuffle_v", include_str!("../../resources/effects/182_shuffle_v.splash")),
    ("183_mirror_strip", include_str!("../../resources/effects/183_mirror_strip.splash")),
    ("184_mirror_rotate_a", include_str!("../../resources/effects/184_mirror_rotate_a.splash")),
    ("185_mirror_rotate_b", include_str!("../../resources/effects/185_mirror_rotate_b.splash")),
    ("186_mirror_quad", include_str!("../../resources/effects/186_mirror_quad.splash")),
    ("187_wave", include_str!("../../resources/effects/187_wave.splash")),
    ("188_warper", include_str!("../../resources/effects/188_warper.splash")),
    ("189_vortex", include_str!("../../resources/effects/189_vortex.splash")),
    ("190_suck", include_str!("../../resources/effects/190_suck.splash")),
    ("191_zigzag_twirl", include_str!("../../resources/effects/191_zigzag_twirl.splash")),
    ("192_bar_magnifier", include_str!("../../resources/effects/192_bar_magnifier.splash")),
    ("193_circle_magnifier", include_str!("../../resources/effects/193_circle_magnifier.splash")),
    ("194_fresnel", include_str!("../../resources/effects/194_fresnel.splash")),
    ("195_refraction", include_str!("../../resources/effects/195_refraction.splash")),
    ("196_rays", include_str!("../../resources/effects/196_rays.splash")),
    ("197_petal_fold", include_str!("../../resources/effects/197_petal_fold.splash")),
    ("198_disc", include_str!("../../resources/effects/198_disc.splash")),
    ("199_bug_eye", include_str!("../../resources/effects/199_bug_eye.splash")),
    ("200_maze", include_str!("../../resources/effects/200_maze.splash")),
    ("201_hue_cycle", include_str!("../../resources/effects/201_hue_cycle.splash")),
    ("202_channel_contrast", include_str!("../../resources/effects/202_channel_contrast.splash")),
    ("203_duotone", include_str!("../../resources/effects/203_duotone.splash")),
    ("204_tint", include_str!("../../resources/effects/204_tint.splash")),
    ("205_strobe", include_str!("../../resources/effects/205_strobe.splash")),
    ("206_channel_offset", include_str!("../../resources/effects/206_channel_offset.splash")),
    ("207_keyed_trails", include_str!("../../resources/effects/207_keyed_trails.splash")),
    ("208_trail_add", include_str!("../../resources/effects/208_trail_add.splash")),
    ("209_dual_blend", include_str!("../../resources/effects/209_dual_blend.splash")),
    ("210_perspective", include_str!("../../resources/effects/210_perspective.splash")),
    ("211_trans_turn_away", include_str!("../../resources/effects/211_trans_turn_away.splash")),
    ("212_trans_tumble_away", include_str!("../../resources/effects/212_trans_tumble_away.splash")),
    ("213_trans_zoom_in", include_str!("../../resources/effects/213_trans_zoom_in.splash")),
    ("214_trans_zoom_out", include_str!("../../resources/effects/214_trans_zoom_out.splash")),
    ("215_trans_barn_doors_rotating", include_str!("../../resources/effects/215_trans_barn_doors_rotating.splash")),
    ("216_trans_card_flip", include_str!("../../resources/effects/216_trans_card_flip.splash")),
    ("217_butterfly", include_str!("../../resources/effects/217_butterfly.splash")),
    // Tiles: the per-tile vertex hooks + luma relief.
    ("218_extrude", include_str!("../../resources/effects/218_extrude.splash")),
    ("219_tile_jitter", include_str!("../../resources/effects/219_tile_jitter.splash")),
    ("220_plane_grid", include_str!("../../resources/effects/220_plane_grid.splash")),
    ("221_card_grid", include_str!("../../resources/effects/221_card_grid.splash")),
    // The `hold` stage: one latched frame, indexed by position.
    ("222_freeze", include_str!("../../resources/effects/222_freeze.splash")),
    ("223_time_slice", include_str!("../../resources/effects/223_time_slice.splash")),
    ("224_strip_delay", include_str!("../../resources/effects/224_strip_delay.splash")),
    // The tunnel wall drape.
    ("225_video_tunnel", include_str!("../../resources/effects/225_video_tunnel.splash")),
    ("226_video_torus", include_str!("../../resources/effects/226_video_torus.splash")),
    ("227_spin_cube", include_str!("../../resources/effects/227_spin_cube.splash")),
    ("228_inside_cube", include_str!("../../resources/effects/228_inside_cube.splash")),
    ("229_cube_grid", include_str!("../../resources/effects/229_cube_grid.splash")),
    ("230_video_box", include_str!("../../resources/effects/230_video_box.splash")),
    ("231_mirror_ball", include_str!("../../resources/effects/231_mirror_ball.splash")),
    ("232_video_torus_3d", include_str!("../../resources/effects/232_video_torus_3d.splash")),
    ("233_star_prism", include_str!("../../resources/effects/233_star_prism.splash")),
    ("234_octa_star", include_str!("../../resources/effects/234_octa_star.splash")),
    ("235_octa_ring", include_str!("../../resources/effects/235_octa_ring.splash")),
    ("236_corridor", include_str!("../../resources/effects/236_corridor.splash")),
    ("237_maze_run", include_str!("../../resources/effects/237_maze_run.splash")),
    ("238_blimp", include_str!("../../resources/effects/238_blimp.splash")),
    ("239_sphere_relief", include_str!("../../resources/effects/239_sphere_relief.splash")),
    ("240_beam_fan", include_str!("../../resources/effects/240_beam_fan.splash")),
    ("241_slat_depth", include_str!("../../resources/effects/241_slat_depth.splash")),
    ("242_trans_ball", include_str!("../../resources/effects/242_trans_ball.splash")),
    // ---- THE AUDIO VISUALISERS (260-279) ------------------------------
    // Every one of these reads the live AUDIO PICTURE (effects/audio_tex.rs)
    // through the shader helpers `audio_fft(f, age)` / `audio_wave(t)` and
    // the `audio_env` uniform, and every one carries an idle figure so a
    // silent rig still performs.
    ("260_spectrum_bar_field", include_str!("../../resources/effects/260_spectrum_bar_field.splash")),
    ("261_radial_spectrum_bloom", include_str!("../../resources/effects/261_radial_spectrum_bloom.splash")),
    ("262_oscilloscope_ribbon", include_str!("../../resources/effects/262_oscilloscope_ribbon.splash")),
    ("263_waveform_tunnel", include_str!("../../resources/effects/263_waveform_tunnel.splash")),
    ("264_spectrum_sea", include_str!("../../resources/effects/264_spectrum_sea.splash")),
    ("265_level_pulse_rings", include_str!("../../resources/effects/265_level_pulse_rings.splash")),
    ("266_spectro_kaleido", include_str!("../../resources/effects/266_spectro_kaleido.splash")),
    ("267_bass_warp_drape", include_str!("../../resources/effects/267_bass_warp_drape.splash")),
    ("268_spectrogram_curtain", include_str!("../../resources/effects/268_spectrogram_curtain.splash")),
    ("269_band_lattice", include_str!("../../resources/effects/269_band_lattice.splash")),
    ("270_harmonic_petals", include_str!("../../resources/effects/270_harmonic_petals.splash")),
    ("271_scope_horizon", include_str!("../../resources/effects/271_scope_horizon.splash")),
    ]
}

/// The alias a bundled preset seeds under.
pub fn preset_alias(name: &str) -> String {
    format!("vjfx/{name}")
}

/// The catalog tag that marks an effect as TRANSITION-SUITED — the VJ's
/// TRANSITION filter chip queries it, and the transition slot's arm gesture
/// prefers that lane.
pub const TRANSITION_TAG: &str = "transition";

/// Bundled presets that work as CROSSFADE TRANSITIONS: the screen/warp
/// family plus the input-driven tiles/lens looks — every one of them shapes
/// `input0` (the program mix, in transition-slot mode) rather than replacing
/// it with an unrelated scene.
/// ORDER IS THE LANE ORDER: everyday → rare. The plain crossfade leads,
/// then the broadcast classics (wipes, iris, push, blinds), the MX50
/// keys/overlays, and the art transitions last. `transition_rank` turns an
/// alias into its index here — the TRANSITION chip sorts by it, so the
/// lane reads the same every night regardless of publish stamps.
pub const TRANSITION_PRESETS: &[&str] = &[
    // The everyday one, #1 by law.
    "101_trans_dissolve",
    // Broadcast classics.
    "102_trans_wipe",
    "103_trans_wipe_v",
    "104_trans_iris",
    "105_trans_push",
    "106_trans_blinds",
    // Geometric mask wipes.
    "114_trans_barn_doors_h",
    "115_trans_barn_doors_v",
    "116_trans_doors_h",
    "117_trans_doors_v",
    "118_trans_slat_sweep_u",
    "119_trans_slat_sweep_d",
    "120_trans_slat_sweep_l",
    "121_trans_slat_sweep_r",
    "122_trans_bar_curtain_h",
    "123_trans_bar_curtain_v",
    "124_trans_jaws_h",
    "125_trans_jaws_v",
    "126_trans_brickwork",
    "127_trans_checker_tiles",
    "128_trans_checker_bricks",
    "129_trans_checker_sweep",
    "130_trans_ring_bloom",
    "131_trans_clock_sweep",
    "137_trans_box_iris",
    "138_trans_grain_dissolve",
    "132_trans_honeycomb",
    "133_trans_hex_field",
    "134_trans_square_field",
    "135_trans_tri_field",
    "136_trans_rect_field",
    "139_trans_pinwheel",
    // Pattern wipes.
    "140_trans_ripple_wipe",
    "141_trans_panel_wipe",
    "142_trans_press_wipe",
    "143_trans_roto_wipe",
    "144_trans_shift_wipe",
    // MX50 keys / overlays.
    "107_trans_lumakey",
    "108_trans_chromakey",
    "109_trans_additive",
    "110_trans_negative",
    "111_trans_pip",
    "112_trans_screenmix",
    // Blend / combine modes.
    "145_trans_multiply",
    "146_trans_multiply_invert",
    "147_trans_add_invert",
    "148_trans_hard_multiply",
    "149_trans_dither_fade",
    "150_trans_dither_multiply",
    "151_trans_add_fade",
    "152_trans_colour_key",
    "153_trans_tint_blend",
    "154_trans_overlay",
    "155_trans_alpha_layer",
    // Art transitions, rarest last.
    "17_video_trails",
    "18_video_tiltshift",
    "16_kaleido_video",
    "48_mirror_hall",
    "49_vhs_breakup",
    "50_mosaic_pump",
    "51_uv_maelstrom",
    "52_spectral_smear",
    "58_kaleido_bloom_feed",
    "59_zoom_echo",
    "79_tile_lagoon",
    "80_bar_shatter",
    "81_conveyor_wall",
    "89_beat_lens",
    // Plane-in-3D: the picture as a card in a room.
    "211_trans_turn_away",
    "212_trans_tumble_away",
    "213_trans_zoom_in",
    "214_trans_zoom_out",
    "215_trans_barn_doors_rotating",
    "216_trans_card_flip",
    // The two-deck videomesh art transition (video shell off a sphere).
    "242_trans_ball",
];

pub fn is_transition_preset(name: &str) -> bool {
    TRANSITION_PRESETS.contains(&name)
}

/// The alias's position in the rarity order above; unknown docs (imported
/// or generated transitions) sort after every bundled one.
pub fn transition_rank(alias: &str) -> usize {
    let name = alias.rsplit('/').next().unwrap_or(alias);
    TRANSITION_PRESETS
        .iter()
        .position(|preset| *preset == name)
        .unwrap_or(usize::MAX)
}

fn preset_tags(name: &str) -> Vec<String> {
    let mut tags = vec!["vjeffect".to_string(), "builtin".to_string()];
    if is_transition_preset(name) {
        tags.push(TRANSITION_TAG.to_string());
    }
    tags
}

/// One seeding outcome, for an honest startup log line.
#[derive(Default, Debug)]
pub struct SeedReport {
    pub present: usize,
    pub published: usize,
    /// OUR heads whose bundled source changed on disk and were republished
    /// as a new revision of the same asset (self-healing reconcile).
    pub updated: usize,
    /// Already-seeded presets whose search annotation gained the
    /// `transition` tag (stores seeded before the tag existed).
    pub retagged: usize,
    pub failed: Vec<(String, String)>,
}

/// Publish-if-absent every bundled preset. Blocking (network) — run from a
/// worker thread, never the UI thread. Errors on one preset never stop the
/// rest.
pub fn seed_presets(client: &mut AssetClient) -> SeedReport {
    let mut report = SeedReport::default();
    // Lazily-built tag sets for the retag pass (one search each, first time
    // a present transition preset needs the check).
    let mut tagged = RetagSets::default();
    for (name, source) in bundled_presets() {
        let alias_str = preset_alias(name);
        let Ok(alias) = alias_str.parse() else {
            report.failed.push((alias_str, "bad alias".to_string()));
            continue;
        };
        match client.resolve_alias(&alias) {
            Ok(dto) => {
                // Present. SELF-HEALING: a head WE seeded (builtin-tagged)
                // whose bundled source has since changed on disk is
                // republished as a NEW REVISION of the same asset — that is
                // how dial-declaration and other doc backfills reach
                // existing stores. A user-edited head (no builtin tag) is
                // NEVER touched; a merely-untagged transition preset gets
                // its annotation updated in place.
                report.present += 1;
                match head_matches_bundle(client, &dto, source, &mut tagged) {
                    Ok(HeadState::Theirs) | Ok(HeadState::Current) => {}
                    Ok(HeadState::Stale) => {
                        match seed_one(client, name, source, &alias_str, Some(dto.asset_id)) {
                            Ok(()) => {
                                report.updated += 1;
                                continue;
                            }
                            Err(e) => {
                                report.failed.push((alias_str, format!("update: {e}")));
                                continue;
                            }
                        }
                    }
                    Err(e) => {
                        report.failed.push((alias_str.clone(), format!("head check: {e}")));
                        continue;
                    }
                }
                if is_transition_preset(name) {
                    match retag_transition(client, &dto, name, source, &mut tagged) {
                        Ok(true) => report.retagged += 1,
                        Ok(false) => {}
                        Err(e) => report.failed.push((alias_str, format!("retag: {e}"))),
                    }
                }
                continue;
            }
            Err(ClientError::NotFound { .. }) => {}
            Err(e) => {
                // Transport trouble on the lookup: publishing blind could
                // duplicate, so record and move on; the next launch retries.
                report.failed.push((alias_str, e.to_string()));
                continue;
            }
        }
        match seed_one(client, name, source, &alias_str, None) {
            Ok(()) => report.published += 1,
            Err(e) => report.failed.push((alias_str, e)),
        }
    }
    report
}

/// Which assets carry `tag`, from ONE tag-filtered search (paged; bounded).
fn assets_with_tag(
    client: &mut AssetClient,
    tag: &str,
) -> Result<std::collections::HashSet<makepad_asset_data::AssetId>, String> {
    let mut out = std::collections::HashSet::new();
    let mut query = makepad_asset_client::CatalogQuery::browse(100);
    query.kind = Some(AssetKind::VjEffect);
    query.tag = Some(tag.to_string());
    let mut cursor = None;
    for _ in 0..8 {
        let page = client
            .catalog_search(&query, cursor.as_ref())
            .map_err(|e| e.to_string())?;
        out.extend(page.hits.iter().map(|hit| hit.asset_id));
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Ok(out)
}

/// What the alias head is, relative to the bundled preset dir.
enum HeadState {
    /// A user-edited head (not builtin-tagged): never touched.
    Theirs,
    /// Our head, content identical to the bundled file.
    Current,
    /// Our head, but the bundled source changed: republish it.
    Stale,
}

/// Compare the alias head against the bundled source by CONTENT DIGEST
/// (the manifest's Source blob id is the payload's sha256).
fn head_matches_bundle(
    client: &mut AssetClient,
    dto: &makepad_asset_client::AliasDto,
    source: &str,
    sets: &mut RetagSets,
) -> Result<HeadState, String> {
    if sets.builtin.is_none() {
        sets.builtin = Some(assets_with_tag(client, "builtin")?);
    }
    if !sets.builtin.as_ref().is_some_and(|set| set.contains(&dto.asset_id)) {
        return Ok(HeadState::Theirs);
    }
    let manifest = client
        .fetch_asset_manifest(&dto.head_revision)
        .map_err(|e| e.to_string())?;
    let head_blob = manifest
        .files
        .iter()
        .find(|f| f.role == FileRole::Source)
        .map(|f| f.blob)
        .ok_or_else(|| "head has no Source file".to_string())?;
    let bundled = makepad_asset_data::BlobId::hash_of(source.as_bytes());
    if head_blob == bundled {
        Ok(HeadState::Current)
    } else {
        Ok(HeadState::Stale)
    }
}

/// Post-seed acceptance count: how many vjeffect rows the store actually
/// lists, against how many the binary bundles. The caller logs a shortfall
/// LOUDLY — a store showing a subset of the shipped library is a bug, and
/// this line is how it gets seen.
pub fn library_check(client: &mut AssetClient) -> Result<(u64, usize), String> {
    let mut query = makepad_asset_client::CatalogQuery::browse(1);
    query.kind = Some(AssetKind::VjEffect);
    let page = client.catalog_search(&query, None).map_err(|e| e.to_string())?;
    Ok((page.total, bundled_presets().len()))
}

/// The two lazily-built ownership/state sets the retag pass consults.
#[derive(Default)]
struct RetagSets {
    /// Assets already carrying the transition tag (nothing to do).
    transition: Option<std::collections::HashSet<makepad_asset_data::AssetId>>,
    /// Assets carrying the `builtin` tag — OUR seeds. A user edit publishes
    /// a fresh asset under the alias without it, and is never touched.
    builtin: Option<std::collections::HashSet<makepad_asset_data::AssetId>>,
}

/// Add the transition tag to an already-seeded preset's search annotation.
/// Manifest CONTENT is immutable and untouched — annotations are updatable
/// in place, and only a `builtin`-tagged (seeded-by-us) head is rewritten.
/// Returns whether a write happened.
fn retag_transition(
    client: &mut AssetClient,
    dto: &makepad_asset_client::AliasDto,
    name: &str,
    source: &str,
    sets: &mut RetagSets,
) -> Result<bool, String> {
    if sets.transition.is_none() {
        sets.transition = Some(assets_with_tag(client, TRANSITION_TAG)?);
    }
    if sets.transition.as_ref().is_some_and(|set| set.contains(&dto.asset_id)) {
        return Ok(false);
    }
    if sets.builtin.is_none() {
        sets.builtin = Some(assets_with_tag(client, "builtin")?);
    }
    if !sets.builtin.as_ref().is_some_and(|set| set.contains(&dto.asset_id)) {
        return Ok(false);
    }
    let ann = makepad_asset_client::AnnotationUpload {
        title: title_of(source, name),
        description: description_of(source),
        kind: Some(AssetKind::VjEffect),
        categories: Vec::new(),
        tags: preset_tags(name),
        creator: String::new(),
        generator: "makepad-vj effects".to_string(),
        backend: String::new(),
        model: String::new(),
        prompt: String::new(),
        provenance: "bundled preset library (apps/vj/resources/effects)".to_string(),
        private: false,
    };
    client
        .put_annotation(&dto.asset_id, &ann)
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Publish one bundled preset. `reuse` republishes as a new revision of an
/// EXISTING asset (the self-healing update path); `None` mints a fresh one.
fn seed_one(
    client: &mut AssetClient,
    name: &str,
    source: &str,
    alias_str: &str,
    reuse: Option<makepad_asset_data::AssetId>,
) -> Result<(), String> {
    let title = title_of(source, name);
    let description = description_of(source);
    let (jpeg, w, h) = placeholder_thumbnail(name);
    let mut bundle = PublishBundle::new(
        "vjfx",
        AssetKind::VjEffect,
        title,
        vec![PublishBundleFile {
            role: FileRole::Source,
            tier: DeviceTier::Any,
            lod: 0,
            media: MediaType::Text,
            bytes: source.as_bytes().to_vec(),
            reference: None,
            dims: None,
        }],
        PublishThumbnail::plain(jpeg, ThumbnailMedia::Jpeg, w, h),
        PublishRights::generated_cc0(),
    );
    bundle.alias = alias_str.parse().ok();
    bundle.asset_id = reuse;
    bundle.description = description;
    bundle.tags = preset_tags(name);
    bundle.generator = "makepad-vj effects".to_string();
    bundle.provenance = "bundled preset library (apps/vj/resources/effects)".to_string();
    client
        .publish_bundle(&bundle)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// `name:` from the document (first occurrence), else the file stem.
fn title_of(source: &str, fallback: &str) -> String {
    for line in source.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("name:") {
            let v = rest.trim().trim_matches('"');
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    fallback.to_string()
}

/// The document's leading comment block, stripped of `//`.
fn description_of(source: &str) -> String {
    let mut out = String::new();
    for line in source.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("//") {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(rest.trim());
        } else if !t.is_empty() {
            break;
        }
        if out.len() > 300 {
            break;
        }
    }
    out
}

/// A modest but never-flat placeholder: a per-name colored diagonal weave
/// with a bright diagonal band, 256x256 JPEG. The VJ's lazy animated
/// thumbnail replaces it on first sight.
fn placeholder_thumbnail(name: &str) -> (Vec<u8>, u32, u32) {
    const W: usize = 256;
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 360.0;
    let hue2 = (hue + 0.33).fract();
    let mut bgra = vec![0u32; W * W];
    for y in 0..W {
        for x in 0..W {
            let u = x as f32 / W as f32;
            let v = y as f32 / W as f32;
            let t = (u * 0.7 + v * 0.3 + ((u * 9.0).sin() * 0.03)).fract();
            let hh = hue + (hue2 - hue) * t;
            let band = 1.0 - ((u + v - 1.0).abs() * 2.5).min(1.0);
            let (r, g, b) = hsv(hh, 0.75, 0.28 + 0.62 * band);
            bgra[y * W + x] = 0xff00_0000
                | (((r * 255.0) as u32) << 16)
                | (((g * 255.0) as u32) << 8)
                | ((b * 255.0) as u32);
        }
    }
    let jpeg = encode_jpeg_bgra(&bgra, W, W);
    (jpeg, W as u32, W as u32)
}

fn hsv(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = (h.fract() + 1.0).fract() * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as u32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

fn encode_jpeg_bgra(bgra: &[u32], width: usize, height: usize) -> Vec<u8> {
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(bgra.as_ptr() as *const u8, bgra.len() * 4)
    };
    let mut out = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut out, 88);
    // SAFETY note: the transmute above is the same BGRA byte view the
    // importer's thumbs.rs uses with this encoder.
    let _ = encoder.encode(
        bytes,
        width as u16,
        height as u16,
        jpeg_encoder::ColorType::Bgra,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_preset_has_a_name_line_and_parses_metadata() {
        let presets = bundled_presets();
        assert!(presets.len() >= 60, "the library shrank to {}", presets.len());
        for (name, source) in presets {
            assert!(!source.is_empty());
            let title = title_of(source, name);
            assert!(!title.is_empty());
            let desc = description_of(source);
            assert!(!desc.is_empty(), "{name} has no leading comment block");
        }
    }

    #[test]
    fn placeholder_thumbnails_are_real_jpegs_and_differ_by_name(){
        let (a, w, h) = placeholder_thumbnail("01_fireworks");
        let (b, _, _) = placeholder_thumbnail("09_synthwave");
        assert_eq!((w, h), (256, 256));
        assert!(a.len() > 500, "suspiciously small jpeg");
        assert!(a.starts_with(&[0xff, 0xd8]), "not a jpeg");
        assert_ne!(a, b, "two presets must not share a placeholder");
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    /// THE LIBRARY LAW: every .splash in the preset directory is registered
    /// in `bundled_presets` — a store can never be seeded a subset because
    /// someone forgot the include line. (Compile-time include_str! means the
    /// registry cannot cover files it does not name; this test names them.)
    #[test]
    fn every_preset_file_is_registered_and_every_entry_has_a_file() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources/effects");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .expect("preset dir")
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.strip_suffix(".splash").map(str::to_string)
            })
            .collect();
        on_disk.sort();
        let mut registered: Vec<String> =
            bundled_presets().iter().map(|(n, _)| n.to_string()).collect();
        registered.sort();
        assert_eq!(
            on_disk, registered,
            "resources/effects and seed.rs bundled_presets disagree — \
             register the missing preset (or delete the stale entry)"
        );
    }

    /// THE DOC-CARRIES-ITS-SHADER LAW (CONTRACT.md).
    ///
    /// A preset is a COMPLETE, FORKABLE UNIT: the pixel math it runs is in
    /// the file, not hidden in a Rust engine, so an AI (or a person)
    /// rewriting `42_whatever.splash` can make a genuinely new look instead
    /// of only re-tuning parameters. This test pins that every shipped
    /// preset carries a `shader:` block that SUBCLASSES its engine's draw
    /// shader — the one structural mistake (a plain `{...}` object) is
    /// rejected at parse and would leave the doc silently running the
    /// engine default.
    ///
    /// The `screen` family is the documented exception: it draws no scene
    /// pass at all (input0 goes straight into the stage chain), so it has
    /// no pixel stage to carry. Remove it from here the day it grows one.
    #[test]
    fn every_bundled_preset_carries_its_own_shader() {
        let mut without = Vec::new();
        let mut screens = 0;
        for (name, source) in bundled_presets() {
            let engine = source
                .lines()
                .find_map(|l| l.trim().strip_prefix("engine: \""))
                .and_then(|l| l.split('"').next())
                .unwrap_or("");
            if engine == "screen" {
                screens += 1;
                continue;
            }
            let has = source
                .lines()
                .any(|l| l.trim_start().starts_with("shader: draw.DrawVjFx"));
            if !has {
                without.push(*name);
            }
        }
        assert!(
            without.is_empty(),
            "these presets have no inline shader — every preset must carry \
             the pixel math it runs (see CONTRACT.md, \"A doc carries its \
             shader\"): {without:?}"
        );
        assert!(screens > 0, "the screen family vanished — update this test");
    }

    /// THE BOUNDED-CLOCK TRIPWIRE (CONTRACT.md, "a look must be a BOUNDED
    /// function of `time` and `beat`").
    ///
    /// `self.time_beat.y` is the BEAT COUNT — the host's song clock, which
    /// on a deck is already in the thousands when the effect is cued and
    /// grows all night. Every use of it has to be bounded at the point it
    /// steers something (`fract`, a `sin`/`cos` phase, a clamp); a raw
    /// monotone term walks the look off the screen. 86_fractal_descent
    /// shipped `angle = 0.52 + beat * 0.010`, which is a lovely fold for
    /// the two seconds a gallery capture ever runs and a BLACK FRAME on
    /// the deck, and that is what this list exists to stop repeating.
    ///
    /// Sound analysis of "is this use bounded?" needs dataflow we do not
    /// have here, so this is a REVIEW GATE, not a checker: a preset that
    /// reaches for the beat count must be named here, and naming it means
    /// someone looked. If this test fails, do not just add the file —
    /// grab it at t = 0 / 60 / 600 / 3600 s first:
    ///   `VJFX_ONLY=<name> VJFX_CAPTURE=600@1.0 VJFX_SWEEP=/tmp/x \
    ///    ./target/release/examples/effect_gallery`
    #[test]
    fn every_beat_count_use_is_reviewed() {
        // name -> why the unbounded beat count is safe in that preset.
        const REVIEWED: &[(&str, &str)] = &[
            ("15_acid_bloom", "sine phase — the term only ever enters sin()"),
            ("86_fractal_descent", "two slow sines + clamp into the live band"),
            ("87_molten_glass", "fract() — a material band index"),
            // The 2026-08-23 batch, swept at t = 0/600/3600 via the gallery
            // capture — no flat/black frames, means 30-163, sd 48-62.
            ("158_pixel_walls", "sine phase into the wall spring"),
            ("162_pixel_jitter", "floor()-quantised hash seed"),
            ("163_pixel_hop", "floor()-quantised hop seed"),
            ("181_shuffle_h", "floor()-quantised per-strip shuffle seed"),
            ("182_shuffle_v", "floor()-quantised per-strip shuffle seed"),
            ("185_mirror_rotate_b", "sine phase on the mirror axis"),
            ("187_wave", "sine phase on the displacement"),
            ("191_zigzag_twirl", "slow sine phase on the twirl"),
            ("196_rays", "slow sine phase on the ray origin"),
            ("197_petal_fold", "slow sine phase on the fold"),
            ("198_disc", "slow sine phase on the annulus"),
            ("200_maze", "beat-quantised maze re-roll via fract()"),
            ("205_strobe", "floor()-quantised gate hash + pulse envelope"),
        ];
        let mut unreviewed = Vec::new();
        for (name, source) in bundled_presets() {
            if !source.contains("time_beat.y") {
                continue;
            }
            if !REVIEWED.iter().any(|(n, _)| n == name) {
                unreviewed.push(*name);
            }
        }
        assert!(
            unreviewed.is_empty(),
            "these presets steer their look with the raw BEAT COUNT and are \
             not in the reviewed list: {unreviewed:?} — see CONTRACT.md \
             \"a look must be a BOUNDED function of time and beat\", verify \
             the preset still renders at t = 600 s, then add it here"
        );
        for (name, _) in REVIEWED {
            assert!(
                bundled_presets()
                    .iter()
                    .any(|(n, s)| n == name && s.contains("time_beat.y")),
                "{name} no longer uses the beat count — drop it from the \
                 reviewed list"
            );
        }
    }

    #[test]
    fn transition_set_names_only_registered_presets() {
        for name in TRANSITION_PRESETS {
            assert!(
                bundled_presets().iter().any(|(n, _)| n == name),
                "transition preset {name} is not in the bundled registry"
            );
        }
    }
}
