#[derive(Clone, Copy, Debug)]
pub struct LeafletPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct LeafletBounds {
    pub min: LeafletPoint,
    pub max: LeafletPoint,
}

impl LeafletPoint {
    fn from_tuple(point: (f32, f32)) -> Self {
        Self {
            x: point.0,
            y: point.1,
        }
    }

    fn to_tuple(self) -> (f32, f32) {
        (self.x, self.y)
    }
}

fn sq_dist(a: LeafletPoint, b: LeafletPoint) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    dx * dx + dy * dy
}

fn sq_closest_point_on_segment(point: LeafletPoint, a: LeafletPoint, b: LeafletPoint) -> f32 {
    let mut x = a.x;
    let mut y = a.y;
    let mut dx = b.x - x;
    let mut dy = b.y - y;
    let dot = dx * dx + dy * dy;

    if dot > 0.0 {
        let t = ((point.x - x) * dx + (point.y - y) * dy) / dot;
        if t > 1.0 {
            x = b.x;
            y = b.y;
        } else if t > 0.0 {
            x += dx * t;
            y += dy * t;
        }
    }

    dx = point.x - x;
    dy = point.y - y;
    dx * dx + dy * dy
}

fn simplify_dp_step(
    points: &[LeafletPoint],
    markers: &mut [bool],
    sq_tolerance: f32,
    first: usize,
    last: usize,
) {
    if last <= first + 1 {
        return;
    }

    let mut max_sq_dist = 0.0_f32;
    let mut index = first;
    for i in first + 1..last {
        let sq = sq_closest_point_on_segment(points[i], points[first], points[last]);
        if sq > max_sq_dist {
            max_sq_dist = sq;
            index = i;
        }
    }

    if max_sq_dist > sq_tolerance {
        markers[index] = true;
        simplify_dp_step(points, markers, sq_tolerance, first, index);
        simplify_dp_step(points, markers, sq_tolerance, index, last);
    }
}

pub fn simplify_polyline(points: &[(f32, f32)], tolerance: f32) -> Vec<(f32, f32)> {
    if tolerance <= f32::EPSILON || points.len() < 2 {
        return points.to_vec();
    }

    let sq_tolerance = tolerance * tolerance;
    let points = points
        .iter()
        .copied()
        .map(LeafletPoint::from_tuple)
        .collect::<Vec<_>>();

    let mut reduced = Vec::<LeafletPoint>::with_capacity(points.len());
    reduced.push(points[0]);
    let mut prev = 0_usize;
    for i in 1..points.len() {
        if sq_dist(points[i], points[prev]) > sq_tolerance {
            reduced.push(points[i]);
            prev = i;
        }
    }
    if prev < points.len() - 1 {
        reduced.push(*points.last().unwrap_or(&points[0]));
    }

    if reduced.len() < 3 {
        return reduced.into_iter().map(LeafletPoint::to_tuple).collect();
    }

    let len = reduced.len();
    let mut markers = vec![false; len];
    markers[0] = true;
    markers[len - 1] = true;
    simplify_dp_step(&reduced, &mut markers, sq_tolerance, 0, len - 1);

    let mut out = Vec::<(f32, f32)>::with_capacity(len);
    for (index, point) in reduced.into_iter().enumerate() {
        if markers[index] {
            out.push(point.to_tuple());
        }
    }
    out
}

fn bit_code(point: LeafletPoint, bounds: LeafletBounds) -> u8 {
    let mut code = 0_u8;
    if point.x < bounds.min.x {
        code |= 1;
    } else if point.x > bounds.max.x {
        code |= 2;
    }
    if point.y < bounds.min.y {
        code |= 4;
    } else if point.y > bounds.max.y {
        code |= 8;
    }
    code
}

fn edge_intersection(
    a: LeafletPoint,
    b: LeafletPoint,
    out_code: u8,
    bounds: LeafletBounds,
) -> Option<LeafletPoint> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let (x, y) = if (out_code & 8) != 0 {
        if dy.abs() <= f32::EPSILON {
            return None;
        }
        (a.x + dx * (bounds.max.y - a.y) / dy, bounds.max.y)
    } else if (out_code & 4) != 0 {
        if dy.abs() <= f32::EPSILON {
            return None;
        }
        (a.x + dx * (bounds.min.y - a.y) / dy, bounds.min.y)
    } else if (out_code & 2) != 0 {
        if dx.abs() <= f32::EPSILON {
            return None;
        }
        (bounds.max.x, a.y + dy * (bounds.max.x - a.x) / dx)
    } else {
        if dx.abs() <= f32::EPSILON {
            return None;
        }
        (bounds.min.x, a.y + dy * (bounds.min.x - a.x) / dx)
    };
    Some(LeafletPoint { x, y })
}

fn clip_segment(
    mut a: LeafletPoint,
    mut b: LeafletPoint,
    bounds: LeafletBounds,
    use_last_code: bool,
    last_code: &mut u8,
) -> Option<(LeafletPoint, LeafletPoint)> {
    let mut code_a = if use_last_code {
        *last_code
    } else {
        bit_code(a, bounds)
    };
    let mut code_b = bit_code(b, bounds);
    *last_code = code_b;

    let mut guard = 0_u8;
    loop {
        if (code_a | code_b) == 0 {
            return Some((a, b));
        }
        if (code_a & code_b) != 0 {
            return None;
        }
        if guard > 8 {
            return None;
        }
        guard += 1;

        let code_out = if code_a != 0 { code_a } else { code_b };
        let point = edge_intersection(a, b, code_out, bounds)?;
        let new_code = bit_code(point, bounds);
        if code_out == code_a {
            a = point;
            code_a = new_code;
        } else {
            b = point;
            code_b = new_code;
        }
    }
}

pub fn clip_polyline_parts(
    points: &[(f32, f32)],
    bounds: LeafletBounds,
    no_clip: bool,
) -> Vec<Vec<(f32, f32)>> {
    if points.len() < 2 {
        return Vec::new();
    }
    if no_clip {
        return vec![points.to_vec()];
    }

    let mut parts = Vec::<Vec<(f32, f32)>>::new();
    let mut k = 0_usize;
    let mut last_code = 0_u8;
    let len = points.len();

    for j in 0..len - 1 {
        let a = LeafletPoint::from_tuple(points[j]);
        let b = LeafletPoint::from_tuple(points[j + 1]);
        let Some((s0, s1)) = clip_segment(a, b, bounds, j > 0, &mut last_code) else {
            continue;
        };

        if parts.len() <= k {
            parts.push(Vec::new());
        }
        parts[k].push(s0.to_tuple());

        if s1.to_tuple() != points[j + 1] || j == len - 2 {
            parts[k].push(s1.to_tuple());
            k += 1;
        }
    }

    parts.retain(|part| part.len() >= 2);
    parts
}

pub fn build_polyline_parts(
    points: &[(f32, f32)],
    bounds: LeafletBounds,
    no_clip: bool,
    smooth_factor: f32,
) -> Vec<Vec<(f32, f32)>> {
    let mut parts = clip_polyline_parts(points, bounds, no_clip);
    if smooth_factor > f32::EPSILON {
        for part in &mut parts {
            *part = simplify_polyline(part, smooth_factor);
        }
        parts.retain(|part| part.len() >= 2);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_segment_keeps_crossing_line() {
        let bounds = LeafletBounds {
            min: LeafletPoint { x: 0.0, y: 0.0 },
            max: LeafletPoint { x: 10.0, y: 10.0 },
        };
        let parts = clip_polyline_parts(&[(-5.0, 5.0), (15.0, 5.0)], bounds, false);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].len(), 2);
        assert!((parts[0][0].0 - 0.0).abs() < 1e-5);
        assert!((parts[0][1].0 - 10.0).abs() < 1e-5);
    }

    #[test]
    fn simplify_reduces_dense_straight_line() {
        let points = vec![
            (0.0, 0.0),
            (1.0, 0.01),
            (2.0, 0.0),
            (3.0, -0.01),
            (4.0, 0.0),
        ];
        let simplified = simplify_polyline(&points, 0.2);
        assert!(simplified.len() <= 3);
        assert_eq!(simplified.first().copied(), Some((0.0, 0.0)));
        assert_eq!(simplified.last().copied(), Some((4.0, 0.0)));
    }
}
