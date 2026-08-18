//! Blender-free mesh ingestion, normalization and surface sampling for
//! SkinTokens.
//!
//! This mirrors the inference-only path in upstream `BpyParser.load`,
//! `AugmentAffine([-1, 1])`, `AugmentNormalize`, and
//! `SamplerMix(num_samples=54000, num_vertex_samples=16384)`.  It intentionally
//! does not depend on Blender, trimesh, NumPy, or SciPy.

use crate::skin_tokens::{
    SKIN_TOKENS_INFERENCE_VERTEX_SAMPLE_COUNT, SKIN_TOKENS_SAMPLE_COUNT,
};
use crate::{DiffusionError, Result};
use makepad_gltf::{decode_mesh_primitive, load_gltf_from_bytes, GltfNode, LoadedGltf};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

#[derive(Clone, Debug, PartialEq)]
pub struct SkinTokensMeshPart {
    pub node_index: Option<usize>,
    pub mesh_index: usize,
    pub primitive_index: usize,
    pub vertex_start: usize,
    pub vertex_count: usize,
    pub index_start: usize,
    pub index_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkinTokensNormalization {
    pub center: [f32; 3],
    /// Float32 scale stored by upstream's affine matrix.
    pub scale: f32,
    /// Float32 translation stored by upstream's affine matrix.  NumPy applies
    /// `source_f64 * scale_f32 + translation_f32` in float64.
    pub translation: [f32; 3],
}

impl SkinTokensNormalization {
    pub fn normalize(&self, point: [f32; 3]) -> [f32; 3] {
        self.normalize_f64(point.map(f64::from)).map(|value| value as f32)
    }

    fn normalize_f64(&self, point: [f64; 3]) -> [f64; 3] {
        let scale = self.scale as f64;
        [
            point[0] * scale + self.translation[0] as f64,
            point[1] * scale + self.translation[1] as f64,
            point[2] * scale + self.translation[2] as f64,
        ]
    }

    pub fn denormalize(&self, point: [f32; 3]) -> [f32; 3] {
        let scale = self.scale as f64;
        [
            ((point[0] as f64 - self.translation[0] as f64) / scale) as f32,
            ((point[1] as f64 - self.translation[1] as f64) / scale) as f32,
            ((point[2] as f64 - self.translation[2] as f64) / scale) as f32,
        ]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkinTokensMesh {
    /// Source/world-space merged vertices.  Parts retain the ranges needed to
    /// write predicted weights back to each original primitive.
    pub source_positions: Vec<[f32; 3]>,
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub face_normals: Vec<[f32; 3]>,
    pub vertex_normals: Vec<[f32; 3]>,
    pub parts: Vec<SkinTokensMeshPart>,
    pub normalization: SkinTokensNormalization,
    // The model consumes f32, but official surface selection and barycentric
    // interpolation run on BpyParser's float64 vertices.  Retaining this
    // internal view avoids rare seeded face-pick changes near CDF boundaries.
    normalized_f64: Vec<[f64; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkinTokensSamples {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// `Some(vertex)` for the original-vertex prefix, `None` for surface
    /// samples.  The model itself does not consume this; parity diagnostics do.
    pub source_vertices: Vec<Option<usize>>,
    pub source_faces: Vec<Option<usize>>,
}

impl SkinTokensSamples {
    /// Interleaved `[x, y, z, nx, ny, nz]` rows consumed by both native
    /// SkinTokens condition encoders and the per-joint skin decoder.
    pub fn condition_f32(&self) -> Result<Vec<f32>> {
        if self.positions.len() != self.normals.len() {
            return Err(DiffusionError::workflow(format!(
                "SkinTokens samples have {} positions and {} normals",
                self.positions.len(),
                self.normals.len(),
            )));
        }
        let mut condition = Vec::with_capacity(self.positions.len() * 6);
        for (&position, &normal) in self.positions.iter().zip(&self.normals) {
            if position
                .iter()
                .chain(&normal)
                .any(|value| !value.is_finite())
            {
                return Err(DiffusionError::workflow(
                    "SkinTokens sample condition contains non-finite values",
                ));
            }
            condition.extend_from_slice(&position);
            condition.extend_from_slice(&normal);
        }
        Ok(condition)
    }
}

impl SkinTokensMesh {
    pub fn from_glb(bytes: &[u8]) -> Result<Self> {
        let loaded = load_gltf_from_bytes(bytes, None).map_err(|err| {
            DiffusionError::workflow(format!("SkinTokens GLB load failed: {err}"))
        })?;
        Self::from_loaded(&loaded)
    }

    pub fn from_loaded(loaded: &LoadedGltf) -> Result<Self> {
        let nodes = loaded.document.nodes_slice();
        let worlds = node_world_matrices(nodes)?;
        let mut occurrences: Vec<(Option<usize>, usize, [f64; 16])> = nodes
            .iter()
            .enumerate()
            .filter_map(|(node_index, node)| {
                node.mesh
                    .map(|mesh_index| (Some(node_index), mesh_index, worlds[node_index]))
            })
            .collect();
        if occurrences.is_empty() {
            occurrences.extend(
                loaded
                    .document
                    .meshes_slice()
                    .iter()
                    .enumerate()
                    .map(|(mesh_index, _)| (None, mesh_index, identity4())),
            );
        }
        if occurrences.is_empty() {
            return Err(DiffusionError::workflow(
                "SkinTokens input has no mesh primitives",
            ));
        }

        let mut source_positions_f64 = Vec::new();
        let mut indices = Vec::new();
        let mut parts = Vec::new();
        for (node_index, mesh_index, world) in occurrences {
            let mesh = loaded
                .document
                .meshes_slice()
                .get(mesh_index)
                .ok_or_else(|| {
                    DiffusionError::workflow(format!(
                        "SkinTokens node references missing mesh {mesh_index}",
                    ))
                })?;
            for primitive_index in 0..mesh.primitives.len() {
                let decoded = decode_mesh_primitive(loaded, mesh_index, primitive_index)
                    .map_err(|err| {
                        DiffusionError::workflow(format!(
                            "SkinTokens mesh {mesh_index}/{primitive_index}: {err}",
                        ))
                    })?;
                let vertex_start = source_positions_f64.len();
                let index_start = indices.len();
                source_positions_f64.extend(
                    decoded
                        .positions
                        .iter()
                        // Blender's glTF importer converts the spec's Y-up
                        // coordinates to Blender Z-up before BpyParser reads
                        // object vertices: (x, y, z) -> (x, -z, y).
                        .map(|point| gltf_to_bpy(transform_point(world, *point))),
                );
                if decoded.indices.len() % 3 != 0 {
                    return Err(DiffusionError::workflow(format!(
                        "SkinTokens mesh {mesh_index}/{primitive_index} has a non-triangle index buffer",
                    )));
                }
                // BpyParser reconstructs every imported polygon loop from the
                // first id in CPython's `list(set(sorted(nodes)))`.  For
                // triangles this is a cyclic rotation (winding is unchanged),
                // but it affects the exact face array, cumulative areas and
                // therefore seeded surface sampling.  Canonicalize here before
                // merging parts.
                for triangle in decoded.indices.chunks_exact(3) {
                    let triangle = rotate_triangle_like_bpy([
                        triangle[0],
                        triangle[1],
                        triangle[2],
                    ]);
                    indices.extend(
                        triangle
                            .into_iter()
                            .map(|index| index + vertex_start as u32),
                    );
                }
                parts.push(SkinTokensMeshPart {
                    node_index,
                    mesh_index,
                    primitive_index,
                    vertex_start,
                    vertex_count: decoded.positions.len(),
                    index_start,
                    index_count: decoded.indices.len(),
                });
            }
        }
        if source_positions_f64.is_empty() || indices.is_empty() || indices.len() % 3 != 0 {
            return Err(DiffusionError::workflow(
                "SkinTokens input has no non-empty triangle geometry",
            ));
        }

        let normalization = normalization(&source_positions_f64)?;
        let normalized_f64 = source_positions_f64
            .iter()
            .map(|point| normalization.normalize_f64(*point))
            .collect::<Vec<_>>();
        let source_positions = source_positions_f64
            .iter()
            .map(|point| point.map(|value| value as f32))
            .collect::<Vec<_>>();
        let positions = normalized_f64
            .iter()
            .map(|point| point.map(|value| value as f32))
            .collect::<Vec<_>>();
        let (face_normals, vertex_normals) = angle_weighted_normals(&normalized_f64, &indices)?;
        Ok(Self {
            source_positions,
            positions,
            indices,
            face_normals,
            vertex_normals,
            parts,
            normalization,
            normalized_f64,
        })
    }

    /// Exact legacy-MT19937 sampling contract used by `np.random.seed(seed)`
    /// plus upstream `sample_vertex_groups`.  The production inference call
    /// is surface-only: upstream's configured `num_vertex_samples=16384` is
    /// not forwarded on this unrigged-mesh branch.  The returned values are
    /// f32 because the reference converts the NumPy arrays with `.float()`
    /// before entering either neural encoder.
    pub fn sample(&self, seed: u32) -> Result<SkinTokensSamples> {
        // `AugmentAffine.transform` calls `np.random.rand()` for both its
        // random-scale and random-shift predicates even when their default
        // probabilities are zero.  These two draws precede SamplerMix in the
        // official predict transform.
        self.sample_counts_after_draws(
            seed,
            SKIN_TOKENS_SAMPLE_COUNT,
            SKIN_TOKENS_INFERENCE_VERTEX_SAMPLE_COUNT,
            2,
        )
    }

    pub fn sample_counts(
        &self,
        seed: u32,
        sample_count: usize,
        vertex_sample_count: usize,
    ) -> Result<SkinTokensSamples> {
        self.sample_counts_after_draws(seed, sample_count, vertex_sample_count, 0)
    }

    fn sample_counts_after_draws(
        &self,
        seed: u32,
        sample_count: usize,
        vertex_sample_count: usize,
        discarded_random_draws: usize,
    ) -> Result<SkinTokensSamples> {
        if sample_count == 0 {
            return Err(DiffusionError::workflow(
                "SkinTokens sample count must be non-zero",
            ));
        }
        let vertex_count = vertex_sample_count
            .min(sample_count)
            .min(self.positions.len());
        let mut rng = NumpyMt19937::new(seed);
        for _ in 0..discarded_random_draws {
            let _ = rng.random_f64();
        }
        let mut permutation: Vec<usize> = (0..self.positions.len()).collect();
        rng.shuffle(&mut permutation);

        let surface_count = sample_count - vertex_count;
        let mut positions = Vec::with_capacity(sample_count);
        let mut normals = Vec::with_capacity(sample_count);
        let mut source_vertices = Vec::with_capacity(sample_count);
        let mut source_faces = Vec::with_capacity(sample_count);
        for &vertex in permutation.iter().take(vertex_count) {
            positions.push(self.positions[vertex]);
            normals.push(self.vertex_normals[vertex]);
            source_vertices.push(Some(vertex));
            source_faces.push(None);
        }

        let mut cumulative = Vec::with_capacity(self.indices.len() / 3);
        let mut total = 0.0f64;
        for tri in self.indices.chunks_exact(3) {
            let a = self.normalized_f64[tri[0] as usize];
            let b = self.normalized_f64[tri[1] as usize];
            let c = self.normalized_f64[tri[2] as usize];
            total += length3(cross3(sub3(b, a), sub3(c, a)));
            cumulative.push(total);
        }
        if !total.is_finite() || total <= 0.0 {
            return Err(DiffusionError::workflow(
                "SkinTokens mesh has zero total triangle area",
            ));
        }

        // NumPy creates the complete face-pick vector first, followed by the
        // complete `[surface_count, 2, 1]` barycentric random tensor.  Keep
        // those RNG phases separate; interleaving them changes every sample.
        let mut picked_faces = Vec::with_capacity(surface_count);
        for _ in 0..surface_count {
            let target = rng.random_f64() * total;
            let face = cumulative.partition_point(|value| *value < target);
            picked_faces.push(face.min(cumulative.len() - 1));
        }
        let mut barycentric = Vec::with_capacity(surface_count);
        for _ in 0..surface_count {
            let mut u = rng.random_f64();
            let mut v = rng.random_f64();
            if u + v > 1.0 {
                u = (u - 1.0).abs();
                v = (v - 1.0).abs();
            }
            barycentric.push((u, v));
        }
        for (sample, &(u, v)) in picked_faces.iter().zip(&barycentric) {
            let tri = &self.indices[sample * 3..sample * 3 + 3];
            let a = self.normalized_f64[tri[0] as usize];
            let b = self.normalized_f64[tri[1] as usize];
            let c = self.normalized_f64[tri[2] as usize];
            let point = add3(a, add3(scale3(sub3(b, a), u), scale3(sub3(c, a), v)));
            positions.push([point[0] as f32, point[1] as f32, point[2] as f32]);
            normals.push(self.face_normals[*sample]);
            source_vertices.push(None);
            source_faces.push(Some(*sample));
        }
        Ok(SkinTokensSamples {
            positions,
            normals,
            source_vertices,
            source_faces,
        })
    }

    /// Transfer the decoder's dense weights from the sampled surface back to
    /// every original mesh vertex using the exact reference boundary:
    /// eight nearest samples, inverse-distance weights `1 / (d + 1e-8)`, and
    /// a weighted sum per joint. The independent sigmoid columns emitted by
    /// SkinVAE are deliberately not normalized across joints here; top-four
    /// normalization belongs to the GLB export boundary.
    pub fn transfer_sample_weights(
        &self,
        samples: &SkinTokensSamples,
        sample_weights: &[f32],
        joint_count: usize,
    ) -> Result<Vec<f64>> {
        self.transfer_sample_weights_with_progress(samples, sample_weights, joint_count, None)
    }

    /// Cancellable/progress-reporting form of [`Self::transfer_sample_weights`].
    pub fn transfer_sample_weights_with_progress(
        &self,
        samples: &SkinTokensSamples,
        sample_weights: &[f32],
        joint_count: usize,
        mut progress: Option<crate::ProgressHook<'_>>,
    ) -> Result<Vec<f64>> {
        if samples.positions.is_empty() {
            return Err(DiffusionError::workflow(
                "SkinTokens weight transfer requires sampled points",
            ));
        }
        if joint_count == 0 {
            return Err(DiffusionError::workflow(
                "SkinTokens weight transfer requires at least one joint",
            ));
        }
        let expected = samples
            .positions
            .len()
            .checked_mul(joint_count)
            .ok_or_else(|| DiffusionError::workflow("SkinTokens weight matrix size overflow"))?;
        if sample_weights.len() != expected {
            return Err(DiffusionError::workflow(format!(
                "SkinTokens sampled weights contain {} values, expected {} samples x {} joints = {expected}",
                sample_weights.len(),
                samples.positions.len(),
                joint_count,
            )));
        }
        if samples
            .positions
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
            || self
                .normalized_f64
                .iter()
                .flatten()
                .any(|value| !value.is_finite())
        {
            return Err(DiffusionError::workflow(
                "SkinTokens weight transfer positions contain non-finite values",
            ));
        }
        if sample_weights
            .iter()
            .any(|weight| !weight.is_finite() || *weight < 0.0)
        {
            return Err(DiffusionError::workflow(
                "SkinTokens sampled weights contain non-finite or negative values",
            ));
        }

        crate::emit_progress(&mut progress, "transfer-build-index", 0.0)?;
        let tree = KdTree::new(&samples.positions);
        crate::emit_progress(&mut progress, "transfer-build-index", 1.0)?;
        let neighbor_count = 8.min(samples.positions.len());
        let output_len = self
            .normalized_f64
            .len()
            .checked_mul(joint_count)
            .ok_or_else(|| DiffusionError::workflow("SkinTokens output weight size overflow"))?;
        // cKDTree distances and NumPy's multiply/reduction are float64. Keep
        // that precision through top-four normalization; glTF converts only
        // the four final lanes to FLOAT/f32. Casting here was visually tiny
        // but changed nearly half of the oracle's exported weight lanes.
        let mut output = vec![0.0f64; output_len];
        let mut neighbors = Vec::with_capacity(neighbor_count);
        let mut interpolation = Vec::with_capacity(neighbor_count);
        // SciPy's cKDTree stores the sampled f32 points as float64 internally,
        // but upstream queries it with BpyParser's normalized ORIGINAL
        // vertices, which are still float64. Do not query with `positions`:
        // that is the model-facing f32 cast and can change a boundary neighbor
        // (and therefore the exported top-four skin) by one ULP.
        for (vertex_index, &point) in self.normalized_f64.iter().enumerate() {
            tree.nearest(point, neighbor_count, &mut neighbors);
            interpolation.clear();
            let mut total = 0.0f64;
            for neighbor in &neighbors {
                let weight = 1.0 / (neighbor.distance2.sqrt() + 1.0e-8);
                interpolation.push(weight);
                total += weight;
            }
            let row = &mut output
                [vertex_index * joint_count..(vertex_index + 1) * joint_count];
            for (joint, out) in row.iter_mut().enumerate() {
                let mut value = 0.0f64;
                for (neighbor, interpolation_weight) in neighbors.iter().zip(&interpolation) {
                    // NumPy promotes the float32 sigmoid values to float64
                    // when multiplying by cKDTree's float64 distances, then
                    // reduces the eight-neighbor axis before the export cast.
                    value += sample_weights[neighbor.index * joint_count + joint] as f64
                        * (*interpolation_weight / total);
                }
                *out = value;
            }
            if vertex_index % 256 == 255 || vertex_index + 1 == self.normalized_f64.len() {
                crate::emit_progress(
                    &mut progress,
                    "transfer-skin-weights",
                    (vertex_index + 1) as f64 / self.normalized_f64.len() as f64,
                )?;
            }
        }
        Ok(output)
    }
}

#[derive(Clone, Copy, Debug)]
struct KdNeighbor {
    distance2: f64,
    index: usize,
}

impl PartialEq for KdNeighbor {
    fn eq(&self, other: &Self) -> bool {
        self.distance2.to_bits() == other.distance2.to_bits() && self.index == other.index
    }
}

impl Eq for KdNeighbor {}

impl PartialOrd for KdNeighbor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for KdNeighbor {
    fn cmp(&self, other: &Self) -> Ordering {
        self.distance2
            .total_cmp(&other.distance2)
            .then_with(|| self.index.cmp(&other.index))
    }
}

#[derive(Clone, Copy, Debug)]
struct KdNode {
    point: usize,
    axis: usize,
    left: Option<usize>,
    right: Option<usize>,
}

struct KdTree<'a> {
    points: &'a [[f32; 3]],
    nodes: Vec<KdNode>,
    root: Option<usize>,
}

impl<'a> KdTree<'a> {
    fn new(points: &'a [[f32; 3]]) -> Self {
        let mut indices = (0..points.len()).collect::<Vec<_>>();
        let mut nodes = Vec::with_capacity(points.len());
        let root = Self::build(points, &mut indices, 0, &mut nodes);
        Self {
            points,
            nodes,
            root,
        }
    }

    fn build(
        points: &[[f32; 3]],
        indices: &mut [usize],
        depth: usize,
        nodes: &mut Vec<KdNode>,
    ) -> Option<usize> {
        if indices.is_empty() {
            return None;
        }
        let axis = depth % 3;
        let middle = indices.len() / 2;
        indices.select_nth_unstable_by(middle, |left, right| {
            points[*left][axis]
                .total_cmp(&points[*right][axis])
                .then_with(|| left.cmp(right))
        });
        let (left, rest) = indices.split_at_mut(middle);
        let (point, right) = rest.split_first_mut().expect("non-empty median");
        let node_index = nodes.len();
        nodes.push(KdNode {
            point: *point,
            axis,
            left: None,
            right: None,
        });
        let left_node = Self::build(points, left, depth + 1, nodes);
        let right_node = Self::build(points, right, depth + 1, nodes);
        nodes[node_index].left = left_node;
        nodes[node_index].right = right_node;
        Some(node_index)
    }

    fn nearest(&self, point: [f64; 3], count: usize, output: &mut Vec<KdNeighbor>) {
        let mut heap = BinaryHeap::with_capacity(count + 1);
        self.query(self.root, point, count, &mut heap);
        output.clear();
        output.extend(heap.into_vec());
        output.sort_unstable_by(|left, right| {
            left.distance2
                .total_cmp(&right.distance2)
                .then_with(|| left.index.cmp(&right.index))
        });
    }

    fn query(
        &self,
        node: Option<usize>,
        point: [f64; 3],
        count: usize,
        heap: &mut BinaryHeap<KdNeighbor>,
    ) {
        let Some(node_index) = node else {
            return;
        };
        let node = self.nodes[node_index];
        let candidate_point = self.points[node.point];
        let dx = point[0] - candidate_point[0] as f64;
        let dy = point[1] - candidate_point[1] as f64;
        let dz = point[2] - candidate_point[2] as f64;
        let candidate = KdNeighbor {
            distance2: dx * dx + dy * dy + dz * dz,
            index: node.point,
        };
        if heap.len() < count {
            heap.push(candidate);
        } else if heap.peek().is_some_and(|worst| candidate < *worst) {
            heap.pop();
            heap.push(candidate);
        }

        let delta = point[node.axis] - candidate_point[node.axis] as f64;
        let (near, far) = if delta <= 0.0 {
            (node.left, node.right)
        } else {
            (node.right, node.left)
        };
        self.query(near, point, count, heap);
        let worst = heap.peek().map(|neighbor| neighbor.distance2);
        if heap.len() < count || worst.is_some_and(|distance2| delta * delta <= distance2) {
            self.query(far, point, count, heap);
        }
    }
}

fn rotate_triangle_like_bpy(triangle: [u32; 3]) -> [u32; 3] {
    // Upstream's BpyParser chooses `first = list(set(sorted(nodes)))[0]`.
    // CPython's compact eight-slot set table therefore matters: integer hash
    // is the integer itself, and iteration returns the first occupied slot,
    // not necessarily the minimum value (e.g. {6,7,8} -> [8,6,7]).  Emulate
    // the three-entry insertion table instead of depending on Rust HashSet's
    // randomized order.
    let mut sorted = triangle;
    sorted.sort_unstable();
    let mut table = [None; 8];
    for value in sorted {
        let mut slot = value as usize & 7;
        let mut perturb = value as usize;
        loop {
            match table[slot] {
                Some(found) if found == value => break,
                Some(_) => {
                    perturb >>= 5;
                    slot = (slot * 5 + 1 + perturb) & 7;
                }
                None => {
                    table[slot] = Some(value);
                    break;
                }
            }
        }
    }
    let first = table.into_iter().flatten().next().unwrap_or(triangle[0]);
    match triangle.iter().position(|value| *value == first).unwrap_or(0) {
        1 => [triangle[1], triangle[2], triangle[0]],
        2 => [triangle[2], triangle[0], triangle[1]],
        _ => triangle,
    }
}

fn normalization(points: &[[f64; 3]]) -> Result<SkinTokensNormalization> {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for point in points {
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    let center_f64 = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let extent = (max[0] - min[0])
        .max(max[1] - min[1])
        .max(max[2] - min[2]);
    if !extent.is_finite() || extent <= 0.0 {
        return Err(DiffusionError::workflow(
            "SkinTokens cannot normalize a zero-extent mesh",
        ));
    }
    // AugmentAffine builds translation and scale as separate float32 4x4
    // matrices, then multiplies them in float32 before applying the result to
    // float64 vertices.  Preserve those rounding points exactly.
    let scale = (2.0 / extent) as f32;
    let center = center_f64.map(|value| value as f32);
    let translation = center.map(|value| scale * -value);
    Ok(SkinTokensNormalization {
        center,
        scale,
        translation,
    })
}

fn angle_weighted_normals(
    positions: &[[f64; 3]],
    indices: &[u32],
) -> Result<(Vec<[f32; 3]>, Vec<[f32; 3]>)> {
    let mut face_normals = Vec::with_capacity(indices.len() / 3);
    let mut vertex_sums = vec![[0.0f64; 3]; positions.len()];
    for tri in indices.chunks_exact(3) {
        let ids = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        if ids.iter().any(|id| *id >= positions.len()) {
            return Err(DiffusionError::workflow(
                "SkinTokens mesh index exceeds its vertex buffer",
            ));
        }
        let p = [
            positions[ids[0]],
            positions[ids[1]],
            positions[ids[2]],
        ];
        let normal64 = unit3(cross3(sub3(p[1], p[0]), sub3(p[2], p[0])));
        face_normals.push([
            normal64[0] as f32,
            normal64[1] as f32,
            normal64[2] as f32,
        ]);
        if length3(normal64) <= 0.5 {
            continue;
        }
        for corner in 0..3 {
            let a = unit3(sub3(p[(corner + 1) % 3], p[corner]));
            let b = unit3(sub3(p[(corner + 2) % 3], p[corner]));
            let angle = dot3(a, b).clamp(-1.0, 1.0).acos();
            vertex_sums[ids[corner]] = add3(
                vertex_sums[ids[corner]],
                scale3(normal64, angle),
            );
        }
    }
    let vertex_normals = vertex_sums
        .into_iter()
        .map(unit3)
        .map(|normal| [normal[0] as f32, normal[1] as f32, normal[2] as f32])
        .collect();
    Ok((face_normals, vertex_normals))
}

fn identity4() -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
        0.0, 1.0,
    ]
}

fn mul4(a: [f64; 16], b: [f64; 16]) -> [f64; 16] {
    let mut out = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            out[column * 4 + row] = (0..4)
                .map(|k| a[k * 4 + row] * b[column * 4 + k])
                .sum();
        }
    }
    out
}

fn node_matrix(node: &GltfNode) -> [f64; 16] {
    if let Some(matrix) = node.matrix {
        return matrix.map(f64::from);
    }
    let t = node.translation.unwrap_or([0.0; 3]).map(f64::from);
    let q = node
        .rotation
        .unwrap_or([0.0, 0.0, 0.0, 1.0])
        .map(f64::from);
    let s = node.scale.unwrap_or([1.0; 3]).map(f64::from);
    let [x, y, z, w] = q;
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let xz = x * z;
    let yz = y * z;
    let wx = w * x;
    let wy = w * y;
    let wz = w * z;
    [
        (1.0 - 2.0 * (yy + zz)) * s[0],
        (2.0 * (xy + wz)) * s[0],
        (2.0 * (xz - wy)) * s[0],
        0.0,
        (2.0 * (xy - wz)) * s[1],
        (1.0 - 2.0 * (xx + zz)) * s[1],
        (2.0 * (yz + wx)) * s[1],
        0.0,
        (2.0 * (xz + wy)) * s[2],
        (2.0 * (yz - wx)) * s[2],
        (1.0 - 2.0 * (xx + yy)) * s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

fn node_world_matrices(nodes: &[GltfNode]) -> Result<Vec<[f64; 16]>> {
    let mut parents = vec![None; nodes.len()];
    for (parent, node) in nodes.iter().enumerate() {
        for child in node.children.as_deref().unwrap_or(&[]) {
            if *child >= nodes.len() {
                return Err(DiffusionError::workflow(format!(
                    "SkinTokens GLB node {parent} has out-of-range child {child}",
                )));
            }
            if parents[*child].replace(parent).is_some() {
                return Err(DiffusionError::workflow(format!(
                    "SkinTokens GLB node {child} has multiple parents",
                )));
            }
        }
    }
    fn resolve(
        index: usize,
        nodes: &[GltfNode],
        parents: &[Option<usize>],
        state: &mut [u8],
        worlds: &mut [[f64; 16]],
    ) -> Result<[f64; 16]> {
        match state[index] {
            2 => return Ok(worlds[index]),
            1 => {
                return Err(DiffusionError::workflow(
                    "SkinTokens GLB node hierarchy contains a cycle",
                ))
            }
            _ => {}
        }
        state[index] = 1;
        let local = node_matrix(&nodes[index]);
        let world = match parents[index] {
            Some(parent) => mul4(resolve(parent, nodes, parents, state, worlds)?, local),
            None => local,
        };
        worlds[index] = world;
        state[index] = 2;
        Ok(world)
    }
    let mut worlds = vec![identity4(); nodes.len()];
    let mut state = vec![0u8; nodes.len()];
    for index in 0..nodes.len() {
        resolve(index, nodes, &parents, &mut state, &mut worlds)?;
    }
    Ok(worlds)
}

fn transform_point(matrix: [f64; 16], point: [f32; 3]) -> [f64; 3] {
    let p = point.map(f64::from);
    [
        matrix[0] * p[0] + matrix[4] * p[1] + matrix[8] * p[2] + matrix[12],
        matrix[1] * p[0] + matrix[5] * p[1] + matrix[9] * p[2] + matrix[13],
        matrix[2] * p[0] + matrix[6] * p[1] + matrix[10] * p[2] + matrix[14],
    ]
}

fn gltf_to_bpy(point: [f64; 3]) -> [f64; 3] {
    [point[0], -point[2], point[1]]
}
fn add3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale3(v: [f64; 3], scale: f64) -> [f64; 3] {
    [v[0] * scale, v[1] * scale, v[2] * scale]
}
fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length3(v: [f64; 3]) -> f64 {
    dot3(v, v).sqrt()
}
fn unit3(v: [f64; 3]) -> [f64; 3] {
    let length = length3(v);
    if length > 1e-10 && length.is_finite() {
        scale3(v, length.recip())
    } else {
        [0.0; 3]
    }
}

/// NumPy's legacy singleton uses MT19937 plus 53-bit `random_sample` values.
/// SkinTokens calls `np.random.permutation/rand`, so matching only Rust's seed
/// value is insufficient for oracle parity.
#[derive(Clone)]
struct NumpyMt19937 {
    mt: [u32; 624],
    index: usize,
}

impl NumpyMt19937 {
    fn new(seed: u32) -> Self {
        let mut mt = [0u32; 624];
        mt[0] = seed;
        for i in 1..624 {
            mt[i] = 1_812_433_253u32
                .wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30))
                .wrapping_add(i as u32);
        }
        Self { mt, index: 624 }
    }

    fn twist(&mut self) {
        for i in 0..624 {
            let y = (self.mt[i] & 0x8000_0000) | (self.mt[(i + 1) % 624] & 0x7fff_ffff);
            self.mt[i] = self.mt[(i + 397) % 624] ^ (y >> 1);
            if y & 1 != 0 {
                self.mt[i] ^= 0x9908_b0df;
            }
        }
        self.index = 0;
    }

    fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            self.twist();
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    fn random_f64(&mut self) -> f64 {
        let a = (self.next_u32() >> 5) as u64;
        let b = (self.next_u32() >> 6) as u64;
        ((a << 26) + b) as f64 / 9_007_199_254_740_992.0
    }

    fn interval(&mut self, max: u32) -> u32 {
        if max == 0 {
            return 0;
        }
        let mut mask = max;
        mask |= mask >> 1;
        mask |= mask >> 2;
        mask |= mask >> 4;
        mask |= mask >> 8;
        mask |= mask >> 16;
        loop {
            let value = self.next_u32() & mask;
            if value <= max {
                return value;
            }
        }
    }

    fn shuffle<T>(&mut self, values: &mut [T]) {
        for i in (1..values.len()).rev() {
            let j = self.interval(i as u32) as usize;
            values.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numpy_random_sample_matches_random_state() {
        let mut rng = NumpyMt19937::new(42);
        let expected: [f64; 4] = [
            0.3745401188473625,
            0.9507143064099162,
            0.7319939418114051,
            0.5986584841970366,
        ];
        for value in expected {
            assert_eq!(rng.random_f64().to_bits(), value.to_bits());
        }
    }

    #[test]
    fn normalization_roundtrip() {
        let n = normalization(&[[-2.0, 0.0, 1.0], [2.0, 1.0, 3.0]]).unwrap();
        assert_eq!(n.normalize([-2.0, 0.0, 1.0]), [-1.0, -0.25, -0.5]);
        let point = [0.25, -0.5, 0.75];
        let out = n.denormalize(n.normalize(point));
        for axis in 0..3 {
            assert!((point[axis] - out[axis]).abs() < 1e-6);
        }
    }

    #[test]
    fn triangle_samples_are_on_surface() {
        let mesh = SkinTokensMesh {
            source_positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            face_normals: vec![[0.0, 0.0, 1.0]],
            vertex_normals: vec![[0.0, 0.0, 1.0]; 3],
            parts: Vec::new(),
            normalization: SkinTokensNormalization {
                center: [0.0; 3],
                scale: 1.0,
                translation: [0.0; 3],
            },
            normalized_f64: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        };
        let samples = mesh.sample_counts(7, 100, 3).unwrap();
        assert_eq!(samples.positions.len(), 100);
        for point in &samples.positions {
            assert!(point[0] >= 0.0 && point[1] >= 0.0);
            assert!(point[0] + point[1] <= 1.0 + 1e-6);
            assert_eq!(point[2], 0.0);
        }
    }

    fn brute_transfer(
        vertices: &[[f32; 3]],
        samples: &[[f32; 3]],
        sample_weights: &[f32],
        joints: usize,
    ) -> Vec<f64> {
        let mut output = vec![0.0f64; vertices.len() * joints];
        for (vertex_index, vertex) in vertices.iter().enumerate() {
            let mut neighbors = samples
                .iter()
                .enumerate()
                .map(|(index, sample)| {
                    let dx = vertex[0] as f64 - sample[0] as f64;
                    let dy = vertex[1] as f64 - sample[1] as f64;
                    let dz = vertex[2] as f64 - sample[2] as f64;
                    KdNeighbor {
                        distance2: dx * dx + dy * dy + dz * dz,
                        index,
                    }
                })
                .collect::<Vec<_>>();
            neighbors.sort_unstable();
            neighbors.truncate(8.min(samples.len()));
            let inverse = neighbors
                .iter()
                .map(|neighbor| 1.0 / (neighbor.distance2.sqrt() + 1.0e-8))
                .collect::<Vec<_>>();
            let total = inverse.iter().sum::<f64>();
            let inverse = inverse
                .into_iter()
                .map(|weight| weight / total)
                .collect::<Vec<_>>();
            for joint in 0..joints {
                output[vertex_index * joints + joint] = neighbors
                    .iter()
                    .zip(&inverse)
                    .map(|(neighbor, weight)| {
                        sample_weights[neighbor.index * joints + joint] as f64 * weight
                    })
                    .sum::<f64>();
            }
        }
        output
    }

    #[test]
    fn kd_transfer_matches_reference_brute_force() {
        let positions = vec![
            [-0.75, -0.25, 0.1],
            [-0.1, 0.5, -0.4],
            [0.2, -0.6, 0.3],
            [0.8, 0.1, -0.2],
            [0.45, 0.75, 0.5],
        ];
        let sample_positions = (0..29)
            .map(|index| {
                let x = index as f32;
                [
                    (x * 0.731).sin() * 0.9,
                    (x * 1.173).cos() * 0.8,
                    (x * 0.417 + 0.2).sin() * 0.7,
                ]
            })
            .collect::<Vec<_>>();
        let samples = SkinTokensSamples {
            positions: sample_positions.clone(),
            normals: vec![[0.0, 1.0, 0.0]; sample_positions.len()],
            source_vertices: vec![None; sample_positions.len()],
            source_faces: vec![None; sample_positions.len()],
        };
        let joints = 5;
        let sample_weights = (0..sample_positions.len() * joints)
            .map(|index| ((index * 37 % 101) as f32 + 0.5) / 101.0)
            .collect::<Vec<_>>();
        let mesh = SkinTokensMesh {
            source_positions: positions.clone(),
            positions: positions.clone(),
            indices: vec![],
            face_normals: vec![],
            vertex_normals: vec![],
            parts: vec![],
            normalization: SkinTokensNormalization {
                center: [0.0; 3],
                scale: 1.0,
                translation: [0.0; 3],
            },
            normalized_f64: positions.iter().map(|point| point.map(f64::from)).collect(),
        };
        let expected = brute_transfer(&positions, &sample_positions, &sample_weights, joints);
        let actual = mesh
            .transfer_sample_weights(&samples, &sample_weights, joints)
            .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn weight_transfer_validates_shape_and_values() {
        let mesh = SkinTokensMesh {
            source_positions: vec![[0.0; 3]],
            positions: vec![[0.0; 3]],
            indices: vec![],
            face_normals: vec![],
            vertex_normals: vec![],
            parts: vec![],
            normalization: SkinTokensNormalization {
                center: [0.0; 3],
                scale: 1.0,
                translation: [0.0; 3],
            },
            normalized_f64: vec![[0.0; 3]],
        };
        let samples = SkinTokensSamples {
            positions: vec![[0.0; 3]],
            normals: vec![[0.0; 3]],
            source_vertices: vec![None],
            source_faces: vec![None],
        };
        assert!(mesh.transfer_sample_weights(&samples, &[0.5], 2).is_err());
        assert!(mesh
            .transfer_sample_weights(&samples, &[f32::NAN], 1)
            .is_err());
        assert!(mesh.transfer_sample_weights(&samples, &[], 0).is_err());
    }

    #[test]
    fn condition_rows_interleave_positions_and_normals() {
        let samples = SkinTokensSamples {
            positions: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            normals: vec![[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
            source_vertices: vec![None; 2],
            source_faces: vec![None; 2],
        };
        assert_eq!(
            samples.condition_f32().unwrap(),
            [1.0, 2.0, 3.0, 0.0, 1.0, 0.0, 4.0, 5.0, 6.0, -1.0, 0.0, 0.0]
        );
    }
}
