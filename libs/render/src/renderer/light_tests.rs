use super::*;
use crate::lightmap::LmLight;

fn omni(x: f32, y: f32, z: f32, intensity: f32, radius: f32) -> LmLight {
    LmLight::omni(vec3f(x, y, z), vec3f(intensity, intensity, intensity), radius)
}

/// The village's own numbers: its fixed midday sun and one harvested
/// street lamp — bulb at ~2.82 on a 3.2-unit pole, photometry solved by
/// `lightmap::lamp_photometry` exactly as `harvest_lamps` does it.
fn village_sun() -> crate::sun::SunLight {
    crate::sun::SunLight {
        dir: vec3f(0.55, 0.56, 0.62).normalize(),
        ..Default::default()
    }
}

/// The same village at the hour a street lamp is FOR: sun on its way
/// down, a few degrees up.
fn dusk_sun() -> crate::sun::SunLight {
    crate::sun::SunLight {
        dir: vec3f(0.55, 0.1, 0.62).normalize(),
        ..Default::default()
    }
}

fn street_lamp() -> LmLight {
    let (radius, strength) = crate::lightmap::lamp_photometry(2.82);
    LmLight {
        pos: vec3f(0.0, 2.82, 0.0),
        color: vec3f(strength, strength * 0.775, strength * 0.475),
        radius,
        dir: vec3f(0.0, -1.0, 0.0),
        spot: 1.0,
        ..Default::default()
    }
}

fn flat() -> Receiver<'static> {
    Receiver { base_y: 0.0, terrain: None, statics: &[] }
}

/// A caster on grass at the edge of a PROUD road slab, sun laying the
/// silhouette across the slab: the quad plane must rise to the slab's
/// top, or the slab depth-buries the whole silhouette and the shadow
/// vanishes at exactly that heading (and flickers with camera angle at
/// the slab's edge) — the walking-into-the-road report this pins.
#[test]
fn sdf_quad_rides_a_raised_receiver_under_the_silhouette() {
    // Road slab top 8 cm proud, starting 0.5 units down-sun of the
    // anchor — the silhouette's far half lands on it.
    let slabs = [(vec3f(-8.0, -1.0, -8.0), vec3f(-0.5, 0.08, 8.0))];
    let receiver = Receiver { base_y: 0.0, terrain: None, statics: &slabs };
    let anchor = vec3f(0.0, 0.0, 0.0);
    // Light from +x: the silhouette runs toward -x, onto the slab.
    let y = sdf_quad_ground(anchor, &receiver, 1.0, 0.0, 2.0);
    assert!(
        (y - 0.08).abs() < 1.0e-6,
        "quad must ride the slab top, got y = {y}"
    );
    // Silhouette running AWAY from the slab (toward +x): stays put.
    let y = sdf_quad_ground(anchor, &receiver, -1.0, 0.0, 2.0);
    assert_eq!(y, 0.0, "no slab under the run — plane must not move");
    // No statics at all: the anchor's own height wins.
    let y = sdf_quad_ground(vec3f(0.0, 0.3, 0.0), &flat(), 1.0, 0.0, 2.0);
    assert_eq!(y, 0.3);
}

/// Standing 1.5 units from a lit street lamp at DUSK, the lamp must
/// visibly own the shadow: the LEAN points along the anti-lamp azimuth
/// by a readable amount (sub-0.2-unit leans read as nothing — the report
/// this pins), the shadow darkens — and the ROOT stays at the boots,
/// because a lean redirects the silhouette's body, never its contact.
/// This is the live scene's exact arithmetic, so if a tuning change makes
/// the lean invisible again, this fails before a user says it.
///
/// Dusk, not noon: ownership is decided by which source is actually
/// lighting the character, and once a lamp emits what a lamp emits
/// (`lightmap::LM_LAMP_GROUND_PEAK`, well under the sun's 0.72 direct
/// term) it takes the shadow as the sun goes down — never at midday.
/// `the_midday_sun_keeps_the_shadow_from_a_lamp` pins the other half.
#[test]
fn a_dusk_street_lamp_visibly_leans_a_nearby_shadow() {
    let lights = [street_lamp()];
    let feet = vec3f(1.5, 0.0, 0.0);
    let a = character_shadow_anchor(feet, &flat(), &dusk_sun(), &lights)
        .expect("grounded character must have a shadow");
    assert!(
        a.lamp_w > 0.5,
        "beside the pole at dusk the lamp should dominate, lamp_w = {}",
        a.lamp_w
    );
    assert!(
        a.lean.x > 0.4,
        "the lean must point away from the lamp and be readable \
         (got {:?}, want x > 0.4)",
        a.lean
    );
    assert!(a.lean.y.abs() < 0.05, "off-azimuth lean: {:?}", a.lean);
    // THE contract the lean must never break: grounded, the foot end
    // of the shadow is the caster's own contact point (the floating
    // sideways-silhouette report this pins).
    assert!(
        (a.root.x - feet.x).abs() < 1.0e-6 && (a.root.z - feet.z).abs() < 1.0e-6,
        "a grounded lamp lean must not move the root, got {:?}",
        a.root
    );
    // Darkened over the plain sun shadow at the same spot.
    let plain = character_shadow_anchor(feet, &flat(), &dusk_sun(), &[])
        .expect("baseline");
    assert!(a.alpha > plain.alpha, "a dominant lamp must darken the fan");
    assert_eq!(plain.lamp_w, 0.0);
    // And the baseline grounded root stays under the feet too.
    assert!((plain.root.x - feet.x).abs() < 1.0e-6);
    // The sun-baked silhouette compresses toward the feet as the lamp
    // takes over; the plain sun shadow keeps its full size.
    assert!(
        a.size_mul < 0.75,
        "a dominant lamp must compress the fan, size_mul = {}",
        a.size_mul
    );
    assert!((plain.size_mul - 1.0).abs() < 1.0e-6);
}

/// Directly under the bulb after sundown: no direction to lean, so lean
/// AND root stay pinned at the feet — but the lamp owns the shadow
/// outright, so it must be strongly compressed and darkened, never the
/// sun's long dusk silhouette parked under a lamp (the report this pins).
#[test]
fn under_the_bulb_the_shadow_pins_small_at_the_feet() {
    let lights = [street_lamp()];
    let feet = vec3f(0.1, 0.0, 0.0);
    for sun in [
        {
            let mut s = village_sun();
            s.dir = vec3f(0.55, 0.05, 0.62).normalize();
            s
        },
        {
            let mut s = village_sun();
            s.dir = vec3f(0.55, 0.02, 0.62).normalize();
            s
        },
    ] {
        let a = character_shadow_anchor(feet, &flat(), &sun, &lights)
            .expect("anchor");
        assert!(a.lamp_w > 0.85, "under the bulb lamp_w = {}", a.lamp_w);
        assert!(
            a.lean.x.abs() < 0.05 && a.lean.y.abs() < 0.05,
            "no lean under the bulb, got {:?}",
            a.lean
        );
        assert!(
            (a.root.x - feet.x).abs() < 1.0e-6
                && (a.root.z - feet.z).abs() < 1.0e-6,
            "grounded root must sit at the feet, got {:?}",
            a.root
        );
        assert!(
            a.size_mul < 0.55,
            "the shadow must compress under the bulb, size_mul = {}",
            a.size_mul
        );
    }
}

/// The other half of the ownership contract, and the one the overbright
/// bake used to get wrong: at MIDDAY the sun keeps the shadow. A street
/// lamp emits a fraction of daylight (`lightmap::LM_LAMP_GROUND_PEAK`
/// against the sun's 0.72 direct term), so a character beside a pole at
/// noon casts ONE shadow, pointing away from the sun.
///
/// This failed before the photometry rewrite: the harvested lamp was
/// pinned at the atlas's 2.0 encode ceiling, which put 0.87 on the ground
/// — brighter than noon — and let a lamp swing shadows in broad daylight.
#[test]
fn the_midday_sun_keeps_the_shadow_from_a_lamp() {
    // The frame never shows an unrailed lamp: static_lights_for scales
    // every baked light by the daylight headroom before anything —
    // anchor weighting included — sees it. The night peak raise made
    // railing here load-bearing: the raw 0.72-class strength would
    // out-vote a noon sun no shipped frame ever pits it against.
    let sun = village_sun();
    let mut lamp = street_lamp();
    let s = crate::lightmap::lamp_daylight_scale(crate::lightmap::daylight_on_ground(
        sun.dir, sun.color, sun.sky,
    ));
    lamp.color = lamp.color * s;
    let lights = [lamp];
    for feet in [vec3f(1.5, 0.0, 0.0), vec3f(0.1, 0.0, 0.0)] {
        let a = character_shadow_anchor(feet, &flat(), &sun, &lights).expect("anchor");
        assert!(
            a.lamp_w < 0.35,
            "a lamp must not own a noon shadow, lamp_w = {} at {feet:?}",
            a.lamp_w
        );
        // Still the sun's own silhouette: pointing anti-sun, full size.
        let plain = character_shadow_anchor(feet, &flat(), &sun, &[]).expect("baseline");
        assert!(
            (a.size_mul - plain.size_mul).abs() < 0.35,
            "the noon silhouette must keep its size, {} vs {}",
            a.size_mul,
            plain.size_mul
        );
    }
}

/// Sun on the horizon (night edge): the lamp saturates and the LEAN is
/// the bulb's TRUE mid-body projection, not a fraction of it — while
/// the root never leaves the boots.
#[test]
fn at_night_the_lamp_owns_the_shadow_outright() {
    let mut sun = village_sun();
    sun.dir = vec3f(0.55, 0.02, 0.62).normalize();
    let lights = [street_lamp()];
    let feet = vec3f(1.5, 0.0, 0.0);
    let a = character_shadow_anchor(feet, &flat(), &sun, &lights).expect("anchor");
    assert!(a.lamp_w > 0.95, "night lamp_w = {}", a.lamp_w);
    // True projection: rho * MID / (bulb - MID) = 1.5*0.9/1.92 = 0.703.
    assert!(
        (a.lean.x - 0.703).abs() < 0.05,
        "night lean should be the full projection (~0.70), got {}",
        a.lean.x
    );
    assert!(
        (a.root.x - feet.x).abs() < 1.0e-6 && (a.root.z - feet.z).abs() < 1.0e-6,
        "the night lean must not detach the root from the boots, got {:?}",
        a.root
    );
}

/// The whole-policy pinning contract in one sweep: for EVERY grounded
/// scenario — no lamp, day lamp beside, night lamp beside, dead under
/// the bulb — the silhouette's foot end is the caster's own contact
/// point. Only height may move it: a sun jump slides the root down the
/// anti-sun azimuth by exactly `h/sun.y`, and a lamp-owned jump slides
/// it by the projection's height growth — never by the standing lean.
#[test]
fn the_shadow_root_never_leaves_the_feet_on_the_ground() {
    let lights = [street_lamp()];
    let night = {
        let mut s = village_sun();
        s.dir = vec3f(0.55, 0.02, 0.62).normalize();
        s
    };
    let cases: [(&str, Vec3f, &[crate::lightmap::LmLight], &SunLight); 4] = [
        ("no lamp", vec3f(3.0, 0.0, 1.0), &[], &village_sun()),
        ("day lamp beside", vec3f(1.5, 0.0, 0.0), &lights, &village_sun()),
        ("night lamp beside", vec3f(1.5, 0.0, 0.0), &lights, &night),
        ("under the bulb", vec3f(0.1, 0.0, 0.0), &lights, &night),
    ];
    for (name, feet, lights, sun) in cases {
        let a = character_shadow_anchor(feet, &flat(), sun, lights)
            .unwrap_or_else(|| panic!("{name}: anchor"));
        let d = ((a.root.x - feet.x).powi(2) + (a.root.z - feet.z).powi(2)).sqrt();
        assert!(
            d < 1.0e-6,
            "{name}: grounded root drifted {d} units from the feet"
        );
    }
    // Airborne under the sun alone: the root carries the whole height
    // projection — the jump shadow still slides off the feet.
    let sun = village_sun();
    let feet = vec3f(3.0, 1.2, 1.0);
    let a = character_shadow_anchor(feet, &flat(), &sun, &[]).expect("air anchor");
    let expect = vec2f(
        -sun.dir.x / sun.dir.y.max(0.2) * 1.2,
        -sun.dir.z / sun.dir.y.max(0.2) * 1.2,
    );
    assert!(
        (a.root.x - (feet.x + expect.x)).abs() < 1.0e-5
            && (a.root.z - (feet.z + expect.y)).abs() < 1.0e-5,
        "sun jump must slide the root by the full height projection"
    );
    // Airborne beside the night lamp: the root moves only by the
    // projection's GROWTH with height, which is strictly less than the
    // standing lean it explicitly excludes.
    let feet = vec3f(1.5, 0.8, 0.0);
    let a = character_shadow_anchor(feet, &flat(), &night, &lights)
        .expect("lamp air anchor");
    let slide = ((a.root.x - feet.x).powi(2) + (a.root.z - feet.z).powi(2)).sqrt();
    assert!(
        slide > 0.05,
        "a lamp-owned jump must still slide the shadow, slide = {slide}"
    );
    assert!(
        slide < a.lean.x,
        "the airborne root slide ({slide}) must stay under the full \
         standing lean ({}) it excludes",
        a.lean.x
    );
}

/// The car SDF sprite's tilt/air gate: flat-and-grounded draws the
/// sprite; a rolled car or one launched off a ramp falls back to the
/// blob (the atlas bakes the car FLAT, so a flat sprite under a tilted
/// car is a lie).
#[test]
fn tilted_or_airborne_cars_fall_back_to_the_blob() {
    assert!(car_sprite_allowed(1.0, 0.0), "parked flat");
    assert!(car_sprite_allowed(0.95, 0.4), "moderate ramp");
    assert!(!car_sprite_allowed(0.5, 0.0), "rolled onto its side");
    assert!(!car_sprite_allowed(1.0, 3.0), "big air");
    assert!(!car_sprite_allowed(0.3, 4.0), "both at once");
}

/// Out of the lamp's radius nothing leans — the pavement case that made
/// "the lean never fires" so easy to reproduce: the south-verge walk is
/// 8+ units from the north-verge bulbs, outside radius 8.
#[test]
fn out_of_radius_the_sun_keeps_the_shadow() {
    let lights = [street_lamp()];
    let feet = vec3f(8.5, 0.0, 0.0);
    let a = character_shadow_anchor(feet, &flat(), &village_sun(), &lights)
        .expect("anchor");
    assert_eq!(a.lamp_w, 0.0);
    assert!((a.root.x - feet.x).abs() < 1.0e-6);
    assert!(a.lean.x.abs() < 1.0e-6 && a.lean.y.abs() < 1.0e-6);
}

/// The core contract: up to 8 slots, strongest (intensity × attenuation
/// at the nearest anchor) first, and a light whose radius reaches no
/// anchor never makes the list however bright it is.
#[test]
fn selection_takes_the_strongest_eight_and_rejects_by_radius() {
    let mut lights = Vec::new();
    // Ten candidates in a row, each 2 units further away, same radius.
    for i in 0..10 {
        lights.push(omni(2.0 * i as f32, 3.0, 0.0, 1.0, 40.0));
    }
    // A blazing light whose radius cannot reach the anchor.
    lights.push(omni(100.0, 3.0, 0.0, 50.0, 5.0));
    let anchors = [vec3f(0.0, 0.0, 0.0)];
    let (mut rank, mut sel) = (Vec::new(), Vec::new());
    select_strongest_lights(
        &lights,
        0..lights.len(),
        &anchors,
        MAX_DYNAMIC_LIGHTS,
        &mut rank,
        &mut sel,
    );
    assert_eq!(sel.len(), 8, "hard cap at 8");
    assert!(!sel.contains(&10), "out-of-radius light must be rejected");
    // Nearest first: identical intensity and radius, so attenuation
    // orders them by distance.
    assert_eq!(&sel[..3], &[0, 1, 2]);
    // The two furthest in-radius candidates are the ones cut.
    assert!(!sel.contains(&8) && !sel.contains(&9));
}

/// Intensity can outrank proximity: a far bright light beats a near dim
/// one when the attenuation gap does not cancel it.
#[test]
fn selection_weighs_intensity_against_attenuation() {
    let lights = vec![
        omni(4.0, 0.0, 0.0, 0.1, 50.0),  // near, dim
        omni(10.0, 0.0, 0.0, 10.0, 50.0), // further, blazing
    ];
    let anchors = [vec3f(0.0, 0.0, 0.0)];
    let (mut rank, mut sel) = (Vec::new(), Vec::new());
    select_strongest_lights(&lights, 0..2, &anchors, 8, &mut rank, &mut sel);
    assert_eq!(sel[0], 1, "brightness must be able to win");
}

/// Multi-anchor: a light near ANY anchor counts — the batch selection
/// must not starve a lamp that owns one far-flung character.
#[test]
fn selection_uses_the_nearest_anchor() {
    let lights = vec![omni(100.0, 0.0, 0.0, 1.0, 8.0)];
    let far_only = [vec3f(0.0, 0.0, 0.0)];
    let with_near = [vec3f(0.0, 0.0, 0.0), vec3f(99.0, 0.0, 0.0)];
    let (mut rank, mut sel) = (Vec::new(), Vec::new());
    select_strongest_lights(&lights, 0..1, &far_only, 8, &mut rank, &mut sel);
    assert!(sel.is_empty());
    select_strongest_lights(&lights, 0..1, &with_near, 8, &mut rank, &mut sel);
    assert_eq!(sel, vec![0]);
}

/// Appending a second ranked range (the transients-then-lamps split the
/// prop batch uses) respects the remaining slot budget and never
/// duplicates an index.
#[test]
fn split_selection_fills_remaining_slots_without_duplicates() {
    let mut lights = Vec::new();
    for i in 0..6 {
        lights.push(omni(i as f32, 0.0, 0.0, 1.0, 30.0)); // "lamps"
    }
    for i in 0..6 {
        lights.push(omni(0.5 + i as f32, 1.0, 0.0, 2.0, 30.0)); // "transients"
    }
    let anchors = [vec3f(0.0, 0.0, 0.0)];
    let (mut rank, mut sel) = (Vec::new(), Vec::new());
    // Transients first (indices 6..12), then lamps fill what is left.
    select_strongest_lights(&lights, 6..12, &anchors, MAX_DYNAMIC_LIGHTS, &mut rank, &mut sel);
    let split = sel.len();
    assert_eq!(split, 6);
    select_strongest_lights(
        &lights,
        0..6,
        &anchors,
        MAX_DYNAMIC_LIGHTS - split,
        &mut rank,
        &mut sel,
    );
    assert_eq!(sel.len(), 8);
    let mut seen = sel.clone();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 8, "no duplicate slots");
    assert!(sel[..6].iter().all(|i| *i >= 6), "transients keep the prefix");
    assert!(sel[6..].iter().all(|i| *i < 6), "lamps fill the tail");
}

/// The firework flash: nothing while climbing, a bright pop at the
/// burst, decaying over the shell's life, coloured like the shell.
#[test]
fn firework_flash_derivation() {
    let mk = |age: f32| crate::firework::FireworkInstance {
        origin: vec3f(10.0, 35.0, -20.0),
        age,
        life: 4.0,
        speed: 30.0,
        seed: 1.0,
        color: vec4(1.0, 0.4, 0.2, 1.0),
        color_tail: vec4(1.0, 0.2, 0.1, 1.0),
        launch: vec3f(10.0, 0.5, -20.0),
        style: 0.0,
    };
    assert!(firework_flash_light(&mk(-0.5)).is_none(), "climbing shell has no flash");
    assert!(firework_flash_light(&mk(4.5)).is_none(), "expired shell has no flash");
    let burst = firework_flash_light(&mk(0.0)).expect("burst flash");
    let late = firework_flash_light(&mk(3.0)).expect("late glow");
    assert_eq!(burst.pos, vec3f(10.0, 35.0, -20.0), "flash sits at the burst point");
    assert!(burst.radius > 0.0 && burst.spot == 0.0, "omni with a real radius");
    assert!(
        light_intensity(&burst) > 4.0 * light_intensity(&late),
        "flash must decay over the shell's life: {} vs {}",
        light_intensity(&burst),
        light_intensity(&late)
    );
    // Shell hue carried through: red-dominant shell, red-dominant flash.
    assert!(burst.color.x > burst.color.y && burst.color.y > burst.color.z);
}
