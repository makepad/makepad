use crate::{context::invalid, geometry::*, mesh::check_position, ops::normalize_weights, *};
use std::collections::{BTreeMap, BTreeSet};

impl Mesh {
    /// Simultaneous uniform Laplacian iterations. Unselected vertices stay fixed;
    /// fixed-boundary mode also fixes selected vertices on boundary/loose edges.
    pub fn smooth(
        &mut self,
        vertices: &[VertexId],
        iterations: u32,
        factor: f64,
        preserve_boundary: bool,
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        if iterations == 0
            || iterations > 64
            || !factor.is_finite()
            || !(0.0..=1.0).contains(&factor)
        {
            return Err(invalid(
                "smooth requires 1..64 iterations and factor in [0,1]",
            ));
        }
        let selected = self.selected_vertices(vertices, ctx)?;
        let growth = self
            .vertices
            .len()
            .saturating_mul(256)
            .saturating_add(self.corners.len().saturating_mul(320));
        Ok(self
            .edit(ctx, growth, |m, ctx| {
                let adjacency = m.adjacency(ctx)?;
                let report = m.validate(ctx)?;
                for issue in &report.issues {
                    if issue.kind == ValidationKind::NonManifoldVertex
                        && matches!(issue.element,ElementId::Vertex(id) if selected.contains(&id))
                    {
                        return Err(invalid(
                            "smoothing requires manifold selected vertex neighborhoods",
                        ));
                    }
                }
                let indices: BTreeMap<_, _> = m
                    .vertices
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (v.id, i))
                    .collect();
                let mut positions = m.vertices.iter().map(|v| v.position).collect::<Vec<_>>();
                let mut next = positions.clone();
                for _ in 0..iterations {
                    for &id in &selected {
                        ctx.checkpoint(1)?;
                        let edges = &adjacency.vertex_edges[&id];
                        if edges.is_empty()
                            || (preserve_boundary
                                && edges.iter().any(|edge| adjacency.radial(*edge).len() < 2))
                        {
                            continue;
                        }
                        let mut average = [0.; 3];
                        for edge in edges {
                            ctx.checkpoint(1)?;
                            let other = if edge.0 == id { edge.1 } else { edge.0 };
                            average = add(
                                average,
                                mul(positions[indices[&other]], 1. / edges.len() as f64),
                            );
                        }
                        let i = indices[&id];
                        next[i] = add(mul(positions[i], 1. - factor), mul(average, factor));
                        check_position(next[i])?;
                    }
                    std::mem::swap(&mut positions, &mut next);
                }
                let mut changed = BTreeSet::new();
                for (v, p) in m.vertices.iter_mut().zip(positions) {
                    if v.position != p {
                        changed.insert(v.id);
                    }
                    v.position = p;
                }
                invalidate_normals(m, &changed);
                Ok(((), Vec::new()))
            })?
            .1)
    }

    /// Compact smoothstep grab brush. The maximum displacement is checked
    /// explicitly; a large delta is refused instead of silently clamped.
    pub fn brush(
        &mut self,
        vertices: &[VertexId],
        center: [f64; 3],
        radius: f64,
        delta: [f64; 3],
        max_displacement: f64,
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        check_position(center)?;
        check_position(delta)?;
        if !radius.is_finite()
            || radius <= 0.
            || !max_displacement.is_finite()
            || max_displacement < 0.
            || length(delta) > max_displacement
        {
            return Err(invalid(
                "brush requires positive radius and delta within max displacement",
            ));
        }
        let selected = self.selected_vertices(vertices, ctx)?;
        Ok(self
            .edit(ctx, selected.len().saturating_mul(64), |m, ctx| {
                let mut changed = BTreeSet::new();
                for v in &mut m.vertices {
                    ctx.checkpoint(1)?;
                    if !selected.contains(&v.id) {
                        continue;
                    }
                    let distance = length(sub(v.position, center));
                    if distance >= radius {
                        continue;
                    }
                    let t = 1. - distance / radius;
                    let weight = t * t * (3. - 2. * t);
                    let position = add(v.position, mul(delta, weight));
                    check_position(position)?;
                    if position != v.position {
                        v.position = position;
                        changed.insert(v.id);
                    }
                }
                invalidate_normals(m, &changed);
                Ok(((), Vec::new()))
            })?
            .1)
    }

    /// Project selected face corners onto YZ/ZX/XY for dropped X/Y/Z axis.
    /// Selection is one transaction; neighboring faces retain independent UVs.
    pub fn project_uv(
        &mut self,
        faces: &[FaceId],
        axis: usize,
        scale: [f64; 2],
        offset: [f64; 2],
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        if axis >= 3 {
            return Err(invalid("UV projection axis must be 0..2"));
        }
        crate::mesh::check_uv(scale)?;
        crate::mesh::check_uv(offset)?;
        let selected = select_faces(self, faces, ctx)?;
        Ok(self
            .edit(ctx, faces.len().saturating_mul(64), |m, ctx| {
                for face in &m.faces {
                    ctx.checkpoint(1)?;
                    if !selected.contains(&face.id) {
                        continue;
                    }
                    let start = face.first_corner as usize;
                    for i in start..start + face.corner_count as usize {
                        ctx.checkpoint(1)?;
                        if m.uv_pins.contains(&m.corners[i].id){continue;}
                        let p = m.vertex(m.corners[i].vertex).unwrap().position;
                        let projected = match axis {
                            0 => [p[1], p[2]],
                            1 => [p[2], p[0]],
                            _ => [p[0], p[1]],
                        };
                        let uv = std::array::from_fn(|d| projected[d] * scale[d] + offset[d]);
                        crate::mesh::check_uv(uv)?;
                        m.corners[i].uv = uv;
                    }
                }
                Ok(((), Vec::new()))
            })?
            .1)
    }

    /// Assign many faces without one whole-mesh candidate per face.
    pub fn set_face_materials(
        &mut self,
        faces: &[FaceId],
        material: u32,
        ctx: &mut Context<'_>,
    ) -> Result<ChangeSet> {
        let selected = select_faces(self, faces, ctx)?;
        Ok(self
            .edit(ctx, faces.len().saturating_mul(64), |m, ctx| {
                for face in &mut m.faces {
                    ctx.checkpoint(1)?;
                    if selected.contains(&face.id) {
                        face.material = material;
                    }
                }
                Ok(((), Vec::new()))
            })?
            .1)
    }

    /// Catmull–Clark on an oriented manifold surface with no loose elements.
    /// Boundaries use the cubic boundary stencil. Crease strength blends the
    /// smooth and sharp stencils; seam and crease markers propagate to children.
    /// UVs use face-local bilinear refinement, retaining discontinuous islands.
    pub fn subdivide(&mut self, levels: u32, ctx: &mut Context<'_>) -> Result<ChangeSet> {
        if levels == 0 || levels > 3 {
            return Err(invalid("subdivision requires 1..3 levels"));
        }
        let (mut nv, mut nf, mut nc) = (self.vertices.len(), self.faces.len(), self.corners.len());
        for _ in 0..levels {
            nv = nv.saturating_add(nc).saturating_add(nf);
            nf = nc;
            nc = nc.saturating_mul(4);
            ctx.counts(nv, nf, nc, nc)?;
        }
        let growth = nv
            .saturating_mul(1536)
            .saturating_add(nc.saturating_mul(256));
        Ok(self
            .edit(ctx, growth, |m, ctx| {
                let original_faces = m.faces.iter().map(|f| f.id).collect::<Vec<_>>();
                let mut descendants: BTreeMap<FaceId, Vec<FaceId>> =
                    original_faces.iter().map(|&f| (f, vec![f])).collect();
                let mut remaps = Vec::new();
                for level in 0..levels {
                    ctx.checkpoint(1)?;
                    let (next, face_children, local_remaps) = subdivide_once(m, ctx)?;
                    for faces in descendants.values_mut() {
                        *faces = faces
                            .iter()
                            .flat_map(|f| face_children[f].iter().copied())
                            .collect();
                    }
                    // Only source IDs from the pre-operation mesh belong in the
                    // final remap. Face descendants are composed across levels.
                    if level == 0 {
                        remaps = local_remaps;
                    } else {
                        let mappings = local_remaps.into_iter().collect::<BTreeMap<_, _>>();
                        for (_, targets) in &mut remaps {
                            *targets = targets
                                .iter()
                                .flat_map(|target| {
                                    mappings
                                        .get(target)
                                        .cloned()
                                        .unwrap_or_else(|| vec![*target])
                                })
                                .collect();
                        }
                    }
                    *m = next;
                }
                remaps.retain(|(source, _)| !matches!(source, ElementId::Face(_)));
                for (face, children) in descendants {
                    remaps.push((
                        ElementId::Face(face),
                        children.into_iter().map(ElementId::Face).collect(),
                    ));
                }
                smooth_normals(m, ctx)?;
                Ok(((), remaps))
            })?
            .1)
    }
}

pub(crate) fn select_faces(mesh: &Mesh, faces: &[FaceId], ctx: &mut Context<'_>) -> Result<BTreeSet<FaceId>> {
    ctx.checkpoint(faces.len() as u64)?;
    ctx.limit(
        "selected faces",
        faces.len() as u64,
        ctx.limits.max_faces as u64,
    )?;
    ctx.bytes(
        mesh.memory_bytes()
            .saturating_add(faces.len().saturating_mul(64)),
    )?;
    let set: BTreeSet<_> = faces.iter().copied().collect();
    if set.len() != faces.len() {
        return Err(invalid("duplicate face selection"));
    }
    for &face in &set {
        if mesh.face(face).is_none() {
            return Err(MeshError::UnknownElement(ElementId::Face(face)));
        }
    }
    Ok(set)
}
pub(crate) fn invalidate_normals(mesh: &mut Mesh, vertices: &BTreeSet<VertexId>) {
    for face in &mesh.faces {
        let cs = &mut mesh.corners
            [face.first_corner as usize..(face.first_corner + face.corner_count) as usize];
        if cs.iter().any(|c| vertices.contains(&c.vertex)) {
            for c in cs {
                c.normal = None;
            }
        }
    }
}

type Stencil = BTreeMap<VertexId, f64>;
fn add_stencil(target: &mut Stencil, source: &Stencil, factor: f64) {
    for (&id, &weight) in source {
        *target.entry(id).or_default() += weight * factor;
    }
}
fn face_stencil(mesh: &Mesh, face: FaceId) -> Stencil {
    let corners = mesh.face_corners(face).unwrap();
    corners
        .iter()
        .map(|c| (c.vertex, 1. / corners.len() as f64))
        .collect()
}
fn evaluate_stencil(
    mesh: &Mesh,
    stencil: &Stencil,
    ctx: &mut Context<'_>,
) -> Result<([f64; 3], Vec<JointWeight>)> {
    let mut position = [0.; 3];
    let mut weights = BTreeMap::<u32, f64>::new();
    for (&id, &factor) in stencil {
        ctx.checkpoint(1)?;
        if factor < 0. {
            return Err(invalid(
                "subdivision produced a negative interpolation weight",
            ));
        }
        let v = mesh.vertex(id).unwrap();
        position = add(position, mul(v.position, factor));
        for w in &v.weights {
            *weights.entry(w.joint).or_default() += w.weight * factor;
        }
    }
    check_position(position)?;
    let weights = weights
        .into_iter()
        .map(|(joint, weight)| JointWeight { joint, weight })
        .collect::<Vec<_>>();
    Ok((position, normalize_weights(&weights, ctx)?))
}
fn subdivide_once(
    mesh: &Mesh,
    ctx: &mut Context<'_>,
) -> Result<(
    Mesh,
    BTreeMap<FaceId, Vec<FaceId>>,
    Vec<(ElementId, Vec<ElementId>)>,
)> {
    let report = mesh.validate(ctx)?;
    if !report.is_valid_surface
        || report.loose_edges > 0
        || report.isolated_vertices > 0
        || mesh.faces.is_empty()
    {
        return Err(invalid(
            "subdivision requires an oriented manifold surface without loose elements",
        ));
    }
    if mesh.edge_data.values().any(|d| d.loose) {
        return Err(invalid("subdivision does not consume explicit loose edges"));
    }
    let adjacency = mesh.adjacency(ctx)?;
    ctx.counts(
        mesh.vertices
            .len()
            .saturating_add(adjacency.edges.len())
            .saturating_add(mesh.faces.len()),
        mesh.corners.len(),
        mesh.corners.len().saturating_mul(4),
        mesh.corners.len().saturating_mul(4),
    )?;
    let mut next = Mesh::new();
    next.next_id = mesh.next_id;
    for v in &mesh.vertices {
        ctx.checkpoint(1)?;
        let edges = &adjacency.vertex_edges[&v.id];
        let boundary = edges
            .iter()
            .filter(|e| adjacency.radial(**e).len() == 1)
            .copied()
            .collect::<Vec<_>>();
        let mut stencil = Stencil::new();
        if !boundary.is_empty() {
            if boundary.len() != 2 {
                return Err(invalid("invalid subdivision boundary fan"));
            }
            stencil.insert(v.id, 0.75);
            for &e in &boundary {
                let other = if e.0 == v.id { e.1 } else { e.0 };
                *stencil.entry(other).or_default() += 0.125;
            }
        } else {
            let n = edges.len();
            if n < 3 {
                return Err(invalid(
                    "subdivision interior valence must be at least three",
                ));
            }
            let faces = &adjacency.vertex_faces[&v.id];
            if faces.len() != n {
                return Err(invalid("invalid subdivision vertex fan"));
            }
            stencil.insert(v.id, (n as f64 - 3.) / n as f64);
            for &face in faces {
                ctx.checkpoint(1)?;
                add_stencil(&mut stencil, &face_stencil(mesh, face), 1. / (n * n) as f64);
            }
            for e in edges {
                let other = if e.0 == v.id { e.1 } else { e.0 };
                *stencil.entry(v.id).or_default() += 1. / (n * n) as f64;
                *stencil.entry(other).or_default() += 1. / (n * n) as f64;
            }
        }
        if boundary.is_empty() {
            let mut sharp=edges.iter().filter_map(|e|mesh.edge_data.get(e).filter(|d|d.attributes.crease>0.).map(|d|(*e,d.attributes.crease))).collect::<Vec<_>>();
            sharp.sort_by(|a,b|b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            if sharp.len()>=2 {let mut crease=Stencil::new();let strength=if sharp.len()>=3 {crease.insert(v.id,1.);sharp[2].1}else{
                crease.insert(v.id,0.75);for &(e,_) in &sharp[..2]{let other=if e.0==v.id{e.1}else{e.0};*crease.entry(other).or_default()+=0.125;}sharp[1].1};
                for weight in stencil.values_mut(){*weight*=1.-strength;}add_stencil(&mut stencil,&crease,strength);
            }
        }
        let (position, weights) = evaluate_stencil(mesh, &stencil, ctx)?;
        next.vertices.push(Vertex {
            id: v.id,
            position,
            weights,
        });
    }
    let mut edge_points = BTreeMap::new();
    let mut remaps = Vec::new();
    for (&edge, uses) in &adjacency.edges {
        ctx.checkpoint(1)?;
        let mut stencil = Stencil::new();
        if uses.len() == 1 {
            stencil.insert(edge.0, 0.5);
            stencil.insert(edge.1, 0.5);
        } else {
            stencil.insert(edge.0, 0.25);
            stencil.insert(edge.1, 0.25);
            for u in uses {
                add_stencil(&mut stencil, &face_stencil(mesh, u.face), 0.25);
            }
            let sharp=mesh.edge_data.get(&edge).map_or(0.,|d|d.attributes.crease);
            if sharp>0.{for value in stencil.values_mut(){*value*=1.-sharp;}*stencil.entry(edge.0).or_default()+=sharp*0.5;*stencil.entry(edge.1).or_default()+=sharp*0.5;}
        }
        let (p, w) = evaluate_stencil(mesh, &stencil, ctx)?;
        let id = next.push_vertex(p, w)?;
        edge_points.insert(edge, id);
        let children = [EdgeKey::new(edge.0, id), EdgeKey::new(id, edge.1)];
        remaps.push((
            ElementId::Edge(edge),
            children.into_iter().map(ElementId::Edge).collect(),
        ));
        if let Some(data) = mesh.edge_data.get(&edge) {
            for child in children {
                next.edge_data.insert(
                    child,
                    EdgeData {
                        attributes: data.attributes,
                        loose: false,
                    },
                );
            }
        }
    }
    let mut centers = BTreeMap::new();
    for f in &mesh.faces {
        let (p, w) = evaluate_stencil(mesh, &face_stencil(mesh, f.id), ctx)?;
        let id = next.push_vertex(p, w)?;
        centers.insert(f.id, id);
    }
    let mut face_children = BTreeMap::new();
    for f in &mesh.faces {
        ctx.checkpoint(1)?;
        let cs = mesh.face_corners(f.id)?;
        let mut center_uv = [0.; 2];
        for c in cs {
            for d in 0..2 {
                center_uv[d] += c.uv[d] / cs.len() as f64;
            }
        }
        let mut children = Vec::with_capacity(cs.len());
        for i in 0..cs.len() {
            ctx.checkpoint(1)?;
            let prev = &cs[(i + cs.len() - 1) % cs.len()];
            let c = &cs[i];
            let after = &cs[(i + 1) % cs.len()];
            let outgoing = edge_points[&EdgeKey::new(c.vertex, after.vertex)];
            let incoming = edge_points[&EdgeKey::new(prev.vertex, c.vertex)];
            let uv_after = std::array::from_fn(|d| (c.uv[d] + after.uv[d]) * 0.5);
            let uv_before = std::array::from_fn(|d| (prev.uv[d] + c.uv[d]) * 0.5);
            let face = next.push_face(
                &[
                    (c.vertex, c.uv, None),
                    (outgoing, uv_after, None),
                    (centers[&f.id], center_uv, None),
                    (incoming, uv_before, None),
                ],
                f.material,
            )?;
            let corner = next.face_corners(face)?[0].id;
            remaps.push((ElementId::Corner(c.id), vec![ElementId::Corner(corner)]));
            children.push(face);
        }
        face_children.insert(f.id, children);
    }
    next.check_structure(ctx)?;
    next.check_faces(ctx)?;
    Ok((next, face_children, remaps))
}
fn smooth_normals(mesh: &mut Mesh, ctx: &mut Context<'_>) -> Result<()> {
    crate::shading::derive_normals(mesh,true,std::f64::consts::PI,ctx)
}
