// CSG Unicorn — A stylish low-poly unicorn built entirely from CSG primitives
//
// Coordinate system: Y = up, Z = forward (toward head), X = left/right
// All primitives (cylinder, cone, etc.) grow along +Y by default.
// We use a `point_y_toward(dir)` helper to orient them in any direction,
// avoiding confusing Euler angle rotations entirely.

use makepad_csg::Solid;
use makepad_csg_math::{dvec3, Mat4d, Vec3d};
use std::f64::consts::PI;
use std::time::Instant;

/// Returns a rotation matrix that maps the +Y axis to the given direction.
/// All CSG primitives grow along +Y, so this lets us aim them anywhere.
fn point_y_toward(dir: Vec3d) -> Mat4d {
    let to = dir.normalize();
    let from = Vec3d::Y;
    let cross = from.cross(to);
    let dot = from.dot(to);

    if cross.length() < 1e-10 {
        if dot > 0.0 {
            return Mat4d::identity(); // already aligned
        } else {
            // 180° flip — rotate around X or Z
            return Mat4d::rotation(Vec3d::X, PI);
        }
    }

    let axis = cross.normalize();
    let angle = dot.acos();
    Mat4d::rotation(axis, angle)
}

/// Place a primitive: rotate its +Y axis to point along `dir`, then translate to `pos`.
fn oriented(solid: &Solid, dir: Vec3d, pos: Vec3d) -> Solid {
    solid
        .transform(point_y_toward(dir))
        .translate(pos.x, pos.y, pos.z)
}

fn timed(t: &Instant, msg: &str) {
    println!("[{:6.2}s] {}", t.elapsed().as_secs_f64(), msg);
}

fn write(t: &Instant, solid: &Solid, dir: &str, name: &str) {
    let path = format!("{}/{}.stl", dir, name);
    solid.write_stl(&path).unwrap();
    timed(
        t,
        &format!(
            "  {} -- {} tris, vol={:.1}",
            name,
            solid.triangle_count(),
            solid.volume()
        ),
    );
}

fn main() {
    let out_dir = "/Users/admin/makepad/makepad/libs/csg/output";
    std::fs::create_dir_all(out_dir).unwrap();
    let t = Instant::now();

    timed(&t, "Building unicorn...");
    let unicorn = build_unicorn();
    write(&t, &unicorn, out_dir, "unicorn");

    timed(&t, "All done!");
    println!("\nUnicorn STL written to: {}", out_dir);
}

fn build_unicorn() -> Solid {
    let seg = 16;
    let seg_hi = 24;

    // Directions we'll reuse
    let down = dvec3(0.0, -1.0, 0.0);

    // ================================================================
    // BODY — horizontal cylinder along Z
    // ================================================================
    let body_len = 11.0;
    let body_r = 2.5;
    let body_dir = dvec3(0.0, 0.0, 1.0); // along +Z
    let body = oriented(
        &Solid::cylinder(body_r, body_len, seg_hi, true),
        body_dir,
        dvec3(0.0, 0.0, 0.0),
    );
    // Body spans: Y [-2.5, 2.5], Z [-8, 8]

    let body_cap_front = Solid::sphere(body_r, seg_hi, seg / 2)
        .scale(1.0, 1.0, 0.5)
        .translate(0.0, 0.0, body_len / 2.0);
    let body_cap_back = Solid::sphere(body_r, seg_hi, seg / 2)
        .scale(1.0, 1.0, 0.5)
        .translate(0.0, 0.0, -body_len / 2.0);
    let body = body.merge(&body_cap_front).merge(&body_cap_back);

    // ================================================================
    // LEGS — point straight down, overlap into body
    // ================================================================
    let leg_r = 0.8;
    let leg_h = 5.0;
    let hoof_r = 1.0;
    let hoof_h = 0.6;
    let body_bottom = -body_r; // Y = -2.5
    let leg_embed = 1.0; // how much leg penetrates into body

    let leg_positions = [
        (1.3, body_len / 2.0 - 1.5),   // front-left  (x, z)
        (-1.3, body_len / 2.0 - 1.5),  // front-right
        (1.3, -body_len / 2.0 + 1.5),  // back-left
        (-1.3, -body_len / 2.0 + 1.5), // back-right
    ];

    let mut legs = Solid::empty();
    for &(lx, lz) in &leg_positions {
        // Leg starts embedded in body, goes down
        let leg_top = dvec3(lx, body_bottom + leg_embed, lz);
        let leg = oriented(
            &Solid::tapered_cylinder(leg_r, leg_r * 0.65, leg_h + leg_embed, seg, false),
            down,
            leg_top,
        );

        // Hoof at bottom of leg
        let hoof_top_y = body_bottom + leg_embed - (leg_h + leg_embed);
        let hoof = oriented(
            &Solid::cylinder(hoof_r, hoof_h, seg, false),
            down,
            dvec3(lx, hoof_top_y, lz),
        );

        // Fetlock ring where leg meets hoof
        let fetlock = Solid::torus(leg_r * 0.7, 0.15, 10, 6).translate(lx, hoof_top_y, lz);

        legs = legs.merge(&leg).merge(&hoof).merge(&fetlock);
    }

    // ================================================================
    // NECK — angled up and forward from body front
    // Direction: mostly up (+Y), tilted forward (+Z)
    // ================================================================
    let neck_r_bottom = 2.0;
    let neck_r_top = 1.6;
    let neck_len = 5.0;
    let neck_dir = dvec3(0.0, 0.85, 0.45).normalize(); // up-and-forward
    let neck_base = dvec3(0.0, body_r * 0.2, body_len / 2.0 - 2.0);
    // Embed the neck base 1 unit into the body
    let neck_start = dvec3(
        neck_base.x - neck_dir.x * 1.0,
        neck_base.y - neck_dir.y * 1.0,
        neck_base.z - neck_dir.z * 1.0,
    );
    let neck = oriented(
        &Solid::tapered_cylinder(neck_r_bottom, neck_r_top, neck_len + 1.0, seg, false),
        neck_dir,
        neck_start,
    );

    // Neck top position (for head placement)
    let neck_top = dvec3(
        neck_base.x + neck_dir.x * neck_len,
        neck_base.y + neck_dir.y * neck_len,
        neck_base.z + neck_dir.z * neck_len,
    );

    // ================================================================
    // HEAD — sphere at neck top
    // ================================================================
    let head_r = 2.8;
    let head_pos = neck_top;
    let head = Solid::sphere(head_r, seg_hi, seg)
        .scale(0.9, 1.05, 0.85)
        .translate(head_pos.x, head_pos.y, head_pos.z);

    // ================================================================
    // MUZZLE — extends forward and downward ~30° from head center
    // ================================================================
    let muzzle_angle = 30.0_f64.to_radians();
    let muzzle_dist = head_r * 0.7;
    let muzzle_pos = dvec3(
        0.0,
        head_pos.y - muzzle_dist * muzzle_angle.sin(),
        head_pos.z + muzzle_dist * muzzle_angle.cos(),
    );
    let muzzle = Solid::sphere(1.6, seg, seg / 2)
        .scale(0.75, 0.65, 1.2)
        .translate(muzzle_pos.x, muzzle_pos.y, muzzle_pos.z);

    // Nostrils at tip of muzzle, also angled down
    let nostril_dist = 1.5;
    let nostril_center = dvec3(
        0.0,
        muzzle_pos.y - nostril_dist * muzzle_angle.sin(),
        muzzle_pos.z + nostril_dist * muzzle_angle.cos(),
    );
    let nostril_l = Solid::sphere(0.3, 8, 6).translate(0.5, nostril_center.y, nostril_center.z);
    let nostril_r = Solid::sphere(0.3, 8, 6).translate(-0.5, nostril_center.y, nostril_center.z);

    // ================================================================
    // EYES — simple ball eyes protruding from head
    // ================================================================
    let eye_y = head_pos.y + 0.5;
    let eye_z = head_pos.z + head_r * 0.55;
    let eye_x = 1.5;

    let eye_l = Solid::sphere(0.6, 12, 8).translate(eye_x, eye_y, eye_z);
    let eye_r = Solid::sphere(0.6, 12, 8).translate(-eye_x, eye_y, eye_z);

    // ================================================================
    // HORN — spiraling upward from head top, tilted slightly forward
    // ================================================================
    let horn_h = 5.0;
    let horn_r = 0.65;
    let horn_profile: Vec<[f64; 2]> = (0..8)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / 8.0;
            [horn_r * a.cos(), horn_r * a.sin()]
        })
        .collect();
    let horn_dir = dvec3(0.0, 0.7, 0.5).normalize(); // up and noticeably forward
    let horn_base = dvec3(0.0, head_pos.y + head_r * 0.85, head_pos.z);
    let horn = oriented(
        &Solid::linear_extrude(&horn_profile, horn_h, 360.0, 0.05, 16),
        horn_dir,
        horn_base,
    );

    // ================================================================
    // EARS — small cones angling up and outward
    // ================================================================
    let ear_h = 1.6;
    let ear_r = 0.45;
    let ear_l_dir = dvec3(0.4, 0.9, -0.1).normalize(); // up-left-back
    let ear_r_dir = dvec3(-0.4, 0.9, -0.1).normalize(); // up-right-back
    let ear_l_pos = dvec3(1.0, head_pos.y + head_r * 0.7, head_pos.z - 0.5);
    let ear_r_pos = dvec3(-1.0, head_pos.y + head_r * 0.7, head_pos.z - 0.5);

    let ear_l = oriented(&Solid::cone(ear_r, ear_h, 8, false), ear_l_dir, ear_l_pos);
    let ear_r_shape = oriented(&Solid::cone(ear_r, ear_h, 8, false), ear_r_dir, ear_r_pos);

    // ================================================================
    // TAIL — flows backward (-Z) and slightly upward from body rear
    // ================================================================
    let tail_dir = dvec3(0.0, -0.7, -0.5).normalize(); // mostly downward, angled backward
    let tail_base_pos = dvec3(0.0, body_r * 0.5, -body_len / 2.0);

    // Main tail bone — embedded 0.5 into body
    let tail_bone_start = dvec3(
        tail_base_pos.x - tail_dir.x * 0.5,
        tail_base_pos.y - tail_dir.y * 0.5,
        tail_base_pos.z - tail_dir.z * 0.5,
    );
    let tail_base = oriented(
        &Solid::tapered_cylinder(0.7, 0.1, 5.5, seg, false),
        tail_dir,
        tail_bone_start,
    );

    // Tail strands — fan out from tail base
    let strand_profile: Vec<[f64; 2]> = (0..6)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / 6.0;
            [0.2 * a.cos(), 0.2 * a.sin()]
        })
        .collect();

    let mut tail_strands = Solid::empty();
    for i in 0..5 {
        let x_spread = (i as f64 - 2.0) * 0.08;
        let y_spread = (i as f64 - 2.0) * 0.06;
        let strand_dir = dvec3(x_spread, -0.7 + y_spread, -0.5).normalize();
        let twist = 60.0 + (i as f64) * 25.0;

        let strand_start = dvec3(
            tail_base_pos.x - strand_dir.x * 0.3,
            tail_base_pos.y - strand_dir.y * 0.3,
            tail_base_pos.z - strand_dir.z * 0.3,
        );
        let strand = oriented(
            &Solid::linear_extrude(&strand_profile, 5.0, twist, 0.25, 8),
            strand_dir,
            strand_start,
        );
        tail_strands = tail_strands.merge(&strand);
    }

    // ================================================================
    // ASSEMBLY
    // ================================================================
    timed(&Instant::now(), "Assembling unicorn...");

    let mut unicorn = body
        .merge(&neck)
        .merge(&head)
        .merge(&muzzle)
        .merge(&horn)
        .merge(&ear_l)
        .merge(&ear_r_shape)
        .merge(&legs)
        .merge(&tail_base)
        .merge(&tail_strands);

    // Add ball eyes
    unicorn = unicorn.merge(&eye_l).merge(&eye_r);

    // Carve nostrils
    unicorn = unicorn.difference(&nostril_l);
    unicorn = unicorn.difference(&nostril_r);

    // Rotate whole model so the viewer (looking up from -Y) sees the unicorn
    // standing upright: head along +Y, feet along -Y.
    // Our build has head at +Z, feet at -Y. Rotate -90° around X to swap Z→Y.
    let unicorn = unicorn.rotate_x(-90.0);

    timed(&Instant::now(), "Unicorn complete!");
    unicorn
}
