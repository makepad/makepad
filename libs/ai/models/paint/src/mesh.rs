//! Minimal indexed triangle mesh used by the geometry stages (view selection,
//! G-buffer rendering, UV baking). GLB import/export intentionally lives
//! outside this crate — the integration seam converts to/from `libs/gltf`
//! types so this crate stays dependency-free and testable in isolation.

#[derive(Clone, Debug, Default)]
pub struct TriMesh {
    pub positions: Vec<[f32; 3]>,
    /// Per-vertex normals. May be empty; call [`TriMesh::compute_vertex_normals`].
    pub normals: Vec<[f32; 3]>,
    /// Per-vertex UV0 coordinates in `[0,1]`. May be empty for geometry-only stages.
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<[u32; 3]>,
}

pub const MAX_MESH_VERTICES: usize = 2_000_000;
pub const MAX_MESH_TRIANGLES: usize = 4_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshError {
    EmptyPositions,
    EmptyIndices,
    TooManyVertices(usize),
    TooManyTriangles(usize),
    AllocationSizeOverflow,
    NormalCountMismatch { positions: usize, normals: usize },
    UvCountMismatch { positions: usize, uvs: usize },
    NonFinitePosition(usize),
    NonFiniteNormal(usize),
    NonFiniteUv(usize),
    UvOutOfRange(usize),
    IndexOutOfRange { face: usize, index: u32, vertices: usize },
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPositions => write!(f, "mesh has no positions"),
            Self::EmptyIndices => write!(f, "mesh has no triangles"),
            Self::TooManyVertices(count) => {
                write!(f, "mesh has {count} vertices (limit {MAX_MESH_VERTICES})")
            }
            Self::TooManyTriangles(count) => {
                write!(f, "mesh has {count} triangles (limit {MAX_MESH_TRIANGLES})")
            }
            Self::AllocationSizeOverflow => write!(f, "mesh allocation size overflows address space"),
            Self::NormalCountMismatch { positions, normals } => write!(
                f,
                "mesh has {positions} positions but {normals} normals"
            ),
            Self::UvCountMismatch { positions, uvs } => {
                write!(f, "mesh has {positions} positions but {uvs} UVs")
            }
            Self::NonFinitePosition(index) => write!(f, "position {index} is not finite"),
            Self::NonFiniteNormal(index) => write!(f, "normal {index} is not finite"),
            Self::NonFiniteUv(index) => write!(f, "UV {index} is not finite"),
            Self::UvOutOfRange(index) => write!(f, "UV {index} lies outside [0,1]"),
            Self::IndexOutOfRange { face, index, vertices } => write!(
                f,
                "triangle {face} references vertex {index}, but mesh has {vertices} vertices"
            ),
        }
    }
}

impl std::error::Error for MeshError {}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

impl TriMesh {
    /// Validate all public mesh data before it reaches rasterization. When
    /// `require_uvs` is true the UV0 atlas must be complete and in `[0,1]`.
    pub fn validate(&self, require_uvs: bool) -> Result<(), MeshError> {
        if self.positions.is_empty() {
            return Err(MeshError::EmptyPositions);
        }
        if self.indices.is_empty() {
            return Err(MeshError::EmptyIndices);
        }
        if self.positions.len() > MAX_MESH_VERTICES {
            return Err(MeshError::TooManyVertices(self.positions.len()));
        }
        if self.indices.len() > MAX_MESH_TRIANGLES {
            return Err(MeshError::TooManyTriangles(self.indices.len()));
        }
        let allocation_sizes = [
            self.positions
                .len()
                .checked_mul(std::mem::size_of::<[f32; 3]>()),
            self.normals
                .len()
                .checked_mul(std::mem::size_of::<[f32; 3]>()),
            self.uvs
                .len()
                .checked_mul(std::mem::size_of::<[f32; 2]>()),
            self.indices
                .len()
                .checked_mul(std::mem::size_of::<[u32; 3]>()),
        ];
        let mut allocation_total = 0usize;
        for bytes in allocation_sizes {
            allocation_total = allocation_total
                .checked_add(bytes.ok_or(MeshError::AllocationSizeOverflow)?)
                .ok_or(MeshError::AllocationSizeOverflow)?;
        }
        if !self.normals.is_empty() && self.normals.len() != self.positions.len() {
            return Err(MeshError::NormalCountMismatch {
                positions: self.positions.len(),
                normals: self.normals.len(),
            });
        }
        if require_uvs && self.uvs.len() != self.positions.len() {
            return Err(MeshError::UvCountMismatch {
                positions: self.positions.len(),
                uvs: self.uvs.len(),
            });
        }
        if !self.uvs.is_empty() && self.uvs.len() != self.positions.len() {
            return Err(MeshError::UvCountMismatch {
                positions: self.positions.len(),
                uvs: self.uvs.len(),
            });
        }
        for (index, position) in self.positions.iter().enumerate() {
            if position.iter().any(|value| !value.is_finite()) {
                return Err(MeshError::NonFinitePosition(index));
            }
        }
        for (index, normal) in self.normals.iter().enumerate() {
            if normal.iter().any(|value| !value.is_finite()) {
                return Err(MeshError::NonFiniteNormal(index));
            }
        }
        for (index, uv) in self.uvs.iter().enumerate() {
            if uv.iter().any(|value| !value.is_finite()) {
                return Err(MeshError::NonFiniteUv(index));
            }
            if require_uvs && uv.iter().any(|value| !(0.0..=1.0).contains(value)) {
                return Err(MeshError::UvOutOfRange(index));
            }
        }
        for (face, triangle) in self.indices.iter().enumerate() {
            for index in triangle {
                if (*index as usize) >= self.positions.len() {
                    return Err(MeshError::IndexOutOfRange {
                        face,
                        index: *index,
                        vertices: self.positions.len(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Unit square in the XY plane facing +Z, centered at the origin, full UV range.
    pub fn unit_quad() -> Self {
        Self {
            positions: vec![
                [-0.5, -0.5, 0.0],
                [0.5, -0.5, 0.0],
                [0.5, 0.5, 0.0],
                [-0.5, 0.5, 0.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            indices: vec![[0, 1, 2], [0, 2, 3]],
        }
    }

    /// Axis-aligned unit cube centered at the origin: 24 vertices (per-face
    /// normals/UVs), 12 triangles, CCW winding seen from outside.
    pub fn unit_cube() -> Self {
        // (normal, u_axis, v_axis) with u x v = normal.
        const FACES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ];
        let mut mesh = TriMesh::default();
        for (n, u, v) in FACES {
            let base = mesh.positions.len() as u32;
            let corners = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)];
            let corner_uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
            for ((cu, cv), uv) in corners.iter().zip(corner_uvs.iter()) {
                mesh.positions.push([
                    n[0] * 0.5 + u[0] * cu + v[0] * cv,
                    n[1] * 0.5 + u[1] * cu + v[1] * cv,
                    n[2] * 0.5 + u[2] * cu + v[2] * cv,
                ]);
                mesh.normals.push(n);
                mesh.uvs.push(*uv);
            }
            mesh.indices.push([base, base + 1, base + 2]);
            mesh.indices.push([base, base + 2, base + 3]);
        }
        mesh
    }

    /// Unit cube whose six faces map to distinct cells of a 3x2 UV atlas
    /// (non-overlapping, as the bake contract requires). Face order matches
    /// [`TriMesh::unit_cube`]: [+X, -X, +Y, -Y, +Z, -Z]; face f occupies
    /// u in [ (f%3)/3, (f%3+1)/3 ), v in [ (f/3)/2, (f/3+1)/2 ).
    pub fn unit_cube_atlas() -> Self {
        let mut mesh = Self::unit_cube();
        for face in 0..6usize {
            let col = (face % 3) as f32;
            let row = (face / 3) as f32;
            let cell = [
                [col / 3.0, row / 2.0],
                [(col + 1.0) / 3.0, row / 2.0],
                [(col + 1.0) / 3.0, (row + 1.0) / 2.0],
                [col / 3.0, (row + 1.0) / 2.0],
            ];
            for corner in 0..4 {
                mesh.uvs[face * 4 + corner] = cell[corner];
            }
        }
        mesh
    }

    pub fn bbox(&self) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for axis in 0..3 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
        (min, max)
    }

    pub fn face_area(&self, face: usize) -> f32 {
        let Some([i0, i1, i2]) = self.indices.get(face).copied() else {
            return 0.0;
        };
        let (Some(a), Some(b), Some(c)) = (
            self.positions.get(i0 as usize).copied(),
            self.positions.get(i1 as usize).copied(),
            self.positions.get(i2 as usize).copied(),
        ) else {
            return 0.0;
        };
        0.5 * norm(cross(sub(b, a), sub(c, a)))
    }

    pub fn face_areas(&self) -> Vec<f32> {
        (0..self.indices.len()).map(|f| self.face_area(f)).collect()
    }

    pub fn total_area(&self) -> f32 {
        self.face_areas().iter().sum()
    }

    /// Area-weighted vertex normals from face geometry. Overwrites `normals`.
    pub fn compute_vertex_normals(&mut self) {
        if self.validate(false).is_err() {
            return;
        }
        let mut acc = vec![[0.0f32; 3]; self.positions.len()];
        for [i0, i1, i2] in &self.indices {
            let a = self.positions[*i0 as usize];
            let b = self.positions[*i1 as usize];
            let c = self.positions[*i2 as usize];
            let face_n = cross(sub(b, a), sub(c, a)); // length = 2 * area, weights by area
            for i in [*i0, *i1, *i2] {
                let v = &mut acc[i as usize];
                v[0] += face_n[0];
                v[1] += face_n[1];
                v[2] += face_n[2];
            }
        }
        for v in &mut acc {
            let len = norm(*v);
            if len > 1e-20 {
                v[0] /= len;
                v[1] /= len;
                v[2] /= len;
            }
        }
        self.normals = acc;
    }

    /// Translate the mesh center to the origin and uniformly scale so the
    /// largest half-extent equals `half`. Returns the applied scale factor.
    /// This mirrors the upstream renderer's mesh normalization step; the exact
    /// upstream `scale_factor` semantics are re-verified at oracle-parity time.
    pub fn normalize_to_half_extent(&mut self, half: f32) -> f32 {
        let (min, max) = self.bbox();
        let center = [
            0.5 * (min[0] as f64 + max[0] as f64),
            0.5 * (min[1] as f64 + max[1] as f64),
            0.5 * (min[2] as f64 + max[2] as f64),
        ];
        let mut max_half = 0.0f64;
        for axis in 0..3 {
            max_half = max_half.max(0.5 * (max[axis] as f64 - min[axis] as f64));
        }
        let scale = if max_half > 1e-20 {
            half as f64 / max_half
        } else {
            1.0
        };
        for p in &mut self.positions {
            for axis in 0..3 {
                p[axis] = ((p[axis] as f64 - center[axis]) * scale) as f32;
            }
        }
        scale as f32
    }

    /// The upstream renderer's mesh frame change (MeshRender.py `set_mesh`):
    /// `vtx[:, [0,1]] = -vtx[:, [0,1]]; vtx[:, [1,2]] = vtx[:, [2,1]]`, i.e.
    /// `(x, y, z) -> (-x, z, -y)` — glTF Y-up into the Z-up world the paint
    /// cameras orbit. Without it every "azimuth ring" view renders the mesh
    /// top-down and the diffusion conditioning contradicts the reference.
    /// Normals transform by the same orthogonal map.
    pub fn apply_paint_frame(&mut self) {
        for p in &mut self.positions {
            *p = [-p[0], p[2], -p[1]];
        }
        for n in &mut self.normals {
            *n = [-n[0], n[2], -n[1]];
        }
    }

    /// The upstream renderer's normalization (MeshRender.py `set_mesh`,
    /// `auto_center=True`): center on the bbox midpoint, then scale by
    /// `scale_factor / (2 * max ||v - center||)` so the largest radial
    /// distance becomes `scale_factor / 2` (upstream default 1.15). Position
    /// conditioning then encodes `0.5 - p / scale_factor` into [0, 1].
    pub fn normalize_paint_radial(&mut self, scale_factor: f32) -> f32 {
        let (min, max) = self.bbox();
        let center = [
            0.5 * (min[0] as f64 + max[0] as f64),
            0.5 * (min[1] as f64 + max[1] as f64),
            0.5 * (min[2] as f64 + max[2] as f64),
        ];
        let mut max_norm_sq = 0.0f64;
        for p in &self.positions {
            let dx = p[0] as f64 - center[0];
            let dy = p[1] as f64 - center[1];
            let dz = p[2] as f64 - center[2];
            max_norm_sq = max_norm_sq.max(dx * dx + dy * dy + dz * dz);
        }
        let diameter = 2.0 * max_norm_sq.sqrt();
        let scale = if diameter > 1e-20 {
            scale_factor as f64 / diameter
        } else {
            1.0
        };
        for p in &mut self.positions {
            for axis in 0..3 {
                p[axis] = ((p[axis] as f64 - center[axis]) * scale) as f32;
            }
        }
        scale as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_geometry() {
        let cube = TriMesh::unit_cube();
        cube.validate(true).unwrap();
        assert_eq!(cube.positions.len(), 24);
        assert_eq!(cube.indices.len(), 12);
        let (min, max) = cube.bbox();
        assert_eq!(min, [-0.5, -0.5, -0.5]);
        assert_eq!(max, [0.5, 0.5, 0.5]);
        let total = cube.total_area();
        assert!((total - 6.0).abs() < 1e-5, "total area {total}");
    }

    #[test]
    fn validation_rejects_hostile_indices_and_nonfinite_values() {
        let mut mesh = TriMesh::unit_quad();
        mesh.indices[0][2] = u32::MAX;
        assert!(matches!(
            mesh.validate(true),
            Err(MeshError::IndexOutOfRange { .. })
        ));

        let mut mesh = TriMesh::unit_quad();
        mesh.positions[0][0] = f32::NAN;
        assert_eq!(mesh.validate(true), Err(MeshError::NonFinitePosition(0)));

        let mut mesh = TriMesh::unit_quad();
        mesh.normals[1][1] = f32::INFINITY;
        assert_eq!(mesh.validate(true), Err(MeshError::NonFiniteNormal(1)));

        let mut mesh = TriMesh::unit_quad();
        mesh.uvs[2] = [1.5, 0.5];
        assert_eq!(mesh.validate(true), Err(MeshError::UvOutOfRange(2)));
    }

    #[test]
    fn validation_rejects_incomplete_vertex_attributes_and_excess_counts() {
        let mut mesh = TriMesh::unit_quad();
        mesh.normals.pop();
        assert!(matches!(
            mesh.validate(true),
            Err(MeshError::NormalCountMismatch { .. })
        ));

        let mesh = TriMesh {
            positions: vec![[0.0; 3]; MAX_MESH_VERTICES + 1],
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: vec![[0, 0, 0]],
        };
        assert_eq!(
            mesh.validate(false),
            Err(MeshError::TooManyVertices(MAX_MESH_VERTICES + 1))
        );
    }

    #[test]
    fn cube_winding_faces_outward() {
        let cube = TriMesh::unit_cube();
        for (face, [i0, i1, i2]) in cube.indices.iter().enumerate() {
            let a = cube.positions[*i0 as usize];
            let b = cube.positions[*i1 as usize];
            let c = cube.positions[*i2 as usize];
            let n = cross(sub(b, a), sub(c, a));
            let stored = cube.normals[*i0 as usize];
            let dot = n[0] * stored[0] + n[1] * stored[1] + n[2] * stored[2];
            assert!(dot > 0.0, "face {face} winding disagrees with its normal");
        }
    }

    #[test]
    fn quad_area_and_normals() {
        let mut quad = TriMesh::unit_quad();
        assert!((quad.total_area() - 1.0).abs() < 1e-6);
        quad.normals.clear();
        quad.compute_vertex_normals();
        for n in &quad.normals {
            assert!((n[2] - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn normalize_centers_and_scales() {
        let mut cube = TriMesh::unit_cube();
        for p in &mut cube.positions {
            p[0] = p[0] * 4.0 + 3.0;
            p[1] *= 4.0;
            p[2] *= 4.0;
        }
        let scale = cube.normalize_to_half_extent(0.5);
        assert!((scale - 0.25).abs() < 1e-6);
        let (min, max) = cube.bbox();
        for axis in 0..3 {
            assert!((min[axis] + 0.5).abs() < 1e-5);
            assert!((max[axis] - 0.5).abs() < 1e-5);
        }
    }

    #[test]
    fn normalize_extreme_finite_coordinates_stays_finite() {
        let mut mesh = TriMesh::unit_quad();
        mesh.positions[0][0] = f32::MAX;
        mesh.positions[1][0] = -f32::MAX;
        mesh.normalize_to_half_extent(0.5);
        assert!(mesh.positions.iter().flatten().all(|value| value.is_finite()));
    }
}
