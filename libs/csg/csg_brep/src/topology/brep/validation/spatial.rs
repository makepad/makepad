//! Call-local spatial candidates. Iterative balanced construction and traversal;
//! no recursion. Native predicates still decide acceptance, in original order.
use crate::geom::extent3d::Extent3d;
use crate::math::Point3;

#[derive(Clone)]
struct Entry {
    ordinal: usize,
    bbox: Extent3d,
}
struct Node {
    bbox: Extent3d,
    children: Option<(usize, usize)>,
    entries: Vec<usize>,
}

pub(crate) struct SpatialIndex {
    entries: Vec<Entry>,
    nodes: Vec<Node>,
    unbounded: Vec<usize>,
    len: usize,
}

fn finite(b: &Extent3d) -> bool {
    (0..3).all(|axis| {
        b.min_at(axis).is_finite() && b.max_at(axis).is_finite() && b.min_at(axis) <= b.max_at(axis)
    })
}
impl SpatialIndex {
    pub(crate) fn new(boxes: impl IntoIterator<Item = Option<Extent3d>>) -> Self {
        let mut entries = Vec::new();
        let mut unbounded = Vec::new();
        let mut len = 0;
        for (ordinal, bbox) in boxes.into_iter().enumerate() {
            len += 1;
            if let Some(bbox) = bbox.filter(finite) {
                entries.push(Entry { ordinal, bbox });
            } else {
                unbounded.push(ordinal);
            }
        }
        let mut out = Self {
            entries,
            nodes: Vec::new(),
            unbounded,
            len,
        };
        if out.entries.is_empty() {
            return out;
        }
        let bounds = out.entries.iter().skip(1).fold(
            out.entries.first().map_or_else(Extent3d::new, |e| e.bbox),
            |b, e| b.union(&e.bbox),
        );
        out.nodes.push(Node {
            bbox: bounds,
            children: None,
            entries: Vec::new(),
        });
        let mut pending = vec![(0, 0, out.entries.len(), 0)];
        while let Some((node, begin, end, axis)) = pending.pop() {
            if end - begin <= 8 {
                if let Some(n) = out.nodes.get_mut(node) {
                    n.entries.extend(begin..end);
                }
                continue;
            }
            if let Some(slice) = out.entries.get_mut(begin..end) {
                slice.sort_by(|a, b| {
                    a.bbox
                        .min_at(axis)
                        .total_cmp(&b.bbox.min_at(axis))
                        .then(a.ordinal.cmp(&b.ordinal))
                });
            }
            let mid = begin + (end - begin) / 2;
            let left = out.nodes.len();
            for (start, stop) in [(begin, mid), (mid, end)] {
                let bbox = out
                    .entries
                    .get(start)
                    .map_or_else(Extent3d::new, |e| e.bbox);
                let bbox = out
                    .entries
                    .get(start + 1..stop)
                    .unwrap_or_default()
                    .iter()
                    .fold(bbox, |b, e| b.union(&e.bbox));
                out.nodes.push(Node {
                    bbox,
                    children: None,
                    entries: Vec::new(),
                });
            }
            if let Some(n) = out.nodes.get_mut(node) {
                n.children = Some((left, left + 1));
            }
            pending.push((left + 1, mid, end, (axis + 1) % 3));
            pending.push((left, begin, mid, (axis + 1) % 3));
        }
        out
    }
    fn select(&self, keep: impl Fn(&Extent3d) -> bool) -> Vec<usize> {
        let mut result = self.unbounded.clone();
        let mut pending = Vec::new();
        if !self.nodes.is_empty() {
            pending.push(0);
        }
        while let Some(i) = pending.pop() {
            let Some(n) = self.nodes.get(i) else {
                continue;
            };
            if !keep(&n.bbox) {
                continue;
            }
            if let Some((left, right)) = n.children {
                pending.push(right);
                pending.push(left);
            } else {
                for &i in &n.entries {
                    if let Some(e) = self.entries.get(i) {
                        if keep(&e.bbox) {
                            result.push(e.ordinal);
                        }
                    }
                }
            }
        }
        result.sort_unstable();
        result
    }
    pub(crate) fn points_in_box(&self, bbox: &Extent3d, tol: f64) -> Vec<usize> {
        if !finite(bbox) || !tol.is_finite() || tol < 0.0 {
            return (0..self.len).collect();
        }
        self.select(|node| !node.are_disjoint(bbox, tol))
    }
    pub(crate) fn boxes_in_box(&self, bbox: &Extent3d, tol: f64) -> Vec<usize> {
        if !finite(bbox) || !tol.is_finite() || tol < 0.0 {
            return (0..self.len).collect();
        }
        self.select(|node| !bbox.are_disjoint(node, tol))
    }
    pub(crate) fn near_point(&self, p: Point3, tol: f64) -> Vec<usize> {
        if ![p.x, p.y, p.z, tol].iter().all(|x| x.is_finite()) || tol < 0.0 {
            return (0..self.len).collect();
        }
        self.select(|b| {
            (0..3).all(|axis| {
                let q = match axis {
                    0 => p.x,
                    1 => p.y,
                    _ => p.z,
                };
                !((q > b.max_at(axis) && q - b.max_at(axis) > tol)
                    || (q < b.min_at(axis) && b.min_at(axis) - q > tol))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn candidates_preserve_order_and_boundaries() {
        let points: Vec<_> = (0..1000)
            .map(|i| Point3::new(f64::from(i), 0.0, 0.0))
            .collect();
        let index = SpatialIndex::new(points.iter().copied().map(Extent3d::from_point).map(Some));
        for p in [Point3::new(500.0, 0.0, 0.0), Point3::new(500.5, 0.0, 0.0)] {
            for tol in [0.0, 0.5, 1.0, f64::NAN] {
                let expected: Vec<_> = points
                    .iter()
                    .enumerate()
                    .filter(|(_, q)| p.closer_than(tol, **q))
                    .map(|(i, _)| i)
                    .collect();
                let candidates = index.near_point(p, tol);
                assert!(expected.iter().all(|x| candidates.contains(x)));
                assert!(candidates.windows(2).all(|x| x[0] < x[1]));
            }
        }
    }
}

#[cfg(test)]
mod numeric_tests {
    use super::*;
    #[test]
    fn spatial_pruning_never_removes_a_native_closer_than_match() {
        let values = [
            0.0,
            f64::from_bits(1),
            1e-300,
            1e-7,
            1.0,
            f64::from_bits(1.0_f64.to_bits() + 1),
            1e100,
            1e300,
            f64::MAX,
            f64::INFINITY,
            f64::NAN,
        ];
        let points: Vec<_> = values
            .iter()
            .flat_map(|&v| [Point3::new(v, 0.0, 0.0), Point3::new(-v, v, v)])
            .collect();
        let index = SpatialIndex::new(points.iter().copied().map(Extent3d::from_point).map(Some));
        for &p in &points {
            for tolerance in values {
                let candidates = index.near_point(p, tolerance);
                for (i, &q) in points.iter().enumerate() {
                    if p.closer_than(tolerance, q) {
                        assert!(candidates.contains(&i), "p={p:?} q={q:?} tol={tolerance}");
                    }
                }
            }
        }
    }
}
