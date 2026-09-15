use crate::*;
use makepad_csg_math::robust::orient2d;
use std::collections::BTreeSet;

pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
pub(crate) fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
pub(crate) fn mul(a: [f64; 3], s: f64) -> [f64; 3] {
    a.map(|v| v * s)
}
pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(crate) fn length(a: [f64; 3]) -> f64 {
    a[0].hypot(a[1]).hypot(a[2])
}
pub(crate) fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    orient2d(a[0], a[1], b[0], b[1], c[0], c[1])
}

pub(crate) struct FaceGeometry {
    pub points: Vec<[f64; 2]>,
    pub normal: [f64; 3],
    pub planar: bool,
}
fn fail(face: FaceId, kind: ValidationKind) -> MeshError {
    MeshError::InvalidGeometry { face, kind }
}

pub(crate) fn face_geometry(
    mesh: &Mesh,
    face: FaceId,
    ctx: &mut Context<'_>,
) -> Result<FaceGeometry> {
    let corners = mesh.face_corners(face)?;
    ctx.checkpoint(corners.len() as u64)?;
    if corners.len() < 3 {
        return Err(fail(face, ValidationKind::DegenerateFace));
    }
    let mut seen = BTreeSet::new();
    let mut p = Vec::with_capacity(corners.len());
    for c in corners {
        if !seen.insert(c.vertex) {
            return Err(fail(face, ValidationKind::RepeatedVertex));
        }
        p.push(
            mesh.vertex(c.vertex)
                .ok_or(MeshError::UnknownElement(ElementId::Vertex(c.vertex)))?
                .position,
        );
    }
    let origin = p[0];
    let scale = p
        .iter()
        .flat_map(|p| sub(*p, origin))
        .fold(0f64, |s, v| s.max(v.abs()));
    if scale == 0. || !scale.is_finite() {
        return Err(fail(face, ValidationKind::DegenerateFace));
    }
    for q in &mut p {
        *q = sub(*q, origin).map(|v| v / scale);
    }
    let mut normal = [0.; 3];
    for i in 0..p.len() {
        if length(sub(p[i], p[(i + 1) % p.len()])) <= 1e-12 {
            return Err(fail(face, ValidationKind::DegenerateEdge));
        }
        normal = add(normal, cross(p[i], p[(i + 1) % p.len()]));
    }
    let mut len = length(normal);
    let degenerate = len <= 1e-12;
    if degenerate {
        // A bow tie can cancel its Newell area exactly. A nonzero local fan
        // normal still lets us identify its crossing before reporting zero area.
        for i in 1..p.len() - 1 {
            ctx.checkpoint(1)?;
            let candidate = cross(p[i], p[i + 1]);
            let candidate_len = length(candidate);
            if candidate_len > len {
                normal = candidate;
                len = candidate_len;
            }
        }
        if len <= 1e-12 {
            return Err(fail(face, ValidationKind::DegenerateFace));
        }
    }
    normal = mul(normal, 1. / len);
    let planar = p.iter().all(|p| dot(*p, normal).abs() <= 1e-8);
    let drop = (0..3)
        .max_by(|&a, &b| normal[a].abs().total_cmp(&normal[b].abs()))
        .unwrap();
    let points: Vec<_> = p
        .iter()
        .map(|p| match drop {
            0 => [p[1], p[2]],
            1 => [p[2], p[0]],
            _ => [p[0], p[1]],
        })
        .collect();
    for i in 0..points.len() {
        let ni = (i + 1) % points.len();
        for j in i + 1..points.len() {
            ctx.checkpoint(1)?;
            let nj = (j + 1) % points.len();
            if ni == j || nj == i {
                continue;
            }
            if segments_intersect(points[i], points[ni], points[j], points[nj]) {
                return Err(fail(face, ValidationKind::SelfIntersectingFace));
            }
        }
    }
    if degenerate {
        return Err(fail(face, ValidationKind::DegenerateFace));
    }
    Ok(FaceGeometry {
        points,
        normal,
        planar,
    })
}

fn on_segment(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    orient(a, b, p) == 0. && (0..2).all(|i| p[i] >= a[i].min(b[i]) && p[i] <= a[i].max(b[i]))
}
fn segments_intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let o = [
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    ];
    (o[0] * o[1] < 0. && o[2] * o[3] < 0.)
        || (o[0] == 0. && on_segment(a, b, c))
        || (o[1] == 0. && on_segment(a, b, d))
        || (o[2] == 0. && on_segment(c, d, a))
        || (o[3] == 0. && on_segment(c, d, b))
}

/// Input has already passed simple-planar-boundary validation. Every ear is
/// checked; failure never returns an incomplete or overlapping triangle fan.
pub(crate) fn ear_clip(
    p: &[[f64; 2]],
    face: FaceId,
    ctx: &mut Context<'_>,
) -> Result<Vec<[usize; 3]>> {
    let area = (1..p.len() - 1)
        .map(|i| orient(p[0], p[i], p[i + 1]))
        .sum::<f64>();
    if area == 0. {
        return Err(fail(face, ValidationKind::DegenerateFace));
    }
    let sign = area.signum();
    let mut remaining: Vec<_> = (0..p.len()).collect();
    let mut out = Vec::with_capacity(p.len() - 2);
    while remaining.len() > 3 {
        let mut ear = None;
        for i in 0..remaining.len() {
            ctx.checkpoint(1)?;
            let t = [
                remaining[(i + remaining.len() - 1) % remaining.len()],
                remaining[i],
                remaining[(i + 1) % remaining.len()],
            ];
            if orient(p[t[0]], p[t[1]], p[t[2]]) * sign <= 0. {
                continue;
            }
            let mut contains = false;
            for &q in &remaining {
                ctx.checkpoint(1)?;
                if t.contains(&q) {
                    continue;
                }
                if orient(p[t[0]], p[t[1]], p[q]) * sign >= 0.
                    && orient(p[t[1]], p[t[2]], p[q]) * sign >= 0.
                    && orient(p[t[2]], p[t[0]], p[q]) * sign >= 0.
                {
                    contains = true;
                    break;
                }
            }
            if !contains {
                ear = Some((i, t));
                break;
            }
        }
        let (i, t) = ear.ok_or_else(|| fail(face, ValidationKind::TriangulationFailed))?;
        out.push(t);
        remaining.remove(i);
    }
    let t = [remaining[0], remaining[1], remaining[2]];
    if orient(p[t[0]], p[t[1]], p[t[2]]) * sign <= 0. {
        return Err(fail(face, ValidationKind::TriangulationFailed));
    }
    out.push(t);
    Ok(out)
}
