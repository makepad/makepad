//! Automatic UV unwrap for arbitrary triangle meshes.
//!
//! The pipeline: weld and pair edges into a surface ([`topology`]), segment it
//! into charts by normal and flatten each one ([`chart`]), planar projection
//! for flat charts and a Least Squares Conformal Map for curved ones solved by
//! preconditioned conjugate gradients ([`solve`]), scale each chart to its true
//! area, and nest the charts into an atlas with padding in texels ([`pack`]).
//!
//! Runs on the calling thread and touches no global state: call it from a
//! worker. Results depend only on the input, on every platform.
//!
//! Charts split when they would stretch; a triangle is never dropped. Every
//! input triangle has a chart and three uvs, including collapsed ones.

mod chart;
mod pack;
mod solve;
mod topology;

pub use chart::triangle_singular_values;
pub use pack::{pack_items, Atlas, Mask, Placement};

use std::fmt;

/// Called with an estimate of the work about to be done. Return false to
/// cancel: the operation then stops with [`UnwrapError::Cancelled`].
pub type Checkpoint<'a> = dyn FnMut(u64) -> bool + 'a;

#[derive(Clone, Debug, PartialEq)]
pub enum UnwrapError {
    Cancelled,
    InvalidInput(String),
    /// The charts cannot fit at all, even shrunk to a single texel each, or
    /// not at the density that was asked for.
    AtlasTooSmall {
        width: u32,
        height: u32,
        charts: usize,
        padding: u32,
        /// A side length that would be needed, as an estimate.
        needed: u32,
    },
}

impl fmt::Display for UnwrapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "unwrap cancelled"),
            Self::InvalidInput(s) => write!(f, "invalid unwrap input: {s}"),
            Self::AtlasTooSmall { width, height, charts, padding, needed } => write!(
                f,
                "a {width}x{height} atlas cannot hold {charts} charts with {padding} texels of padding: it needs about {needed}x{needed}"
            ),
        }
    }
}
impl std::error::Error for UnwrapError {}

#[derive(Clone, Copy, Debug)]
pub struct MeshInput<'a> {
    pub positions: &'a [[f64; 3]],
    /// Three per triangle.
    pub indices: &'a [u32],
    /// Polygon id per triangle. Triangles sharing an id never get separated.
    pub polygons: Option<&'a [u32]>,
    /// Vertex pairs whose edge must be a chart boundary.
    pub seams: &'a [[u32; 2]],
}

#[derive(Clone, Copy, Debug)]
pub struct ChartOptions {
    /// An edge whose two normals have a smaller dot product is a chart boundary.
    pub crease_cos: f64,
    /// A chart must keep 95% of its area within this much stretch in its worst
    /// direction, once scaled to its true area. 1 is an isometry. Lower means
    /// more charts and less distortion; a chart over the limit splits.
    pub max_stretch: f64,
    /// Vertices closer than this fraction of the mesh size weld together. Zero
    /// trusts the indices.
    pub weld_tolerance: f64,
}

impl Default for ChartOptions {
    fn default() -> Self {
        Self { crease_cos: 0.5, max_stretch: 1.25, weld_tolerance: 1.0e-6 }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AtlasTarget {
    /// Fixed size: the texel density goes as high as fits.
    Fit { width: u32, height: u32 },
    /// Fixed density: the atlas is as small as fits, up to a square of
    /// `max_size`.
    Density { texels_per_unit: f64, max_size: u32 },
    /// This density if it fits in a square of `max_size`, else the highest that
    /// does. [`Atlas::texels_per_unit`] says which it got.
    UpTo { texels_per_unit: f64, max_size: u32 },
}

#[derive(Clone, Copy, Debug)]
pub struct PackOptions<'a> {
    pub target: AtlasTarget,
    /// Free texels kept between any two charts and around the atlas edge.
    pub padding: u32,
    /// Triangles, in texels, that are already taken.
    pub obstacles: &'a [[[f64; 2]; 3]],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub triangles: Vec<u32>,
    /// Surface area, which is also the chart's area in chart space.
    pub area: f64,
    /// Projected onto its plane rather than solved.
    pub flat: bool,
    /// The measured stretch that [`ChartOptions::max_stretch`] limits.
    pub stretch: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Charting {
    /// Chart-space position of each corner, `3 * triangle + slot`, in the
    /// mesh's own units.
    pub corner_uv: Vec<[f64; 2]>,
    pub triangle_chart: Vec<u32>,
    pub charts: Vec<Chart>,
}

impl Charting {
    fn items(&self) -> Vec<Vec<[[f64; 2]; 3]>> {
        self.charts
            .iter()
            .map(|chart| {
                chart
                    .triangles
                    .iter()
                    .map(|&t| {
                        let t = t as usize * 3;
                        [self.corner_uv[t], self.corner_uv[t + 1], self.corner_uv[t + 2]]
                    })
                    .collect()
            })
            .collect()
    }
}

/// Segments and flattens. Charts sit in their own spaces, unplaced.
pub fn parameterize(
    input: &MeshInput<'_>,
    options: &ChartOptions,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Charting, UnwrapError> {
    chart::charts(input, options, checkpoint)
}

/// Places charts from [`parameterize`] into an atlas, in `charting.charts` order.
pub fn pack(
    charting: &Charting,
    options: &PackOptions<'_>,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Atlas, UnwrapError> {
    pack_items(charting.items(), options, checkpoint)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Unwrapped {
    pub charting: Charting,
    pub atlas: Atlas,
    /// Atlas uv in 0..1 of each corner, `3 * triangle + slot`.
    pub uv: Vec<[f64; 2]>,
    /// Split vertices: the input vertex each came from, and its chart.
    pub vertices: Vec<OutputVertex>,
    /// Three per triangle, in input order, indexing `vertices`.
    pub indices: Vec<u32>,
    /// Charts [`layout`] folded onto one shared texel: pieces under a texel at
    /// the density, and what did not fit when the mesh has more separate
    /// pieces than the atlas has room for.
    pub collapsed_charts: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputVertex {
    pub source: u32,
    pub chart: u32,
    pub uv: [f64; 2],
}

/// [`parameterize`], [`pack`], and the vertex split a renderer needs.
pub fn unwrap(
    input: &MeshInput<'_>,
    chart_options: &ChartOptions,
    pack_options: &PackOptions<'_>,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Unwrapped, UnwrapError> {
    let charting = parameterize(input, chart_options, checkpoint)?;
    let atlas = pack(&charting, pack_options, checkpoint)?;
    Ok(assemble(input, charting, atlas, 0))
}

fn assemble(input: &MeshInput<'_>, charting: Charting, atlas: Atlas, collapsed_charts: usize) -> Unwrapped {
    let triangles = charting.triangle_chart.len();
    let mut uv = vec![[0.0; 2]; triangles * 3];
    let (w, h) = (atlas.width as f64, atlas.height as f64);
    for t in 0..triangles {
        let placement = &atlas.placements[charting.triangle_chart[t] as usize];
        for k in 0..3 {
            let p = placement.map(charting.corner_uv[t * 3 + k]);
            uv[t * 3 + k] = [p[0] / w, p[1] / h];
        }
    }
    // One output vertex per distinct (source, chart, uv).
    let bits = |p: [f64; 2]| [(p[0] + 0.0).to_bits(), (p[1] + 0.0).to_bits()];
    let mut keys: Vec<(u32, u32, [u64; 2], u32)> = (0..triangles * 3)
        .map(|c| {
            (input.indices[c], charting.triangle_chart[c / 3], bits(uv[c]), c as u32)
        })
        .collect();
    keys.sort_unstable();
    let mut indices = vec![0u32; triangles * 3];
    let mut vertices: Vec<OutputVertex> = Vec::new();
    let mut at = 0;
    while at < keys.len() {
        let key = (keys[at].0, keys[at].1, keys[at].2);
        let index = vertices.len() as u32;
        vertices.push(OutputVertex { source: key.0, chart: key.1, uv: uv[keys[at].3 as usize] });
        while at < keys.len() && (keys[at].0, keys[at].1, keys[at].2) == key {
            indices[keys[at].3 as usize] = index;
            at += 1;
        }
    }
    Unwrapped { charting, atlas, uv, vertices, indices, collapsed_charts }
}

/// What [`layout`] is asked for.
#[derive(Clone, Copy, Debug)]
pub struct LayoutRequest {
    pub texels_per_unit: f64,
    /// Longest edge of the atlas.
    pub max_size: u32,
    /// Free texels between charts and at the atlas edge.
    pub padding: u32,
    /// As [`ChartOptions::weld_tolerance`].
    pub weld_tolerance: f64,
}

/// A density under which a layout tries coarser charts before settling.
const LAYOUT_DENSITY_FLOOR: f64 = 0.7;
/// Below this fill, with more than `LAYOUT_FEW_CHARTS` charts, padding is
/// taking the atlas: the charts are too many for the density.
const LAYOUT_WASTE_FILL: f64 = 0.3;
const LAYOUT_FEW_CHARTS: usize = 16;

/// Laplacian smoothing, each vertex moving halfway to its neighbours' mean.
/// Vertices of one weld end up at one place, so charting welds them again. A
/// piece finer than a texel may shrink to a point here, and then reads one
/// shared texel, which is all it could resolve anyway.
fn smoothed(input: &MeshInput<'_>, tolerance: f64, iterations: u32) -> Vec<[f64; 3]> {
    let weld = topology::weld_vertices(input.positions, tolerance);
    let mut current = input.positions.to_vec();
    let mut sum = vec![[0.0f64; 3]; current.len()];
    let mut count = vec![0u32; current.len()];
    for _ in 0..iterations {
        sum.fill([0.0; 3]);
        count.fill(0);
        for t in input.indices.chunks_exact(3) {
            let w = [weld[t[0] as usize], weld[t[1] as usize], weld[t[2] as usize]];
            for k in 0..3 {
                let v = w[k] as usize;
                for other in [w[(k + 1) % 3], w[(k + 2) % 3]] {
                    let p = current[other as usize];
                    for d in 0..3 {
                        sum[v][d] += p[d];
                    }
                    count[v] += 1;
                }
            }
        }
        for v in 0..current.len() {
            if count[v] > 0 {
                let n = count[v] as f64;
                for d in 0..3 {
                    current[v][d] = current[v][d] * 0.5 + sum[v][d] / n * 0.5;
                }
            }
        }
    }
    for v in 0..current.len() {
        current[v] = current[weld[v] as usize];
    }
    current
}

/// Smoothing shrinks closed pieces. Scales each chart back by how much its
/// vertices' extent shrank, so a chart made from a smoothed copy still spends
/// texels on the size of the real thing, not on its finer folds.
fn restore_extent(charting: &mut Charting, input: &MeshInput<'_>, proxy: &[[f64; 3]]) {
    let diagonal = |points: &[[f64; 3]], chart: &Chart| {
        let mut low = [f64::INFINITY; 3];
        let mut high = [f64::NEG_INFINITY; 3];
        for &t in &chart.triangles {
            for k in 0..3 {
                let p = points[input.indices[t as usize * 3 + k] as usize];
                for d in 0..3 {
                    low[d] = low[d].min(p[d]);
                    high[d] = high[d].max(p[d]);
                }
            }
        }
        topology::length(topology::sub(high, low))
    };
    for index in 0..charting.charts.len() {
        let (real, smooth) = {
            let chart = &charting.charts[index];
            (diagonal(input.positions, chart), diagonal(proxy, chart))
        };
        if !(smooth > 0.0) {
            continue;
        }
        let factor = real / smooth;
        let chart = &mut charting.charts[index];
        chart.area *= factor * factor;
        for &t in &chart.triangles {
            for k in 0..3 {
                let uv = &mut charting.corner_uv[t as usize * 3 + k];
                *uv = [uv[0] * factor, uv[1] * factor];
            }
        }
    }
}

/// Share of the real surface on charts that read less than one texel.
fn unresolved(charting: &Charting, input: &MeshInput<'_>, density_squared: f64) -> f64 {
    let area = |t: u32| {
        let p = [0, 1, 2].map(|k| input.positions[input.indices[t as usize * 3 + k] as usize]);
        topology::length(topology::cross(topology::sub(p[1], p[0]), topology::sub(p[2], p[0]))) * 0.5
    };
    let (mut lost, mut total) = (0.0, 0.0);
    for chart in &charting.charts {
        let real: f64 = chart.triangles.iter().map(|&t| area(t)).sum();
        total += real;
        if chart.area * density_squared < 1.0 {
            lost += real;
        }
    }
    if total > 0.0 { lost / total } else { 0.0 }
}

/// Share of the surface a layout may leave unresolved to save padding.
const LAYOUT_MAX_UNRESOLVED: f64 = 0.05;

fn mean_edge(input: &MeshInput<'_>) -> f64 {
    let (mut total, mut count) = (0.0, 0usize);
    for t in input.indices.chunks_exact(3) {
        for k in 0..3 {
            let (a, b) = (input.positions[t[k] as usize], input.positions[t[(k + 1) % 3] as usize]);
            total += topology::length(topology::sub(a, b));
            count += 1;
        }
    }
    total / count.max(1) as f64
}

/// A chart layout for lighting: always produces one, whatever the geometry.
///
/// Non-manifold, degenerate and disconnected input all degrade rather than
/// fail. When padding takes the atlas or the density is out of reach, charts
/// come from a smoothed copy of the surface, more smoothed each try: detail
/// finer than a texel has nothing to say about where charts go, so a dense or
/// rough mesh does not become thousands of islands. The density then counts the
/// surface at that scale, not its finer folds. What still does not fit
/// lowers the density, read back from [`Atlas::texels_per_unit`]. As a
/// last resort the smallest charts share a single texel too, counted in
/// [`Unwrapped::collapsed_charts`]; only a `max_size` with no room for even one
/// chart is an error, along with non-finite or out-of-range input.
///
/// To lay out several layers of one model together, concatenate them: output
/// is in input order, so each layer's triangles are its slice of
/// [`Unwrapped::indices`], and [`OutputVertex::source`] is the vertex remap.
///
/// Pure and deterministic: the same input gives the same bits, with no hashing
/// or thread order involved. Runs on the calling thread; call it from a worker.
pub fn layout(
    input: &MeshInput<'_>,
    request: &LayoutRequest,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Unwrapped, UnwrapError> {
    let target = AtlasTarget::UpTo { texels_per_unit: request.texels_per_unit, max_size: request.max_size };
    let options = PackOptions { target, padding: request.padding, obstacles: &[] };
    // Smoothing diffuses over about the square root of its steps, in edges: this
    // many reach one texel. Facets a texel or larger are all the detail there is.
    // The scale that matters is a padded cell: a fold smaller than that costs
    // more padding as a chart than it could ever show.
    let cell = request.padding as f64 + 1.0;
    let edges_per_cell = cell / (request.texels_per_unit * mean_edge(input)).max(1.0e-9);
    let reach = (edges_per_cell * edges_per_cell).clamp(1.0, 64.0) as u32;
    let stages = [(0, 1.25, 0.5), (0, 1.6, -2.0), (reach, 1.6, -2.0), (reach * 4, 2.5, -2.0)];
    let mut best: Option<(Charting, Atlas, usize)> = None;
    let mut last = None;
    let padded = (request.padding as f64 + 1.0) * (request.padding as f64 + 1.0);
    let density_squared = request.texels_per_unit * request.texels_per_unit;
    for (stage, (steps, max_stretch, crease_cos)) in stages.into_iter().enumerate() {
        let chart_options = ChartOptions { crease_cos, max_stretch, weld_tolerance: request.weld_tolerance };
        let charting = if steps == 0 {
            parameterize(input, &chart_options, checkpoint)?
        } else {
            let proxy = smoothed(input, request.weld_tolerance, steps);
            let charted = MeshInput { positions: &proxy, ..*input };
            let mut charting = parameterize(&charted, &chart_options, checkpoint)?;
            restore_extent(&mut charting, input, &proxy);
            charting
        };
        // From a smoothed copy, pieces under one texel share a single texel:
        // a chart of their own would be all padding and resolve nothing.
        let (charting, folded) = if steps == 0 {
            (charting, 0)
        } else {
            let tiny = charting.charts.iter().filter(|c| c.area * density_squared < 1.0).count();
            if tiny > 1 {
                let keep = charting.charts.len() - tiny + 1;
                collapse_smallest(&charting, keep)
            } else {
                (charting, 0)
            }
        };
        // Clearly all padding: not worth a pack when coarser charts are next.
        // Folding is only worth it while what it hides is a sliver of the
        // surface; a stage that hides more never replaces an earlier one.
        let honest = steps == 0 || unresolved(&charting, input, density_squared) <= LAYOUT_MAX_UNRESOLVED;
        if !honest && best.is_some() {
            continue;
        }
        let surface: f64 = charting.charts.iter().map(|c| c.area).sum::<f64>() * density_squared;
        let cost = charting.charts.len() as f64 * padded;
        // The first stage always packs: it is the honest answer the coarser
        // stages have to beat.
        if stage > 0
            && stage + 1 < stages.len()
            && charting.charts.len() > LAYOUT_FEW_CHARTS
            && surface < (surface + cost) * LAYOUT_WASTE_FILL * 0.5
        {
            last = Some(charting);
            continue;
        }
        match pack(&charting, &options, checkpoint) {
            Ok(atlas) => {
                let dense = |a: &Atlas| a.texels_per_unit >= request.texels_per_unit * LAYOUT_DENSITY_FLOOR;
                let wasteful = charting.charts.len() > LAYOUT_FEW_CHARTS && atlas.fill < LAYOUT_WASTE_FILL;
                let done = dense(&atlas) && !wasteful;
                let better = best.as_ref().map_or(true, |b| match (dense(&atlas), dense(&b.1)) {
                    (true, true) => charting.charts.len() < b.0.charts.len(),
                    (false, false) => atlas.texels_per_unit > b.1.texels_per_unit,
                    (now, _) => now,
                });
                if better {
                    best = Some((charting, atlas, folded));
                }
                if done {
                    break;
                }
            }
            Err(UnwrapError::AtlasTooSmall { .. }) => last = Some(charting),
            Err(other) => return Err(other),
        }
    }
    if let Some((charting, atlas, folded)) = best {
        return Ok(assemble(input, charting, atlas, folded));
    }
    let Some(mut charting) = last else {
        return Err(UnwrapError::InvalidInput("the layout has no stages".into()));
    };
    // More pieces than room, however coarse: keep the largest, and let the rest
    // read one shared texel. The room halves until the pack holds.
    let cell = (request.padding as f64 + 1.0).max(1.0);
    let side = (request.max_size as f64 - 2.0 * request.padding as f64).max(0.0);
    let mut room = (((side + request.padding as f64) / cell).floor() as usize).pow(2).max(1);
    loop {
        let (reduced, collapsed) = collapse_smallest(&charting, room);
        match pack(&reduced, &options, checkpoint) {
            Ok(atlas) => return Ok(assemble(input, reduced, atlas, collapsed)),
            Err(UnwrapError::AtlasTooSmall { .. }) if room > 1 => {
                room /= 2;
                charting = reduced;
            }
            Err(other) => return Err(other),
        }
    }
}

/// Keeps the `room - 1` largest charts and maps all the others to one point
/// chart, listed last. Returns how many were folded in.
fn collapse_smallest(charting: &Charting, room: usize) -> (Charting, usize) {
    let count = charting.charts.len();
    if count <= room {
        return (charting.clone(), 0);
    }
    let mut order: Vec<u32> = (0..count as u32).collect();
    order.sort_unstable_by(|&a, &b| {
        charting.charts[b as usize].area.total_cmp(&charting.charts[a as usize].area).then(a.cmp(&b))
    });
    let keep = room.saturating_sub(1);
    let mut kept = vec![false; count];
    for &c in &order[..keep] {
        kept[c as usize] = true;
    }
    let mut result = Charting {
        corner_uv: charting.corner_uv.clone(),
        triangle_chart: charting.triangle_chart.clone(),
        charts: Vec::with_capacity(room),
    };
    let mut remap = vec![0u32; count];
    let mut point = Chart { triangles: Vec::new(), area: 0.0, flat: true, stretch: 1.0 };
    let mut collapsed = 0;
    for (c, chart) in charting.charts.iter().enumerate() {
        if kept[c] {
            remap[c] = result.charts.len() as u32;
            result.charts.push(chart.clone());
        } else {
            collapsed += 1;
            remap[c] = u32::MAX;
            point.triangles.extend_from_slice(&chart.triangles);
        }
    }
    let point_index = result.charts.len() as u32;
    for &t in &point.triangles {
        for k in 0..3 {
            result.corner_uv[t as usize * 3 + k] = [0.0, 0.0];
        }
    }
    result.charts.push(point);
    for chart in result.triangle_chart.iter_mut() {
        let mapped = remap[*chart as usize];
        *chart = if mapped == u32::MAX { point_index } else { mapped };
    }
    (result, collapsed)
}
