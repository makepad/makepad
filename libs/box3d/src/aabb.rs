// Port of box3d/src/aabb.h + aabb.c

use crate::math_functions::*;

// Similar to Real-time Collision Detection, p179.
// Ray cast an AABB. This is a custom function used by height fields.
pub fn ray_cast_aabb(a: AABB, p1: Vec3, p2: Vec3, min_fraction: &mut f32, max_fraction: &mut f32) -> bool {
    // Ray direction and length
    let d = sub(p2, p1);
    let ray_length = length(d);

    // Handle degenerate ray
    if ray_length < f32::EPSILON {
        // Check if point is inside AABB
        if p1.x >= a.lower_bound.x
            && p1.x <= a.upper_bound.x
            && p1.y >= a.lower_bound.y
            && p1.y <= a.upper_bound.y
            && p1.z >= a.lower_bound.z
            && p1.z <= a.upper_bound.z
        {
            *min_fraction = 0.0;
            *max_fraction = 0.0;
            return true;
        }

        return false;
    }

    let ray_dir = mul_sv(1.0 / ray_length, d);

    // Slab method for ray-AABB intersection
    let mut t_min = 0.0;
    let mut t_max = ray_length;

    // x-axis
    {
        let ray_component = ray_dir.x;
        let ray_start = p1.x;
        let box_min = a.lower_bound.x;
        let box_max = a.upper_bound.x;

        if abs_float(ray_component) < f32::EPSILON {
            // Ray is parallel to slab, check if ray origin is within slab
            if ray_start < box_min || ray_start > box_max {
                return false;
            }
        } else {
            // Compute intersection distances
            let mut t1 = (box_min - ray_start) / ray_component;
            let mut t2 = (box_max - ray_start) / ray_component;

            // Ensure t1 <= t2
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }

            // Update intersection interval
            t_min = max_float(t_min, t1);
            t_max = min_float(t_max, t2);

            // Check for no intersection
            if t_min > t_max {
                return false;
            }
        }
    }

    // y-axis
    {
        let ray_component = ray_dir.y;
        let ray_start = p1.y;
        let box_min = a.lower_bound.y;
        let box_max = a.upper_bound.y;

        if abs_float(ray_component) < f32::EPSILON {
            // Ray is parallel to slab, check if ray origin is within slab
            if ray_start < box_min || ray_start > box_max {
                return false;
            }
        } else {
            // Compute intersection distances
            let mut t1 = (box_min - ray_start) / ray_component;
            let mut t2 = (box_max - ray_start) / ray_component;

            // Ensure t1 <= t2
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }

            // Update intersection interval
            t_min = max_float(t_min, t1);
            t_max = min_float(t_max, t2);

            // Check for no intersection
            if t_min > t_max {
                return false;
            }
        }
    }

    // z-axis
    {
        let ray_component = ray_dir.z;
        let ray_start = p1.z;
        let box_min = a.lower_bound.z;
        let box_max = a.upper_bound.z;

        if abs_float(ray_component) < f32::EPSILON {
            // Ray is parallel to slab, check if ray origin is within slab
            if ray_start < box_min || ray_start > box_max {
                return false;
            }
        } else {
            // Compute intersection distances
            let mut t1 = (box_min - ray_start) / ray_component;
            let mut t2 = (box_max - ray_start) / ray_component;

            // Ensure t1 <= t2
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }

            // Update intersection interval
            t_min = max_float(t_min, t1);
            t_max = min_float(t_max, t2);

            // Check for no intersection
            if t_min > t_max {
                return false;
            }
        }
    }

    // Check if intersection is behind ray start
    if t_max < 0.0 {
        return false;
    }

    // Convert distances to fractions
    *min_fraction = clamp_float(t_min / ray_length, 0.0, 1.0);
    *max_fraction = clamp_float(t_max / ray_length, 0.0, 1.0);

    true
}

// Get the surface area (perimeter)
#[inline]
pub fn perimeter(a: AABB) -> f32 {
    let wx = a.upper_bound.x - a.lower_bound.x;
    let wy = a.upper_bound.y - a.lower_bound.y;
    let wz = a.upper_bound.z - a.lower_bound.z;
    2.0 * (wx * wz + wy * wx + wz * wy)
}

/// Enlarge a to contain b
/// @return true if the AABB grew
#[inline]
pub fn enlarge_aabb(a: &mut AABB, b: AABB) -> bool {
    let mut changed = false;
    if b.lower_bound.x < a.lower_bound.x {
        a.lower_bound.x = b.lower_bound.x;
        changed = true;
    }

    if b.lower_bound.y < a.lower_bound.y {
        a.lower_bound.y = b.lower_bound.y;
        changed = true;
    }

    if b.lower_bound.z < a.lower_bound.z {
        a.lower_bound.z = b.lower_bound.z;
        changed = true;
    }

    if a.upper_bound.x < b.upper_bound.x {
        a.upper_bound.x = b.upper_bound.x;
        changed = true;
    }

    if a.upper_bound.y < b.upper_bound.y {
        a.upper_bound.y = b.upper_bound.y;
        changed = true;
    }

    if a.upper_bound.z < b.upper_bound.z {
        a.upper_bound.z = b.upper_bound.z;
        changed = true;
    }

    changed
}

#[inline]
pub fn farthest_point_on_aabb(b: AABB, p: Vec3) -> Vec3 {
    Vec3 {
        x: if (p.x - b.lower_bound.x) > (b.upper_bound.x - p.x) { b.lower_bound.x } else { b.upper_bound.x },
        y: if (p.y - b.lower_bound.y) > (b.upper_bound.y - p.y) { b.lower_bound.y } else { b.upper_bound.y },
        z: if (p.z - b.lower_bound.z) > (b.upper_bound.z - p.z) { b.lower_bound.z } else { b.upper_bound.z },
    }
}
