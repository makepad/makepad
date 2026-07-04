// Port of box3d/test/test_height_field.c
// The C `hf->version == B3_HEIGHT_FIELD_VERSION` blob-metadata assertion has no
// Rust counterpart (the Vec-based HeightFieldData carries no version field).

use makepad_box3d::distance::shape_cast;
use makepad_box3d::height_field::*;
use makepad_box3d::math_functions::*;
use makepad_box3d::simd::{intersect_ray_triangle, load_vec3};
use makepad_box3d::types::{
    CastOutput, HeightFieldData, HeightFieldDef, RayCastInput, ShapeCastInput, ShapeCastPairInput,
    ShapeProxy, HEIGHT_FIELD_HOLE,
};
use makepad_box3d::{ensure, ensure_small};

#[test]
fn height_field_create() {
    let scale = vec3(1.0, 1.0, 1.0);
    let hf = create_grid(4, 4, scale, false);

    ensure!(hf.row_count == 4);
    ensure!(hf.column_count == 4);
    ensure!(hf.clockwise == false);
    // C: ENSURE( hf->version == B3_HEIGHT_FIELD_VERSION ) — no version field in the port.

    ensure_small!(hf.aabb.lower_bound.x, f32::EPSILON);
    ensure_small!(hf.aabb.lower_bound.y, f32::EPSILON);
    ensure_small!(hf.aabb.lower_bound.z, f32::EPSILON);

    ensure_small!(hf.aabb.upper_bound.x - 3.0, f32::EPSILON);
    ensure_small!(hf.aabb.upper_bound.y, f32::EPSILON);
    ensure_small!(hf.aabb.upper_bound.z - 3.0, f32::EPSILON);
}

#[test]
fn height_field_triangle_index() {
    // Asymmetric grid (rows != cols) catches off-by-one errors between
    // vertex stride (columnCount) and cell stride (columnCount - 1).
    let row_count = 4;
    let column_count = 5;
    let scale = vec3(1.0, 1.0, 1.0);
    let hf = create_grid(row_count, column_count, scale, false);

    let triangle_count = 2 * (row_count - 1) * (column_count - 1);

    for triangle_index in 0..triangle_count {
        let quad_index = triangle_index >> 1;
        let sub = triangle_index & 1;
        let row = quad_index / (column_count - 1);
        let column = quad_index - row * (column_count - 1);

        let index11 = row * column_count + column;
        let index12 = index11 + 1;
        let index21 = (row + 1) * column_count + column;
        let index22 = index21 + 1;

        let t = get_height_field_triangle(&hf, triangle_index);

        if sub == 0 {
            // Triangle 0 (CCW): {11, 21, 12}
            ensure!(t.i1 == index11);
            ensure!(t.i2 == index21);
            ensure!(t.i3 == index12);
        } else {
            // Triangle 1 (CCW): {22, 12, 21}
            ensure!(t.i1 == index22);
            ensure!(t.i2 == index12);
            ensure!(t.i3 == index21);
        }
    }
}

#[test]
fn height_field_winding() {
    // Build the same flat 3x3 field with CCW and CW winding. The cross-product
    // normal of triangle 0 must flip sign accordingly.
    let heights = [0.0f32; 9];
    let materials = [0u8; 4];

    let mut def = HeightFieldDef {
        heights: &heights,
        material_indices: &materials,
        scale: vec3(1.0, 1.0, 1.0),
        count_x: 3,
        count_z: 3,
        global_minimum_height: -1.0,
        global_maximum_height: 1.0,
        ..Default::default()
    };

    def.clockwise_winding = false;
    let ccw = create_height_field(&def);

    def.clockwise_winding = true;
    let cw = create_height_field(&def);

    let ta = get_height_field_triangle(&ccw, 0);
    let tb = get_height_field_triangle(&cw, 0);

    let na = normalize(cross(sub(ta.vertices[1], ta.vertices[0]), sub(ta.vertices[2], ta.vertices[0])));
    let nb = normalize(cross(sub(tb.vertices[1], tb.vertices[0]), sub(tb.vertices[2], tb.vertices[0])));

    ensure_small!(na.x, f32::EPSILON);
    ensure_small!(na.y - 1.0, f32::EPSILON);
    ensure_small!(na.z, f32::EPSILON);

    ensure_small!(nb.x, f32::EPSILON);
    ensure_small!(nb.y + 1.0, f32::EPSILON);
    ensure_small!(nb.z, f32::EPSILON);
}

#[test]
fn ray_cast_flat_field() {
    // Build a flat 4x4 field with a tight quantization range so the recovered
    // surface stays within ~1e-5 of y=0 (create_grid uses -256..256 which
    // blows the 1/UINT16_MAX quantum up to ~4e-3 in y).
    let heights = [0.0f32; 16];
    let materials = [0u8; 9];

    let def = HeightFieldDef {
        heights: &heights,
        material_indices: &materials,
        scale: vec3(1.0, 1.0, 1.0),
        count_x: 4,
        count_z: 4,
        global_minimum_height: -1.0,
        global_maximum_height: 1.0,
        clockwise_winding: false,
        ..Default::default()
    };

    let hf = create_height_field(&def);

    // Origin sits clearly inside triangle 0 of cell (1, 1) — off the cell
    // diagonal x+z = 3. The translation overshoots the surface so the hit
    // fraction is strictly less than maxFraction.
    let input = RayCastInput {
        origin: vec3(1.25, 10.0, 1.25),
        translation: vec3(0.0, -20.0, 0.0),
        max_fraction: 1.0,
    };

    let out = ray_cast_height_field(&hf, &input);

    ensure!(out.hit == true);
    ensure_small!(out.fraction - 0.5, 1e-5);
    ensure_small!(out.normal.x, 1e-5);
    ensure_small!(out.normal.y - 1.0, 1e-5);
    ensure_small!(out.normal.z, 1e-5);
}

#[test]
fn overlap_at_surface() {
    let scale = vec3(1.0, 1.0, 1.0);
    let hf = create_grid(4, 4, scale, false);

    // Sphere center 1.0 above the surface, radius 0.5 — clear gap.
    let above = [vec3(1.5, 1.0, 1.5)];
    let proxy_above = ShapeProxy { points: &above, radius: 0.5 };
    let hit_above = overlap_height_field(&hf, Transform::IDENTITY, &proxy_above);
    ensure!(hit_above == false);

    // Sphere centered on the surface — radius pokes through.
    let through = [vec3(1.5, 0.0, 1.5)];
    let proxy_through = ShapeProxy { points: &through, radius: 0.5 };
    let hit_through = overlap_height_field(&hf, Transform::IDENTITY, &proxy_through);
    ensure!(hit_through == true);
}

#[test]
fn file_roundtrip() {
    let heights = [0.0f32, 0.5, -0.3, 0.1, 0.0, 0.0, 0.0, 0.2, 0.0];
    let materials = [0u8, HEIGHT_FIELD_HOLE, 1, 2];

    let def = HeightFieldDef {
        heights: &heights,
        material_indices: &materials,
        scale: vec3(1.5, 2.0, 0.75),
        count_x: 3,
        count_z: 3,
        global_minimum_height: -1.0,
        global_maximum_height: 1.0,
        clockwise_winding: true,
        ..Default::default()
    };

    let path = std::env::temp_dir().join("test_height_field_roundtrip.dat");
    let path = path.to_str().unwrap();
    dump_height_data(&def, path);

    let loaded = load_height_field(path);
    let _ = std::fs::remove_file(path);

    ensure!(loaded.is_some());
    let loaded = loaded.unwrap();
    ensure!(loaded.row_count == def.count_z);
    ensure!(loaded.column_count == def.count_x);
    ensure!(loaded.clockwise == def.clockwise_winding);

    ensure_small!(loaded.scale.x - def.scale.x, f32::EPSILON);
    ensure_small!(loaded.scale.y - def.scale.y, f32::EPSILON);
    ensure_small!(loaded.scale.z - def.scale.z, f32::EPSILON);
    ensure_small!(loaded.min_height - def.global_minimum_height, f32::EPSILON);
    ensure_small!(loaded.max_height - def.global_maximum_height, f32::EPSILON);

    let cell_count = ((def.count_x - 1) * (def.count_z - 1)) as usize;
    for i in 0..cell_count {
        ensure!(loaded.materials[i] == materials[i]);
    }

    // Recovered heights round-trip within the quantization tolerance.
    let quantum = (def.global_maximum_height - def.global_minimum_height) / u16::MAX as f32;
    for i in 0..(def.count_x * def.count_z) as usize {
        let recovered = loaded.min_height + loaded.height_scale * loaded.heights[i] as f32;
        ensure_small!(recovered - heights[i], 2.0 * quantum);
    }
}

#[test]
fn shape_cast_vertical_straddle() {
    // Regression: a vertical shape cast whose swept volume straddles a cell
    // boundary must test every cell it overlaps. The field is flat at y = 0 with
    // only cell (0,0) solid; the surrounding cells are holes. Each sphere is
    // dropped straight down with its center nudged just past a boundary of the
    // solid cell, so that cell sits on the trailing (-x / -z) side of the sweep.
    // A cull AABB pinned to the leading box corner skips the solid cell entirely
    // and reports a miss.
    let heights = [0.0f32; 9];
    let materials = [0u8, HEIGHT_FIELD_HOLE, HEIGHT_FIELD_HOLE, HEIGHT_FIELD_HOLE];

    let def = HeightFieldDef {
        heights: &heights,
        material_indices: &materials,
        scale: vec3(1.0, 1.0, 1.0),
        count_x: 3,
        count_z: 3,
        global_minimum_height: -1.0,
        global_maximum_height: 1.0,
        clockwise_winding: false,
        ..Default::default()
    };

    let hf = create_height_field(&def);

    // Solid cell (0,0) spans x,z in [0,1]. Radius 0.3 with the center 0.05 past a
    // boundary still reaches back into the solid cell.
    let radius = 0.3f32;

    // Straddle the x = 1 edge: solid cell is on the -x side. Contact lands on the
    // cell edge, sqrt(0.05^2 + cy^2) = radius -> cy = 0.2958040,
    // fraction = (10 - cy) / 20 = 0.4852098.
    {
        let center = [vec3(1.05, 10.0, 0.5)];
        let input = ShapeCastInput {
            proxy: ShapeProxy { points: &center, radius },
            translation: vec3(0.0, -20.0, 0.0),
            max_fraction: 1.0,
            can_encroach: false,
        };

        let out = shape_cast_height_field(&hf, &input);
        ensure!(out.hit == true);
        ensure_small!(out.fraction - 0.4852098, 2e-3);
    }

    // Straddle the z = 1 edge: solid cell is on the -z side (same geometry).
    {
        let center = [vec3(0.5, 10.0, 1.05)];
        let input = ShapeCastInput {
            proxy: ShapeProxy { points: &center, radius },
            translation: vec3(0.0, -20.0, 0.0),
            max_fraction: 1.0,
            can_encroach: false,
        };

        let out = shape_cast_height_field(&hf, &input);
        ensure!(out.hit == true);
        ensure_small!(out.fraction - 0.4852098, 2e-3);
    }

    // Straddle the (1,1) corner: solid cell is diagonally trailing. Contact lands
    // on the corner vertex, sqrt(2*0.05^2 + cy^2) = radius -> cy = 0.2915476,
    // fraction = (10 - cy) / 20 = 0.4854226.
    {
        let center = [vec3(1.05, 10.0, 1.05)];
        let input = ShapeCastInput {
            proxy: ShapeProxy { points: &center, radius },
            translation: vec3(0.0, -20.0, 0.0),
            max_fraction: 1.0,
            can_encroach: false,
        };

        let out = shape_cast_height_field(&hf, &input);
        ensure!(out.hit == true);
        ensure_small!(out.fraction - 0.4854226, 2e-3);
    }
}

// Brute-force shape cast: cast the proxy against every (non-hole) triangle and
// keep the closest hit. This is the ground truth for shape_cast_height_field.
fn brute_force_shape_cast(hf: &HeightFieldData, input: &ShapeCastInput) -> CastOutput {
    let mut best = CastOutput::default();
    let mut best_fraction = input.max_fraction;

    let triangle_count = get_height_field_triangle_count(hf);
    for t in 0..triangle_count {
        let cell_index = t >> 1;
        if hf.materials[cell_index as usize] == HEIGHT_FIELD_HOLE {
            continue;
        }

        let tri = get_height_field_triangle(hf, t);

        let pair = ShapeCastPairInput {
            proxy_a: ShapeProxy { points: &tri.vertices, radius: 0.0 },
            proxy_b: input.proxy,
            transform: Transform::IDENTITY,
            translation_b: input.translation,
            max_fraction: best_fraction,
            can_encroach: input.can_encroach,
        };

        let out = shape_cast(&pair);
        if out.hit && out.fraction < best_fraction {
            best_fraction = out.fraction;
            best = out;
            best.triangle_index = t;
        }
    }

    best
}

#[test]
fn shape_cast_brute_force() {
    // shape_cast_height_field walks the grid and culls cells; the brute-force cast
    // against every triangle is the ground truth. The grid walk must never miss a
    // closer hit, regardless of cast direction, origin or radius.
    let scale = vec3(2.0, 1.5, 2.0);
    let hf = create_wave(10, 10, scale, 0.1, 0.03333, false);

    // Documented repro from sample/sample_mesh.cpp "Height Field": a sphere cast
    // that moves only in z (and y). Body at (-9,0,-9), world origin (5.5,4,2.913)
    // -> local (14.5,4,11.913). The grid walk used to terminate one row early
    // because it compared a clamped-sweep fraction against an input-space one.
    {
        let origin = [vec3(14.5, 4.0, 11.913)];
        let input = ShapeCastInput {
            proxy: ShapeProxy { points: &origin, radius: 0.2 },
            translation: vec3(0.0, -8.0, 6.397),
            max_fraction: 1.0,
            can_encroach: false,
        };

        let grid = shape_cast_height_field(&hf, &input);
        let brute = brute_force_shape_cast(&hf, &input);
        ensure!(brute.hit == true);
        ensure!(grid.hit == brute.hit);
        ensure_small!(grid.fraction - brute.fraction, 2e-3);
    }

    // Sweep origins across the field with assorted directions and radii.
    let radii = [0.15f32, 0.4, 0.9];
    let deltas = [
        vec3(0.0, -8.0, 0.0),  // vertical
        vec3(0.0, -8.0, 6.4),  // z only (+ y)
        vec3(5.1, -8.0, 0.0),  // x only (+ y)
        vec3(0.0, -8.0, -6.4), // -z
        vec3(-5.1, -8.0, 0.0), // -x
        vec3(6.0, -8.0, 5.0),  // diagonal
        vec3(-7.0, -8.0, 4.0), // diagonal, mixed sign
        vec3(9.0, -3.0, -9.0), // shallow diagonal
    ];

    let mut failures = 0;
    for xi in 0..5 {
        for zi in 0..5 {
            // 0.05 nudge keeps the swept box straddling cell boundaries.
            let origin = [vec3(1.0 + 4.0 * xi as f32 + 0.05, 4.0, 1.0 + 4.0 * zi as f32 + 0.05)];

            for di in 0..deltas.len() {
                for ri in 0..radii.len() {
                    let input = ShapeCastInput {
                        proxy: ShapeProxy { points: &origin, radius: radii[ri] },
                        translation: deltas[di],
                        max_fraction: 1.0,
                        can_encroach: false,
                    };

                    let grid = shape_cast_height_field(&hf, &input);
                    let brute = brute_force_shape_cast(&hf, &input);

                    let mut diff = grid.fraction - brute.fraction;
                    diff = if diff < 0.0 { -diff } else { diff };

                    if grid.hit != brute.hit || (brute.hit && diff > 2e-3) {
                        println!(
                            "  mismatch: origin=({:.2},{:.2},{:.2}) delta=({:.2},{:.2},{:.2}) r={:.2} grid(hit={},f={:.5}) brute(hit={},f={:.5} tri={})",
                            origin[0].x, origin[0].y, origin[0].z,
                            deltas[di].x, deltas[di].y, deltas[di].z, radii[ri],
                            grid.hit, grid.fraction, brute.hit, brute.fraction, brute.triangle_index
                        );
                        failures += 1;
                    }
                }
            }
        }
    }

    ensure!(failures == 0);
}

// Brute-force ray cast: cast the ray against every (non-hole) triangle and keep
// the closest hit. get_height_field_triangle returns vertices in the same winding
// order that shape_cast_height_field feeds to intersect_ray_triangle, so this is a
// pure traversal/culling check — the per-triangle math is identical.
fn brute_force_ray_cast(hf: &HeightFieldData, input: &RayCastInput) -> CastOutput {
    let mut best = CastOutput::default();
    let mut best_fraction = input.max_fraction;

    let ray_start = load_vec3(input.origin);
    let ray_delta = load_vec3(input.translation);

    let triangle_count = get_height_field_triangle_count(hf);
    for t in 0..triangle_count {
        let cell_index = t >> 1;
        if hf.materials[cell_index as usize] == HEIGHT_FIELD_HOLE {
            continue;
        }

        let tri = get_height_field_triangle(hf, t);
        let v1 = load_vec3(tri.vertices[0]);
        let v2 = load_vec3(tri.vertices[1]);
        let v3 = load_vec3(tri.vertices[2]);

        // intersect_ray_triangle returns 1.0 on a miss.
        let alpha = intersect_ray_triangle(ray_start, ray_delta, v1, v2, v3);
        if alpha < best_fraction {
            best_fraction = alpha;
            best.hit = true;
            best.fraction = alpha;
            best.triangle_index = t;
        }
    }

    best
}

#[test]
fn ray_cast_brute_force() {
    // ray_cast_height_field routes through the same grid walk as the shape cast
    // (radius-zero point proxy), so it is subject to the same early-termination
    // bug. The brute-force cast against every triangle is the ground truth.
    let scale = vec3(2.0, 1.5, 2.0);
    let hf = create_wave(10, 10, scale, 0.1, 0.03333, false);

    let deltas = [
        vec3(0.0, -8.0, 0.0),    // straight down
        vec3(0.0, -8.0, 12.0),   // down + z
        vec3(12.0, -8.0, 0.0),   // down + x
        vec3(0.0, -8.0, -12.0),  // down - z
        vec3(-12.0, -8.0, 0.0),  // down - x
        vec3(14.0, -8.0, 11.0),  // diagonal
        vec3(-13.0, -8.0, 9.0),  // diagonal, mixed sign
        vec3(16.0, -4.0, -15.0), // shallow diagonal
    ];

    let mut failures = 0;
    for xi in 0..5 {
        for zi in 0..5 {
            // 0.05 nudge keeps the ray off cell boundaries.
            let origin = vec3(1.0 + 4.0 * xi as f32 + 0.05, 4.0, 1.0 + 4.0 * zi as f32 + 0.05);

            for di in 0..deltas.len() {
                let input = RayCastInput { origin, translation: deltas[di], max_fraction: 1.0 };

                let grid = ray_cast_height_field(&hf, &input);
                let brute = brute_force_ray_cast(&hf, &input);

                let mut diff = grid.fraction - brute.fraction;
                diff = if diff < 0.0 { -diff } else { diff };

                if grid.hit != brute.hit || (brute.hit && diff > 1e-4) {
                    println!(
                        "  mismatch: origin=({:.2},{:.2},{:.2}) delta=({:.2},{:.2},{:.2}) grid(hit={},f={:.6} tri={}) brute(hit={},f={:.6} tri={})",
                        origin.x, origin.y, origin.z,
                        deltas[di].x, deltas[di].y, deltas[di].z,
                        grid.hit, grid.fraction, grid.triangle_index,
                        brute.hit, brute.fraction, brute.triangle_index
                    );
                    failures += 1;
                }
            }
        }
    }

    ensure!(failures == 0);
}
