// CSG Alien — A goofy alien figurine with blobby SDF body, limbs, and arms
//
// Coordinate system: Y = up, Z = forward (toward face), X = left/right
// Body, head, eyes, legs are SDF blob chains for organic blobby look.
// Only antennae and ears remain as mesh primitives.

use makepad_csg::{SdfBlobChain, SdfCapsule, SdfEllipsoid, SdfSphere as SdfSph, SdfWarp, Solid};
use makepad_csg_math::{dvec3, Vec3d};
use std::f64::consts::PI;
use std::time::Instant;

fn point_y_toward(dir: Vec3d) -> makepad_csg_math::Mat4d {
    let to = dir.normalize();
    let from = Vec3d::Y;
    let cross = from.cross(to);
    let dot = from.dot(to);
    if cross.length() < 1e-10 {
        if dot > 0.0 {
            return makepad_csg_math::Mat4d::identity();
        } else {
            return makepad_csg_math::Mat4d::rotation(Vec3d::X, PI);
        }
    }
    makepad_csg_math::Mat4d::rotation(cross.normalize(), dot.acos())
}

fn oriented(solid: &Solid, dir: Vec3d, pos: Vec3d) -> Solid {
    solid
        .transform(point_y_toward(dir))
        .translate(pos.x, pos.y, pos.z)
}

fn timed(t: &Instant, msg: &str) {
    println!("[{:6.2}s] {}", t.elapsed().as_secs_f64(), msg);
}

fn main() {
    let out_dir = "/Users/admin/makepad/makepad/libs/csg/output";
    std::fs::create_dir_all(out_dir).unwrap();
    let t = Instant::now();

    timed(
        &t,
        "Building fully blobby alien (except antennae & ears)...",
    );
    let alien = build_alien(&t);

    let path = format!("{}/alien.stl", out_dir);
    alien.write_stl(&path).unwrap();
    timed(
        &t,
        &format!(
            "  alien -- {} tris, vol={:.1}",
            alien.triangle_count(),
            alien.volume()
        ),
    );
    timed(&t, "All done!");
}

fn build_alien(t: &Instant) -> Solid {
    // ================================================================
    // Key positions (same proportions as before)
    // ================================================================
    let head_r = 2.75;
    let head_pos = dvec3(0.0, 9.5, 0.0);
    let neck_r = 0.95;
    let neck_len = 2.5;
    let neck_pos = dvec3(0.0, head_pos.y - head_r * 1.1, 0.0);
    let torso_top_y = neck_pos.y - neck_len;
    let torso_pos = dvec3(0.0, torso_top_y - 1.5, 0.0);
    let torso_radius = 2.6;
    let belly_radius = 3.2;
    let belly_y_scale = 0.92;
    let belly_bottom = torso_pos.y - 1.0 - belly_radius * belly_y_scale;
    let chip_center = dvec3(0.0, torso_pos.y + 0.75, torso_pos.z + 4.1);

    // ================================================================
    // BODY BLOB — head + cranium + neck + torso + belly
    // All smooth-unioned into one organic shape
    // ================================================================
    timed(t, "  Building blobby body...");
    let body_gloop = 0.72;

    let body_blob = SdfBlobChain::new(body_gloop)
        // Head — big ellipsoidal dome
        .add(SdfEllipsoid::new(
            head_pos,
            dvec3(head_r * 1.15, head_r * 1.3, head_r * 0.95),
        ))
        // Cranium bulge on top
        .add(SdfEllipsoid::new(
            dvec3(0.0, head_pos.y + 3.2, 0.0),
            dvec3(2.8 * 1.1, 2.8 * 0.7, 2.8 * 1.0),
        ))
        // Neck — capsule connecting head to torso
        .add(SdfCapsule::new(
            neck_pos,
            dvec3(0.0, neck_pos.y - neck_len, 0.0),
            neck_r,
        ))
        // Upper torso
        .add(SdfEllipsoid::new(
            dvec3(torso_pos.x, torso_pos.y + 0.5, torso_pos.z),
            dvec3(torso_radius, torso_radius * 1.1, torso_radius * 1.0),
        ))
        // Shoulder blobs for a softer, blob-like transition into arms
        .add(SdfEllipsoid::new(
            dvec3(torso_pos.x + 1.9, torso_pos.y + 1.1, torso_pos.z + 0.1),
            dvec3(1.0, 0.9, 0.85),
        ))
        .add(SdfEllipsoid::new(
            dvec3(torso_pos.x - 1.9, torso_pos.y + 1.1, torso_pos.z + 0.1),
            dvec3(1.0, 0.9, 0.85),
        ))
        // Belly — much larger for a chubby silhouette
        .add(SdfEllipsoid::new(
            dvec3(torso_pos.x, torso_pos.y - 1.0, torso_pos.z + 0.2),
            dvec3(
                belly_radius,
                belly_radius * belly_y_scale,
                belly_radius * 1.15,
            ),
        ));

    // Add wavy warp to the body for organic alien skin texture
    let body_wavy = SdfWarp::new(body_blob, dvec3(0.0, 7.0, 0.0), |_center, p, d| {
        d + 0.05 * (p.x * 3.8).sin() * (p.y * 4.6).sin() * (p.z * 4.2).cos()
    });

    // Mesh the body blob
    let body_bounds_min = dvec3(-6.0, belly_bottom - 1.0, -5.0);
    let body_bounds_max = dvec3(6.0, head_pos.y + 6.0, 5.0);
    let body_solid = Solid::from_sdf(body_wavy, body_bounds_min, body_bounds_max, 7);
    timed(
        t,
        &format!("    Body blob: {} tris", body_solid.triangle_count()),
    );

    // ================================================================
    // EYES — flatter organic eye pads (less protruding)
    // ================================================================
    timed(t, "  Building blobby eyes...");
    let eye_y = head_pos.y + 0.35;
    let eye_z = head_pos.z + head_r * 0.68;
    let eye_gloop = 0.48;

    let left_eye = SdfBlobChain::new(eye_gloop)
        .add(SdfEllipsoid::new(
            dvec3(1.25, eye_y, eye_z),
            dvec3(0.95, 0.58, 0.30),
        ))
        .add(SdfEllipsoid::new(
            dvec3(1.15, eye_y + 0.1, eye_z - 0.11),
            dvec3(0.62, 0.34, 0.20),
        ));

    let right_eye = SdfBlobChain::new(eye_gloop)
        .add(SdfEllipsoid::new(
            dvec3(-1.25, eye_y, eye_z),
            dvec3(0.95, 0.58, 0.30),
        ))
        .add(SdfEllipsoid::new(
            dvec3(-1.15, eye_y + 0.1, eye_z - 0.11),
            dvec3(0.62, 0.34, 0.20),
        ));

    // Mesh the eyes
    let eye_bounds_min = dvec3(-3.8, eye_y - 1.6, eye_z - 1.6);
    let eye_bounds_max = dvec3(3.8, eye_y + 1.8, eye_z + 1.8);
    let left_eye_solid = Solid::from_sdf(left_eye, eye_bounds_min, eye_bounds_max, 6);
    let right_eye_solid = Solid::from_sdf(right_eye, eye_bounds_min, eye_bounds_max, 6);
    timed(
        t,
        &format!(
            "    Eyes: {} + {} tris",
            left_eye_solid.triangle_count(),
            right_eye_solid.triangle_count()
        ),
    );

    // ================================================================
    // ARMS — blobby SDF capsule chain posing to hold the chip
    // ================================================================
    timed(t, "  Building blobby SDF arms...");

    let arm_r = 0.45;
    let arm_gloop = 0.82;

    let arm_l_start = dvec3(2.6, torso_pos.y + 1.0, 0.1);
    let elbow_l = dvec3(2.95, torso_pos.y + 0.55, 2.1);
    let hand_l_pos = dvec3(chip_center.x + 2.05, chip_center.y + 0.02, chip_center.z + 0.0);

    let arm_r_start = dvec3(-2.6, torso_pos.y + 1.0, 0.1);
    let elbow_r = dvec3(-2.95, torso_pos.y + 0.55, 2.1);
    let hand_r_pos = dvec3(chip_center.x - 2.05, chip_center.y + 0.02, chip_center.z + 0.0);

    let finger_len = 1.2;
    let finger_r = 0.12;
    let finger_tip_r = 0.22;

    let finger_dirs_l = [
        dvec3(-0.95, -0.12, 0.25).normalize(),
        dvec3(-0.98, 0.0, 0.1).normalize(),
        dvec3(-0.9, 0.08, -0.25).normalize(),
    ];
    let finger_dirs_r = [
        dvec3(0.95, -0.12, 0.25).normalize(),
        dvec3(0.98, 0.0, 0.1).normalize(),
        dvec3(0.9, 0.08, -0.25).normalize(),
    ];

    let mut left_arm = SdfBlobChain::new(arm_gloop)
        .add(SdfSph::new(arm_l_start, arm_r * 1.3))
        .add(SdfCapsule::new(arm_l_start, elbow_l, arm_r))
        .add(SdfSph::new(elbow_l, arm_r * 0.9))
        .add(SdfCapsule::new(elbow_l, hand_l_pos, arm_r * 0.7))
        .add(SdfSph::new(hand_l_pos, 0.5));

    for dir in &finger_dirs_l {
        let tip = hand_l_pos + *dir * finger_len;
        left_arm = left_arm
            .add(SdfCapsule::new(hand_l_pos, tip, finger_r))
            .add(SdfSph::new(tip, finger_tip_r));
    }

    let mut right_arm = SdfBlobChain::new(arm_gloop)
        .add(SdfSph::new(arm_r_start, arm_r * 1.3))
        .add(SdfCapsule::new(arm_r_start, elbow_r, arm_r))
        .add(SdfSph::new(elbow_r, arm_r * 0.9))
        .add(SdfCapsule::new(elbow_r, hand_r_pos, arm_r * 0.7))
        .add(SdfSph::new(hand_r_pos, 0.5));

    for dir in &finger_dirs_r {
        let tip = hand_r_pos + *dir * finger_len;
        right_arm = right_arm
            .add(SdfCapsule::new(hand_r_pos, tip, finger_r))
            .add(SdfSph::new(tip, finger_tip_r));
    }

    let left_arm_wavy = SdfWarp::new(left_arm, arm_l_start, |_center, p, d| {
        d + 0.06 * (p.x * 4.2).sin() * (p.y * 5.7).sin() * (p.z * 5.4).cos()
    });
    let right_arm_wavy = SdfWarp::new(right_arm, arm_r_start, |_center, p, d| {
        d + 0.06 * (p.x * 4.2).sin() * (p.y * 5.7).sin() * (p.z * 5.4).cos()
    });

    let arm_bounds_min = dvec3(-12.0, -6.0, -4.0);
    let arm_bounds_max = dvec3(12.0, torso_pos.y + 3.0, 6.0);
    let left_arm_solid = Solid::from_sdf(left_arm_wavy, arm_bounds_min, arm_bounds_max, 7);
    timed(
        t,
        &format!("    Left arm: {} tris", left_arm_solid.triangle_count()),
    );
    let right_arm_solid = Solid::from_sdf(right_arm_wavy, arm_bounds_min, arm_bounds_max, 7);
    timed(
        t,
        &format!("    Right arm: {} tris", right_arm_solid.triangle_count()),
    );

    // ================================================================
    // LEGS — blobby SDF blob chains (leg + foot + toes per leg)
    // ================================================================
    timed(t, "  Building blobby legs...");
    let leg_r = 0.7;
    let leg_h = 3.5;
    let leg_gloop = 0.7;

    let leg_l_dir = dvec3(0.15, -1.0, 0.0).normalize();
    let leg_r_dir = dvec3(-0.15, -1.0, 0.0).normalize();
    let leg_l_start = dvec3(0.9, belly_bottom + 0.5, 0.0);
    let leg_r_start = dvec3(-0.9, belly_bottom + 0.5, 0.0);
    let foot_l_pos = leg_l_start + leg_l_dir * leg_h;
    let foot_r_pos = leg_r_start + leg_r_dir * leg_h;

    let toe_dirs = [
        dvec3(0.3, 0.0, 1.0).normalize(),
        dvec3(0.0, 0.0, 1.0),
        dvec3(-0.3, 0.0, 1.0).normalize(),
    ];
    let toe_len = 0.8;
    let toe_r = 0.15;
    let toe_tip_r = 0.12;

    // Left leg blob: tapered capsule + flat foot ellipsoid + toes
    let mut left_leg = SdfBlobChain::new(leg_gloop)
        // Hip joint
        .add(SdfSph::new(leg_l_start, leg_r * 1.1))
        // Leg shaft — capsule from hip to ankle
        .add(SdfCapsule::new(leg_l_start, foot_l_pos, leg_r * 0.6))
        // Ankle/foot — flat wide ellipsoid
        .add(SdfEllipsoid::new(
            dvec3(foot_l_pos.x, foot_l_pos.y - 0.1, foot_l_pos.z + 0.5),
            dvec3(0.8, 0.35, 1.5),
        ));

    // Toes on left foot
    for dir in &toe_dirs {
        let toe_start = dvec3(foot_l_pos.x, foot_l_pos.y - 0.1, foot_l_pos.z + 1.0);
        let toe_tip = toe_start + *dir * toe_len;
        left_leg = left_leg
            .add(SdfCapsule::new(toe_start, toe_tip, toe_r))
            .add(SdfSph::new(toe_tip, toe_tip_r));
    }

    // Right leg blob
    let mut right_leg = SdfBlobChain::new(leg_gloop)
        .add(SdfSph::new(leg_r_start, leg_r * 1.1))
        .add(SdfCapsule::new(leg_r_start, foot_r_pos, leg_r * 0.6))
        .add(SdfEllipsoid::new(
            dvec3(foot_r_pos.x, foot_r_pos.y - 0.1, foot_r_pos.z + 0.5),
            dvec3(0.8, 0.35, 1.5),
        ));

    for dir in &toe_dirs {
        let toe_start = dvec3(foot_r_pos.x, foot_r_pos.y - 0.1, foot_r_pos.z + 1.0);
        let toe_tip = toe_start + *dir * toe_len;
        right_leg = right_leg
            .add(SdfCapsule::new(toe_start, toe_tip, toe_r))
            .add(SdfSph::new(toe_tip, toe_tip_r));
    }

    // Add wavy warp to legs
    let left_leg_wavy = SdfWarp::new(left_leg, leg_l_start, |_center, p, d| {
        d + 0.038 * (p.x * 5.2).sin() * (p.y * 6.8).sin() * (p.z * 4.9).cos()
    });
    let right_leg_wavy = SdfWarp::new(right_leg, leg_r_start, |_center, p, d| {
        d + 0.038 * (p.x * 5.2).sin() * (p.y * 6.8).sin() * (p.z * 4.9).cos()
    });

    // Mesh the legs
    let leg_bounds_min = dvec3(-4.0, foot_l_pos.y - 2.0, -3.0);
    let leg_bounds_max = dvec3(4.0, belly_bottom + 2.0, 4.0);
    let left_leg_solid = Solid::from_sdf(left_leg_wavy, leg_bounds_min, leg_bounds_max, 7);
    timed(
        t,
        &format!("    Left leg: {} tris", left_leg_solid.triangle_count()),
    );
    let right_leg_solid = Solid::from_sdf(right_leg_wavy, leg_bounds_min, leg_bounds_max, 7);
    timed(
        t,
        &format!("    Right leg: {} tris", right_leg_solid.triangle_count()),
    );

    // ================================================================
    // ANTENNAE (mesh primitives — kept as-is)
    // ================================================================
    let seg = 16;
    let antenna_r = 0.12;
    let antenna_h = 3.5;
    let bobble_r = 0.5;
    let ant_l_dir = dvec3(0.4, 0.9, -0.15).normalize();
    let ant_l_base = dvec3(0.8, head_pos.y + head_r * 1.15, head_pos.z - 0.5);
    let ant_l = oriented(
        &Solid::tapered_cylinder(antenna_r, antenna_r * 0.7, antenna_h, 8, false),
        ant_l_dir,
        ant_l_base,
    );
    let ant_l_tip = ant_l_base + ant_l_dir * antenna_h;
    let bobble_l = Solid::sphere(bobble_r, 10, 8).translate(ant_l_tip.x, ant_l_tip.y, ant_l_tip.z);

    let ant_r_dir = dvec3(-0.4, 0.9, -0.15).normalize();
    let ant_r_base = dvec3(-0.8, head_pos.y + head_r * 1.15, head_pos.z - 0.5);
    let ant_r = oriented(
        &Solid::tapered_cylinder(antenna_r, antenna_r * 0.7, antenna_h, 8, false),
        ant_r_dir,
        ant_r_base,
    );
    let ant_r_tip = ant_r_base + ant_r_dir * antenna_h;
    let bobble_r_shape =
        Solid::sphere(bobble_r, 10, 8).translate(ant_r_tip.x, ant_r_tip.y, ant_r_tip.z);

    // ================================================================
    // EARS (mesh primitives — kept as-is)
    // ================================================================
    let ear_h = 1.2;
    let ear_r = 0.35;
    let ear_l_pos = dvec3(head_r * 0.95, head_pos.y - 0.3, head_pos.z - 0.3);
    let ear_r_pos = dvec3(-head_r * 0.95, head_pos.y - 0.3, head_pos.z - 0.3);
    let ear_l = oriented(
        &Solid::cone(ear_r, ear_h, 8, false),
        dvec3(1.0, 0.3, 0.0).normalize(),
        ear_l_pos,
    );
    let ear_r_shape = oriented(
        &Solid::cone(ear_r, ear_h, 8, false),
        dvec3(-1.0, 0.3, 0.0).normalize(),
        ear_r_pos,
    );

    // ================================================================
    // QUANTUM CHIP DETAIL (mesh primitives)
    // A handheld module inspired by superconducting quantum hardware.
    // ================================================================
    let chip_base = Solid::cube(3.8, 0.42, 2.7, true).translate(
        chip_center.x,
        chip_center.y,
        chip_center.z,
    );
    let resonator_ring = Solid::torus(0.62, 0.11, 28, 14)
        .rotate_x(90.0)
        .translate(chip_center.x, chip_center.y + 0.05, chip_center.z + 0.08);
    let resonator_ring_l = Solid::torus(0.32, 0.08, 22, 12)
        .rotate_x(90.0)
        .translate(chip_center.x - 1.0, chip_center.y + 0.03, chip_center.z + 0.12);
    let resonator_ring_r = Solid::torus(0.32, 0.08, 22, 12)
        .rotate_x(90.0)
        .translate(chip_center.x + 1.0, chip_center.y + 0.03, chip_center.z + 0.12);

    let mut quantum_chip = chip_base
        .merge(&resonator_ring)
        .merge(&resonator_ring_l)
        .merge(&resonator_ring_r);

    for i in -3..=3 {
        let z = chip_center.z + (i as f64) * 0.34;
        let pin_l = Solid::cube(0.7, 0.08, 0.08, true).translate(
            chip_center.x - 2.15,
            chip_center.y - 0.02,
            z,
        );
        let pin_r = Solid::cube(0.7, 0.08, 0.08, true).translate(
            chip_center.x + 2.15,
            chip_center.y - 0.02,
            z,
        );
        quantum_chip = quantum_chip.merge(&pin_l).merge(&pin_r);
    }

    for (x, z) in [(-1.0, -0.55), (0.0, -0.45), (1.0, -0.55), (-0.5, 0.58), (0.5, 0.58)] {
        let qubit_node = Solid::sphere(0.14, 16, 10).translate(
            chip_center.x + x,
            chip_center.y + 0.11,
            chip_center.z + z,
        );
        quantum_chip = quantum_chip.merge(&qubit_node);
    }

    // ================================================================
    // MOUTH + NOSTRILS + BELLY BUTTON (carved out as before)
    // ================================================================
    let mouth_y = head_pos.y - 2.0;
    let mouth_z = head_pos.z + head_r * 0.9;
    let mouth = Solid::sphere(1.2, seg, 8)
        .scale(1.8, 0.35, 0.6)
        .translate(0.0, mouth_y, mouth_z);
    let nostril_y = head_pos.y - 1.0;
    let nostril_z = head_pos.z + head_r * 1.0;
    let nostril_l = Solid::sphere(0.2, 8, 6).translate(0.35, nostril_y, nostril_z);
    let nostril_r = Solid::sphere(0.2, 8, 6).translate(-0.35, nostril_y, nostril_z);
    let belly_button =
        Solid::sphere(0.25, 8, 6).translate(0.0, torso_pos.y - 1.2, torso_pos.z + 2.2);
    let cutouts = mouth
        .merge(&nostril_l)
        .merge(&nostril_r)
        .merge(&belly_button);
    let body_solid = body_solid.difference(&cutouts);

    // ================================================================
    // ASSEMBLY
    // ================================================================
    timed(t, "  Assembling...");

    let alien = body_solid
        // Blobby eyes
        .merge(&left_eye_solid)
        .merge(&right_eye_solid)
        // Blobby SDF arms
        .merge(&left_arm_solid)
        .merge(&right_arm_solid)
        // Blobby SDF legs
        .merge(&left_leg_solid)
        .merge(&right_leg_solid)
        // Quantum-computer-inspired handheld module
        .merge(&quantum_chip)
        // Mesh ears
        .merge(&ear_l)
        .merge(&ear_r_shape)
        // Mesh antennae
        .merge(&ant_l)
        .merge(&ant_r)
        .merge(&bobble_l)
        .merge(&bobble_r_shape);

    // Rotate upright
    alien.rotate_x(-90.0)
}
