//! Built-in presets: seven skies and five studio set-ups, as plain `HdriParams`.
//!
//! Each preset starts from `HdriParams::default()` and sets only what makes its look, so fields
//! added later (all `Option`s) keep their defaults. Every value sits inside its documented range,
//! so a preset passes `clamp()` unchanged; a test pins that.
//!
//! Sky presets:
//! - "Clear noon" runs on Time mode, so the date and place controls show something real.
//! - The rest use a Manual sun, which pins the sun height the look depends on. Their clock and
//!   date are still set to a matching time, for anything that reads them.
//!
//! Studio azimuths assume the camera looks north (−Z, the engine's forward and the centre column
//! of the engine's equirect). Lights on the camera side therefore sit near azimuth 180, and rims
//! and back lights near 0.
//!
//! Levels are in the model's scene units (atmosphere.rs, "Radiance scale"): a clear noon zenith
//! is a few tenths (0.25-0.45) and a white wall in the noon sun about 5.6. A light at EV e
//! has radiance 2^e at luminance 1, so a key at EV 4 is 16. The backdrops stay well below that, so
//! the lights read as lights.
//!
//! These are not the engine's `ibl::EnvPreset`s (studio, softbox, sunset, overcast, night, neon,
//! gradient), which stay as they are; two names overlap and that is intended.

use super::{HdriParams, LightParams};

pub const PRESET_NAMES: [&str; 12] = [
    "Clear noon",
    "Golden hour",
    "Sunset",
    "Overcast",
    "Blue hour",
    "Moonlit night",
    "Starry night",
    "Three-point",
    "Top softbox",
    "Rim pair",
    "Overcast dome",
    "Ring light",
];

/// A built-in preset by name, or None for an unknown name. Case, surrounding space, and '-'
/// versus '_' versus ' ' do not matter, so "three_point" and "THREE POINT" both find "Three-point".
pub fn preset(name: &str) -> Option<HdriParams> {
    let wanted = normalise(name);
    let canonical = PRESET_NAMES.iter().copied().find(|known| normalise(known) == wanted)?;
    Some(match canonical {
        "Clear noon" => clear_noon(),
        "Golden hour" => golden_hour(),
        "Sunset" => sunset(),
        "Overcast" => overcast(),
        "Blue hour" => blue_hour(),
        "Moonlit night" => moonlit_night(),
        "Starry night" => starry_night(),
        "Three-point" => three_point(),
        "Top softbox" => top_softbox(),
        "Rim pair" => rim_pair(),
        "Overcast dome" => overcast_dome(),
        "Ring light" => ring_light(),
        _ => return None,
    })
}

fn normalise(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| if c == '-' || c == '_' { ' ' } else { c.to_ascii_lowercase() })
        .collect()
}

/// A Sky-mode starting point with a manual sun. `month`, `day` and `hour` are set to a time that
/// matches the sun at 45°N, so switching to Time mode lands somewhere sensible.
fn sky(seed: u32, elevation_deg: f32, azimuth_deg: f32, month: u32, day: u32, hour: f32) -> HdriParams {
    let mut p = HdriParams {
        mode: "sky".to_string(),
        seed,
        ..Default::default()
    };
    let sun = &mut p.sky.sun;
    sun.mode = "manual".to_string();
    sun.elevation_deg = elevation_deg;
    sun.azimuth_deg = azimuth_deg;
    sun.year = 2026;
    sun.month = month;
    sun.day = day;
    sun.hour = hour;
    sun.tz_offset = 0.0;
    sun.latitude = 45.0;
    sun.longitude = 0.0;
    p
}

/// Midsummer noon at 45°N on the prime meridian: the sun about 68° up, due south, in air a little
/// clearer than the default, with a trace of cirrus for texture.
fn clear_noon() -> HdriParams {
    let mut p = sky(1, 68.4, 180.0, 6, 21, 12.0);
    p.sky.sun.mode = "time".to_string();
    p.sky.atmosphere.haze = 0.8;
    p.sky.clouds.coverage = 0.0;
    p.sky.clouds.cirrus = 0.15;
    p
}

/// A summer evening with the sun 6° up in the west-northwest: long warm light, a little evening
/// haze, scattered cumulus, and cirrus to catch the colour.
fn golden_hour() -> HdriParams {
    let mut p = sky(2, 6.0, 295.0, 6, 21, 19.0);
    p.sky.atmosphere.haze = 1.5;
    p.sky.sun_disc.softness = 0.25;
    p.sky.clouds.coverage = 0.15;
    p.sky.clouds.sharpness = 0.6;
    p.sky.clouds.scale = 1.2;
    p.sky.clouds.altitude_m = 3000.0;
    p.sky.clouds.cirrus = 0.35;
    p
}

/// The sun on the horizon: heavy haze reddens it, and broken cloud and cirrus light up from below.
fn sunset() -> HdriParams {
    let mut p = sky(3, 0.8, 302.0, 6, 21, 19.7);
    p.sky.atmosphere.haze = 2.2;
    p.sky.sun_disc.softness = 0.3;
    p.sky.clouds.coverage = 0.3;
    p.sky.clouds.sharpness = 0.55;
    p.sky.clouds.scale = 1.5;
    p.sky.clouds.altitude_m = 2500.0;
    p.sky.clouds.cirrus = 0.5;
    p
}

/// A full, soft, low cloud deck with the sun somewhere behind it: even light over dim, damp ground.
fn overcast() -> HdriParams {
    let mut p = sky(7, 40.0, 160.0, 3, 13, 11.1);
    p.sky.atmosphere.haze = 3.0;
    p.sky.atmosphere.ground_color = [0.12, 0.13, 0.11];
    p.sky.clouds.coverage = 1.0;
    p.sky.clouds.sharpness = 0.1;
    p.sky.clouds.scale = 1.5;
    p.sky.clouds.altitude_m = 1500.0;
    p.sky.clouds.cirrus = 0.0;
    p
}

/// Civil twilight, with the sun 5° below the horizon: the deep blue after sunset, the first
/// stars, and no moon.
fn blue_hour() -> HdriParams {
    let mut p = sky(4, -5.0, 308.0, 6, 21, 20.3);
    p.sky.atmosphere.haze = 1.2;
    p.sky.clouds.coverage = 0.1;
    p.sky.clouds.cirrus = 0.2;
    let night = &mut p.sky.night;
    night.moon = false;
    night.stars = 0.25;
    night.star_brightness = 0.6;
    night.glow_strength = 1.0;
    p
}

/// A winter evening with the sun 30° under the west-northwest horizon and the moon 30° up
/// opposite it, in the east-southeast. Sitting opposite the sun is what makes the moon nearly
/// full. Moonlight washes out the faint stars, so the star field is sparse, and a few thin clouds
/// catch the moon.
fn moonlit_night() -> HdriParams {
    let mut p = sky(5, -30.0, 285.0, 2, 19, 20.3);
    p.sky.atmosphere.haze = 1.0;
    p.sky.clouds.coverage = 0.2;
    p.sky.clouds.sharpness = 0.5;
    p.sky.clouds.altitude_m = 2500.0;
    let night = &mut p.sky.night;
    night.moon = true;
    night.moon_elevation_deg = 30.0;
    night.moon_azimuth_deg = 105.0;
    night.moon_size_deg = 0.52;
    night.moon_brightness = 1.5;
    night.stars = 0.35;
    night.star_brightness = 0.8;
    night.glow_strength = 0.8;
    p
}

/// A dark, dry, moonless winter night far from town: dense bright stars and a faint airglow.
fn starry_night() -> HdriParams {
    let mut p = sky(6, -50.0, 330.0, 2, 26, 22.9);
    p.sky.atmosphere.haze = 0.6;
    p.sky.clouds.coverage = 0.0;
    p.sky.clouds.cirrus = 0.0;
    let night = &mut p.sky.night;
    night.moon = false;
    night.stars = 1.0;
    night.star_brightness = 2.0;
    night.glow_strength = 1.5;
    p
}

/// A Studio-mode starting point: the backdrop gradient (linear radiance) and its light list.
fn studio(seed: u32, top: [f32; 3], horizon: [f32; 3], floor: [f32; 3], horizon_softness: f32, lights: Vec<LightParams>) -> HdriParams {
    let mut p = HdriParams {
        mode: "studio".to_string(),
        seed,
        ..Default::default()
    };
    p.studio.top = top;
    p.studio.horizon = horizon;
    p.studio.floor = floor;
    p.studio.horizon_softness = horizon_softness;
    p.lights = lights;
    p
}

/// One light: its shape, placement and size in degrees, level in EV, and colour temperature.
/// Everything else starts from `LightParams::default()`.
#[allow(clippy::too_many_arguments)]
fn light(name: &str, shape: &str, azimuth_deg: f32, elevation_deg: f32, width_deg: f32, height_deg: f32, intensity_ev: f32, kelvin: f32) -> LightParams {
    LightParams {
        name: name.to_string(),
        shape: shape.to_string(),
        azimuth_deg,
        elevation_deg,
        width_deg,
        height_deg,
        intensity_ev,
        kelvin,
        ..LightParams::default()
    }
}

/// Edge and falloff: corner rounding (rect only), edge softness and the centre hotspot.
fn look(mut light: LightParams, corner: f32, softness: f32, hotspot: f32) -> LightParams {
    light.corner = corner;
    light.softness = softness;
    light.hotspot = hotspot;
    light
}

/// Marks the light the engine uses as its sun.
fn as_key(mut light: LightParams) -> LightParams {
    light.key = true;
    light
}

/// Classic portrait lighting on a dark grey set:
/// - a large key 45° to camera right and 30° up;
/// - a broad fill low on the left, 2.5 stops under the key (about 3.4:1 once its larger area is
///   counted);
/// - a narrow strip rim behind the subject.
fn three_point() -> HdriParams {
    studio(8, [0.02, 0.02, 0.022], [0.05, 0.05, 0.05], [0.03, 0.03, 0.03], 0.3, vec![
        as_key(look(light("Key", "rect", 135.0, 30.0, 40.0, 30.0, 4.0, 5600.0), 0.15, 0.3, 0.2)),
        look(light("Fill", "rect", 230.0, 15.0, 50.0, 40.0, 1.5, 5600.0), 0.25, 0.6, 0.0),
        look(light("Rim", "rect", 20.0, 25.0, 8.0, 50.0, 4.0, 6500.0), 0.1, 0.2, 0.1),
    ])
}

/// Product lighting:
/// - a big softbox overhead, tipped toward the camera;
/// - a low white bounce card in front;
/// - a thin kicker strip behind on the left for an edge highlight.
fn top_softbox() -> HdriParams {
    studio(9, [0.03, 0.03, 0.03], [0.12, 0.12, 0.12], [0.08, 0.08, 0.08], 0.25, vec![
        as_key(look(light("Top softbox", "rect", 180.0, 75.0, 70.0, 45.0, 3.5, 5500.0), 0.1, 0.35, 0.15)),
        look(light("Bounce card", "rect", 180.0, 5.0, 70.0, 15.0, -0.5, 5500.0), 0.3, 0.8, 0.0),
        look(light("Kicker", "rect", 300.0, 20.0, 6.0, 45.0, 2.5, 6500.0), 0.1, 0.2, 0.0),
    ])
}

/// Two tall strips behind the subject, 40° either side, carve both edges out of a near-black set.
/// A soft frontal fill, 5.5 stops down, keeps the face from going to black.
fn rim_pair() -> HdriParams {
    studio(10, [0.005, 0.005, 0.006], [0.015, 0.015, 0.015], [0.01, 0.01, 0.01], 0.2, vec![
        as_key(look(light("Rim left", "rect", 320.0, 15.0, 10.0, 70.0, 4.5, 6500.0), 0.1, 0.25, 0.1)),
        look(light("Rim right", "rect", 40.0, 15.0, 10.0, 70.0, 4.5, 6500.0), 0.1, 0.25, 0.1),
        look(light("Front fill", "rect", 180.0, 10.0, 40.0, 30.0, -1.0, 5600.0), 0.3, 0.8, 0.0),
    ])
}

/// A huge diffuse dome overhead and two broad side panels on a pale backdrop: the shadowless,
/// even light of a light tent or a bright cloudy day.
fn overcast_dome() -> HdriParams {
    studio(11, [0.8, 0.82, 0.85], [0.6, 0.6, 0.6], [0.2, 0.19, 0.18], 0.6, vec![
        as_key(look(light("Dome", "disc", 0.0, 85.0, 110.0, 110.0, 0.5, 7000.0), 0.0, 1.0, 0.0)),
        look(light("Side east", "rect", 90.0, 20.0, 80.0, 50.0, -1.5, 6500.0), 0.5, 1.0, 0.0),
        look(light("Side west", "rect", 270.0, 20.0, 80.0, 50.0, -2.0, 6500.0), 0.5, 1.0, 0.0),
    ])
}

/// A beauty ring light on the camera axis (the dark centre is where the lens looks through), with
/// a warm soft wash on the background behind the subject.
fn ring_light() -> HdriParams {
    let mut ring = as_key(look(light("Ring", "ring", 180.0, 0.0, 24.0, 24.0, 4.5, 5600.0), 0.0, 0.15, 0.0));
    ring.inner = 0.75;
    studio(12, [0.04, 0.04, 0.045], [0.1, 0.1, 0.1], [0.06, 0.06, 0.06], 0.2, vec![
        ring,
        look(light("Background", "rect", 0.0, 10.0, 70.0, 30.0, -0.5, 3200.0), 0.2, 0.9, 0.0),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::atmosphere::sun_direction;
    use crate::hdri::{LightShape, Mode, SunMode};

    const SKY: [&str; 7] = ["Clear noon", "Golden hour", "Sunset", "Overcast", "Blue hour", "Moonlit night", "Starry night"];
    const STUDIO: [&str; 5] = ["Three-point", "Top softbox", "Rim pair", "Overcast dome", "Ring light"];

    /// The sun's elevation as the model computes it, in Time mode (NOAA) or Manual mode alike.
    fn sun_elevation(p: &HdriParams) -> f32 {
        sun_direction(&p.sky.sun).y.clamp(-1.0, 1.0).asin().to_degrees()
    }

    #[test]
    fn every_name_resolves_case_insensitively() {
        for name in PRESET_NAMES {
            assert!(preset(name).is_some(), "{name}");
            assert_eq!(preset(&name.to_uppercase()), preset(name), "{name}");
            assert_eq!(preset(&name.to_lowercase()), preset(name), "{name}");
            assert_eq!(preset(&format!("  {name} ")), preset(name), "{name}");
        }
        assert_eq!(preset("three_point"), preset("Three-point"));
        assert_eq!(preset("RING-LIGHT"), preset("Ring light"));
        assert!(preset("Nowhere").is_none());
        assert!(preset("").is_none());
    }

    #[test]
    fn presets_are_already_clamped() {
        for name in PRESET_NAMES {
            let original = preset(name).unwrap();
            let mut clamped = original.clone();
            let changed = clamped.clamp();
            assert!(changed.is_empty(), "{name} changed under clamp: {changed:?}");
            assert_eq!(clamped, original, "{name}");
        }
    }

    #[test]
    fn presets_round_trip_through_json() {
        for name in PRESET_NAMES {
            let p = preset(name).unwrap();
            let back = HdriParams::from_json(&p.to_json()).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(back, p, "{name}");
        }
    }

    #[test]
    fn sky_presets_are_sky_and_studio_presets_are_studio() {
        assert_eq!(SKY.len() + STUDIO.len(), PRESET_NAMES.len());
        for name in SKY {
            let p = preset(name).unwrap();
            assert_eq!(p.mode(), Mode::Sky, "{name}");
            assert!(p.lights.is_empty(), "{name}");
        }
        for name in STUDIO {
            let p = preset(name).unwrap();
            assert_eq!(p.mode(), Mode::Studio, "{name}");
            assert!((2..=4).contains(&p.lights.len()), "{name}: {} lights", p.lights.len());
            assert_eq!(p.lights.iter().filter(|l| l.key).count(), 1, "{name} needs exactly one key light");
            assert!(p.lights.iter().all(|l| l.enabled), "{name}");
        }
        let ring = preset("Ring light").unwrap();
        assert_eq!(ring.lights[0].shape(), LightShape::Ring);
        assert!(ring.lights[0].key);
    }

    #[test]
    fn the_sun_sits_where_each_sky_preset_says() {
        let el = |name: &str| sun_elevation(&preset(name).unwrap());
        assert!(el("Clear noon") > 60.0, "Clear noon: {}", el("Clear noon"));
        assert!((3.0..10.0).contains(&el("Golden hour")), "Golden hour: {}", el("Golden hour"));
        assert!((-1.0..3.0).contains(&el("Sunset")), "Sunset: {}", el("Sunset"));
        assert!((-6.0..-3.0).contains(&el("Blue hour")), "Blue hour: {}", el("Blue hour"));
        for name in ["Moonlit night", "Starry night"] {
            assert!(el(name) < -6.0, "{name}: the sun is at {}", el(name));
        }
        let moonlit = preset("Moonlit night").unwrap();
        assert!(moonlit.sky.night.moon && moonlit.sky.night.moon_elevation_deg > 0.0);
        assert!(!preset("Starry night").unwrap().sky.night.moon);
    }

    #[test]
    fn a_manual_sky_presets_clock_puts_the_sun_where_the_manual_sun_is() {
        let mut manual_presets = 0;
        let mut off = Vec::new();
        for name in SKY {
            let manual = preset(name).unwrap();
            if manual.sky.sun.mode() != SunMode::Manual {
                continue;
            }
            manual_presets += 1;
            let mut timed = manual.clone();
            timed.sky.sun.mode = "time".to_string();
            let (a, b) = (sun_direction(&manual.sky.sun), sun_direction(&timed.sky.sun));
            let angle = a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
            if angle >= 2.5 {
                off.push(format!("{name} ({angle:.1} deg)"));
            }
        }
        assert_eq!(manual_presets, 6, "every sky preset but Clear noon is Manual");
        assert!(off.is_empty(), "Time mode puts the sun away from the manual sun in: {}", off.join(", "));
    }

    #[test]
    fn presets_are_all_different() {
        let jsons: Vec<String> = PRESET_NAMES.iter().map(|n| preset(n).unwrap().to_json()).collect();
        for i in 0..jsons.len() {
            for j in i + 1..jsons.len() {
                assert_ne!(jsons[i], jsons[j], "{} equals {}", PRESET_NAMES[i], PRESET_NAMES[j]);
            }
        }
    }
}
