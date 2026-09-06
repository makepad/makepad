use crate::{context::invalid, geometry::*, mesh::*, *};
use std::collections::{BTreeMap, BTreeSet};

type Remaps = Vec<(ElementId, Vec<ElementId>)>;

impl Mesh {
    pub(crate) fn edit<T>(
        &mut self,
        ctx: &mut Context<'_>,
        growth: usize,
        op: impl FnOnce(&mut Mesh, &mut Context<'_>) -> Result<(T, Remaps)>,
    ) -> Result<(T, ChangeSet)> {
        ctx.checkpoint(0)?;
        ctx.counts(
            self.vertices.len(),
            self.faces.len(),
            self.corners.len(),
            self.edge_data.len(),
        )?;
        let ids = self
            .vertices
            .len()
            .saturating_add(self.faces.len())
            .saturating_add(self.corners.len().saturating_mul(2))
            .saturating_add(self.edge_data.len());
        ctx.bytes(
            self.estimated_bytes()
                .saturating_mul(2)
                .saturating_add(growth)
                .saturating_add(ids.saturating_mul(256)),
        )?;
        let mut candidate = self.clone();
        let (result, mut remapped) = op(&mut candidate, ctx)?;
        let surviving_corners=candidate.corners.iter().map(|c|c.id).collect::<BTreeSet<_>>();
        candidate.uv_pins.retain(|c|surviving_corners.contains(c));
        for (source,targets) in &remapped {if let ElementId::Corner(corner)=source {if self.uv_pins.contains(corner){
            for target in targets {if let ElementId::Corner(id)=target {if surviving_corners.contains(id){candidate.uv_pins.insert(*id);}}}
        }}}
        candidate.check_structure(ctx)?;
        candidate.check_faces(ctx)?;
        let before = self.element_ids(ctx)?;
        let after = candidate.element_ids(ctx)?;
        for (source, targets) in &mut remapped {
            ctx.checkpoint(1)?;
            if !before.contains(source) {
                return Err(invalid("operation produced an invalid source mapping"));
            }
            targets.retain(|target| after.contains(target));
        }
        ctx.bytes(
            self.estimated_bytes()
                .saturating_add(candidate.estimated_bytes())
                .saturating_add((before.len() + after.len()).saturating_mul(192)),
        )?;
        let changes = ChangeSet {
            created: after.difference(&before).copied().collect(),
            retained: before.intersection(&after).copied().collect(),
            deleted: before.difference(&after).copied().collect(),
            remapped,
        };
        ctx.checkpoint(0)?;
        *self = candidate;
        Ok((result, changes))
    }
    fn element_ids(&self, ctx: &mut Context<'_>) -> Result<BTreeSet<ElementId>> {
        let mut ids = BTreeSet::new();
        for v in &self.vertices {
            ctx.checkpoint(1)?;
            ids.insert(ElementId::Vertex(v.id));
        }
        for f in &self.faces {
            ctx.checkpoint(1)?;
            ids.insert(ElementId::Face(f.id));
            let cs = self.face_corners(f.id)?;
            for i in 0..cs.len() {
                ctx.checkpoint(1)?;
                ids.insert(ElementId::Corner(cs[i].id));
                ids.insert(ElementId::Edge(EdgeKey::new(
                    cs[i].vertex,
                    cs[(i + 1) % cs.len()].vertex,
                )));
            }
        }
        for (&e, d) in &self.edge_data {
            if d.loose {
                ids.insert(ElementId::Edge(e));
            }
        }
        Ok(ids)
    }
    pub(crate) fn repack_faces(&mut self, remove: &BTreeSet<FaceId>, ctx: &mut Context<'_>) -> Result<()> {
        let mut faces = Vec::with_capacity(self.faces.len() - remove.len());
        let mut corners = Vec::with_capacity(self.corners.len());
        for f in &self.faces {
            ctx.checkpoint(1)?;
            if !remove.contains(&f.id) {
                let mut face = f.clone();
                face.first_corner = corners.len() as u32;
                corners.extend_from_slice(self.face_corners(f.id)?);
                faces.push(face);
            }
        }
        self.faces = faces;
        self.corners = corners;
        self.prune_edge_attributes();
        Ok(())
    }
    pub(crate) fn prune_edge_attributes(&mut self) {
        let mut used = BTreeSet::new();
        for f in &self.faces {
            let cs =
                &self.corners[f.first_corner as usize..(f.first_corner + f.corner_count) as usize];
            for i in 0..cs.len() {
                used.insert(EdgeKey::new(cs[i].vertex, cs[(i + 1) % cs.len()].vertex));
            }
        }
        self.edge_data.retain(|e, d| d.loose || used.contains(e));
    }
    pub(crate) fn selected_vertices(
        &self,
        ids: &[VertexId],
        ctx: &mut Context<'_>,
    ) -> Result<BTreeSet<VertexId>> {
        ctx.checkpoint(ids.len() as u64)?;
        ctx.limit(
            "selected vertices",
            ids.len() as u64,
            ctx.limits.max_vertices as u64,
        )?;
        ctx.bytes(
            self.estimated_bytes()
                .saturating_add(ids.len().saturating_mul(64)),
        )?;
        let set: BTreeSet<_> = ids.iter().copied().collect();
        if set.len() != ids.len() {
            return Err(invalid("vertex selection contains duplicates"));
        }
        for &id in &set {
            if self.vertex(id).is_none() {
                return Err(MeshError::UnknownElement(ElementId::Vertex(id)));
            }
        }
        Ok(set)
    }

    pub fn set_vertex_weights(
        &mut self,
        id: VertexId,
        weights: &[JointWeight],
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        let normalized = normalize_weights(weights, ctx)?;
        let (_, changes) = self.edit(ctx, normalized.len().saturating_mul(32), |m, _| {
            let i = m
                .vertices
                .binary_search_by_key(&id, |v| v.id)
                .map_err(|_| MeshError::UnknownElement(ElementId::Vertex(id)))?;
            m.vertices[i].weights = normalized;
            Ok(((), Vec::new()))
        })?;
        Ok(changes)
    }
    /// Apply a whole weight field in one candidate edit, preserving all IDs.
    pub fn set_weights_bulk(
        &mut self,
        weights: &[(VertexId, Vec<JointWeight>)],
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        ctx.checkpoint(weights.len() as u64)?;
        ctx.limit(
            "selected vertices",
            weights.len() as u64,
            ctx.limits.max_vertices as u64,
        )?;
        let count = weights
            .iter()
            .fold(0usize, |n, (_, w)| n.saturating_add(w.len()));
        ctx.bytes(
            self.estimated_bytes()
                .saturating_add(count.saturating_mul(48))
                .saturating_add(weights.len().saturating_mul(96)),
        )?;
        let mut updates = BTreeMap::new();
        for (id, values) in weights {
            if self.vertex(*id).is_none() {
                return Err(MeshError::UnknownElement(ElementId::Vertex(*id)));
            }
            if updates
                .insert(*id, normalize_weights(values, ctx)?)
                .is_some()
            {
                return Err(invalid("duplicate vertex in weight update"));
            }
        }
        Ok(self
            .edit(
                ctx,
                count
                    .saturating_mul(48)
                    .saturating_add(weights.len().saturating_mul(96)),
                |m, ctx| {
                    for (id, values) in updates {
                        ctx.checkpoint(1)?;
                        let i = m.vertices.binary_search_by_key(&id, |v| v.id).unwrap();
                        m.vertices[i].weights = values;
                    }
                    Ok(((), Vec::new()))
                },
            )?
            .1)
    }
    pub fn set_face_material(
        &mut self,
        id: FaceId,
        material: u32,
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        Ok(self
            .edit(ctx, 0, |m, _| {
                let i = m
                    .faces
                    .binary_search_by_key(&id, |f| f.id)
                    .map_err(|_| MeshError::UnknownElement(ElementId::Face(id)))?;
                m.faces[i].material = material;
                Ok(((), Vec::new()))
            })?
            .1)
    }
    pub fn set_corner_uv(
        &mut self,
        id: CornerId,
        uv: [f64; 2],
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        check_uv(uv)?;
        Ok(self
            .edit(ctx, 0, |m, _| {
                let c = m
                    .corners
                    .iter_mut()
                    .find(|c| c.id == id)
                    .ok_or(MeshError::UnknownElement(ElementId::Corner(id)))?;
                c.uv = uv;
                Ok(((), Vec::new()))
            })?
            .1)
    }
    pub fn set_corner_normal(
        &mut self,
        id: CornerId,
        normal: Option<[f64; 3]>,
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        let normal = normal
            .map(|n| -> Result<[f64; 3]> {
                check_position(n)?;
                let len = length(n);
                if len == 0. {
                    return Err(invalid("normal must be nonzero"));
                }
                Ok(mul(n, 1. / len))
            })
            .transpose()?;
        Ok(self
            .edit(ctx, 0, |m, _| {
                let c = m
                    .corners
                    .iter_mut()
                    .find(|c| c.id == id)
                    .ok_or(MeshError::UnknownElement(ElementId::Corner(id)))?;
                c.normal = normal;
                Ok(((), Vec::new()))
            })?
            .1)
    }
    pub fn set_edge_attributes(
        &mut self,
        key: EdgeKey,
        attributes: EdgeAttributes,
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        check_edge_attributes(attributes)?;
        if key.0 >= key.1 {
            return Err(invalid("edge key endpoints must be distinct and sorted"));
        }
        Ok(self
            .edit(ctx, 128, |m, ctx| {
                if !m.adjacency(ctx)?.edges.contains_key(&key) {
                    return Err(MeshError::UnknownElement(ElementId::Edge(key)));
                }
                let loose = m.edge_data.get(&key).is_some_and(|d| d.loose);
                if loose || attributes != EdgeAttributes::default() {
                    m.edge_data.insert(key, EdgeData { attributes, loose });
                } else {
                    m.edge_data.remove(&key);
                }
                Ok(((), Vec::new()))
            })?
            .1)
    }
    pub fn add_loose_edge(
        &mut self,
        a: VertexId,
        b: VertexId,
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        if a == b {
            return Err(invalid("loose edge needs distinct vertices"));
        }
        self.selected_vertices(&[a, b], ctx)?;
        Ok(self
            .edit(ctx, 256, |m, _| {
                m.edge_data.entry(EdgeKey::new(a, b)).or_default().loose = true;
                Ok(((), Vec::new()))
            })?
            .1)
    }

    /// Row-major affine matrix. An empty selection is a no-op, not "all".
    /// Affected split normals are invalidated and recomputed on evaluation.
    pub fn transform(
        &mut self,
        vertices: &[VertexId],
        matrix: [[f64; 4]; 4],
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        if matrix.iter().flatten().any(|v| !v.is_finite()) || matrix[3] != [0., 0., 0., 1.] {
            return Err(invalid(
                "transform must be a finite row-major affine matrix",
            ));
        }
        let selected = self.selected_vertices(vertices, ctx)?;
        Ok(self
            .edit(ctx, selected.len().saturating_mul(64), |m, ctx| {
                let columns:[[f64;3];3]=std::array::from_fn(|j|std::array::from_fn(|i|matrix[i][j]));
                let cofactors=[cross(columns[1],columns[2]),cross(columns[2],columns[0]),cross(columns[0],columns[1])];
                let reverse_all=selected.len()==m.vertices.len()&&dot(columns[0],cofactors[0])<0.;
                for v in &mut m.vertices {
                    ctx.checkpoint(1)?;
                    if selected.contains(&v.id) {
                        v.position = std::array::from_fn(|i| {
                            matrix[i][0] * v.position[0]
                                + matrix[i][1] * v.position[1]
                                + matrix[i][2] * v.position[2]
                                + matrix[i][3]
                        });
                        check_position(v.position)?;
                    }
                }
                for f in &m.faces {
                    ctx.checkpoint(1)?;
                    let cs = &mut m.corners
                        [f.first_corner as usize..(f.first_corner + f.corner_count) as usize];
                    if cs.iter().any(|c| selected.contains(&c.vertex)) {
                        let complete=cs.iter().all(|c|selected.contains(&c.vertex));
                        for c in cs.iter_mut() {
                            c.normal=if complete {c.normal.and_then(|n|{let mut mapped=[0.;3];for d in 0..3{mapped=add(mapped,mul(cofactors[d],n[d]));}
                                if reverse_all{mapped=mul(mapped,-1.);}let len=length(mapped);if len>1e-12&&len.is_finite(){Some(mul(mapped,1./len))}else{None}})}else{None};
                        }
                        if reverse_all{cs.reverse();}
                    }
                }
                Ok(((), Vec::new()))
            })?
            .1)
    }

    /// Deletes only the selected faces. Vertices survive as editable loose points;
    /// unreferenced non-loose edge attributes are discarded.
    pub fn delete_faces(&mut self, faces: &[FaceId], ctx: &mut Context<'_>) -> Result<ChangeSet> {
        ctx.checkpoint(faces.len() as u64)?;
        ctx.limit(
            "selected faces",
            faces.len() as u64,
            ctx.limits.max_faces as u64,
        )?;
        ctx.bytes(
            self.estimated_bytes()
                .saturating_add(faces.len().saturating_mul(64)),
        )?;
        let remove: BTreeSet<_> = faces.iter().copied().collect();
        if remove.len() != faces.len() {
            return Err(invalid("face selection contains duplicates"));
        }
        for &id in &remove {
            if self.face(id).is_none() {
                return Err(MeshError::UnknownElement(ElementId::Face(id)));
            }
        }
        Ok(self
            .edit(ctx, self.corners.len().saturating_mul(80), |m, ctx| {
                m.repack_faces(&remove, ctx)?;
                Ok(((), Vec::new()))
            })?
            .1)
    }

    /// Extrudes one manifold face, preserving its ID and corner UVs as the cap.
    /// Sides inherit its material and receive a continuous perimeter/height UV
    /// strip in object units. Copied vertices inherit weights; rim edges inherit
    /// source seam/crease attributes. Offset must leave the source face plane.
    pub fn extrude_face(
        &mut self,
        face: FaceId,
        offset: [f64; 3],
        ctx: &mut Context<'_>,
    ) -> Result<ExtrudeResult> {
        check_position(offset)?;
        let n = self.face_corners(face)?.len();
        ctx.counts(
            self.vertices.len().saturating_add(n),
            self.faces.len().saturating_add(n),
            self.corners.len().saturating_add(n.saturating_mul(4)),
            self.edge_data.len().saturating_add(n),
        )?;
        let growth = n.saturating_mul(8192);
        let ((side_faces, rim_edges), mut changes) = self.edit(ctx, growth, |m, ctx| {
            let geometry = face_geometry(m, face, ctx)?;
            let h = dot(offset, geometry.normal).abs();
            if h <= length(offset) * 1e-10 || h == 0. {
                return Err(invalid("extrusion offset must leave the face plane"));
            }
            let adjacency = m.adjacency(ctx)?;
            let report = m.validate(ctx)?;
            let source = m.face_corners(face)?.to_vec();
            for c in &source {
                if report.issues.iter().any(|i| {
                    i.element == ElementId::Vertex(c.vertex)
                        && i.kind == ValidationKind::NonManifoldVertex
                }) {
                    return Err(invalid("extrusion needs a manifold vertex neighborhood"));
                }
            }
            for i in 0..n {
                let uses =
                    adjacency.radial(EdgeKey::new(source[i].vertex, source[(i + 1) % n].vertex));
                if uses.len() > 2 || (uses.len() == 2 && uses[0].from == uses[1].from) {
                    return Err(invalid(
                        "extrusion needs consistently oriented manifold edges",
                    ));
                }
            }
            let material = m.face(face).unwrap().material;
            let mut new_vertices = Vec::with_capacity(n);
            let mut remaps = Vec::new();
            for c in &source {
                ctx.checkpoint(1)?;
                let v = m.vertex(c.vertex).unwrap();
                let position = add(v.position, offset);
                check_position(position)?;
                let id = m.push_vertex(position, v.weights.clone())?;
                new_vertices.push(id);
                remaps.push((
                    ElementId::Vertex(c.vertex),
                    vec![ElementId::Vertex(c.vertex), ElementId::Vertex(id)],
                ));
            }
            let mut side_faces = Vec::with_capacity(n);
            let mut rim_edges = Vec::with_capacity(n);
            let mut u = 0.;
            for i in 0..n {
                ctx.checkpoint(1)?;
                let j = (i + 1) % n;
                let width = length(sub(
                    m.vertex(source[i].vertex).unwrap().position,
                    m.vertex(source[j].vertex).unwrap().position,
                ));
                side_faces.push(m.push_face(
                    &[
                        (source[i].vertex, [u, 0.], None),
                        (source[j].vertex, [u + width, 0.], None),
                        (new_vertices[j], [u + width, length(offset)], None),
                        (new_vertices[i], [u, length(offset)], None),
                    ],
                    material,
                )?);
                let old = EdgeKey::new(source[i].vertex, source[j].vertex);
                let rim = EdgeKey::new(new_vertices[i], new_vertices[j]);
                if let Some(d) = m.edge_data.get(&old).copied() {
                    if d.attributes != EdgeAttributes::default() {
                        m.edge_data.insert(
                            rim,
                            EdgeData {
                                attributes: d.attributes,
                                loose: false,
                            },
                        );
                    }
                }
                remaps.push((
                    ElementId::Edge(old),
                    vec![ElementId::Edge(old), ElementId::Edge(rim)],
                ));
                rim_edges.push(rim);
                u += width;
            }
            let f = m.face(face).unwrap();
            let start = f.first_corner as usize;
            for (i, &v) in new_vertices.iter().enumerate() {
                m.corners[start + i].vertex = v;
            }
            Ok(((side_faces, rim_edges), remaps))
        })?;
        // Cap and its corner identities survive the operation.
        changes
            .remapped
            .push((ElementId::Face(face), vec![ElementId::Face(face)]));
        Ok(ExtrudeResult {
            cap: face,
            side_faces,
            rim_edges,
            changes,
        })
    }

    /// Duplicates the entire mesh reflected about coordinate `axis = offset`.
    /// Winding is reversed, UV seams and sparse weights are preserved. Welding
    /// coincident vertices is a separate explicit operation.
    pub fn mirror(&mut self, axis: usize, offset: f64, ctx: &mut Context<'_>) -> Result<ChangeSet> {
        if axis >= 3 || !offset.is_finite() || offset.abs() > 1e100 {
            return Err(invalid("mirror requires axis 0..2 and finite plane offset"));
        }
        ctx.counts(
            self.vertices.len().saturating_mul(2),
            self.faces.len().saturating_mul(2),
            self.corners.len().saturating_mul(2),
            self.edge_data.len().saturating_mul(2),
        )?;
        let growth = self.estimated_bytes().saturating_mul(3);
        Ok(self
            .edit(ctx, growth, |m, ctx| {
                let old_vertices = m.vertices.len();
                let old_faces = m.faces.len();
                let mut mapping = BTreeMap::new();
                let mut remaps = Vec::new();
                for i in 0..old_vertices {
                    ctx.checkpoint(1)?;
                    let v = &m.vertices[i];
                    let old = v.id;
                    let mut p = v.position;
                    p[axis] = 2. * offset - p[axis];
                    check_position(p)?;
                    let id = m.push_vertex(p, v.weights.clone())?;
                    mapping.insert(old, id);
                    remaps.push((
                        ElementId::Vertex(old),
                        vec![ElementId::Vertex(old), ElementId::Vertex(id)],
                    ));
                }
                for i in 0..old_faces {
                    ctx.checkpoint(1)?;
                    let f = &m.faces[i];
                    let old = f.id;
                    let material = f.material;
                    let corners = m
                        .face_corners(old)?
                        .iter()
                        .rev()
                        .map(|c| {
                            let normal = c.normal.map(|mut n| {
                                n[axis] = -n[axis];
                                n
                            });
                            (mapping[&c.vertex], c.uv, normal)
                        })
                        .collect::<Vec<_>>();
                    let id = m.push_face(&corners, material)?;
                    let src = m.face_corners(old)?;
                    let dst = m.face_corners(id)?;
                    for (a, b) in src.iter().rev().zip(dst) {
                        remaps.push((
                            ElementId::Corner(a.id),
                            vec![ElementId::Corner(a.id), ElementId::Corner(b.id)],
                        ));
                    }
                    remaps.push((
                        ElementId::Face(old),
                        vec![ElementId::Face(old), ElementId::Face(id)],
                    ));
                }
                let edges = m.edge_data.clone();
                for (e, d) in edges {
                    ctx.checkpoint(1)?;
                    m.edge_data
                        .insert(EdgeKey::new(mapping[&e.0], mapping[&e.1]), d);
                }
                Ok(((), remaps))
            })?
            .1)
    }

    /// Deterministic greedy weld: ascending stable IDs choose representatives.
    /// Positions remain at the lowest-ID representative; weights average across
    /// its group, corner UVs remain discontinuous, seams OR and creases take max.
    /// Collapsed faces disappear; a pinched/self-crossing surviving face rejects
    /// the whole edit. Nonmanifold neighborhoods remain representable/diagnosable.
    pub fn weld(
        &mut self,
        vertices: &[VertexId],
        distance: f64,
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        if !distance.is_finite() || distance < 0. || distance > 1e100 {
            return Err(invalid("weld distance must be finite and nonnegative"));
        }
        let selected = self.selected_vertices(vertices, ctx)?;
        Ok(self
            .edit(
                ctx,
                self.estimated_bytes()
                    .saturating_add(selected.len().saturating_mul(256)),
                |m, ctx| {
                    let mapping = weld_groups(m, &selected, distance, ctx)?;
                    let mut groups: BTreeMap<VertexId, Vec<VertexId>> = BTreeMap::new();
                    for (&src, &dst) in &mapping {
                        groups.entry(dst).or_default().push(src);
                    }
                    for (&representative, members) in &groups {
                        ctx.checkpoint(members.len() as u64)?;
                        if members.len() < 2 {
                            continue;
                        }
                        let mut weights = BTreeMap::<u32, f64>::new();
                        for &id in members {
                            for w in &m.vertex(id).unwrap().weights {
                                *weights.entry(w.joint).or_default() +=
                                    w.weight / (members.len() as f64);
                            }
                        }
                        let weights = weights
                            .into_iter()
                            .map(|(joint, weight)| JointWeight { joint, weight })
                            .collect::<Vec<_>>();
                        let weights = normalize_weights(&weights, ctx)?;
                        let i = m
                            .vertices
                            .binary_search_by_key(&representative, |v| v.id)
                            .unwrap();
                        m.vertices[i].weights = weights;
                    }
                    let mut remaps = Vec::new();
                    for (&a, &b) in &mapping {
                        if a != b {
                            remaps.push((ElementId::Vertex(a), vec![ElementId::Vertex(b)]));
                        }
                    }
                    let mut corners = Vec::with_capacity(m.corners.len());
                    let mut faces = Vec::with_capacity(m.faces.len());
                    for f in &m.faces {
                        ctx.checkpoint(1)?;
                        let mut cs = Vec::<Corner>::new();
                        let mut changed = false;
                        for c in m.face_corners(f.id)? {
                            ctx.checkpoint(1)?;
                            let mut c = c.clone();
                            if let Some(&id) = mapping.get(&c.vertex) {
                                changed |= c.vertex != id;
                                c.vertex = id;
                            }
                            if cs.last().is_none_or(|last| last.vertex != c.vertex) {
                                cs.push(c);
                            }
                        }
                        if cs.len() > 1 && cs[0].vertex == cs.last().unwrap().vertex {
                            cs.pop();
                        }
                        if cs.len() < 3 {
                            continue;
                        }
                        if changed {
                            for c in &mut cs {
                                c.normal = None;
                            }
                        }
                        let mut face = f.clone();
                        face.first_corner = corners.len() as u32;
                        face.corner_count = cs.len() as u32;
                        corners.extend(cs);
                        faces.push(face);
                    }
                    m.corners = corners;
                    m.faces = faces;
                    let mut edges = BTreeMap::<EdgeKey, EdgeData>::new();
                    for (&edge, d) in &m.edge_data {
                        ctx.checkpoint(1)?;
                        let a = mapping.get(&edge.0).copied().unwrap_or(edge.0);
                        let b = mapping.get(&edge.1).copied().unwrap_or(edge.1);
                        if a == b {
                            continue;
                        }
                        let key = EdgeKey::new(a, b);
                        let out = edges.entry(key).or_default();
                        out.loose |= d.loose;
                        out.attributes.seam |= d.attributes.seam;
                        out.attributes.crease = out.attributes.crease.max(d.attributes.crease);
                        if edge != key {
                            remaps.push((ElementId::Edge(edge), vec![ElementId::Edge(key)]));
                        }
                    }
                    m.edge_data = edges;
                    m.vertices
                        .retain(|v| mapping.get(&v.id).is_none_or(|id| *id == v.id));
                    m.prune_edge_attributes();
                    Ok(((), remaps))
                },
            )?
            .1)
    }

    /// Inward constant-distance offset of a strictly convex planar face. The
    /// original face/corner IDs become the inset cap. Corner UVs and weights are
    /// barycentrically interpolated; concave/collapsing offsets are refused.
    pub fn inset_face(
        &mut self,
        face: FaceId,
        distance: f64,
        ctx: &mut Context<'_>,
    ) -> Result<InsetResult> {
        if !distance.is_finite() || distance <= 0. {
            return Err(invalid("inset distance must be positive and finite"));
        }
        let n = self.face_corners(face)?.len();
        ctx.counts(
            self.vertices.len().saturating_add(n),
            self.faces.len().saturating_add(n),
            self.corners.len().saturating_add(n.saturating_mul(4)),
            self.edge_data.len().saturating_add(n),
        )?;
        let (ring_faces, changes) = self.edit(ctx, n.saturating_mul(8192), |m, ctx| {
            let geometry = face_geometry(m, face, ctx)?;
            if !geometry.planar {
                return Err(invalid("inset requires a planar face"));
            }
            let source = m.face_corners(face)?.to_vec();
            let material = m.face(face).unwrap().material;
            let origin = m.vertex(source[0].vertex).unwrap().position;
            let tangent = sub(m.vertex(source[1].vertex).unwrap().position, origin);
            let tangent = mul(tangent, 1. / length(tangent));
            let bitangent = cross(geometry.normal, tangent);
            let points: Vec<[f64; 2]> = source
                .iter()
                .map(|c| {
                    let p = sub(m.vertex(c.vertex).unwrap().position, origin);
                    [dot(p, tangent), dot(p, bitangent)]
                })
                .collect();
            let extent = points.iter().flatten().fold(0f64, |a, b| a.max(b.abs()));
            let normalized: Vec<_> = points.iter().map(|p| p.map(|v| v / extent)).collect();
            let triangles = ear_clip(&normalized, face, ctx)?;
            for i in 0..n {
                if orient(
                    normalized[(i + n - 1) % n],
                    normalized[i],
                    normalized[(i + 1) % n],
                ) <= 0.
                {
                    return Err(invalid("inset currently requires a strictly convex face"));
                }
            }
            let mut positions = Vec::with_capacity(n);
            let mut uvs = Vec::with_capacity(n);
            let mut vertex_weights = Vec::with_capacity(n);
            for i in 0..n {
                ctx.checkpoint(1)?;
                let prev = points[(i + n - 1) % n];
                let p = points[i];
                let next = points[(i + 1) % n];
                let a = [p[0] - prev[0], p[1] - prev[1]];
                let b = [next[0] - p[0], next[1] - p[1]];
                let la = a[0].hypot(a[1]);
                let lb = b[0].hypot(b[1]);
                let na = [-a[1] / la, a[0] / la];
                let nb = [-b[1] / lb, b[0] / lb];
                let den = 1. + na[0] * nb[0] + na[1] * nb[1];
                if den <= 1e-12 {
                    return Err(invalid("inset corner is too sharp"));
                }
                let q = [
                    p[0] + distance * (na[0] + nb[0]) / den,
                    p[1] + distance * (na[1] + nb[1]) / den,
                ];
                let qn = q.map(|v| v / extent);
                for j in 0..n {
                    ctx.checkpoint(1)?;
                    if orient(normalized[j], normalized[(j + 1) % n], qn) <= 1e-12 {
                        return Err(invalid("inset distance collapses or crosses the face"));
                    }
                }
                let position = add(origin, add(mul(tangent, q[0]), mul(bitangent, q[1])));
                check_position(position)?;
                let (uv, weights) = interpolate(m, &source, &normalized, &triangles, qn, ctx)?;
                positions.push(position);
                uvs.push(uv);
                vertex_weights.push(weights);
            }
            let mut inner = Vec::with_capacity(n);
            let mut remaps = Vec::new();
            for i in 0..n {
                let id = m.push_vertex(positions[i], std::mem::take(&mut vertex_weights[i]))?;
                inner.push(id);
                remaps.push((
                    ElementId::Vertex(source[i].vertex),
                    vec![ElementId::Vertex(source[i].vertex), ElementId::Vertex(id)],
                ));
            }
            let mut ring_faces = Vec::with_capacity(n);
            for i in 0..n {
                let j = (i + 1) % n;
                ring_faces.push(m.push_face(
                    &[
                        (source[i].vertex, source[i].uv, None),
                        (source[j].vertex, source[j].uv, None),
                        (inner[j], uvs[j], None),
                        (inner[i], uvs[i], None),
                    ],
                    material,
                )?);
            }
            let start = m.face(face).unwrap().first_corner as usize;
            for i in 0..n {
                m.corners[start + i].vertex = inner[i];
                m.corners[start + i].uv = uvs[i];
                m.corners[start + i].normal = None;
            }
            Ok((ring_faces, remaps))
        })?;
        Ok(InsetResult {
            inset: face,
            ring_faces,
            changes,
        })
    }
}

pub(crate) fn normalize_weights(
    weights: &[JointWeight],
    ctx: &mut Context<'_>,
) -> Result<Vec<JointWeight>> {
    ctx.limit(
        "weights per vertex",
        weights.len() as u64,
        ctx.limits.max_weights_per_vertex as u64,
    )?;
    let mut merged = BTreeMap::<u32, f64>::new();
    for w in weights {
        ctx.checkpoint(1)?;
        if !w.weight.is_finite() || w.weight < 0. {
            return Err(invalid("weights must be finite and nonnegative"));
        }
        if w.weight > 0. {
            *merged.entry(w.joint).or_default() += w.weight;
        }
    }
    let sum = merged.values().sum::<f64>();
    if !sum.is_finite() {
        return Err(invalid("weight sum overflow"));
    }
    let result = merged
        .into_iter()
        .map(|(joint, weight)| JointWeight {
            joint,
            weight: weight / sum,
        })
        .collect::<Vec<_>>();
    check_weights(&result, ctx)?;
    Ok(result)
}

fn weld_groups(
    mesh: &Mesh,
    selected: &BTreeSet<VertexId>,
    distance: f64,
    ctx: &mut Context<'_>,
) -> Result<BTreeMap<VertexId, VertexId>> {
    let mut mapping = BTreeMap::new();
    if distance == 0. {
        let mut exact = BTreeMap::new();
        for &id in selected {
            ctx.checkpoint(1)?;
            let p = mesh
                .vertex(id)
                .unwrap()
                .position
                .map(|x| if x == 0. { 0 } else { x.to_bits() });
            let representative = *exact.entry(p).or_insert(id);
            mapping.insert(id, representative);
        }
    } else {
        let origin = selected
            .first()
            .map(|id| mesh.vertex(*id).unwrap().position)
            .unwrap_or([0.; 3]);
        let mut grid = BTreeMap::<[i64; 3], Vec<VertexId>>::new();
        for &id in selected {
            ctx.checkpoint(1)?;
            let p = mesh.vertex(id).unwrap().position;
            let relative = sub(p, origin).map(|v| (v / distance).floor());
            if relative
                .iter()
                .any(|v| !v.is_finite() || v.abs() >= i64::MAX as f64 - 4096.)
            {
                return Err(invalid(
                    "weld tolerance is too small for the coordinate span",
                ));
            }
            let cell = relative.map(|v| v as i64);
            let mut representative = None;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(ids) = grid.get(&[cell[0] + dx, cell[1] + dy, cell[2] + dz]) {
                            for &candidate in ids {
                                ctx.checkpoint(1)?;
                                if length(sub(mesh.vertex(candidate).unwrap().position, p))
                                    <= distance
                                    && representative.is_none_or(|old| candidate < old)
                                {
                                    representative = Some(candidate);
                                }
                            }
                        }
                    }
                }
            }
            let representative = representative.unwrap_or(id);
            if representative == id {
                grid.entry(cell).or_default().push(id);
            }
            mapping.insert(id, representative);
        }
    }
    Ok(mapping)
}

fn interpolate(
    mesh: &Mesh,
    corners: &[Corner],
    points: &[[f64; 2]],
    triangles: &[[usize; 3]],
    p: [f64; 2],
    ctx: &mut Context<'_>,
) -> Result<([f64; 2], Vec<JointWeight>)> {
    for t in triangles {
        ctx.checkpoint(1)?;
        let [a, b, c] = t.map(|i| points[i]);
        let area = orient(a, b, c);
        let w = [
            orient(b, c, p) / area,
            orient(c, a, p) / area,
            orient(a, b, p) / area,
        ];
        if w.iter().all(|w| *w >= -1e-10) {
            let mut uv = [0.; 2];
            let mut weights = BTreeMap::<u32, f64>::new();
            for k in 0..3 {
                let c = &corners[t[k]];
                let weight = w[k].max(0.);
                for i in 0..2 {
                    uv[i] += c.uv[i] * weight;
                }
                for jw in &mesh.vertex(c.vertex).unwrap().weights {
                    *weights.entry(jw.joint).or_default() += jw.weight * weight;
                }
            }
            let weights = weights
                .into_iter()
                .map(|(joint, weight)| JointWeight { joint, weight })
                .collect::<Vec<_>>();
            return Ok((uv, normalize_weights(&weights, ctx)?));
        }
    }
    Err(invalid("cannot interpolate inset point in source face"))
}
