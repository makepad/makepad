// Port of box3d/src/height_field.c
//
// Deviations (see PORTING.md):
// - HeightFieldData is a plain struct with Vec fields instead of a single blob
//   with byte offsets; creation returns Arc<HeightFieldData>.
// - The content hash is computed over a canonical little-endian serialization
//   of the fields (order documented at compute_height_field_hash) instead of
//   the raw allocation bytes. Only self-consistency matters.
// - b3V32 element-wise ops used only to build local bounds are expressed as
//   plain Vec3 math (exactly equivalent: the C scalar paths only use the low 3
//   lanes). The two real SIMD algorithms (test_bounds_triangle_overlap,
//   intersect_ray_triangle) are called in crate::simd via load_vec3, mirroring
//   the C b3LoadV call sites.
// - create_wave uses the deterministic math_functions::sin instead of libm sinf.
// - destroy_height_field is not ported (Arc drop).

use std::sync::Arc;

use crate::aabb::ray_cast_aabb;
use crate::b3_assert;
use crate::constants::{linear_slop, max_aabb_margin, MAX_SHAPE_CAST_POINTS};
use crate::core::{hash, non_zero_hash, HASH_INIT};
use crate::distance::{shape_cast, shape_distance};
use crate::math_functions::{
    abs, add, aabb_center, aabb_extents, aabb_overlaps, aabb_transform, clamp_float, cross, dot,
    make_aabb, max, max_float, min, min_float, mul, mul_add, mul_sv, neg, normalize, sin, sub,
    vec3, Plane, Transform, Vec3, AABB, PI,
};
use crate::math_internal::{make_plane_from_points, plane_separation, Triangle};
use crate::types::{
    Capsule, CastOutput, DistanceInput, HeightFieldData, HeightFieldDef, PlaneResult, RayCastInput,
    ShapeCastInput, ShapeCastPairInput, ShapeProxy, SimplexCache, ALL_CONCAVE_EDGES,
    CONCAVE_EDGE1, CONCAVE_EDGE2, CONCAVE_EDGE3, HEIGHT_FIELD_HOLE, INVERSE_CONCAVE_EDGE1,
    INVERSE_CONCAVE_EDGE2, INVERSE_CONCAVE_EDGE3,
};

// C: _Static_assert on the flag bit math.
const _: () = assert!(CONCAVE_EDGE3 == 4 * CONCAVE_EDGE1, "bit math");
const _: () = assert!(INVERSE_CONCAVE_EDGE3 == 4 * INVERSE_CONCAVE_EDGE1, "bit math");
const _: () = assert!(ALL_CONCAVE_EDGES == (CONCAVE_EDGE1 | CONCAVE_EDGE2 | CONCAVE_EDGE3));

/*
    Convention

    index = row * columnCount + column
    height = minHeight + heightScale * compressedHeights[index];

    column = index % columnCount;
    row = index / columnCount;

    x-axis : columns
    z-axis : rows

    00 --- 01 --- 02 --- 03 X
    |  0   |  1   |  2   |
    04 --- 05 --- 06 --- 07
    |  3   |  4   |  5   |
    08 --- 09 --- 10 --- 11
    |  6   |  7   |  8   |
    12 --- 13 --- 14 --- 15
    Z

    The quads exist before the column and row ends: row < rowCount - 1 and column < columnCount - 1
    quadIndex = row * (columnCount - 1) + column

    Quad origin index from quad index (needs row):
    index = quadIndex + row * columnCount

    Triangle index is related to the quad index
    triangleIndex = 2 * quadIndex + (0/1)
    quadIndex = triangleIndex / 2

    Row and column from quad index:
    row = quadIndex / (columnCount - 1)
    column = quadIndex - row * (columnCount - 1)

    The triangle diagonal is fixed.

    triangle0 = {00, 04, 01} -> {11, 21, 12}
    triangle1 = {04, 05, 01} -> {22, 12, 21}

    11      12
    00 ---- 01
    |     / |
    | 0 / 1 | 1
    | /     |
    04 ---- 05
    21     22
*/

/// Content hash over the canonical serialization, hash field excluded.
/// Order: scale.x/y/z, min_height, max_height, height_scale (f32 LE),
/// column_count, row_count (i32 LE), clockwise (u8), heights (u16 LE each),
/// materials (u8 each), flags (u8 each).
fn compute_height_field_hash(hf: &HeightFieldData) -> u32 {
    let mut bytes: Vec<u8> =
        Vec::with_capacity(29 + 2 * hf.heights.len() + hf.materials.len() + hf.flags.len());
    bytes.extend_from_slice(&hf.scale.x.to_le_bytes());
    bytes.extend_from_slice(&hf.scale.y.to_le_bytes());
    bytes.extend_from_slice(&hf.scale.z.to_le_bytes());
    bytes.extend_from_slice(&hf.min_height.to_le_bytes());
    bytes.extend_from_slice(&hf.max_height.to_le_bytes());
    bytes.extend_from_slice(&hf.height_scale.to_le_bytes());
    bytes.extend_from_slice(&hf.column_count.to_le_bytes());
    bytes.extend_from_slice(&hf.row_count.to_le_bytes());
    bytes.push(hf.clockwise as u8);
    for h in &hf.heights {
        bytes.extend_from_slice(&h.to_le_bytes());
    }
    bytes.extend_from_slice(&hf.materials);
    bytes.extend_from_slice(&hf.flags);

    non_zero_hash(hash(HASH_INIT, &bytes))
}

pub fn create_height_field(data: &HeightFieldDef) -> Arc<HeightFieldData> {
    let column_count = data.count_x;
    let row_count = data.count_z;

    let height_count = column_count * row_count;
    b3_assert!(height_count >= 4);

    let cell_count = (column_count - 1) * (row_count - 1);
    let triangle_count = 2 * cell_count;

    let mut hf = HeightFieldData {
        scale: data.scale,
        column_count,
        row_count,
        clockwise: data.clockwise_winding,
        ..Default::default()
    };

    b3_assert!(data.global_minimum_height <= data.global_maximum_height);
    hf.min_height = data.global_minimum_height;
    hf.max_height = data.global_maximum_height;

    let height = max_float(hf.max_height - hf.min_height, linear_slop());
    hf.height_scale = height / u16::MAX as f32;

    let mut lower_height_bound = hf.max_height;
    let mut upper_height_bound = hf.min_height;

    b3_assert!(data.heights.len() as i32 == height_count);

    let mut compressed_heights = vec![0u16; height_count as usize];
    let inv_height_scale = 1.0 / hf.height_scale;
    for i in 0..height_count as usize {
        let clamped_height = clamp_float(data.heights[i], hf.min_height, hf.max_height);
        let scaled_height = (clamped_height - hf.min_height) * inv_height_scale;
        compressed_heights[i] = min_float(scaled_height, u16::MAX as f32) as u16;

        lower_height_bound = min_float(lower_height_bound, clamped_height);
        upper_height_bound = max_float(upper_height_bound, clamped_height);
    }

    // Use decompressed heights for accurate convexity metrics.
    let mut decompressed_heights = vec![0.0f32; height_count as usize];
    for i in 0..height_count as usize {
        decompressed_heights[i] = hf.min_height + hf.height_scale * compressed_heights[i] as f32;
    }
    let heights: &[f32] = &decompressed_heights;

    let mut material_indices = vec![0u8; cell_count as usize];
    if !data.material_indices.is_empty() {
        b3_assert!(data.material_indices.len() as i32 == cell_count);
        material_indices.copy_from_slice(data.material_indices);
    }

    hf.aabb.lower_bound = vec3(0.0, hf.scale.y * lower_height_bound, 0.0);
    hf.aabb.upper_bound = vec3(
        hf.scale.x * (hf.column_count - 1) as f32,
        hf.scale.y * upper_height_bound,
        hf.scale.z * (hf.row_count - 1) as f32,
    );

    let cos5_deg = 0.9962f32;
    let scale = hf.scale;

    let mut flags = vec![0u8; triangle_count as usize];

    let mut triangle_index = 0;
    for row in 0..row_count - 1 {
        for column in 0..column_count - 1 {
            // todo compute convexity flags
            // This requires a couple things
            // - determine all 3 adjacent triangles for each triangle
            // - consider clockwise winding
            // - consider borders where there is no adjacent triangle

            let triangle_index1 = triangle_index;
            let triangle_index2 = triangle_index + 1;
            triangle_index += 2;

            let cell_index = row * (column_count - 1) + column;

            if material_indices[cell_index as usize] == HEIGHT_FIELD_HOLE {
                continue;
            }

            let mut flags1: i32 = 0;
            let mut flags2: i32 = 0;

            let plane1: Plane;
            let plane2: Plane;

            let index11 = row * column_count + column;
            let index12 = index11 + 1;
            let index21 = (row + 1) * column_count + column;
            let index22 = index21 + 1;

            {
                let height11 = heights[index11 as usize];
                let height12 = heights[index12 as usize];
                let height21 = heights[index21 as usize];
                let height22 = heights[index22 as usize];

                let x1 = column as f32;
                let x2 = (column + 1) as f32;
                let z1 = row as f32;
                let z2 = (row + 1) as f32;

                // triangle 0 : 11, 21, 12
                let vs0 = [
                    mul(scale, vec3(x1, height11, z1)),
                    mul(scale, vec3(x1, height21, z2)),
                    mul(scale, vec3(x2, height12, z1)),
                ];
                plane1 = make_plane_from_points(vs0[0], vs0[1], vs0[2]);

                // triangle 1 : 22, 12, 21
                let vs1 = [
                    mul(scale, vec3(x2, height22, z2)),
                    mul(scale, vec3(x2, height12, z1)),
                    mul(scale, vec3(x1, height21, z2)),
                ];
                plane2 = make_plane_from_points(vs1[0], vs1[1], vs1[2]);

                let separation = plane_separation(plane1, vs1[0]);
                let cos_angle = dot(plane1.normal, plane2.normal);
                if separation > 0.0 || cos_angle > cos5_deg {
                    flags1 |= CONCAVE_EDGE2;
                    flags2 |= CONCAVE_EDGE2;
                }
                if separation < 0.0 || cos_angle > cos5_deg {
                    flags1 |= INVERSE_CONCAVE_EDGE2;
                    flags2 |= INVERSE_CONCAVE_EDGE2;
                }
            }

            // top
            let top_cell_index = (row - 1) * (column_count - 1) + column;
            if row > 0 && material_indices[top_cell_index as usize] != HEIGHT_FIELD_HOLE {
                b3_assert!(0 <= top_cell_index && top_cell_index < cell_count);

                let r = row - 1;
                let c = column;

                let i12 = r * column_count + c + 1;
                let i21 = (r + 1) * column_count + c;
                let i22 = i21 + 1;

                b3_assert!(i21 == index11);
                b3_assert!(i22 == index12);

                let h12 = heights[i12 as usize];
                let h21 = heights[i21 as usize];
                let h22 = heights[i22 as usize];

                let x1 = c as f32;
                let x2 = (c + 1) as f32;
                let z1 = r as f32;
                let z2 = (r + 1) as f32;

                // triangle 1
                let vs = [
                    mul(scale, vec3(x2, h22, z2)),
                    mul(scale, vec3(x2, h12, z1)),
                    mul(scale, vec3(x1, h21, z2)),
                ];

                let n = normalize(cross(sub(vs[1], vs[0]), sub(vs[2], vs[0])));

                let separation = plane_separation(plane1, vs[1]);
                let cos_angle = dot(plane1.normal, n);
                if separation > 0.0 || cos_angle > cos5_deg {
                    flags1 |= CONCAVE_EDGE3;
                }
                if separation < 0.0 || cos_angle > cos5_deg {
                    flags1 |= INVERSE_CONCAVE_EDGE3;
                }
            }

            let bottom_cell_index = (row + 1) * (column_count - 1) + column;
            if row + 1 < row_count - 1 && material_indices[bottom_cell_index as usize] != HEIGHT_FIELD_HOLE {
                b3_assert!(0 <= bottom_cell_index && bottom_cell_index < cell_count);

                let r = row + 1;
                let c = column;

                let i11 = r * column_count + c;
                let i12 = i11 + 1;
                let i21 = (r + 1) * column_count + c;

                b3_assert!(i11 == index21);
                b3_assert!(i12 == index22);

                let h11 = heights[i11 as usize];
                let h12 = heights[i12 as usize];
                let h21 = heights[i21 as usize];

                let x1 = c as f32;
                let x2 = (c + 1) as f32;
                let z1 = r as f32;
                let z2 = (r + 1) as f32;

                // triangle 0
                let vs = [
                    mul(scale, vec3(x1, h11, z1)),
                    mul(scale, vec3(x1, h21, z2)),
                    mul(scale, vec3(x2, h12, z1)),
                ];

                let n = normalize(cross(sub(vs[1], vs[0]), sub(vs[2], vs[0])));

                let separation = plane_separation(plane2, vs[1]);
                let cos_angle = dot(plane2.normal, n);
                if separation > 0.0 || cos_angle > cos5_deg {
                    flags2 |= CONCAVE_EDGE3;
                }
                if separation < 0.0 || cos_angle > cos5_deg {
                    flags2 |= INVERSE_CONCAVE_EDGE3;
                }
            }

            let left_cell_index = row * (column_count - 1) + column - 1;
            if column - 1 >= 0 && material_indices[left_cell_index as usize] != HEIGHT_FIELD_HOLE {
                b3_assert!(0 <= left_cell_index && left_cell_index < cell_count);

                let r = row;
                let c = column - 1;

                let i12 = r * column_count + c + 1;
                let i21 = (r + 1) * column_count + c;
                let i22 = i21 + 1;

                b3_assert!(i12 == index11);
                b3_assert!(i22 == index21);

                let h12 = heights[i12 as usize];
                let h21 = heights[i21 as usize];
                let h22 = heights[i22 as usize];

                let x1 = c as f32;
                let x2 = (c + 1) as f32;
                let z1 = r as f32;
                let z2 = (r + 1) as f32;

                // triangle 1
                let vs = [
                    mul(scale, vec3(x2, h22, z2)),
                    mul(scale, vec3(x2, h12, z1)),
                    mul(scale, vec3(x1, h21, z2)),
                ];

                let n = normalize(cross(sub(vs[1], vs[0]), sub(vs[2], vs[0])));

                let separation = plane_separation(plane1, vs[2]);
                let cos_angle = dot(plane1.normal, n);
                if separation > 0.0 || cos_angle > cos5_deg {
                    flags1 |= CONCAVE_EDGE1;
                }
                if separation < 0.0 || cos_angle > cos5_deg {
                    flags1 |= INVERSE_CONCAVE_EDGE1;
                }
            }

            let right_cell_index = row * (column_count - 1) + column + 1;
            if column + 1 < column_count - 1 && material_indices[right_cell_index as usize] != HEIGHT_FIELD_HOLE {
                b3_assert!(0 <= right_cell_index && right_cell_index < cell_count);

                let r = row;
                let c = column + 1;

                let i11 = r * column_count + c;
                let i12 = i11 + 1;
                let i21 = (r + 1) * column_count + c;

                b3_assert!(i11 == index12);
                b3_assert!(i21 == index22);

                let h11 = heights[i11 as usize];
                let h12 = heights[i12 as usize];
                let h21 = heights[i21 as usize];

                let x1 = c as f32;
                let x2 = (c + 1) as f32;
                let z1 = r as f32;
                let z2 = (r + 1) as f32;

                // triangle 0
                let vs = [
                    mul(scale, vec3(x1, h11, z1)),
                    mul(scale, vec3(x1, h21, z2)),
                    mul(scale, vec3(x2, h12, z1)),
                ];

                let n = normalize(cross(sub(vs[1], vs[0]), sub(vs[2], vs[0])));

                let separation = plane_separation(plane2, vs[2]);
                let cos_angle = dot(plane2.normal, n);
                if separation > 0.0 || cos_angle > cos5_deg {
                    flags2 |= CONCAVE_EDGE1;
                }
                if separation < 0.0 || cos_angle > cos5_deg {
                    flags2 |= INVERSE_CONCAVE_EDGE1;
                }
            }

            b3_assert!(0 <= flags1 && flags1 <= u8::MAX as i32);
            b3_assert!(0 <= flags2 && flags2 <= u8::MAX as i32);

            flags[triangle_index1 as usize] = flags1 as u8;
            flags[triangle_index2 as usize] = flags2 as u8;
        }
    }

    b3_assert!(triangle_index == triangle_count);

    hf.heights = compressed_heights;
    hf.materials = material_indices;
    hf.flags = flags;

    // Content hash with the hash field zeroed, like HullData/MeshData.
    hf.hash = 0;
    hf.hash = compute_height_field_hash(&hf);

    Arc::new(hf)
}

// Decode the four corner vertices of a height field cell into local space.
// Output order matches the index naming used throughout this file:
// corners[0] = (column, row), corners[1] = (column + 1, row),
// corners[2] = (column, row + 1), corners[3] = (column + 1, row + 1).
#[inline]
fn get_height_field_cell_corners(hf: &HeightFieldData, row: i32, column: i32) -> [Vec3; 4] {
    b3_assert!(0 <= row && row < hf.row_count - 1 && 0 <= column && column < hf.column_count - 1);

    let column_count = hf.column_count;
    let index11 = row * column_count + column;
    let index12 = index11 + 1;
    let index21 = (row + 1) * column_count + column;
    let index22 = index21 + 1;

    let min_height = hf.min_height;
    let height_scale = hf.height_scale;
    let heights = &hf.heights;

    let height11 = min_height + height_scale * heights[index11 as usize] as f32;
    let height12 = min_height + height_scale * heights[index12 as usize] as f32;
    let height21 = min_height + height_scale * heights[index21 as usize] as f32;
    let height22 = min_height + height_scale * heights[index22 as usize] as f32;

    let x1 = column as f32;
    let x2 = (column + 1) as f32;
    let z1 = row as f32;
    let z2 = (row + 1) as f32;

    let scale = hf.scale;
    [
        mul(scale, vec3(x1, height11, z1)),
        mul(scale, vec3(x2, height12, z1)),
        mul(scale, vec3(x1, height21, z2)),
        mul(scale, vec3(x2, height22, z2)),
    ]
}

/// From shape.h.
#[inline]
pub fn get_height_field_triangle_count(height_field: &HeightFieldData) -> i32 {
    let cell_count = (height_field.row_count - 1) * (height_field.column_count - 1);
    2 * cell_count
}

pub fn get_height_field_triangle(height_field: &HeightFieldData, triangle_index: i32) -> Triangle {
    b3_assert!(0 <= triangle_index);
    b3_assert!(triangle_index < 2 * (height_field.column_count - 1) * (height_field.row_count - 1));

    let column_count = height_field.column_count;
    let quad_index = triangle_index >> 1;
    let row = quad_index / (column_count - 1);
    let column = quad_index - row * (column_count - 1);

    let index11 = row * column_count + column;
    let index12 = index11 + 1;
    let index21 = (row + 1) * column_count + column;
    let index22 = index21 + 1;

    let cell_index = row * (column_count - 1) + column;

    b3_assert!(quad_index == cell_index);
    b3_assert!(height_field.materials[cell_index as usize] != HEIGHT_FIELD_HOLE);
    let _ = cell_index;

    let corners = get_height_field_cell_corners(height_field, row, column);

    let mut triangle = if (triangle_index & 1) == 0 {
        Triangle {
            vertices: [corners[0], corners[2], corners[1]],
            i1: index11,
            i2: index21,
            i3: index12,
            flags: height_field.flags[triangle_index as usize] as i32,
        }
    } else {
        Triangle {
            vertices: [corners[3], corners[1], corners[2]],
            i1: index22,
            i2: index12,
            i3: index21,
            flags: height_field.flags[triangle_index as usize] as i32,
        }
    };

    if height_field.clockwise {
        triangle.vertices.swap(1, 2);
        std::mem::swap(&mut triangle.i2, &mut triangle.i3);

        // Reversing winding swaps edge1 and edge3; edge2 (the diagonal) is preserved.
        let mut flags = triangle.flags;
        let edge1_bits = flags & (CONCAVE_EDGE1 | INVERSE_CONCAVE_EDGE1);
        let edge3_bits = flags & (CONCAVE_EDGE3 | INVERSE_CONCAVE_EDGE3);
        flags &= !(CONCAVE_EDGE1 | CONCAVE_EDGE3 | INVERSE_CONCAVE_EDGE1 | INVERSE_CONCAVE_EDGE3);
        flags |= edge1_bits << 2;
        flags |= edge3_bits >> 2;
        triangle.flags = flags;
    }

    triangle
}

pub fn get_height_field_material(height_field: &HeightFieldData, triangle_index: i32) -> i32 {
    b3_assert!(0 <= triangle_index);
    b3_assert!(triangle_index < 2 * (height_field.column_count - 1) * (height_field.row_count - 1));

    let cell_index = triangle_index >> 1;
    height_field.materials[cell_index as usize] as i32
}

pub fn compute_height_field_aabb(shape: &HeightFieldData, transform: Transform) -> AABB {
    aabb_transform(transform, shape.aabb)
}

pub fn ray_cast_height_field(height_field: &HeightFieldData, input: &RayCastInput) -> CastOutput {
    let shape_cast_input = ShapeCastInput {
        proxy: ShapeProxy { points: std::slice::from_ref(&input.origin), radius: 0.0 },
        translation: input.translation,
        max_fraction: input.max_fraction,
        can_encroach: false,
    };

    shape_cast_height_field(height_field, &shape_cast_input)
}

// todo advance cast to the grid border immediately if it starts outside the row/column range
// todo terminate the cast immediately if it leaves the row/column range
pub fn shape_cast_height_field(height_field: &HeightFieldData, input: &ShapeCastInput) -> CastOutput {
    let shape_bounds = make_aabb(input.proxy.points, input.proxy.radius);
    let shape_translation = input.translation;
    let scale = height_field.scale;

    let shape_start = aabb_center(shape_bounds);
    let shape_delta = mul_sv(input.max_fraction, shape_translation);
    let shape_end = add(shape_start, shape_delta);

    let mut result = CastOutput::default();

    let shape_extents = aabb_extents(shape_bounds);
    let m = max_aabb_margin();
    let margin = vec3(m, m, m);
    let combined_bounds = AABB {
        lower_bound: sub(sub(height_field.aabb.lower_bound, shape_extents), margin),
        upper_bound: add(add(height_field.aabb.upper_bound, shape_extents), margin),
    };

    let mut min_fraction = 0.0f32;
    let mut max_fraction = 0.0f32;
    let intersects =
        ray_cast_aabb(combined_bounds, shape_start, shape_end, &mut min_fraction, &mut max_fraction);
    if !intersects {
        return result;
    }

    // These are for walking the grid, not the triangle cast.
    // The triangle cast uses the unclamped ray and fraction.
    let mut clamped_start = mul_add(shape_start, min_fraction, shape_delta);
    let clamped_delta = mul_sv(max_fraction - min_fraction, shape_delta);
    let mut clamped_end = add(clamped_start, clamped_delta);

    // Preserve the un-shifted center sweep. clampedStart/clampedEnd get pushed out to the
    // leading box corner below to drive the grid DDA, but the swept-volume AABB used to
    // cull cells must stay centered on the actual shape path.
    let center_start = clamped_start;
    let center_end = clamped_end;

    // The grid traversal starts from the leading shape bounds corner
    let sign_x;
    let sign_z;
    if shape_translation.x >= 0.0 {
        clamped_start.x += shape_extents.x;
        sign_x = 1.0f32;
    } else {
        clamped_start.x -= shape_extents.x;
        sign_x = -1.0f32;
    }

    if shape_translation.z >= 0.0 {
        clamped_start.z += shape_extents.z;
        sign_z = 1.0f32;
    } else {
        clamped_start.z -= shape_extents.z;
        sign_z = -1.0f32;
    }

    // Shift the end as well
    clamped_end = add(clamped_start, clamped_delta);

    // Row and column range for the shape cast
    let column_start = (clamped_start.x / scale.x).floor() as i32;
    let column_end = (clamped_end.x / scale.x).floor() as i32;
    let row_start = (clamped_start.z / scale.z).floor() as i32;
    let row_end = (clamped_end.z / scale.z).floor() as i32;

    let abs_clamped_delta = abs(clamped_delta);

    // Precompute increments for row and column traversal.
    // The ray can be slightly tilted yet remain within a single row or column
    // once rasterized.
    let delta_alpha_x;
    let mut next_fraction_x;
    let delta_column;

    if column_start < column_end {
        b3_assert!(abs_clamped_delta.x > 0.0);

        // Going forward on x columns
        delta_alpha_x = scale.x / abs_clamped_delta.x;
        next_fraction_x = (scale.x * (column_start + 1) as f32 - clamped_start.x) / abs_clamped_delta.x;
        delta_column = 1;
    } else if column_end < column_start {
        b3_assert!(abs_clamped_delta.x > 0.0);

        // Going backwards on x columns
        delta_alpha_x = scale.x / abs_clamped_delta.x;
        next_fraction_x = (clamped_start.x - scale.x * column_start as f32) / abs_clamped_delta.x;
        delta_column = -1;
    } else {
        // Cast stays in a single column
        delta_alpha_x = 0.0;
        next_fraction_x = f32::MAX;
        delta_column = 0;
    }

    let delta_alpha_z;
    let mut next_fraction_z;
    let delta_row;

    if row_start < row_end {
        b3_assert!(abs_clamped_delta.z > 0.0);

        // Going forward on z rows
        delta_alpha_z = scale.z / abs_clamped_delta.z;
        next_fraction_z = (scale.z * (row_start + 1) as f32 - clamped_start.z) / abs_clamped_delta.z;
        delta_row = 1;
    } else if row_end < row_start {
        b3_assert!(abs_clamped_delta.z > 0.0);

        // Going backwards on z rows
        delta_alpha_z = scale.z / abs_clamped_delta.z;
        next_fraction_z = (clamped_start.z - scale.z * row_start as f32) / abs_clamped_delta.z;
        delta_row = -1;
    } else {
        // Cast stays in a single row
        delta_alpha_z = 0.0;
        next_fraction_z = f32::MAX;
        delta_row = 0;
    }

    // Column and row range for 2D projected initial shape bounds
    let mut box_column_head = column_start;
    let mut box_row_head = row_start;

    let mut box_column_tail = ((clamped_start.x - 2.0 * sign_x * shape_extents.x) / scale.x).floor() as i32;
    let mut box_row_tail = ((clamped_start.z - 2.0 * sign_z * shape_extents.z) / scale.z).floor() as i32;

    let mut best_fraction = input.max_fraction;

    // nextFractionX / nextFractionZ advance in units of the clamped sweep
    // [minFraction, maxFraction], but bestFraction is a fraction of the full input
    // translation. Precompute the affine map from clamped space to input space so
    // the loop termination test compares like with like — otherwise it can exit
    // early and miss a closer hit in a later cell.
    let grid_fraction_scale = input.max_fraction * (max_fraction - min_fraction);
    let grid_fraction_offset = input.max_fraction * min_fraction;

    let row_count = height_field.row_count;
    let column_count = height_field.column_count;
    let cell_count = (height_field.row_count - 1) * (height_field.column_count - 1);
    let _ = cell_count;

    let cast_bounds = AABB {
        lower_bound: sub(min(center_start, center_end), shape_extents),
        upper_bound: add(max(center_start, center_end), shape_extents),
    };

    let ray_origin = crate::simd::load_vec3(shape_start);
    let ray_translation = crate::simd::load_vec3(shape_translation);

    loop {
        let (column1, column2) = if box_column_tail < box_column_head {
            (box_column_tail, box_column_head)
        } else {
            (box_column_head, box_column_tail)
        };

        let (row1, row2) = if box_row_tail < box_row_head {
            (box_row_tail, box_row_head)
        } else {
            (box_row_head, box_row_tail)
        };

        for row in row1..=row2 {
            if row < 0 || row_count - 1 <= row {
                continue;
            }

            for column in column1..=column2 {
                if column < 0 || column_count - 1 <= column {
                    continue;
                }

                let cell_index = row * (column_count - 1) + column;
                b3_assert!(cell_index < cell_count);

                let material_index = height_field.materials[cell_index as usize];
                if material_index == HEIGHT_FIELD_HOLE {
                    continue;
                }

                let corners = get_height_field_cell_corners(height_field, row, column);
                let point11 = corners[0];
                let point12 = corners[1];
                let point21 = corners[2];
                let point22 = corners[3];

                // I know the min/max x and z values, but not the min/max heights.
                let bounds = AABB {
                    lower_bound: min(min(point11, point12), min(point21, point22)),
                    upper_bound: max(max(point11, point12), max(point21, point22)),
                };

                if !aabb_overlaps(cast_bounds, bounds) {
                    continue;
                }

                let quad_index = row * (column_count - 1) + column;
                let triangle_index1 = 2 * quad_index;
                let triangle_index2 = triangle_index1 + 1;

                if input.proxy.count() == 1 && input.proxy.radius == 0.0 {
                    // Ray cast
                    {
                        let vertex1 = crate::simd::load_vec3(point11);
                        let (vertex2, vertex3) = if height_field.clockwise {
                            (crate::simd::load_vec3(point12), crate::simd::load_vec3(point21))
                        } else {
                            (crate::simd::load_vec3(point21), crate::simd::load_vec3(point12))
                        };

                        let alpha = crate::simd::intersect_ray_triangle(
                            ray_origin,
                            ray_translation,
                            vertex1,
                            vertex2,
                            vertex3,
                        );
                        b3_assert!(0.0 <= alpha && alpha <= 1.0);

                        if alpha < best_fraction {
                            let edge1 = sub(point21, point11);
                            let edge2 = sub(point12, point11);
                            let normal = if height_field.clockwise {
                                cross(edge2, edge1)
                            } else {
                                cross(edge1, edge2)
                            };

                            result.point = mul_add(shape_start, alpha, shape_translation);
                            result.normal = normalize(normal);
                            result.fraction = alpha;
                            result.triangle_index = triangle_index1;
                            result.material_index = material_index as i32;
                            result.hit = true;
                            best_fraction = alpha;
                        }
                    }

                    {
                        let vertex1 = crate::simd::load_vec3(point22);
                        let (vertex2, vertex3) = if height_field.clockwise {
                            (crate::simd::load_vec3(point21), crate::simd::load_vec3(point12))
                        } else {
                            (crate::simd::load_vec3(point12), crate::simd::load_vec3(point21))
                        };

                        let alpha = crate::simd::intersect_ray_triangle(
                            ray_origin,
                            ray_translation,
                            vertex1,
                            vertex2,
                            vertex3,
                        );
                        b3_assert!(0.0 <= alpha && alpha <= 1.0);

                        if alpha < best_fraction {
                            let edge1 = sub(point22, point21);
                            let edge2 = sub(point12, point21);
                            let normal = if height_field.clockwise {
                                cross(edge2, edge1)
                            } else {
                                cross(edge1, edge2)
                            };

                            result.point = mul_add(shape_start, alpha, shape_translation);
                            result.normal = normalize(normal);
                            result.fraction = alpha;
                            result.triangle_index = triangle_index2;
                            result.material_index = material_index as i32;
                            result.hit = true;
                            best_fraction = alpha;
                        }
                    }
                } else {
                    // Shape cast
                    // todo back-side culling
                    {
                        // Shift origin to first vertex
                        let origin = point11;
                        let triangle_vertices = [Vec3::ZERO, sub(point21, origin), sub(point12, origin)];
                        let pair_input = ShapeCastPairInput {
                            proxy_a: ShapeProxy { points: &triangle_vertices, radius: 0.0 },
                            proxy_b: input.proxy,
                            transform: Transform { p: neg(origin), ..Transform::IDENTITY },
                            translation_b: input.translation,
                            max_fraction: best_fraction,
                            can_encroach: input.can_encroach,
                        };

                        let pair_output = shape_cast(&pair_input);

                        if pair_output.hit {
                            best_fraction = pair_output.fraction;
                            result = pair_output;
                            result.point = add(result.point, origin);
                            result.triangle_index = triangle_index1;
                            result.material_index = material_index as i32;
                        }
                    }

                    {
                        // Shift origin to first vertex
                        let origin = point21;
                        let triangle_vertices = [Vec3::ZERO, sub(point22, origin), sub(point12, origin)];
                        let pair_input = ShapeCastPairInput {
                            proxy_a: ShapeProxy { points: &triangle_vertices, radius: 0.0 },
                            proxy_b: input.proxy,
                            transform: Transform { p: neg(origin), ..Transform::IDENTITY },
                            translation_b: input.translation,
                            max_fraction: best_fraction,
                            can_encroach: input.can_encroach,
                        };

                        let pair_output = shape_cast(&pair_input);

                        if pair_output.hit {
                            best_fraction = pair_output.fraction;
                            result = pair_output;
                            result.point = add(result.point, origin);
                            result.triangle_index = triangle_index2;
                            result.material_index = material_index as i32;
                        }
                    }
                }
            }
        }

        // These fractions always increase to guarantee the loop eventually exits.
        // Map them from clamped-sweep space into input-translation space before
        // comparing against bestFraction.
        let input_fraction_x = if next_fraction_x == f32::MAX {
            f32::MAX
        } else {
            grid_fraction_offset + next_fraction_x * grid_fraction_scale
        };
        let input_fraction_z = if next_fraction_z == f32::MAX {
            f32::MAX
        } else {
            grid_fraction_offset + next_fraction_z * grid_fraction_scale
        };
        if input_fraction_x > best_fraction && input_fraction_z > best_fraction {
            break;
        }

        // Advance the cast to the next column or row
        if next_fraction_x <= next_fraction_z {
            if box_column_head == column_end {
                // Hit the end already
                break;
            }

            // Advance to next column
            box_column_head += delta_column;

            // Build a single column to cast
            box_column_tail = box_column_head;

            if shape_extents.z == 0.0 {
                // Single row
                box_row_tail = box_row_head;
            } else {
                // Rasterize shape row
                let row_intercept = clamped_start.z + next_fraction_x * clamped_delta.z;
                box_row_tail = ((row_intercept - 2.0 * sign_z * shape_extents.z) / scale.z).floor() as i32;
            }

            next_fraction_x += delta_alpha_x;
        } else {
            if box_row_head == row_end {
                // Hit the end already
                break;
            }

            // Advance to next row
            box_row_head += delta_row;

            // Build a single row to cast
            box_row_tail = box_row_head;

            if shape_extents.x == 0.0 {
                // Single column
                box_column_tail = box_column_head;
            } else {
                // Rasterize shape column
                let column_intercept = clamped_start.x + next_fraction_z * clamped_delta.x;
                box_column_tail = ((column_intercept - 2.0 * sign_x * shape_extents.x) / scale.x).floor() as i32;
            }

            next_fraction_z += delta_alpha_z;
        }
    }

    result
}

pub fn overlap_height_field(shape: &HeightFieldData, shape_transform: Transform, proxy: &ShapeProxy) -> bool {
    let mut buffer = [Vec3::ZERO; MAX_SHAPE_CAST_POINTS];
    let local_proxy = crate::shape::make_local_proxy(proxy, shape_transform, &mut buffer);
    let aabb = crate::shape::compute_proxy_aabb(&local_proxy);

    let scale = shape.scale;
    let min_row = (aabb.lower_bound.z / scale.z).floor() as i32;
    let max_row = (aabb.upper_bound.z / scale.z).floor() as i32;
    let min_col = (aabb.lower_bound.x / scale.x).floor() as i32;
    let max_col = (aabb.upper_bound.x / scale.x).floor() as i32;

    let bounds_min = aabb.lower_bound;
    let bounds_max = aabb.upper_bound;
    let bounds_center = mul_sv(0.5, add(bounds_min, bounds_max));
    let bounds_extent = sub(bounds_max, bounds_center);
    let bounds_center_v = crate::simd::load_vec3(bounds_center);
    let bounds_extent_v = crate::simd::load_vec3(bounds_extent);

    let mut cache = SimplexCache::default();

    // Outer loop on rows and inner loop on columns so that triangle indices
    // increase monotonically.
    for row in min_row..=max_row {
        if row < 0 || shape.row_count - 1 <= row {
            continue;
        }

        for column in min_col..=max_col {
            if column < 0 || shape.column_count - 1 <= column {
                continue;
            }

            let cell_index = row * (shape.column_count - 1) + column;
            b3_assert!(cell_index < (shape.row_count - 1) * (shape.column_count - 1));
            let material = shape.materials[cell_index as usize];
            if material == HEIGHT_FIELD_HOLE {
                continue;
            }

            let corners = get_height_field_cell_corners(shape, row, column);
            let point11 = corners[0];
            let point12 = corners[1];
            let point21 = corners[2];
            let point22 = corners[3];

            let v11 = crate::simd::load_vec3(point11);
            let v12 = crate::simd::load_vec3(point12);
            let v21 = crate::simd::load_vec3(point21);
            let v22 = crate::simd::load_vec3(point22);

            if crate::simd::test_bounds_triangle_overlap(bounds_center_v, bounds_extent_v, v11, v21, v12) {
                let triangle_vertices = [point11, point21, point12];
                let input = DistanceInput {
                    proxy_a: ShapeProxy { points: &triangle_vertices, radius: 0.0 },
                    proxy_b: local_proxy,
                    transform: Transform::IDENTITY,
                    use_radii: true,
                };

                // reset the cache
                cache.count = 0;

                // get distance between triangle and query shape
                let output = shape_distance(&input, &mut cache, None);

                let tolerance = 0.1 * linear_slop();
                if output.distance < tolerance {
                    // overlap detected
                    return true;
                }
            }

            if crate::simd::test_bounds_triangle_overlap(bounds_center_v, bounds_extent_v, v21, v22, v12) {
                let triangle_vertices = [point22, point12, point21];
                let input = DistanceInput {
                    proxy_a: ShapeProxy { points: &triangle_vertices, radius: 0.0 },
                    proxy_b: local_proxy,
                    transform: Transform::IDENTITY,
                    use_radii: true,
                };

                // reset the cache
                cache.count = 0;

                // get distance between triangle and query shape
                let output = shape_distance(&input, &mut cache, None);

                let tolerance = 0.1 * linear_slop();
                if output.distance < tolerance {
                    // overlap detected
                    return true;
                }
            }
        }
    }

    false
}

/// C: b3QueryHeightField with b3MeshQueryFcn callback. The C query ignores the
/// callback's bool return value, and so does this port.
pub fn query_height_field(
    height_field: &HeightFieldData,
    bounds: AABB,
    fcn: &mut dyn FnMut(Vec3, Vec3, Vec3, i32) -> bool,
) {
    let scale = height_field.scale;

    let min_row = (bounds.lower_bound.z / scale.z).floor() as i32;
    let max_row = (bounds.upper_bound.z / scale.z).floor() as i32;
    let min_col = (bounds.lower_bound.x / scale.x).floor() as i32;
    let max_col = (bounds.upper_bound.x / scale.x).floor() as i32;

    // Outer loop on rows and inner loop on columns so that triangle indices
    // increase monotonically.
    for row in min_row..=max_row {
        if row < 0 || height_field.row_count - 1 <= row {
            continue;
        }

        for column in min_col..=max_col {
            if column < 0 || height_field.column_count - 1 <= column {
                continue;
            }

            let cell_index = row * (height_field.column_count - 1) + column;
            b3_assert!(cell_index < (height_field.row_count - 1) * (height_field.column_count - 1));
            let material = height_field.materials[cell_index as usize];
            if material == HEIGHT_FIELD_HOLE {
                continue;
            }

            let corners = get_height_field_cell_corners(height_field, row, column);
            let point11 = corners[0];
            let point12 = corners[1];
            let point21 = corners[2];
            let point22 = corners[3];

            // I know the min/max x and z values, but not the min/max heights.
            let cell_bound = AABB {
                lower_bound: min(min(point11, point12), min(point21, point22)),
                upper_bound: max(max(point11, point12), max(point21, point22)),
            };

            if aabb_overlaps(bounds, cell_bound) {
                let quad_index = row * (height_field.column_count - 1) + column;
                let triangle_index = 2 * quad_index;

                if height_field.clockwise {
                    fcn(point11, point12, point21, triangle_index);
                    fcn(point22, point21, point12, triangle_index + 1);
                } else {
                    fcn(point11, point21, point12, triangle_index);
                    fcn(point22, point12, point21, triangle_index + 1);
                }
            }
        }
    }
}

/// C signature has (planes, capacity); the slice length is the capacity.
pub fn collide_mover_and_height_field(planes: &mut [PlaneResult], shape: &HeightFieldData, mover: &Capsule) -> i32 {
    let capacity = planes.len() as i32;
    let mover_points = [mover.center1, mover.center2];

    let mut cache = SimplexCache::default();

    let radius = mover.radius;
    let center1 = mover.center1;
    let center2 = mover.center2;
    let r = vec3(radius, radius, radius);
    let bounds_min = sub(min(center1, center2), r);
    let bounds_max = add(max(center1, center2), r);
    let bounds_center = mul_sv(0.5, add(bounds_min, bounds_max));
    let bounds_extent = sub(bounds_max, bounds_center);
    let bounds_center_v = crate::simd::load_vec3(bounds_center);
    let bounds_extent_v = crate::simd::load_vec3(bounds_extent);

    let local_min_x = bounds_min.x;
    let local_min_z = bounds_min.z;
    let local_max_x = bounds_max.x;
    let local_max_z = bounds_max.z;

    let scale = shape.scale;
    let min_row = (local_min_z / scale.z).floor() as i32;
    let max_row = (local_max_z / scale.z).floor() as i32;
    let min_col = (local_min_x / scale.x).floor() as i32;
    let max_col = (local_max_x / scale.x).floor() as i32;

    let mut plane_count = 0;

    // Outer loop on rows and inner loop on columns so that triangle indices
    // increase monotonically.
    for row in min_row..=max_row {
        if row < 0 || shape.row_count - 1 <= row {
            continue;
        }

        for column in min_col..=max_col {
            if column < 0 || shape.column_count - 1 <= column {
                continue;
            }

            let cell_index = row * (shape.column_count - 1) + column;
            b3_assert!(cell_index < (shape.row_count - 1) * (shape.column_count - 1));
            let material = shape.materials[cell_index as usize];
            if material == HEIGHT_FIELD_HOLE {
                continue;
            }

            let corners = get_height_field_cell_corners(shape, row, column);
            let point11 = corners[0];
            let point12 = corners[1];
            let point21 = corners[2];
            let point22 = corners[3];

            let v11 = crate::simd::load_vec3(point11);
            let v12 = crate::simd::load_vec3(point12);
            let v21 = crate::simd::load_vec3(point21);
            let v22 = crate::simd::load_vec3(point22);

            if crate::simd::test_bounds_triangle_overlap(bounds_center_v, bounds_extent_v, v11, v21, v12) {
                let triangle_vertices = [point11, point21, point12];
                let distance_input = DistanceInput {
                    proxy_a: ShapeProxy { points: &triangle_vertices, radius: 0.0 },
                    proxy_b: ShapeProxy { points: &mover_points, radius: 0.0 },
                    transform: Transform::IDENTITY,
                    use_radii: false,
                };

                // reset the cache
                cache.count = 0;

                // get distance between triangle and mover
                let distance_output = shape_distance(&distance_input, &mut cache, None);

                if distance_output.distance == 0.0 {
                    // todo SAT
                } else if distance_output.distance <= mover.radius {
                    let plane =
                        Plane { normal: distance_output.normal, offset: mover.radius - distance_output.distance };
                    planes[plane_count as usize] = PlaneResult { plane, point: distance_output.point_a };
                    plane_count += 1;

                    if plane_count == capacity {
                        return plane_count;
                    }
                }
            }

            if crate::simd::test_bounds_triangle_overlap(bounds_center_v, bounds_extent_v, v21, v22, v12) {
                let triangle_vertices = [point22, point12, point21];
                let distance_input = DistanceInput {
                    proxy_a: ShapeProxy { points: &triangle_vertices, radius: 0.0 },
                    proxy_b: ShapeProxy { points: &mover_points, radius: 0.0 },
                    transform: Transform::IDENTITY,
                    use_radii: false,
                };

                // reset the cache
                cache.count = 0;

                // get distance between triangle and mover
                let distance_output = shape_distance(&distance_input, &mut cache, None);

                if distance_output.distance == 0.0 {
                    // todo SAT
                } else if distance_output.distance <= mover.radius {
                    let plane =
                        Plane { normal: distance_output.normal, offset: mover.radius - distance_output.distance };
                    planes[plane_count as usize] = PlaneResult { plane, point: distance_output.point_a };
                    plane_count += 1;

                    if plane_count == capacity {
                        return plane_count;
                    }
                }
            }
        }
    }

    plane_count
}

pub fn create_grid(row_count: i32, column_count: i32, scale: Vec3, make_holes: bool) -> Arc<HeightFieldData> {
    let height_count = (row_count * column_count) as usize;
    let heights = vec![0.0f32; height_count];

    let cell_count = ((row_count - 1) * (column_count - 1)) as usize;
    let mut material_indices = vec![0u8; cell_count];

    for i in 0..(row_count - 1) {
        for j in 0..(column_count - 1) {
            let k = (i * (column_count - 1) + j) as usize;

            if make_holes && k > 0 && k % 16 == 0 {
                material_indices[k] = HEIGHT_FIELD_HOLE;
            } else {
                material_indices[k] = 0;
            }
        }
    }

    let data = HeightFieldDef {
        heights: &heights,
        material_indices: &material_indices,
        scale,
        count_x: column_count,
        count_z: row_count,
        global_minimum_height: -256.0,
        global_maximum_height: 256.0,
        clockwise_winding: false,
    };

    create_height_field(&data)
}

pub fn create_wave(
    row_count: i32,
    column_count: i32,
    scale: Vec3,
    row_frequency: f32,
    column_frequency: f32,
    make_holes: bool,
) -> Arc<HeightFieldData> {
    let height_count = (row_count * column_count) as usize;
    let mut heights = vec![0.0f32; height_count];

    let omega_z = 2.0 * PI * row_frequency;
    let omega_x = 2.0 * PI * column_frequency;

    for i in 0..row_count {
        // Deterministic sine (C uses sinf here; see file header).
        let row_height = sin(omega_z * i as f32);

        for j in 0..column_count {
            let k = (i * column_count + j) as usize;
            let column_height = sin(omega_x * j as f32);
            heights[k] = row_height * column_height;
        }
    }

    let cell_count = ((row_count - 1) * (column_count - 1)) as usize;
    let mut material_indices = vec![0u8; cell_count];

    for i in 0..(row_count - 1) {
        for j in 0..(column_count - 1) {
            let k = (i * (column_count - 1) + j) as usize;

            if make_holes && k > 0 && k % 16 == 0 {
                material_indices[k] = HEIGHT_FIELD_HOLE;
            } else {
                material_indices[k] = 0;
            }
        }
    }

    let data = HeightFieldDef {
        heights: &heights,
        material_indices: &material_indices,
        scale,
        count_x: column_count,
        count_z: row_count,
        global_minimum_height: -256.0,
        global_maximum_height: 256.0,
        clockwise_winding: false,
    };

    create_height_field(&data)
}

/// Save input height data to a file (text format identical to the C version).
pub fn dump_height_data(data: &HeightFieldDef, file_name: &str) {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "{} {}", data.count_x, data.count_z);
    let _ = writeln!(out, "{:.9} {:.9} {:.9}", data.scale.x, data.scale.y, data.scale.z);
    let _ = writeln!(out, "{:.9} {:.9}", data.global_minimum_height, data.global_maximum_height);
    let _ = writeln!(out, "{}", data.clockwise_winding as i32);

    let height_count = (data.count_x * data.count_z) as usize;
    for i in 0..height_count {
        let _ = writeln!(out, "{:.9}", data.heights[i]);
    }

    let material_count = ((data.count_x - 1) * (data.count_z - 1)) as usize;
    for i in 0..material_count {
        let _ = writeln!(out, "{}", data.material_indices[i]);
    }

    let _ = std::fs::write(file_name, out);
}

/// Create a height field by loading previously saved height data.
pub fn load_height_field(file_name: &str) -> Option<Arc<HeightFieldData>> {
    let text = std::fs::read_to_string(file_name).ok()?;
    let mut tokens = text.split_ascii_whitespace();

    let count_x: i32 = tokens.next()?.parse().ok()?;
    let count_z: i32 = tokens.next()?.parse().ok()?;

    let scale = vec3(
        tokens.next()?.parse().ok()?,
        tokens.next()?.parse().ok()?,
        tokens.next()?.parse().ok()?,
    );

    let global_minimum_height: f32 = tokens.next()?.parse().ok()?;
    let global_maximum_height: f32 = tokens.next()?.parse().ok()?;

    let clockwise: i32 = tokens.next()?.parse().ok()?;

    let height_count = (count_x * count_z) as usize;
    let mut heights = vec![0.0f32; height_count];
    for h in heights.iter_mut() {
        *h = tokens.next()?.parse().ok()?;
    }

    let material_count = ((count_x - 1) * (count_z - 1)) as usize;
    let mut material_indices = vec![0u8; material_count];
    for m in material_indices.iter_mut() {
        let material_index: i32 = tokens.next()?.parse().ok()?;
        *m = material_index as u8;
    }

    let data = HeightFieldDef {
        heights: &heights,
        material_indices: &material_indices,
        scale,
        count_x,
        count_z,
        global_minimum_height,
        global_maximum_height,
        clockwise_winding: clockwise != 0,
    };

    Some(create_height_field(&data))
}
