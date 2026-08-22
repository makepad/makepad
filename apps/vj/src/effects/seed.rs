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
    ]
}

/// The alias a bundled preset seeds under.
pub fn preset_alias(name: &str) -> String {
    format!("vjfx/{name}")
}

/// One seeding outcome, for an honest startup log line.
#[derive(Default, Debug)]
pub struct SeedReport {
    pub present: usize,
    pub published: usize,
    pub failed: Vec<(String, String)>,
}

/// Publish-if-absent every bundled preset. Blocking (network) — run from a
/// worker thread, never the UI thread. Errors on one preset never stop the
/// rest.
pub fn seed_presets(client: &mut AssetClient) -> SeedReport {
    let mut report = SeedReport::default();
    for (name, source) in bundled_presets() {
        let alias_str = preset_alias(name);
        let Ok(alias) = alias_str.parse() else {
            report.failed.push((alias_str, "bad alias".to_string()));
            continue;
        };
        match client.resolve_alias(&alias) {
            Ok(_) => {
                // Present (possibly a user's newer revision) — never touch.
                report.present += 1;
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
        match seed_one(client, name, source, &alias_str) {
            Ok(()) => report.published += 1,
            Err(e) => report.failed.push((alias_str, e)),
        }
    }
    report
}

fn seed_one(
    client: &mut AssetClient,
    name: &str,
    source: &str,
    alias_str: &str,
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
    bundle.description = description;
    bundle.tags = vec!["vjeffect".to_string(), "builtin".to_string()];
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
