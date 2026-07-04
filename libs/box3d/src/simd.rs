// Port of box3d/src/simd.h + simd.c — the B3_SIMD_NONE scalar path only
// (see PORTING.md). b3V32 is a 3-lane float vector used by the mesh/height
// field BVH traversal and triangle tests.

/// scalar math (C b3V32)
#[derive(Clone, Copy, Debug, Default)]
pub struct V32 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub const ZERO_V: V32 = V32 { x: 0.0, y: 0.0, z: 0.0 };
pub const HALF_V: V32 = V32 { x: 0.5, y: 0.5, z: 0.5 };
pub const ONE_V: V32 = V32 { x: 1.0, y: 1.0, z: 1.0 };

#[inline(always)]
pub fn add_v(a: V32, b: V32) -> V32 {
    V32 { x: a.x + b.x, y: a.y + b.y, z: a.z + b.z }
}

#[inline(always)]
pub fn sub_v(a: V32, b: V32) -> V32 {
    V32 { x: a.x - b.x, y: a.y - b.y, z: a.z - b.z }
}

#[inline(always)]
pub fn mul_v(a: V32, b: V32) -> V32 {
    V32 { x: a.x * b.x, y: a.y * b.y, z: a.z * b.z }
}

#[inline(always)]
pub fn div_v(a: V32, b: V32) -> V32 {
    V32 { x: a.x / b.x, y: a.y / b.y, z: a.z / b.z }
}

#[inline(always)]
pub fn neg_v(a: V32) -> V32 {
    V32 { x: -a.x, y: -a.y, z: -a.z }
}

// Unaligned loads are much faster on recent hardware with little to no penalty
#[inline(always)]
pub fn load_v(src: &[f32]) -> V32 {
    V32 { x: src[0], y: src[1], z: src[2] }
}

/// Load from a Vec3 (the common call site pattern `b3LoadV(&v.x)`).
#[inline(always)]
pub fn load_vec3(v: crate::math_functions::Vec3) -> V32 {
    V32 { x: v.x, y: v.y, z: v.z }
}

#[inline(always)]
pub fn zero_v() -> V32 {
    ZERO_V
}

#[inline(always)]
pub fn get_x_v(a: V32) -> f32 {
    a.x
}

#[inline(always)]
pub fn get_y_v(a: V32) -> f32 {
    a.y
}

#[inline(always)]
pub fn get_z_v(a: V32) -> f32 {
    a.z
}

#[inline(always)]
pub fn get_v(a: V32, index: i32) -> f32 {
    let f = [a.x, a.y, a.z];
    f[index as usize]
}

#[inline(always)]
pub fn splat_v(x: f32) -> V32 {
    V32 { x, y: x, z: x }
}

#[inline(always)]
pub fn abs_v(a: V32) -> V32 {
    V32 {
        x: if a.x < 0.0 { -a.x } else { a.x },
        y: if a.y < 0.0 { -a.y } else { a.y },
        z: if a.z < 0.0 { -a.z } else { a.z },
    }
}

#[inline(always)]
pub fn min_v(a: V32, b: V32) -> V32 {
    V32 {
        x: if a.x < b.x { a.x } else { b.x },
        y: if a.y < b.y { a.y } else { b.y },
        z: if a.z < b.z { a.z } else { b.z },
    }
}

#[inline(always)]
pub fn max_v(a: V32, b: V32) -> V32 {
    V32 {
        x: if a.x > b.x { a.x } else { b.x },
        y: if a.y > b.y { a.y } else { b.y },
        z: if a.z > b.z { a.z } else { b.z },
    }
}

#[inline(always)]
pub fn cross_v(a: V32, b: V32) -> V32 {
    V32 {
        x: a.y * b.z - a.z * b.y,
        y: a.z * b.x - a.x * b.z,
        z: a.x * b.y - a.y * b.x,
    }
}

#[inline(always)]
pub fn modified_cross_v(a: V32, b: V32) -> V32 {
    V32 {
        x: a.y * b.z + a.z * b.y,
        y: a.z * b.x + a.x * b.z,
        z: a.x * b.y + a.y * b.x,
    }
}

#[inline(always)]
pub fn any_less_3v(a: V32, b: V32) -> bool {
    a.x < b.x || a.y < b.y || a.z < b.z
}

#[inline(always)]
pub fn any_less_eq_3v(a: V32, b: V32) -> bool {
    a.x <= b.x || a.y <= b.y || a.z <= b.z
}

#[inline(always)]
pub fn any_greater_3v(a: V32, b: V32) -> bool {
    a.x > b.x || a.y > b.y || a.z > b.z
}

#[inline(always)]
pub fn all_less_eq_3v(a: V32, b: V32) -> bool {
    a.x <= b.x && a.y <= b.y && a.z <= b.z
}

#[inline(always)]
pub fn test_bounds_overlap(node_min1: V32, node_max1: V32, node_min2: V32, node_max2: V32) -> bool {
    let separation = max_v(sub_v(node_min2, node_max1), sub_v(node_min1, node_max2));
    all_less_eq_3v(separation, ZERO_V)
}

/// Test a ray for edge separation with an AABB (Gino, p80).
#[inline]
pub fn test_bounds_ray_overlap(node_min: V32, node_max: V32, ray_start: V32, ray_delta: V32) -> bool {
    // Setup node
    let node_center = mul_v(HALF_V, add_v(node_min, node_max));
    let node_extent = sub_v(node_max, node_center);

    // Setup ray
    let ray_start = sub_v(ray_start, node_center);

    // SAT: Edge separation
    let edge_separation = sub_v(
        abs_v(cross_v(ray_delta, ray_start)),
        modified_cross_v(abs_v(ray_delta), node_extent),
    );
    all_less_eq_3v(edge_separation, ZERO_V)
}

// simd.c — scalar (B3_SIMD_NONE) implementations

#[inline(always)]
fn splat_x_v(a: V32) -> V32 {
    V32 { x: a.x, y: a.x, z: a.x }
}

#[inline(always)]
fn splat_y_v(a: V32) -> V32 {
    V32 { x: a.y, y: a.y, z: a.y }
}

#[inline(always)]
fn splat_z_v(a: V32) -> V32 {
    V32 { x: a.z, y: a.z, z: a.z }
}

#[inline(always)]
fn any_greater_eq_3v(a: V32, b: V32) -> bool {
    a.x >= b.x || a.y >= b.y || a.z >= b.z
}

#[inline(always)]
fn dot_3v(a: V32, b: V32) -> V32 {
    let d = a.x * b.x + a.y * b.y + a.z * b.z;
    V32 { x: d, y: d, z: d }
}

// B3_TRANSPOSE3 (scalar path)
#[inline(always)]
fn transpose3(c1: &mut V32, c2: &mut V32, c3: &mut V32) {
    let temp1 = c1.y;
    let temp2 = c1.z;
    let temp3 = c2.z;

    c1.y = c2.x;
    c1.z = c3.x;
    c2.z = c3.y;

    c2.x = temp1;
    c3.x = temp2;
    c3.y = temp3;
}

pub fn test_bounds_triangle_overlap(node_center: V32, node_extent: V32, vertex1: V32, vertex2: V32, vertex3: V32) -> bool {
    let two = splat_v(2.0);

    // Setup triangle
    let vertex1 = sub_v(vertex1, node_center);
    let vertex2 = sub_v(vertex2, node_center);
    let vertex3 = sub_v(vertex3, node_center);

    // Face separation
    let triangle_min = min_v(vertex1, min_v(vertex2, vertex3));
    let triangle_max = max_v(vertex1, max_v(vertex2, vertex3));

    let separation1 = sub_v(triangle_min, node_extent);
    let separation2 = add_v(triangle_max, node_extent);

    let face_separation = max_v(separation1, neg_v(separation2));
    if any_greater_3v(face_separation, ZERO_V) {
        return false;
    }

    // SAT: Face separation
    let edge1 = sub_v(vertex2, vertex1);
    let edge2 = sub_v(vertex3, vertex2);
    let edge3 = sub_v(vertex1, vertex3);

    let normal = cross_v(edge1, edge2);

    let triangle_separation = sub_v(abs_v(dot_3v(normal, vertex1)), dot_3v(abs_v(normal), node_extent));
    if any_greater_3v(triangle_separation, ZERO_V) {
        return false;
    }

    // SAT: Edge separation
    let edge_separation1 = sub_v(
        sub_v(abs_v(cross_v(edge1, add_v(vertex1, vertex3))), abs_v(cross_v(edge1, edge3))),
        mul_v(two, modified_cross_v(abs_v(edge1), node_extent)),
    );
    if any_greater_3v(edge_separation1, ZERO_V) {
        return false;
    }

    let edge_separation2 = sub_v(
        sub_v(abs_v(cross_v(edge2, add_v(vertex1, vertex2))), abs_v(cross_v(edge2, edge1))),
        mul_v(two, modified_cross_v(abs_v(edge2), node_extent)),
    );
    if any_greater_3v(edge_separation2, ZERO_V) {
        return false;
    }

    let edge_separation3 = sub_v(
        sub_v(abs_v(cross_v(edge3, add_v(vertex2, vertex3))), abs_v(cross_v(edge3, edge2))),
        mul_v(two, modified_cross_v(abs_v(edge3), node_extent)),
    );
    if any_greater_3v(edge_separation3, ZERO_V) {
        return false;
    }

    true
}

pub fn intersect_ray_triangle(ray_start: V32, ray_delta: V32, vertex1: V32, vertex2: V32, vertex3: V32) -> f32 {
    // Test if ray intersects this triangle sharing same calculations for each triangle
    {
        let edge1 = sub_v(vertex3, vertex2);
        let edge2 = sub_v(vertex1, vertex3);
        let edge3 = sub_v(vertex2, vertex1);

        let mid_point1 = mul_v(HALF_V, add_v(vertex2, vertex3));
        let mid_point2 = mul_v(HALF_V, add_v(vertex3, vertex1));
        let mid_point3 = mul_v(HALF_V, add_v(vertex1, vertex2));

        let mut normal1 = cross_v(edge1, sub_v(mid_point1, ray_start));
        let mut normal2 = cross_v(edge2, sub_v(mid_point2, ray_start));
        let mut normal3 = cross_v(edge3, sub_v(mid_point3, ray_start));
        transpose3(&mut normal1, &mut normal2, &mut normal3);

        let ray_delta_x = splat_x_v(ray_delta);
        let ray_delta_y = splat_y_v(ray_delta);
        let ray_delta_z = splat_z_v(ray_delta);

        let volumes = add_v(add_v(mul_v(normal1, ray_delta_x), mul_v(normal2, ray_delta_y)), mul_v(normal3, ray_delta_z));
        if any_less_3v(volumes, ZERO_V) {
            return 1.0;
        }
    }

    // Compute intersection with triangle plane
    let edge1 = sub_v(vertex2, vertex1);
    let edge2 = sub_v(vertex3, vertex1);
    let normal = cross_v(edge1, edge2);

    let denominator = dot_3v(normal, ray_delta);
    if any_greater_eq_3v(denominator, ZERO_V) {
        return 1.0;
    }

    let mut lambda = div_v(dot_3v(normal, sub_v(vertex1, ray_start)), denominator);
    if any_less_eq_3v(lambda, ZERO_V) {
        return 1.0;
    }

    lambda = min_v(lambda, ONE_V);
    get_x_v(lambda)
}
