//! The surface as the segmenter sees it: welded corners, paired edges and the
//! polygon atoms no chart boundary may cross.

use crate::{ChartOptions, MeshInput, UnwrapError};

pub(crate) const NONE: u32 = u32::MAX;

/// Uses of one welded edge beyond which the best-pair search stops being worth
/// its quadratic cost. Such an edge is a seam for every triangle on it, which
/// costs a chart boundary and nothing else.
const MAX_PAIRED_FAN: usize = 64;

/// Height over longest edge below which a triangle counts as a sliver: it has
/// no meaningful normal, but it still connects its neighbours.
const SLIVER_ASPECT: f64 = 1.0e-6;

#[inline]
pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[inline]
pub(crate) fn length(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
#[inline]
pub(crate) fn normalized(a: [f64; 3]) -> [f64; 3] {
    let l = length(a);
    if l > 0.0 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0; 3]
    }
}

pub(crate) struct Topology<'a> {
    pub positions: &'a [[f64; 3]],
    /// Welded vertex of each corner, `3 * triangle + slot`.
    pub welded: Vec<u32>,
    pub normal: Vec<[f64; 3]>,
    pub area: Vec<f64>,
    /// Two corners welded together: no surface at all, and no edges.
    pub collapsed: Vec<bool>,
    pub sliver: Vec<bool>,
    /// The paired edge of each directed edge, `3 * triangle + slot` runs from
    /// that corner to the next. `NONE` when unpaired.
    pub opposite: Vec<u32>,
    /// Paired, but a chart boundary from the start (authored seam or crease).
    pub cut: Vec<bool>,
    pub atom_of: Vec<u32>,
    pub atom_start: Vec<u32>,
    pub atom_tris: Vec<u32>,
    pub atom_normal: Vec<[f64; 3]>,
    pub atom_area: Vec<f64>,
}

impl<'a> Topology<'a> {
    pub fn triangle_count(&self) -> usize {
        self.normal.len()
    }
    pub fn atom_count(&self) -> usize {
        self.atom_normal.len()
    }
    pub fn tris_of(&self, atom: u32) -> &[u32] {
        let a = atom as usize;
        &self.atom_tris[self.atom_start[a] as usize..self.atom_start[a + 1] as usize]
    }
    #[inline]
    pub fn corner_position(&self, corner: usize) -> [f64; 3] {
        self.positions[self.welded[corner] as usize]
    }

    pub fn build(input: &MeshInput<'a>, options: &ChartOptions) -> Result<Self, UnwrapError> {
        let positions = input.positions;
        if input.indices.len() % 3 != 0 {
            return Err(UnwrapError::InvalidInput(format!(
                "index count {} is not a multiple of 3",
                input.indices.len()
            )));
        }
        for (i, p) in positions.iter().enumerate() {
            if !p.iter().all(|c| c.is_finite()) {
                return Err(UnwrapError::InvalidInput(format!("vertex {i} is not finite")));
            }
        }
        if let Some(&bad) = input.indices.iter().find(|&&i| i as usize >= positions.len()) {
            return Err(UnwrapError::InvalidInput(format!(
                "index {bad} is out of range for {} vertices",
                positions.len()
            )));
        }
        let triangles = input.indices.len() / 3;
        if let Some(polygons) = input.polygons {
            if polygons.len() != triangles {
                return Err(UnwrapError::InvalidInput(format!(
                    "{} polygon ids for {triangles} triangles",
                    polygons.len()
                )));
            }
        }
        let weld = weld_vertices(positions, options.weld_tolerance);
        let welded: Vec<u32> = input.indices.iter().map(|&i| weld[i as usize]).collect();

        let mut normal = vec![[0.0; 3]; triangles];
        let mut area = vec![0.0; triangles];
        let mut collapsed = vec![false; triangles];
        let mut sliver = vec![false; triangles];
        for t in 0..triangles {
            let w = [welded[t * 3], welded[t * 3 + 1], welded[t * 3 + 2]];
            if w[0] == w[1] || w[1] == w[2] || w[2] == w[0] {
                collapsed[t] = true;
                continue;
            }
            let p = w.map(|i| positions[i as usize]);
            let (e0, e1, e2) = (sub(p[1], p[0]), sub(p[2], p[0]), sub(p[2], p[1]));
            let n = cross(e0, e1);
            let doubled = length(n);
            area[t] = doubled * 0.5;
            let longest = dot(e0, e0).max(dot(e1, e1)).max(dot(e2, e2));
            if doubled <= SLIVER_ASPECT * longest {
                sliver[t] = true;
            } else {
                normal[t] = [n[0] / doubled, n[1] / doubled, n[2] / doubled];
            }
        }

        // Atoms first: the pairing needs to know which edges lie inside one.
        let mut atom_of = vec![NONE; triangles];
        let mut atom_count = 0u32;
        match input.polygons {
            None => {
                for t in 0..triangles {
                    if !collapsed[t] {
                        atom_of[t] = atom_count;
                        atom_count += 1;
                    }
                }
            }
            Some(polygons) => {
                let mut order: Vec<u32> =
                    (0..triangles as u32).filter(|&t| !collapsed[t as usize]).collect();
                order.sort_by_key(|&t| (polygons[t as usize], t));
                let mut last = None;
                for t in order {
                    let id = polygons[t as usize];
                    if last != Some(id) {
                        last = Some(id);
                        atom_count += 1;
                    }
                    atom_of[t as usize] = atom_count - 1;
                }
            }
        }
        let atoms = atom_count as usize;
        let mut atom_start = vec![0u32; atoms + 1];
        for &a in &atom_of {
            if a != NONE {
                atom_start[a as usize + 1] += 1;
            }
        }
        for i in 0..atoms {
            atom_start[i + 1] += atom_start[i];
        }
        let mut next = atom_start.clone();
        let mut atom_tris = vec![0u32; atom_start[atoms] as usize];
        for (t, &a) in atom_of.iter().enumerate() {
            if a != NONE {
                atom_tris[next[a as usize] as usize] = t as u32;
                next[a as usize] += 1;
            }
        }
        let mut atom_normal = vec![[0.0; 3]; atoms];
        let mut atom_area = vec![0.0; atoms];
        for a in 0..atoms {
            let mut sum = [0.0; 3];
            for &t in &atom_tris[atom_start[a] as usize..atom_start[a + 1] as usize] {
                let t = t as usize;
                for d in 0..3 {
                    sum[d] += normal[t][d] * area[t];
                }
                atom_area[a] += area[t];
            }
            atom_normal[a] = normalized(sum);
        }

        // Edge uses sorted by welded key; each run is one geometric edge.
        let mut uses: Vec<(u32, u32, u32)> = Vec::with_capacity(triangles * 3);
        for t in 0..triangles {
            if collapsed[t] {
                continue;
            }
            for k in 0..3 {
                let (a, b) = (welded[t * 3 + k], welded[t * 3 + (k + 1) % 3]);
                uses.push((a.min(b), a.max(b), (t * 3 + k) as u32));
            }
        }
        uses.sort_unstable();
        let mut seams: Vec<(u32, u32)> = input
            .seams
            .iter()
            .filter_map(|s| {
                let (a, b) = (
                    *weld.get(s[0] as usize)?,
                    *weld.get(s[1] as usize)?,
                );
                Some((a.min(b), a.max(b)))
            })
            .collect();
        seams.sort_unstable();
        seams.dedup();

        let mut opposite = vec![NONE; triangles * 3];
        let mut cut = vec![false; triangles * 3];
        let mut at = 0;
        let mut seam_at = 0;
        let mut candidates: Vec<(f64, u32, u32)> = Vec::new();
        while at < uses.len() {
            let key = (uses[at].0, uses[at].1);
            let mut end = at + 1;
            while end < uses.len() && (uses[end].0, uses[end].1) == key {
                end += 1;
            }
            let group = &uses[at..end];
            at = end;
            while seam_at < seams.len() && seams[seam_at] < key {
                seam_at += 1;
            }
            let seam = seam_at < seams.len() && seams[seam_at] == key;
            let forward = |e: u32| welded[e as usize] == key.0;
            // Edges inside one polygon pair first, whatever the geometry says.
            for i in 0..group.len() {
                let e = group[i].2;
                if opposite[e as usize] != NONE {
                    continue;
                }
                for use_j in &group[i + 1..] {
                    let o = use_j.2;
                    if opposite[o as usize] == NONE
                        && forward(e) != forward(o)
                        && atom_of[(e / 3) as usize] == atom_of[(o / 3) as usize]
                    {
                        opposite[e as usize] = o;
                        opposite[o as usize] = e;
                        break;
                    }
                }
            }
            if group.len() <= MAX_PAIRED_FAN {
                // The surface continues into the opposite-direction use whose
                // normal agrees best. That keeps the two sheets of a doubled
                // shell apart, and on a true non-manifold edge it joins the
                // two flattest neighbours and leaves the rest as boundary.
                candidates.clear();
                for i in 0..group.len() {
                    let e = group[i].2;
                    for use_j in &group[i + 1..] {
                        let o = use_j.2;
                        if forward(e) != forward(o)
                            && opposite[e as usize] == NONE
                            && opposite[o as usize] == NONE
                        {
                            let (ta, tb) = ((e / 3) as usize, (o / 3) as usize);
                            let agreement = if sliver[ta] || sliver[tb] {
                                1.0
                            } else {
                                dot(normal[ta], normal[tb])
                            };
                            candidates.push((agreement, e, o));
                        }
                    }
                }
                candidates.sort_by(|a, b| {
                    b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
                });
                for &(agreement, e, o) in &candidates {
                    if opposite[e as usize] == NONE && opposite[o as usize] == NONE {
                        // Facing each other is a fold or a doubled sheet.
                        if agreement <= -0.999 {
                            continue;
                        }
                        opposite[e as usize] = o;
                        opposite[o as usize] = e;
                        if agreement < options.crease_cos || seam {
                            cut[e as usize] = true;
                            cut[o as usize] = true;
                        }
                    }
                }
            }
        }
        Ok(Self {
            positions,
            welded,
            normal,
            area,
            collapsed,
            sliver,
            opposite,
            cut,
            atom_of,
            atom_start,
            atom_tris,
            atom_normal,
            atom_area,
        })
    }
}

/// Canonical vertex of each vertex. A zero tolerance trusts the indices.
pub(crate) fn weld_vertices(positions: &[[f64; 3]], tolerance: f64) -> Vec<u32> {
    if !(tolerance > 0.0) || positions.is_empty() {
        return (0..positions.len() as u32).collect();
    }
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for p in positions {
        for d in 0..3 {
            low[d] = low[d].min(p[d]);
            high[d] = high[d].max(p[d]);
        }
    }
    let span = (0..3).map(|d| high[d] - low[d]).fold(0.0f64, f64::max);
    if !(span > 0.0) {
        return vec![0; positions.len()];
    }
    let cell = span * tolerance;
    let key = |p: &[f64; 3]| {
        [
            ((p[0] - low[0]) / cell).round() as i64,
            ((p[1] - low[1]) / cell).round() as i64,
            ((p[2] - low[2]) / cell).round() as i64,
        ]
    };
    let mut order: Vec<([i64; 3], u32)> =
        positions.iter().enumerate().map(|(i, p)| (key(p), i as u32)).collect();
    order.sort_unstable();
    let mut weld = vec![0u32; positions.len()];
    let mut at = 0;
    while at < order.len() {
        let canonical = order[at].1;
        let k = order[at].0;
        while at < order.len() && order[at].0 == k {
            weld[order[at].1 as usize] = canonical;
            at += 1;
        }
    }
    weld
}
