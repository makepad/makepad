use crate::quad::{make_3d_island, make_3d_quad, MpCompositedQuad, MpInternal3dIsland};
use crate::scene::*;
use crate::*;

#[derive(Clone)]
pub(crate) struct MpEvaluatedNode {
    pub(crate) flat: Mat4f,
    pub(crate) projected: Mat4f,
    pub(crate) projection: Mat4f,
    pub(crate) descendant_projection: Mat4f,
    pub(crate) world_to_local_flat: Option<Mat4f>,
    pub(crate) active_clip: Option<MpClipNodeId>,
    pub(crate) opacity: f32,
    pub(crate) backface_hidden: bool,
    pub(crate) is_3d: bool,
    pub(crate) descendant_is_3d: bool,
    pub(crate) island_root: Option<MpNodeId>,
    pub(crate) descendant_island_root: Option<MpNodeId>,
}

#[derive(Clone, Default)]
struct MpEvaluatedClipNode {
    planes: Vec<Vec4f>,
    masks: Vec<MpEvaluatedMask>,
}

pub(crate) struct MpEvaluatedScene {
    nodes: Vec<MpEvaluatedNode>,
    clip_nodes: Vec<Option<MpEvaluatedClipNode>>,
}

impl MpEvaluatedScene {
    pub(crate) fn node(&self, node_id: MpNodeId) -> &MpEvaluatedNode {
        &self.nodes[node_id]
    }

    fn project_point(
        &self,
        scene: &MpScene,
        node_id: MpNodeId,
        local_point: DVec2,
    ) -> Result<MpProjectedPoint, MpProjectError> {
        ensure_projectable_node(scene, node_id)?;
        let node = self.node(node_id);
        if node.backface_hidden {
            return Err(MpProjectError::BackfaceHidden(node_id));
        }
        let clip = node
            .projected
            .transform_vec4(vec4f(local_point.x as f32, local_point.y as f32, 0.0, 1.0));
        if clip.w.abs() <= 1e-6 {
            return Err(MpProjectError::NotProjectable(node_id));
        }
        let projected = vec2(clip.x / clip.w, clip.y / clip.w);
        if !self.point_inside_clip_chain(scene, node.active_clip, projected) {
            return Err(MpProjectError::Clipped(node_id));
        }
        Ok(MpProjectedPoint {
            screen_point: dvec2(projected.x as f64, projected.y as f64),
            depth: clip.z / clip.w,
        })
    }

    fn unproject_point(
        &self,
        scene: &MpScene,
        node_id: MpNodeId,
        screen_point: DVec2,
    ) -> Result<DVec2, MpProjectError> {
        ensure_projectable_node(scene, node_id)?;
        let node = self.node(node_id);
        if node.is_3d {
            return Err(MpProjectError::NotInvertible(node_id));
        }
        let Some(inverse) = node.world_to_local_flat else {
            return Err(MpProjectError::NotInvertible(node_id));
        };
        let local = inverse.transform_vec4(vec4f(screen_point.x as f32, screen_point.y as f32, 0.0, 1.0));
        if local.w.abs() <= 1e-6 {
            return Err(MpProjectError::NotProjectable(node_id));
        }
        Ok(dvec2((local.x / local.w) as f64, (local.y / local.w) as f64))
    }

    fn hit_test(
        &self,
        scene: &MpScene,
        screen_point: DVec2,
        options: MpHitTestOptions,
    ) -> Vec<MpHit> {
        let mut hits = Vec::new();
        for node_id in 0..scene.nodes.len() {
            if ensure_projectable_node(scene, node_id).is_err() {
                continue;
            }
            let node = self.node(node_id);
            if options.backface && node.backface_hidden {
                continue;
            }
            let clip_hit = self.point_inside_clip_chain(
                scene,
                node.active_clip,
                vec2(screen_point.x as f32, screen_point.y as f32),
            );
            if options.clip && !clip_hit {
                continue;
            }
            let local_point = self
                .unproject_point(scene, node_id, screen_point)
                .unwrap_or(dvec2(0.0, 0.0));
            let depth = self.project_point(scene, node_id, local_point).map(|p| p.depth).unwrap_or(0.0);
            hits.push(MpHit {
                node_id,
                local_point,
                depth,
                clip_hit,
                backface_visible: !node.backface_hidden,
            });
        }
        hits.sort_by(|a, b| {
            b.depth
                .partial_cmp(&a.depth)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits
    }

    fn point_inside_clip_chain(
        &self,
        scene: &MpScene,
        clip_id: Option<MpClipNodeId>,
        screen_point: Vec2f,
    ) -> bool {
        let mut current = clip_id;
        while let Some(id) = current {
            let Some(clip_node) = self.clip_nodes.get(id).and_then(|clip_node| clip_node.as_ref()) else {
                return false;
            };
            if !clip_node
                .planes
                .iter()
                .all(|plane| plane.x * screen_point.x + plane.y * screen_point.y + plane.w >= 0.0)
            {
                return false;
            }
            let clip_point = vec4f(screen_point.x, screen_point.y, 0.0, 1.0);
            if !clip_node
                .masks
                .iter()
                .all(|mask| point_inside_evaluated_mask(mask, clip_point))
            {
                return false;
            }
            current = match scene.node(id) {
                Some(MpNode::Clip(clip)) => clip.prev,
                _ => None,
            };
        }
        true
    }
}

pub(crate) fn evaluate_scene(scene: &MpScene) -> Result<MpEvaluatedScene, MpProjectError> {
    let mut nodes: Vec<Option<MpEvaluatedNode>> = vec![None; scene.nodes.len()];
    let mut clip_nodes: Vec<Option<MpEvaluatedClipNode>> = vec![None; scene.nodes.len()];
    for node_id in 0..scene.nodes.len() {
        let _ = evaluate_node(scene, node_id, &mut nodes, &mut clip_nodes)?;
    }
    Ok(MpEvaluatedScene {
        nodes: nodes.into_iter().map(|node| node.expect("all nodes evaluated")).collect(),
        clip_nodes,
    })
}

fn evaluate_node(
    scene: &MpScene,
    node_id: MpNodeId,
    cache: &mut [Option<MpEvaluatedNode>],
    clip_nodes: &mut [Option<MpEvaluatedClipNode>],
) -> Result<MpEvaluatedNode, MpProjectError> {
    if let Some(node) = cache[node_id].clone() {
        return Ok(node);
    }

    let parent_id = scene_node_parent(scene, node_id)?;
    let parent = match parent_id {
        Some(parent_id) => evaluate_node(scene, parent_id, cache, clip_nodes)?,
        None => MpEvaluatedNode {
            flat: scene.root.page_to_host,
            projected: scene.root.page_to_host,
            projection: Mat4f::identity(),
            descendant_projection: Mat4f::identity(),
            world_to_local_flat: Some(scene.root.page_to_host.invert()),
            active_clip: scene.root.clip,
            opacity: 1.0,
            backface_hidden: false,
            is_3d: false,
            descendant_is_3d: false,
            island_root: None,
            descendant_island_root: None,
        },
    };

    let evaluated = match scene.node(node_id).ok_or(MpProjectError::MissingNode(node_id))? {
        MpNode::ReferenceFrame(reference) => {
            let flat = Mat4f::mul(&parent.flat, &reference.transform);
            let mut projection = parent.descendant_projection;
            let mut is_3d = parent.descendant_is_3d;
            if let Some(perspective) = reference.perspective {
                projection = Mat4f::mul(&perspective, &projection);
                is_3d = true;
            }
            if matches!(reference.transform_style, MpTransformStyle::Preserve3D) {
                is_3d = true;
            }
            let projected = Mat4f::mul(&projection, &flat);
            let active_clip = reference.clip.or(parent.active_clip);
            let backface_hidden = parent.backface_hidden
                || (matches!(reference.backface_visibility, MpBackfaceVisibility::Hidden)
                    && projected_signed_area(&projected, reference.local_rect)
                        .map(|area| area < 0.0)
                        .unwrap_or(false));
            let island_root = if is_3d && !parent.descendant_is_3d {
                Some(node_id)
            } else {
                parent.descendant_island_root
            };
            let mut descendant_projection = projection;
            let mut descendant_is_3d = is_3d;
            let mut descendant_island_root = island_root;
            if reference.flattens_descendants {
                descendant_projection = Mat4f::identity();
                descendant_is_3d = false;
                descendant_island_root = None;
            }
            MpEvaluatedNode {
                flat,
                projected,
                projection,
                descendant_projection,
                world_to_local_flat: Some(flat.invert()),
                active_clip,
                opacity: parent.opacity,
                backface_hidden,
                is_3d,
                descendant_is_3d,
                island_root,
                descendant_island_root,
            }
        }
        MpNode::Clip(clip) => {
            let evaluated_clip = evaluate_clip_node(scene, clip, parent_id, &parent)?;
            clip_nodes[node_id] = Some(evaluated_clip);
            MpEvaluatedNode {
                flat: parent.flat,
                projected: parent.projected,
                projection: parent.projection,
                descendant_projection: parent.descendant_projection,
                world_to_local_flat: parent.world_to_local_flat,
                active_clip: parent.active_clip,
                opacity: parent.opacity,
                backface_hidden: parent.backface_hidden,
                is_3d: parent.is_3d,
                descendant_is_3d: parent.descendant_is_3d,
                island_root: parent.island_root,
                descendant_island_root: parent.descendant_island_root,
            }
        }
        MpNode::Effect(effect) => MpEvaluatedNode {
            flat: parent.flat,
            projected: parent.projected,
            projection: parent.projection,
            descendant_projection: parent.descendant_projection,
            world_to_local_flat: parent.world_to_local_flat,
            active_clip: effect.clip.or(parent.active_clip),
            opacity: (parent.opacity * effect.opacity).clamp(0.0, 1.0),
            backface_hidden: parent.backface_hidden,
            is_3d: parent.is_3d,
            descendant_is_3d: parent.descendant_is_3d,
            island_root: parent.island_root,
            descendant_island_root: parent.descendant_island_root,
        },
        MpNode::Surface(surface) => {
            let local = Mat4f::translation(vec3(
                surface.local_rect.pos.x as f32,
                surface.local_rect.pos.y as f32,
                0.0,
            ));
            let flat = Mat4f::mul(&parent.flat, &local);
            let projection = parent.descendant_projection;
            let projected = Mat4f::mul(&projection, &flat);
            MpEvaluatedNode {
                flat,
                projected,
                projection,
                descendant_projection: projection,
                world_to_local_flat: Some(flat.invert()),
                active_clip: surface.clip.or(parent.active_clip),
                opacity: parent.opacity,
                backface_hidden: parent.backface_hidden,
                is_3d: parent.descendant_is_3d,
                descendant_is_3d: parent.descendant_is_3d,
                island_root: parent.descendant_island_root,
                descendant_island_root: parent.descendant_island_root,
            }
        }
        MpNode::Embed(embed) => {
            let local = Mat4f::translation(vec3(
                embed.local_rect.pos.x as f32,
                embed.local_rect.pos.y as f32,
                0.0,
            ));
            let flat = Mat4f::mul(&parent.flat, &local);
            let projection = parent.descendant_projection;
            let projected = Mat4f::mul(&projection, &flat);
            MpEvaluatedNode {
                flat,
                projected,
                projection,
                descendant_projection: projection,
                world_to_local_flat: Some(flat.invert()),
                active_clip: embed.clip.or(parent.active_clip),
                opacity: parent.opacity,
                backface_hidden: parent.backface_hidden,
                is_3d: parent.descendant_is_3d,
                descendant_is_3d: parent.descendant_is_3d,
                island_root: parent.descendant_island_root,
                descendant_island_root: parent.descendant_island_root,
            }
        }
    };

    cache[node_id] = Some(evaluated.clone());
    Ok(evaluated)
}

fn scene_node_parent(scene: &MpScene, node_id: MpNodeId) -> Result<Option<MpNodeId>, MpProjectError> {
    Ok(match scene.node(node_id).ok_or(MpProjectError::MissingNode(node_id))? {
        MpNode::ReferenceFrame(reference) => reference.parent,
        MpNode::Clip(clip) => clip.parent,
        MpNode::Surface(surface) => Some(surface.parent),
        MpNode::Effect(effect) => Some(effect.parent),
        MpNode::Embed(embed) => Some(embed.parent),
    })
}

fn ensure_projectable_node(scene: &MpScene, node_id: MpNodeId) -> Result<(), MpProjectError> {
    match scene.node(node_id).ok_or(MpProjectError::MissingNode(node_id))? {
        MpNode::Surface(_) | MpNode::Embed(_) => Ok(()),
        _ => Err(MpProjectError::WrongNodeKind(node_id)),
    }
}

fn evaluate_clip_node(
    scene: &MpScene,
    clip: &MpClipNode,
    clip_parent: Option<MpNodeId>,
    parent: &MpEvaluatedNode,
) -> Result<MpEvaluatedClipNode, MpProjectError> {
    let transform = match clip_parent {
        Some(parent_id) => evaluate_clip_transform(scene, parent_id, parent)?,
        None => Mat4f::identity(),
    };
    let mut evaluated = MpEvaluatedClipNode::default();
    match &clip.shape {
        MpClipShape::Rect { rect } => {
            evaluated.planes = project_rect_as_planes(&transform, *rect).unwrap_or_else(|| rect_as_planes(*rect));
        }
        MpClipShape::RoundedRect { rect, radius } => {
            evaluated.masks.push(MpEvaluatedMask {
                kind: MpMaskKind::RoundedRect {
                    rect: *rect,
                    radius: *radius,
                },
                clip_to_local: transform.invert(),
            });
        }
        MpClipShape::ImageMask { rect } => {
            evaluated.masks.push(MpEvaluatedMask {
                kind: MpMaskKind::ImageMask { rect: *rect },
                clip_to_local: transform.invert(),
            });
        }
        MpClipShape::PlaneSet { planes } => {
            let plane_transform = transform.invert().transpose();
            evaluated.planes = planes
                .iter()
                .map(|plane| plane_transform.transform_vec4(*plane))
                .collect();
        }
    }
    Ok(evaluated)
}

fn evaluate_clip_transform(
    scene: &MpScene,
    owner_id: MpNodeId,
    parent: &MpEvaluatedNode,
) -> Result<Mat4f, MpProjectError> {
    let owner = scene.node(owner_id).ok_or(MpProjectError::MissingNode(owner_id))?;
    Ok(match owner {
        MpNode::ReferenceFrame(_) => parent.projected,
        MpNode::Effect(_) => parent.projected,
        MpNode::Surface(surface) => {
            let local = Mat4f::translation(vec3(
                surface.local_rect.pos.x as f32,
                surface.local_rect.pos.y as f32,
                0.0,
            ));
            Mat4f::mul(&parent.descendant_projection, &Mat4f::mul(&parent.flat, &local))
        }
        MpNode::Embed(embed) => {
            let local = Mat4f::translation(vec3(
                embed.local_rect.pos.x as f32,
                embed.local_rect.pos.y as f32,
                0.0,
            ));
            Mat4f::mul(&parent.descendant_projection, &Mat4f::mul(&parent.flat, &local))
        }
        MpNode::Clip(_) => parent.projected,
    })
}

fn project_rect_as_planes(transform: &Mat4f, rect: Rect) -> Option<Vec<Vec4f>> {
    let p0 = project_vec2(transform, rect.pos)?;
    let p1 = project_vec2(transform, dvec2(rect.pos.x + rect.size.x, rect.pos.y))?;
    let p2 = project_vec2(transform, rect.pos + rect.size)?;
    let p3 = project_vec2(transform, dvec2(rect.pos.x, rect.pos.y + rect.size.y))?;
    Some(quad_as_planes([p0, p1, p2, p3]))
}

fn quad_as_planes(points: [Vec2f; 4]) -> Vec<Vec4f> {
    let mut planes = Vec::with_capacity(4);
    for index in 0..4 {
        let from = points[index];
        let to = points[(index + 1) % 4];
        let edge = vec2(to.x - from.x, to.y - from.y);
        let len = (edge.x * edge.x + edge.y * edge.y).sqrt();
        if len <= 1e-6 {
            continue;
        }
        let normal = vec2(-edge.y / len, edge.x / len);
        let distance = -(normal.x * from.x + normal.y * from.y);
        planes.push(vec4(normal.x, normal.y, 0.0, distance));
    }
    planes
}

impl MpEvaluatedScene {
    fn collect_clip_planes(&self, scene: &MpScene, clip_id: Option<MpClipNodeId>) -> Vec<Vec4f> {
        let mut chain = Vec::new();
        let mut current = clip_id;
        while let Some(id) = current {
            if let Some(clip_node) = self.clip_nodes.get(id).and_then(|clip_node| clip_node.as_ref()) {
                chain.extend(clip_node.planes.iter().copied());
            }
            current = match scene.node(id) {
                Some(MpNode::Clip(clip)) => clip.prev,
                _ => None,
            };
        }
        chain
    }

    fn collect_clip_masks(&self, scene: &MpScene, clip_id: Option<MpClipNodeId>) -> MpMaskExec {
        let mut masks = Vec::new();
        let mut current = clip_id;
        while let Some(id) = current {
            if let Some(clip_node) = self.clip_nodes.get(id).and_then(|clip_node| clip_node.as_ref()) {
                masks.extend(clip_node.masks.iter().cloned());
            }
            current = match scene.node(id) {
                Some(MpNode::Clip(clip)) => clip.prev,
                _ => None,
            };
        }
        MpMaskExec { masks }
    }

    pub(crate) fn chain_requires_3d_plane(&self, node_id: MpNodeId) -> bool {
        let node = self.node(node_id);
        node.is_3d && (node.flat.v[2].abs() > 1e-6 || node.flat.v[6].abs() > 1e-6)
    }

    pub(crate) fn lower_surface_node(
        &self,
        scene: &MpScene,
        surface: &MpSurfaceNode,
        node_id: MpNodeId,
    ) -> Result<MpCompositedQuad, MpProjectError> {
        let texture = match &surface.source {
            MpSurfaceSource::Texture(texture) => texture.clone(),
            MpSurfaceSource::SurfaceTexture(texture) => texture.clone(),
        };
        let mut quad = MpCompositedQuad::new(
            texture,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: surface.local_rect.size,
            },
        );
        let node = self.node(node_id);
        quad.transform = node.projected;
        quad.opacity = node.opacity;
        quad.backface_visible = matches!(surface.backface_visibility, MpBackfaceVisibility::Visible);
        quad.depth_write = node.is_3d;
        quad.clip_planes = self.collect_clip_planes(scene, node.active_clip);
        quad.mask = self.collect_clip_masks(scene, node.active_clip);
        Ok(quad)
    }

    fn lower_3d_surface(
        &self,
        scene: &MpScene,
        surface: &MpSurfaceNode,
        node_id: MpNodeId,
    ) -> Result<Mp3dSurfaceExec, MpProjectError> {
        let texture = match &surface.source {
            MpSurfaceSource::Texture(texture) => texture.clone(),
            MpSurfaceSource::SurfaceTexture(texture) => texture.clone(),
        };
        let node = self.node(node_id);
        Ok(Mp3dSurfaceExec {
            texture,
            rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: surface.local_rect.size,
            },
            opacity: node.opacity,
            transform_matrix: node.flat,
            perspective_matrix: node.projection,
            backface_visibility: surface.backface_visibility,
            clip: Mp3dClipState {
                local_planes: self.collect_clip_planes(scene, node.active_clip),
            },
            mask: self.collect_clip_masks(scene, node.active_clip),
        })
    }

    fn projected_rect(&self, rect: Rect, transform: Mat4f) -> Result<Option<Rect>, MpProjectError> {
        let Some(p0) = project_vec2(&transform, rect.pos) else {
            return Ok(None);
        };
        let Some(p1) = project_vec2(&transform, dvec2(rect.pos.x + rect.size.x, rect.pos.y)) else {
            return Ok(None);
        };
        let Some(p2) = project_vec2(&transform, dvec2(rect.pos.x, rect.pos.y + rect.size.y)) else {
            return Ok(None);
        };
        let Some(p3) = project_vec2(&transform, rect.pos + rect.size) else {
            return Ok(None);
        };
        let min_x = p0.x.min(p1.x).min(p2.x).min(p3.x) as f64;
        let max_x = p0.x.max(p1.x).max(p2.x).max(p3.x) as f64;
        let min_y = p0.y.min(p1.y).min(p2.y).min(p3.y) as f64;
        let max_y = p0.y.max(p1.y).max(p2.y).max(p3.y) as f64;
        Ok(Some(Rect {
            pos: dvec2(min_x, min_y),
            size: dvec2(max_x - min_x, max_y - min_y),
        }))
    }

    fn projected_reference_frame_rect(
        &self,
        scene: &MpScene,
        node_id: MpNodeId,
    ) -> Result<Option<Rect>, MpProjectError> {
        let Some(MpNode::ReferenceFrame(reference)) = scene.node(node_id) else {
            return Ok(None);
        };
        self.projected_rect(reference.local_rect, self.node(node_id).projected)
    }

    fn lower_to_internal_quads(
        &self,
        scene: &MpScene,
    ) -> Result<Vec<(MpNodeId, MpCompositedQuad)>, MpProjectError> {
        let mut quads = Vec::new();
        for node_id in 0..scene.nodes.len() {
            match scene.node(node_id).ok_or(MpProjectError::MissingNode(node_id))? {
                MpNode::Surface(surface) => {
                    if self.chain_requires_3d_plane(node_id) {
                        continue;
                    }
                    quads.push((node_id, self.lower_surface_node(scene, surface, node_id)?));
                }
                MpNode::Embed(embed) => {
                    if self.chain_requires_3d_plane(node_id) {
                        continue;
                    }
                    for (child_id, mut quad) in embed.child_scene.lower_to_internal_quads()? {
                        quad.transform = Mat4f::mul(&quad.transform, &self.node(node_id).projected);
                        quads.push((child_id, quad));
                    }
                }
                MpNode::ReferenceFrame(_) | MpNode::Clip(_) | MpNode::Effect(_) => {}
            }
        }
        Ok(quads)
    }

    fn partition_for_execution(
        &self,
        scene: &MpScene,
    ) -> Result<(Vec<MpCompositedQuad>, Vec<MpInternal3dIsland>), MpProjectError> {
        let mut flat_quads = Vec::new();
        let mut island_order = Vec::<MpNodeId>::new();
        let mut island_surfaces: std::collections::HashMap<MpNodeId, Vec<Mp3dSurfaceExec>> = std::collections::HashMap::new();

        for node_id in 0..scene.nodes.len() {
            match scene.node(node_id).ok_or(MpProjectError::MissingNode(node_id))? {
                MpNode::Surface(surface) => {
                    if self.chain_requires_3d_plane(node_id) {
                        let root = self.node(node_id).island_root.unwrap_or(node_id);
                        island_surfaces
                            .entry(root)
                            .or_insert_with(|| {
                                island_order.push(root);
                                Vec::new()
                            })
                            .push(self.lower_3d_surface(scene, surface, node_id)?);
                    } else {
                        flat_quads.push(self.lower_surface_node(scene, surface, node_id)?);
                    }
                }
                MpNode::Embed(embed) => {
                    if self.chain_requires_3d_plane(node_id) {
                        continue;
                    }
                    for (_child_id, mut quad) in embed.child_scene.lower_to_internal_quads()? {
                        quad.transform = Mat4f::mul(&quad.transform, &self.node(node_id).projected);
                        flat_quads.push(quad);
                    }
                }
                MpNode::ReferenceFrame(_) | MpNode::Clip(_) | MpNode::Effect(_) => {}
            }
        }

        let mut islands = Vec::new();
        for root in island_order {
            let surfaces = island_surfaces.remove(&root).unwrap_or_default();
            if surfaces.is_empty() {
                continue;
            }
            let viewport_rect = self
                .projected_reference_frame_rect(scene, root)?
                .unwrap_or(scene.root.host_rect);
            let quads = surfaces
                .iter()
                .map(|surface| {
                    make_3d_quad(
                        surface.texture.clone(),
                        surface.rect,
                        surface.transform_matrix,
                        surface.perspective_matrix,
                        surface.opacity,
                        matches!(surface.backface_visibility, MpBackfaceVisibility::Visible),
                        true,
                        surface.clip.local_planes.clone(),
                        surface.mask.clone(),
                    )
                })
                .collect();
            islands.push(make_3d_island(quads, viewport_rect));
        }

        Ok((flat_quads, islands))
    }
}

fn point_inside_evaluated_mask(mask: &MpEvaluatedMask, clip_point: Vec4f) -> bool {
    let local = mask.clip_to_local.transform_vec4(clip_point);
    if local.w.abs() <= 1e-6 {
        return false;
    }
    let point = dvec2((local.x / local.w) as f64, (local.y / local.w) as f64);
    match &mask.kind {
        MpMaskKind::RoundedRect { rect, radius } => point_inside_rounded_rect(point, *rect, *radius),
        MpMaskKind::ImageMask { rect } => point_inside_rect(point, *rect),
    }
}

fn point_inside_rect(point: DVec2, rect: Rect) -> bool {
    point.x >= rect.pos.x
        && point.y >= rect.pos.y
        && point.x <= rect.pos.x + rect.size.x
        && point.y <= rect.pos.y + rect.size.y
}

fn point_inside_rounded_rect(point: DVec2, rect: Rect, radius: Vec4f) -> bool {
    if !point_inside_rect(point, rect) {
        return false;
    }
    let min_x = rect.pos.x;
    let min_y = rect.pos.y;
    let max_x = rect.pos.x + rect.size.x;
    let max_y = rect.pos.y + rect.size.y;

    let tl = radius.x.max(0.0) as f64;
    if tl > 0.0 && point.x < min_x + tl && point.y < min_y + tl {
        let dx = point.x - (min_x + tl);
        let dy = point.y - (min_y + tl);
        return dx * dx + dy * dy <= tl * tl;
    }

    let tr = radius.y.max(0.0) as f64;
    if tr > 0.0 && point.x > max_x - tr && point.y < min_y + tr {
        let dx = point.x - (max_x - tr);
        let dy = point.y - (min_y + tr);
        return dx * dx + dy * dy <= tr * tr;
    }

    let br = radius.z.max(0.0) as f64;
    if br > 0.0 && point.x > max_x - br && point.y > max_y - br {
        let dx = point.x - (max_x - br);
        let dy = point.y - (max_y - br);
        return dx * dx + dy * dy <= br * br;
    }

    let bl = radius.w.max(0.0) as f64;
    if bl > 0.0 && point.x < min_x + bl && point.y > max_y - bl {
        let dx = point.x - (min_x + bl);
        let dy = point.y - (max_y - bl);
        return dx * dx + dy * dy <= bl * bl;
    }

    true
}

impl MpScene {
    pub fn new(root: MpSceneRoot) -> Self {
        Self {
            root,
            nodes: Vec::new(),
        }
    }

    pub fn push(&mut self, node: MpNode) -> MpNodeId {
        let node_id = self.nodes.len();
        self.nodes.push(node);
        node_id
    }

    pub fn node(&self, node_id: MpNodeId) -> Option<&MpNode> {
        self.nodes.get(node_id)
    }

    pub fn project_point(
        &self,
        node_id: MpNodeId,
        local_point: DVec2,
    ) -> Result<MpProjectedPoint, MpProjectError> {
        let evaluated = evaluate_scene(self)?;
        evaluated.project_point(self, node_id, local_point)
    }

    pub fn unproject_point(
        &self,
        node_id: MpNodeId,
        screen_point: DVec2,
    ) -> Result<DVec2, MpProjectError> {
        let evaluated = evaluate_scene(self)?;
        evaluated.unproject_point(self, node_id, screen_point)
    }

    pub fn hit_test(&self, screen_point: DVec2, options: MpHitTestOptions) -> Vec<MpHit> {
        let Ok(evaluated) = evaluate_scene(self) else {
            return Vec::new();
        };
        evaluated.hit_test(self, screen_point, options)
    }

    pub(crate) fn partition_for_execution(
        &self,
    ) -> Result<(Vec<MpCompositedQuad>, Vec<MpInternal3dIsland>), MpProjectError> {
        let evaluated = evaluate_scene(self)?;
        evaluated.partition_for_execution(self)
    }

    pub(crate) fn lower_to_internal_quads(
        &self,
    ) -> Result<Vec<(MpNodeId, MpCompositedQuad)>, MpProjectError> {
        let evaluated = evaluate_scene(self)?;
        evaluated.lower_to_internal_quads(self)
    }


}

fn rect_as_planes(rect: Rect) -> Vec<Vec4f> {
    vec![
        vec4(1.0, 0.0, 0.0, -(rect.pos.x as f32)),
        vec4(-1.0, 0.0, 0.0, (rect.pos.x + rect.size.x) as f32),
        vec4(0.0, 1.0, 0.0, -(rect.pos.y as f32)),
        vec4(0.0, -1.0, 0.0, (rect.pos.y + rect.size.y) as f32),
    ]
}

fn projected_signed_area(transform: &Mat4f, rect: Rect) -> Option<f32> {
    let p0 = project_vec2(transform, rect.pos)?;
    let p1 = project_vec2(transform, dvec2(rect.pos.x + rect.size.x, rect.pos.y))?;
    let p2 = project_vec2(transform, rect.pos + rect.size)?;
    Some((p1.x - p0.x) * (p2.y - p0.y) - (p1.y - p0.y) * (p2.x - p0.x))
}

fn project_vec2(transform: &Mat4f, point: DVec2) -> Option<Vec2f> {
    let clip = transform.transform_vec4(vec4f(point.x as f32, point.y as f32, 0.0, 1.0));
    if clip.w.abs() <= 1e-6 {
        return None;
    }
    Some(vec2(clip.x / clip.w, clip.y / clip.w))
}
#[cfg(test)]
mod tests {
    use super::*;

    fn test_scene_root() -> (MpScene, MpNodeId) {
        let mut scene = MpScene::new(MpSceneRoot {
            host_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(200.0, 200.0),
            },
            page_to_host: Mat4f::identity(),
            clip: None,
        });
        let root = scene.push(MpNode::ReferenceFrame(MpReferenceFrame {
            parent: None,
            clip: None,
            local_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(200.0, 200.0),
            },
            transform: Mat4f::identity(),
            perspective: None,
            transform_style: MpTransformStyle::Flat,
            backface_visibility: MpBackfaceVisibility::Visible,
            flattens_descendants: true,
        }));
        (scene, root)
    }

    fn surface_texture() -> Texture {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        Texture::new(&mut cx)
    }

    #[test]
    fn rounded_clip_lowers_to_mask_exec_with_non_uniform_radii() {
        let (mut scene, root) = test_scene_root();
        let clip = scene.push(MpNode::Clip(MpClipNode {
            parent: Some(root),
            prev: None,
            shape: MpClipShape::RoundedRect {
                rect: Rect {
                    pos: dvec2(10.0, 20.0),
                    size: dvec2(100.0, 80.0),
                },
                radius: vec4(4.0, 8.0, 12.0, 16.0),
            },
        }));
        scene.push(MpNode::Surface(MpSurfaceNode {
            parent: root,
            clip: Some(clip),
            local_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(120.0, 100.0),
            },
            source: MpSurfaceSource::Texture(surface_texture()),
            backface_visibility: MpBackfaceVisibility::Visible,
        }));

        let quads = scene.lower_to_internal_quads().unwrap();
        let quad = &quads[0].1;

        assert!(quad.clip_planes.is_empty());
        assert_eq!(quad.mask.masks.len(), 1);
        match &quad.mask.masks[0].kind {
            MpMaskKind::RoundedRect { rect, radius } => {
                assert_eq!(rect.pos, dvec2(10.0, 20.0));
                assert_eq!(rect.size, dvec2(100.0, 80.0));
                assert_eq!(*radius, vec4(4.0, 8.0, 12.0, 16.0));
            }
            other => panic!("unexpected mask kind: {other:?}"),
        }
    }

    #[test]
    fn image_mask_lowers_to_mask_exec() {
        let (mut scene, root) = test_scene_root();
        let clip = scene.push(MpNode::Clip(MpClipNode {
            parent: Some(root),
            prev: None,
            shape: MpClipShape::ImageMask {
                rect: Rect {
                    pos: dvec2(25.0, 30.0),
                    size: dvec2(40.0, 50.0),
                },
            },
        }));
        scene.push(MpNode::Surface(MpSurfaceNode {
            parent: root,
            clip: Some(clip),
            local_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(100.0, 100.0),
            },
            source: MpSurfaceSource::Texture(surface_texture()),
            backface_visibility: MpBackfaceVisibility::Visible,
        }));

        let quads = scene.lower_to_internal_quads().unwrap();
        let quad = &quads[0].1;

        assert!(quad.clip_planes.is_empty());
        assert_eq!(quad.mask.masks.len(), 1);
        assert!(matches!(quad.mask.masks[0].kind, MpMaskKind::ImageMask { .. }));
    }

    #[test]
    fn plane_set_lowers_to_geometric_clip_exec() {
        let (mut scene, root) = test_scene_root();
        let clip = scene.push(MpNode::Clip(MpClipNode {
            parent: Some(root),
            prev: None,
            shape: MpClipShape::PlaneSet {
                planes: vec![
                    vec4(1.0, 0.0, 0.0, -10.0),
                    vec4(-1.0, 0.0, 0.0, 90.0),
                    vec4(0.0, 1.0, 0.0, -10.0),
                    vec4(0.0, -1.0, 0.0, 90.0),
                ],
            },
        }));
        scene.push(MpNode::Surface(MpSurfaceNode {
            parent: root,
            clip: Some(clip),
            local_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(100.0, 100.0),
            },
            source: MpSurfaceSource::Texture(surface_texture()),
            backface_visibility: MpBackfaceVisibility::Visible,
        }));

        let quads = scene.lower_to_internal_quads().unwrap();
        let quad = &quads[0].1;

        assert_eq!(quad.clip_planes.len(), 4);
        assert!(quad.mask.masks.is_empty());
    }

    #[test]
    fn hit_test_obeys_mask_backed_rounded_clip() {
        let (mut scene, root) = test_scene_root();
        let clip = scene.push(MpNode::Clip(MpClipNode {
            parent: Some(root),
            prev: None,
            shape: MpClipShape::RoundedRect {
                rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(100.0, 100.0),
                },
                radius: vec4(20.0, 20.0, 20.0, 20.0),
            },
        }));
        let surface = scene.push(MpNode::Surface(MpSurfaceNode {
            parent: root,
            clip: Some(clip),
            local_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(100.0, 100.0),
            },
            source: MpSurfaceSource::Texture(surface_texture()),
            backface_visibility: MpBackfaceVisibility::Visible,
        }));

        assert!(scene
            .hit_test(dvec2(5.0, 5.0), MpHitTestOptions::default())
            .is_empty());
        let hits = scene.hit_test(dvec2(50.0, 50.0), MpHitTestOptions::default());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].node_id, surface);
    }
}
