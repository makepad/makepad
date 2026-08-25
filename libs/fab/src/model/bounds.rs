//! AABB helpers over `makepad_math::Aabb` (which has no notion of "empty").

use makepad_math::{Aabb, Mat4f, Vec3f, Vec4f};

/// An inverted box: any union with a point makes it valid.
pub fn aabb_empty() -> Aabb {
    Aabb {
        min: Vec3f {
            x: f32::INFINITY,
            y: f32::INFINITY,
            z: f32::INFINITY,
        },
        max: Vec3f {
            x: f32::NEG_INFINITY,
            y: f32::NEG_INFINITY,
            z: f32::NEG_INFINITY,
        },
    }
}

pub fn aabb_is_empty(a: &Aabb) -> bool {
    a.min.x > a.max.x || a.min.y > a.max.y || a.min.z > a.max.z
}

pub fn aabb_union_point(a: &Aabb, p: Vec3f) -> Aabb {
    Aabb {
        min: Vec3f::min_componentwise(a.min, p),
        max: Vec3f::max_componentwise(a.max, p),
    }
}

pub fn aabb_union(a: &Aabb, b: &Aabb) -> Aabb {
    if aabb_is_empty(a) {
        return *b;
    }
    if aabb_is_empty(b) {
        return *a;
    }
    Aabb {
        min: Vec3f::min_componentwise(a.min, b.min),
        max: Vec3f::max_componentwise(a.max, b.max),
    }
}

pub fn aabb_center(a: &Aabb) -> Vec3f {
    (a.min + a.max) * 0.5
}

pub fn aabb_extent(a: &Aabb) -> Vec3f {
    a.max - a.min
}

/// Radius of the bounding sphere around the box center.
pub fn aabb_radius(a: &Aabb) -> f32 {
    aabb_extent(a).length() * 0.5
}

/// Bounds of the 8 transformed corners.
pub fn aabb_transform(a: &Aabb, m: &Mat4f) -> Aabb {
    let mut out = aabb_empty();
    for i in 0..8 {
        let c = Vec3f {
            x: if i & 1 == 0 { a.min.x } else { a.max.x },
            y: if i & 2 == 0 { a.min.y } else { a.max.y },
            z: if i & 4 == 0 { a.min.z } else { a.max.z },
        };
        let p = m.transform_vec4(Vec4f {
            x: c.x,
            y: c.y,
            z: c.z,
            w: 1.0,
        });
        out = aabb_union_point(
            &out,
            Vec3f {
                x: p.x,
                y: p.y,
                z: p.z,
            },
        );
    }
    out
}

/// Transform a point by an affine matrix (w = 1).
pub fn transform_point(m: &Mat4f, p: Vec3f) -> Vec3f {
    let v = m.transform_vec4(Vec4f {
        x: p.x,
        y: p.y,
        z: p.z,
        w: 1.0,
    });
    Vec3f {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

/// Transform a direction by an affine matrix (w = 0). Not renormalised.
pub fn transform_dir(m: &Mat4f, d: Vec3f) -> Vec3f {
    let v = m.transform_vec4(Vec4f {
        x: d.x,
        y: d.y,
        z: d.z,
        w: 0.0,
    });
    Vec3f {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}
