use crate::{context::invalid, *};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) faces: Vec<Face>,
    pub(crate) corners: Vec<Corner>,
    pub(crate) edge_data: BTreeMap<EdgeKey, EdgeData>,
    pub(crate) uv_pins: BTreeSet<CornerId>,
    pub(crate) next_id: u64,
}
impl Default for Mesh {
    fn default() -> Self {
        Self::new()
    }
}

impl Mesh {
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            faces: Vec::new(),
            corners: Vec::new(),
            edge_data: BTreeMap::new(),
            uv_pins: BTreeSet::new(),
            next_id: 1,
        }
    }
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }
    pub fn faces(&self) -> &[Face] {
        &self.faces
    }
    pub fn corners(&self) -> &[Corner] {
        &self.corners
    }
    pub fn edge_attributes(&self) -> &BTreeMap<EdgeKey, EdgeData> {
        &self.edge_data
    }
    pub fn uv_pins(&self) -> &BTreeSet<CornerId> { &self.uv_pins }
    /// Conservative owned-storage estimate for document-level worker admission.
    pub fn memory_bytes(&self) -> usize {
        self.estimated_bytes()
    }
    pub fn vertex(&self, id: VertexId) -> Option<&Vertex> {
        self.vertices
            .binary_search_by_key(&id, |v| v.id)
            .ok()
            .map(|i| &self.vertices[i])
    }
    pub fn face(&self, id: FaceId) -> Option<&Face> {
        self.faces
            .binary_search_by_key(&id, |f| f.id)
            .ok()
            .map(|i| &self.faces[i])
    }
    pub fn corner(&self, id: CornerId) -> Option<&Corner> {
        // Corner order follows face ranges, not corner identity after edits.
        self.corners.iter().find(|c| c.id == id)
    }
    pub fn face_corners(&self, id: FaceId) -> Result<&[Corner]> {
        let f = self
            .face(id)
            .ok_or(MeshError::UnknownElement(ElementId::Face(id)))?;
        Ok(&self.corners[f.first_corner as usize..(f.first_corner + f.corner_count) as usize])
    }

    pub fn from_polygons(
        positions: &[[f64; 3]],
        polygons: &[Polygon],
        ctx: &mut Context<'_>,
    ) -> Result<Self> {
        ctx.checkpoint(0)?;
        ctx.counts(positions.len(), polygons.len(), 0, 0)?;
        let nc = polygons.iter().try_fold(0usize, |n, p| {
            ctx.checkpoint(1)?;
            n.checked_add(p.vertices.len())
                .ok_or_else(|| invalid("corner count overflow"))
        })?;
        ctx.counts(positions.len(), polygons.len(), nc, 0)?;
        ctx.bytes(storage_estimate(positions.len(), polygons.len(), nc, 0, 0))?;
        let mut m = Self::new();
        for &p in positions {
            ctx.checkpoint(1)?;
            check_position(p)?;
            m.push_vertex(p, Vec::new())?;
        }
        for p in polygons {
            ctx.checkpoint(p.vertices.len() as u64)?;
            ctx.limit(
                "face corners",
                p.vertices.len() as u64,
                ctx.limits.max_face_corners as u64,
            )?;
            if p.vertices.len() < 3 {
                return Err(invalid("a face needs at least three corners"));
            }
            if !p.uvs.is_empty() && p.uvs.len() != p.vertices.len() {
                return Err(invalid("UV count must equal corner count"));
            }
            let mut corners = Vec::with_capacity(p.vertices.len());
            for (i, &vi) in p.vertices.iter().enumerate() {
                let v = m
                    .vertices
                    .get(vi as usize)
                    .ok_or_else(|| invalid("polygon vertex index out of bounds"))?;
                let uv = p.uvs.get(i).copied().unwrap_or([0.0; 2]);
                check_uv(uv)?;
                corners.push((v.id, uv, None));
            }
            m.push_face(&corners, p.material)?;
        }
        // Surface topology may be open/nonmanifold; individual faces must be usable.
        m.check_structure(ctx)?;
        m.check_faces(ctx)?;
        Ok(m)
    }

    /// Bulk import avoids one whole-mesh transaction per vertex. Each weight
    /// vector is normalized and sorted; empty weights mean unbound. UVs remain
    /// per polygon corner. No input is installed on failure.
    pub fn from_weighted_polygons(
        positions: &[[f64; 3]],
        weights: &[Vec<JointWeight>],
        polygons: &[Polygon],
        ctx: &mut Context<'_>,
    ) -> Result<Self> {
        if weights.len() != positions.len() {
            return Err(invalid("weight vector count must equal vertex count"));
        }
        ctx.checkpoint(0)?;
        ctx.counts(positions.len(), polygons.len(), 0, 0)?;
        let mut total = 0usize;
        for w in weights {
            ctx.checkpoint(1)?;
            ctx.limit(
                "weights per vertex",
                w.len() as u64,
                ctx.limits.max_weights_per_vertex as u64,
            )?;
            total = total.saturating_add(w.len());
        }
        let mut mesh = Self::from_polygons(positions, polygons, ctx)?;
        ctx.bytes(
            mesh.estimated_bytes()
                .saturating_add(total.saturating_mul(32)),
        )?;
        for (v, w) in mesh.vertices.iter_mut().zip(weights) {
            v.weights = crate::ops::normalize_weights(w, ctx)?;
        }
        Ok(mesh)
    }

    /// Centered box with outward face winding and independent corner UVs.
    pub fn cube(size: [f64; 3], ctx: &mut Context<'_>) -> Result<Self> {
        if size.iter().any(|x| !x.is_finite() || *x <= 0.0) {
            return Err(invalid("cube sizes must be positive and finite"));
        }
        let [x, y, z] = size.map(|v| v * 0.5);
        let positions = [
            [-x, -y, -z],
            [x, -y, -z],
            [x, y, -z],
            [-x, y, -z],
            [-x, -y, z],
            [x, -y, z],
            [x, y, z],
            [-x, y, z],
        ];
        let polygons = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [3, 7, 6, 2],
            [0, 4, 7, 3],
            [1, 2, 6, 5],
        ]
        .map(|v| Polygon {
            vertices: v.to_vec(),
            uvs: vec![[0., 0.], [1., 0.], [1., 1.], [0., 1.]],
            material: 0,
        });
        Self::from_polygons(&positions, &polygons, ctx)
    }
    /// Centered XZ plane, normal +Y. Size components are X and Z.
    pub fn plane(size: [f64; 2], ctx: &mut Context<'_>) -> Result<Self> {
        if size.iter().any(|x| !x.is_finite() || *x <= 0.0) {
            return Err(invalid("plane sizes must be positive and finite"));
        }
        let [x, z] = size.map(|v| v * 0.5);
        Self::from_polygons(
            &[[-x, 0., -z], [-x, 0., z], [x, 0., z], [x, 0., -z]],
            &[Polygon {
                vertices: vec![0, 1, 2, 3],
                uvs: vec![[0., 0.], [0., 1.], [1., 1.], [1., 0.]],
                material: 0,
            }],
            ctx,
        )
    }

    pub fn adjacency(&self, ctx: &mut Context<'_>) -> Result<Adjacency> {
        ctx.checkpoint(0)?;
        ctx.bytes(
            self.estimated_bytes()
                .saturating_add(self.corners.len().saturating_mul(320))
                .saturating_add(self.edge_data.len().saturating_mul(256)),
        )?;
        let mut a = Adjacency::default();
        for v in &self.vertices {
            a.vertex_faces.insert(v.id, Vec::new());
            a.vertex_edges.insert(v.id, Vec::new());
        }
        for f in &self.faces {
            let corners = self.face_corners(f.id)?;
            for i in 0..corners.len() {
                ctx.checkpoint(1)?;
                let (c, next) = (&corners[i], &corners[(i + 1) % corners.len()]);
                let key = EdgeKey::new(c.vertex, next.vertex);
                a.edges.entry(key).or_default().push(EdgeUse {
                    face: f.id,
                    corner: c.id,
                    from: c.vertex,
                    to: next.vertex,
                });
                a.vertex_faces
                    .get_mut(&c.vertex)
                    .ok_or_else(|| invalid("missing corner vertex"))?
                    .push(f.id);
            }
        }
        for (&edge, data) in &self.edge_data {
            if data.loose {
                a.edges.entry(edge).or_default();
            }
        }
        ctx.limit("edges", a.edges.len() as u64, ctx.limits.max_edges as u64)?;
        for &edge in a.edges.keys() {
            ctx.checkpoint(1)?;
            a.vertex_edges
                .get_mut(&edge.0)
                .ok_or_else(|| invalid("missing edge vertex"))?
                .push(edge);
            a.vertex_edges
                .get_mut(&edge.1)
                .ok_or_else(|| invalid("missing edge vertex"))?
                .push(edge);
        }
        for faces in a.vertex_faces.values_mut() {
            faces.sort_unstable();
            faces.dedup();
        }
        Ok(a)
    }

    pub fn triangulate(&self, ctx: &mut Context<'_>) -> Result<TriangleMesh> {
        ctx.checkpoint(0)?;
        let nt = self
            .faces
            .iter()
            .map(|f| f.corner_count as usize - 2)
            .sum::<usize>();
        ctx.limit("triangles", nt as u64, ctx.limits.max_triangles as u64)?;
        let weights = self
            .corners
            .iter()
            .map(|c| self.vertex(c.vertex).map_or(0, |v| v.weights.len()))
            .sum::<usize>();
        let output_bytes = self
            .corners
            .len()
            .saturating_mul(std::mem::size_of::<TriangleVertex>())
            .saturating_add(nt.saturating_mul(std::mem::size_of::<Triangle>()))
            .saturating_add(weights.saturating_mul(std::mem::size_of::<JointWeight>()));
        ctx.bytes(
            self.estimated_bytes()
                .saturating_add(output_bytes)
                .saturating_add(self.corners.len().saturating_mul(64)),
        )?;
        let mut out = TriangleMesh {
            vertices: Vec::with_capacity(self.corners.len()),
            triangles: Vec::with_capacity(nt),
        };
        for f in &self.faces {
            let corners = self.face_corners(f.id)?;
            let geometry = crate::geometry::face_geometry(self, f.id, ctx)?;
            let triangles = crate::geometry::ear_clip(&geometry.points, f.id, ctx)?;
            let first = u32::try_from(out.vertices.len())
                .map_err(|_| invalid("triangle vertex count overflow"))?;
            for c in corners {
                ctx.checkpoint(1)?;
                let v = self
                    .vertex(c.vertex)
                    .ok_or(MeshError::UnknownElement(ElementId::Vertex(c.vertex)))?;
                out.vertices.push(TriangleVertex {
                    position: v.position,
                    normal: c.normal.unwrap_or(geometry.normal),
                    uv: c.uv,
                    weights: v.weights.clone(),
                    source_vertex: v.id,
                    source_corner: c.id,
                });
            }
            for t in triangles {
                out.triangles.push(Triangle {
                    indices: t.map(|i| first + i as u32),
                    source_face: f.id,
                    material: f.material,
                });
            }
        }
        Ok(out)
    }

    pub(crate) fn allocate_id(&mut self) -> Result<u64> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| invalid("stable identity space exhausted"))?;
        Ok(id)
    }
    pub(crate) fn push_vertex(
        &mut self,
        position: [f64; 3],
        weights: Vec<JointWeight>,
    ) -> Result<VertexId> {
        let id = VertexId(self.allocate_id()?);
        self.vertices.push(Vertex {
            id,
            position,
            weights,
        });
        Ok(id)
    }
    pub(crate) fn push_face(
        &mut self,
        values: &[(VertexId, [f64; 2], Option<[f64; 3]>)],
        material: u32,
    ) -> Result<FaceId> {
        let id = FaceId(self.allocate_id()?);
        let first_corner =
            u32::try_from(self.corners.len()).map_err(|_| invalid("corner offset overflow"))?;
        for &(vertex, uv, normal) in values {
            let id = CornerId(self.allocate_id()?);
            self.corners.push(Corner {
                id,
                vertex,
                uv,
                normal,
            });
        }
        self.faces.push(Face {
            id,
            first_corner,
            corner_count: values.len() as u32,
            material,
        });
        Ok(id)
    }
    pub(crate) fn estimated_bytes(&self) -> usize {
        storage_estimate(
            self.vertices.len(),
            self.faces.len(),
            self.corners.len(),
            self.edge_data.len(),
            self.vertices.iter().map(|v| v.weights.len()).sum(),
        ).saturating_add(self.uv_pins.len().saturating_mul(64))
    }
    pub(crate) fn check_faces(&self, ctx: &mut Context<'_>) -> Result<()> {
        for f in &self.faces {
            crate::geometry::face_geometry(self, f.id, ctx)?;
        }
        Ok(())
    }
    pub(crate) fn check_structure(&self, ctx: &mut Context<'_>) -> Result<()> {
        ctx.counts(
            self.vertices.len(),
            self.faces.len(),
            self.corners.len(),
            self.edge_data.len(),
        )?;
        ctx.bytes(self.estimated_bytes().saturating_add(
            (self.vertices.len() + self.faces.len() + self.corners.len()).saturating_mul(48),
        ))?;
        let mut ids = BTreeSet::new();
        let mut add = |id| -> Result<()> {
            if id == 0 || id >= self.next_id || !ids.insert(id) {
                return Err(MeshError::CorruptData(
                    "duplicate, zero or unallocated identity",
                ));
            }
            Ok(())
        };
        let mut previous = 0;
        for v in &self.vertices {
            ctx.checkpoint(1)?;
            add(v.id.0)?;
            check_position(v.position)?;
            if v.id.0 <= previous {
                return Err(MeshError::CorruptData("vertices are not in identity order"));
            }
            previous = v.id.0;
            check_weights(&v.weights, ctx)?;
        }
        let mut offset = 0usize;
        previous = 0;
        for f in &self.faces {
            ctx.checkpoint(1)?;
            add(f.id.0)?;
            if f.id.0 <= previous {
                return Err(MeshError::CorruptData("faces are not in identity order"));
            }
            previous = f.id.0;
            if f.first_corner as usize != offset || f.corner_count < 3 {
                return Err(MeshError::CorruptData("invalid face corner range"));
            }
            ctx.limit(
                "face corners",
                f.corner_count as u64,
                ctx.limits.max_face_corners as u64,
            )?;
            offset = offset
                .checked_add(f.corner_count as usize)
                .ok_or(MeshError::CorruptData("corner range overflow"))?;
            if offset > self.corners.len() {
                return Err(MeshError::CorruptData("face corner range out of bounds"));
            }
        }
        if offset != self.corners.len() {
            return Err(MeshError::CorruptData("unowned corners"));
        }
        for c in &self.corners {
            ctx.checkpoint(1)?;
            add(c.id.0)?;
            check_uv(c.uv)?;
            if self.vertex(c.vertex).is_none() {
                return Err(MeshError::CorruptData("corner refers to missing vertex"));
            }
            if let Some(n) = c.normal {
                check_position(n)?;
                if (crate::geometry::length(n) - 1.).abs() > 1e-8 {
                    return Err(invalid("split normal must have unit length"));
                }
            }
        }
        if self.next_id == 0 {
            return Err(MeshError::CorruptData("zero identity watermark"));
        }
        let corner_ids=self.corners.iter().map(|c|c.id).collect::<BTreeSet<_>>();
        for pin in &self.uv_pins {ctx.checkpoint(1)?;if !corner_ids.contains(pin){return Err(MeshError::CorruptData("UV pin refers to missing corner"));}}
        let mut used_edges = BTreeSet::new();
        for f in &self.faces {
            let corners = self.face_corners(f.id)?;
            for i in 0..corners.len() {
                used_edges.insert(EdgeKey::new(
                    corners[i].vertex,
                    corners[(i + 1) % corners.len()].vertex,
                ));
            }
        }
        for (&e, data) in &self.edge_data {
            ctx.checkpoint(1)?;
            if e.0 >= e.1 || self.vertex(e.0).is_none() || self.vertex(e.1).is_none() {
                return Err(MeshError::CorruptData("invalid edge endpoints"));
            }
            if !data.loose && !used_edges.contains(&e) {
                return Err(MeshError::CorruptData("orphan edge attributes"));
            }
            check_edge_attributes(data.attributes)?;
            if !data.loose && data.attributes == EdgeAttributes::default() {
                return Err(MeshError::CorruptData("redundant default edge record"));
            }
        }
        ctx.limit(
            "edges",
            used_edges.len().saturating_add(
                self.edge_data
                    .iter()
                    .filter(|(e, d)| d.loose && !used_edges.contains(e))
                    .count(),
            ) as u64,
            ctx.limits.max_edges as u64,
        )?;
        Ok(())
    }
}

pub(crate) fn storage_estimate(v: usize, f: usize, c: usize, e: usize, w: usize) -> usize {
    // Factor two covers vector capacity slack. BTree node storage is bounded
    // conservatively; operation scratch is charged separately by its caller.
    v.saturating_mul(std::mem::size_of::<Vertex>())
        .saturating_add(f.saturating_mul(std::mem::size_of::<Face>()))
        .saturating_add(c.saturating_mul(std::mem::size_of::<Corner>()))
        .saturating_add(e.saturating_mul(128))
        .saturating_add(w.saturating_mul(std::mem::size_of::<JointWeight>()))
        .saturating_mul(2)
}
pub(crate) fn check_position(p: [f64; 3]) -> Result<()> {
    if p.iter().all(|x| x.is_finite() && x.abs() <= 1e100) {
        Ok(())
    } else {
        Err(invalid(
            "coordinates must be finite and at most 1e100 in magnitude",
        ))
    }
}
pub(crate) fn check_uv(uv: [f64; 2]) -> Result<()> {
    if uv.iter().all(|x| x.is_finite() && x.abs() <= 1e100) {
        Ok(())
    } else {
        Err(invalid("UVs must be finite and at most 1e100 in magnitude"))
    }
}
pub(crate) fn check_edge_attributes(a: EdgeAttributes) -> Result<()> {
    if a.crease.is_finite() && (0.0..=1.0).contains(&a.crease) {
        Ok(())
    } else {
        Err(invalid("edge crease must be in [0,1]"))
    }
}
pub(crate) fn check_weights(weights: &[JointWeight], ctx: &mut Context<'_>) -> Result<()> {
    ctx.limit(
        "weights per vertex",
        weights.len() as u64,
        ctx.limits.max_weights_per_vertex as u64,
    )?;
    let mut prev = None;
    let mut sum = 0.;
    for w in weights {
        ctx.checkpoint(1)?;
        if !w.weight.is_finite() || w.weight <= 0. || prev.is_some_and(|p| p >= w.joint) {
            return Err(invalid(
                "weights must be positive, finite and sorted by unique joint",
            ));
        }
        prev = Some(w.joint);
        sum += w.weight;
    }
    if !weights.is_empty() && (sum - 1.).abs() > 1e-10 {
        return Err(invalid("vertex weights must sum to one"));
    }
    Ok(())
}
