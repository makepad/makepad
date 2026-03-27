use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use makepad_widgets::makepad_draw::Texture;
use makepad_widgets::{dvec2, vec3, Cx, Cx2d, Mat4f, Rect};

use makepad_compositor::MpRenderer as MpCompositorRenderer;

use crate::{
    effect::MpEffectId,
    embed::MpPipelineId,
    primitive::{MpPrimitive, MpPrimitiveId},
    resource::{MpGlyphRunKey, MpGlyphRunResource, MpResourceStore, ResourceRegistry},
    scene::{MpDocument, MpScene},
};

mod clip;
mod geom;
mod image;
mod picture;
mod primitive;
mod retained_patch;
mod text;
mod transform;
mod traversal;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MpRendererStats {
    pub direct_primitive_count: usize,
    pub isolated_boundary_count: usize,
    pub isolated_primitive_count: usize,
    pub compositor_surface_count: usize,
    pub total_offscreen_pixel_area: u64,
    pub scratch_surface_count: usize,
    pub scratch_surface_new_alloc_count: usize,
    pub scratch_surface_reuse_count: usize,
    pub prepared_text_batch_hit_count: usize,
    pub prepared_text_batch_miss_count: usize,
    pub prepared_text_batch_rebuild_count: usize,
    pub glyph_residency_hit_count: usize,
    pub glyph_residency_miss_count: usize,
    pub glyph_cache_reset_count: usize,
    pub atlas_page_alloc_count: usize,
    pub msdf_request_queue_count: usize,
    pub msdf_completion_count: usize,
    pub synchronous_fallback_glyph_generation_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MpRenderError {
    UnsupportedPrimitive(MpPrimitiveId),
    MissingGlyphRunResource(MpPrimitiveId),
    MissingFontResource(MpPrimitiveId),
    MissingImageResource(MpPrimitiveId),
    MissingEmbedDocument(MpPipelineId),
    UnsupportedEffect(MpEffectId),
    UnsupportedSpatial(String),
    Compositor(String),
}

#[derive(Clone, Debug)]
pub struct MpRetainedBrowserScene {
    pub(crate) resource_generation: u64,
    pub(crate) scene: makepad_compositor::MpBrowserScene,
    pub(crate) prepared_text: makepad_compositor::MpPreparedBrowserScene,
    patch: retained_patch::RetainedScenePatch,
    pub(crate) lower_stats: MpRendererStats,
}

impl MpRetainedBrowserScene {
    pub fn retained_scene_id(&self) -> makepad_compositor::MpBrowserRetainedSceneId {
        self.scene.retained_scene_id
    }

    pub fn resource_generation(&self) -> u64 {
        self.resource_generation
    }

    pub fn stable_text_run_count(&self) -> usize {
        count_scene_text_runs(&self.scene)
    }
}

fn log_unsupported_skip_once(reason: impl Into<String>) {
    static LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let reason = reason.into();
    let logged = LOGGED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut logged = logged.lock().unwrap();
    if logged.insert(reason.clone()) {
        eprintln!("[makepad-browser-scene] skipped unsupported content: {reason}");
    }
}

pub(super) fn unsupported_primitive_skip(
    primitive: &MpPrimitive,
    reason: &str,
) -> Result<(), MpRenderError> {
    log_unsupported_skip_once(format!(
        "{reason}: id={:?} kind={:?} effect_id={:?} spatial_id={:?} clip_chain_id={:?}",
        primitive.id,
        primitive.kind,
        primitive.effect_id,
        primitive.spatial_id,
        primitive.clip_chain_id,
    ));
    Ok(())
}

pub struct MpBrowserRenderer {
    pub(super) compositor: MpCompositorRenderer,
    pub(super) image_textures: HashMap<crate::MpImageKey, Texture>,
    pub(super) resource_registry: ResourceRegistry,
    pub(super) next_retained_scene_id: makepad_compositor::MpBrowserRetainedSceneId,
}

pub(super) struct MpSceneLowerer<'a> {
    pub(super) image_textures: &'a mut HashMap<crate::MpImageKey, Texture>,
    pub(super) retained_scene_id: makepad_compositor::MpBrowserRetainedSceneId,
    pub(super) next_stable_text_run_id: makepad_compositor::MpBrowserStableTextRunId,
}

impl MpBrowserRenderer {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            compositor: MpCompositorRenderer::new(cx),
            image_textures: HashMap::new(),
            resource_registry: ResourceRegistry::default(),
            next_retained_scene_id: 1,
        }
    }

    pub fn resource_registry(&self) -> &ResourceRegistry {
        &self.resource_registry
    }

    pub fn resource_registry_mut(&mut self) -> &mut ResourceRegistry {
        &mut self.resource_registry
    }

    pub fn resource_generation(&self) -> u64 {
        self.resource_registry.generation()
    }

    pub fn register_resource_store(&mut self, resources: &MpResourceStore) {
        for (key, image) in &resources.images {
            self.resource_registry.upsert_image(*key, image.clone());
        }
        for (key, font) in &resources.fonts {
            self.resource_registry.upsert_font(*key, font.clone());
        }
        for (key, image) in &resources.external_images {
            self.resource_registry
                .upsert_external_image(*key, image.clone());
        }
    }

    pub fn analyze(&mut self, scene: &MpScene) -> Result<MpRendererStats, MpRenderError> {
        let empty_glyph_runs = HashMap::new();
        let (_, stats) = self.lower_scene(
            None,
            scene,
            &ResourceRegistry::default(),
            &empty_glyph_runs,
            scene.root_viewport_rect(),
        )?;
        Ok(stats)
    }

    pub fn analyze_document(
        &mut self,
        document: &MpDocument,
    ) -> Result<MpRendererStats, MpRenderError> {
        let retained = self.lower_retained_document(document, document.scene.root_viewport_rect())?;
        Ok(retained.lower_stats)
    }

    pub fn lower_retained_document(
        &mut self,
        document: &MpDocument,
        viewport: Rect,
    ) -> Result<MpRetainedBrowserScene, MpRenderError> {
        let retained_scene_id = next_retained_scene_id(&mut self.next_retained_scene_id);
        let (scene, stats, patch) = MpSceneLowerer {
            image_textures: &mut self.image_textures,
            retained_scene_id,
            next_stable_text_run_id: 1,
        }
        .lower_scene_with_patch(
            Some(document),
            &document.scene,
            &self.resource_registry,
            &document.glyph_runs,
            viewport,
        )?;
        Ok(MpRetainedBrowserScene {
            resource_generation: self.resource_registry.generation(),
            prepared_text: makepad_compositor::MpPreparedBrowserScene::new(scene.retained_scene_id),
            scene,
            patch,
            lower_stats: stats,
        })
    }

    pub fn patch_retained_scene_host_rect(
        &mut self,
        retained: &mut MpRetainedBrowserScene,
        host_rect: Rect,
    ) {
        patch_retained_scene_host_rect(&mut retained.scene, host_rect);
    }

    pub fn patch_retained_document_scene(
        &mut self,
        retained: &mut MpRetainedBrowserScene,
        document: &MpDocument,
        host_rect: Rect,
    ) -> Result<(), MpRenderError> {
        retained.scene.host_rect = host_rect;
        retained.scene.primitive_scene.host_rect = host_rect;
        retained_patch::patch_scene_from_document(&mut retained.scene, &retained.patch, document)
    }

    pub fn draw_retained_scene(
        &mut self,
        cx: &mut Cx2d,
        retained: &mut MpRetainedBrowserScene,
    ) -> MpRendererStats {
        let mut stats = retained.lower_stats;
        self.compositor
            .draw_retained_browser_scene(cx, &retained.scene, &mut retained.prepared_text);
        apply_frame_stats(&self.compositor, &mut stats);
        stats
    }

    pub fn draw_document(
        &mut self,
        cx: &mut Cx2d,
        document: &MpDocument,
        viewport: Rect,
    ) -> Result<MpRendererStats, MpRenderError> {
        let mut retained = self.lower_retained_document(document, viewport)?;
        Ok(self.draw_retained_scene(cx, &mut retained))
    }

    pub fn draw_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpScene,
        viewport: Rect,
    ) -> Result<MpRendererStats, MpRenderError> {
        let empty_glyph_runs = HashMap::new();
        let (scene, mut stats) = self.lower_scene(
            None,
            scene,
            &ResourceRegistry::default(),
            &empty_glyph_runs,
            viewport,
        )?;
        self.compositor.draw_browser_scene(cx, &scene);
        apply_frame_stats(&self.compositor, &mut stats);
        Ok(stats)
    }

    pub fn draw_scene_with_resources(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpScene,
        resources: &MpResourceStore,
        viewport: Rect,
    ) -> Result<MpRendererStats, MpRenderError> {
        let registry = ResourceRegistry::from(resources);
        let (scene, mut stats) = self.lower_scene(
            None,
            scene,
            &registry,
            &resources.glyph_runs,
            viewport,
        )?;
        self.compositor.draw_browser_scene(cx, &scene);
        apply_frame_stats(&self.compositor, &mut stats);
        Ok(stats)
    }

    pub(super) fn lower_scene(
        &mut self,
        document: Option<&MpDocument>,
        scene: &MpScene,
        registry: &ResourceRegistry,
        glyph_runs: &HashMap<MpGlyphRunKey, MpGlyphRunResource>,
        viewport: Rect,
    ) -> Result<(makepad_compositor::MpBrowserScene, MpRendererStats), MpRenderError> {
        let retained_scene_id = next_retained_scene_id(&mut self.next_retained_scene_id);
        MpSceneLowerer {
            image_textures: &mut self.image_textures,
            retained_scene_id,
            next_stable_text_run_id: 1,
        }
        .lower_scene(document, scene, registry, glyph_runs, viewport)
    }
}

fn apply_frame_stats(compositor: &MpCompositorRenderer, stats: &mut MpRendererStats) {
    let frame_stats = compositor.browser_scene_frame_stats();
    stats.total_offscreen_pixel_area = frame_stats.total_offscreen_pixel_area;
    stats.scratch_surface_count = frame_stats.scratch_surface_count;
    stats.scratch_surface_new_alloc_count = frame_stats.scratch_surface_new_alloc_count;
    stats.scratch_surface_reuse_count = frame_stats.scratch_surface_reuse_count;
    stats.prepared_text_batch_hit_count = frame_stats.prepared_text_batch_hit_count;
    stats.prepared_text_batch_miss_count = frame_stats.prepared_text_batch_miss_count;
    stats.prepared_text_batch_rebuild_count = frame_stats.prepared_text_batch_rebuild_count;
    stats.glyph_residency_hit_count = frame_stats.glyph_residency_hit_count;
    stats.glyph_residency_miss_count = frame_stats.glyph_residency_miss_count;
    stats.glyph_cache_reset_count = frame_stats.glyph_cache_reset_count;
    stats.atlas_page_alloc_count = frame_stats.atlas_page_alloc_count;
    stats.msdf_request_queue_count = frame_stats.msdf_request_queue_count;
    stats.msdf_completion_count = frame_stats.msdf_completion_count;
    stats.synchronous_fallback_glyph_generation_count =
        frame_stats.synchronous_fallback_glyph_generation_count;
}

fn next_retained_scene_id(
    next_id: &mut makepad_compositor::MpBrowserRetainedSceneId,
) -> makepad_compositor::MpBrowserRetainedSceneId {
    let id = *next_id;
    *next_id = next_id.wrapping_add(1);
    id
}

fn patch_retained_scene_host_rect(scene: &mut makepad_compositor::MpBrowserScene, host_rect: Rect) {
    let delta = host_rect.pos - scene.host_rect.pos;
    scene.host_rect = host_rect;
    scene.primitive_scene.host_rect = host_rect;
    if delta == dvec2(0.0, 0.0) {
        return;
    }
    let delta_transform = Mat4f::translation(vec3(delta.x as f32, delta.y as f32, 0.0));
    for transform in &mut scene.primitive_scene.transforms {
        transform.scene_from_origin = Mat4f::mul(&delta_transform, &transform.scene_from_origin);
    }
}

fn count_scene_text_runs(scene: &makepad_compositor::MpBrowserScene) -> usize {
    scene.text_runs.len()
        + scene
            .tasks
            .iter()
            .map(|task| match &task.kind {
                makepad_compositor::MpBrowserTaskKind::Scene(task_scene) => count_scene_text_runs(task_scene),
                makepad_compositor::MpBrowserTaskKind::Blur { .. } => 0,
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use makepad_widgets::{dvec2, vec3, vec4, Cx, Mat4f, Rect};

    use crate::{
        effect::{MpEffectNode, MpIsolation},
        primitive::MpPrimitive,
        resource::{
            MpFontKey, MpFontResource, MpGlyphRunKey, MpGlyphRunMetrics, MpGlyphRunResource,
            MpPositionedGlyph,
        },
        scene::{MpDocument, MpDocumentId, MpScene, MpSceneId},
        MpBackfaceVisibility, MpBrowserRenderer, MpClipChain, MpClipKind, MpClipNode, MpFilter,
        MpReferenceFrame, MpScrollFrame, MpSpatialKind, MpSpatialNode, MpTransformStyle,
    };

    #[test]
    fn patch_retained_document_scene_updates_scroll_owned_outputs_in_place() {
        let mut scene = MpScene::new(
            MpSceneId(1),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let scroll_id = scene.push_spatial_node(MpSpatialNode {
            parent: Some(scene.root_spatial_id),
            kind: MpSpatialKind::ScrollFrame(MpScrollFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(200.0, 120.0),
                },
                content_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(200.0, 500.0),
                },
                scroll_offset: dvec2(0.0, 0.0),
            }),
        });
        let clip = scene.push_clip(MpClipNode {
            spatial_id: scroll_id,
            kind: MpClipKind::Rect {
                rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(120.0, 80.0),
                },
            },
        });
        let clip_chain = scene.push_clip_chain(MpClipChain {
            parent: Some(scene.root_clip_chain_id),
            clips: vec![clip],
        });
        scene.push_primitive(MpPrimitive::solid_rect(
            crate::MpPrimitiveId(0),
            scroll_id,
            clip_chain,
            Rect {
                pos: dvec2(10.0, 20.0),
                size: dvec2(30.0, 40.0),
            },
            vec4(1.0, 0.0, 0.0, 1.0),
        ));
        scene.push_primitive(MpPrimitive::text_run(
            crate::MpPrimitiveId(1),
            scroll_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(15.0, 70.0),
                size: dvec2(90.0, 20.0),
            },
            MpGlyphRunKey(1),
            vec4(1.0, 1.0, 1.0, 1.0),
        ));
        let plain_effect = scene.push_effect(MpEffectNode {
            spatial_id: scroll_id,
            clip_chain_id: scene.root_clip_chain_id,
            opacity: 1.0,
            filters: vec![MpFilter::Opacity(0.8)],
            blend_mode: crate::MpBlendMode::Normal,
            isolation: MpIsolation::Isolate,
            mask: None,
        });
        let mut plain_effect_primitive = MpPrimitive::solid_rect(
            crate::MpPrimitiveId(2),
            scroll_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(40.0, 110.0),
                size: dvec2(50.0, 24.0),
            },
            vec4(0.0, 1.0, 0.0, 1.0),
        );
        plain_effect_primitive.effect_id = Some(plain_effect);
        scene.push_primitive(plain_effect_primitive);
        let rotated_id = scene.push_spatial_node(MpSpatialNode {
            parent: Some(scroll_id),
            kind: MpSpatialKind::ReferenceFrame(MpReferenceFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(160.0, 120.0),
                },
                placement_origin: dvec2(80.0, 150.0),
                transform: Some(Mat4f::rotation(vec3(0.0, 0.0, 0.2))),
                perspective: None,
                transform_style: MpTransformStyle::Flat,
                backface_visibility: MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        });
        let transformed_effect = scene.push_effect(MpEffectNode {
            spatial_id: rotated_id,
            clip_chain_id: scene.root_clip_chain_id,
            opacity: 1.0,
            filters: vec![MpFilter::Opacity(0.9)],
            blend_mode: crate::MpBlendMode::Normal,
            isolation: MpIsolation::Isolate,
            mask: None,
        });
        let mut transformed_effect_primitive = MpPrimitive::solid_rect(
            crate::MpPrimitiveId(3),
            rotated_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(5.0, 7.0),
                size: dvec2(28.0, 18.0),
            },
            vec4(0.0, 0.0, 1.0, 1.0),
        );
        transformed_effect_primitive.effect_id = Some(transformed_effect);
        scene.push_primitive(transformed_effect_primitive);

        let document = MpDocument {
            id: MpDocumentId(1),
            epoch: 0,
            scene,
            glyph_runs: std::iter::once((
                MpGlyphRunKey(1),
                MpGlyphRunResource {
                    text: "scroll".to_string(),
                    font_keys: vec![MpFontKey(1)],
                    glyphs: vec![MpPositionedGlyph {
                        glyph_id: 7,
                        font_size_px: 14.0,
                        origin: dvec2(5.0, 12.0),
                        font_slot: 0,
                    }],
                    metrics: MpGlyphRunMetrics::default(),
                    decorations: Default::default(),
                },
            ))
            .collect(),
            child_documents: Vec::new(),
        };

        let mut renderer = MpBrowserRenderer::new(&mut Cx::new(Box::new(|_, _| {})));
        renderer.resource_registry.upsert_font(
            MpFontKey(1),
            MpFontResource {
                bytes: Arc::from(vec![1, 2, 3]),
                face_index: 0,
            },
        );
        let mut retained = renderer
            .lower_retained_document(&document, document.scene.root_viewport_rect())
            .unwrap();
        let primitive_clip_chain_id = retained.scene.primitive_scene.primitives[0].clip_chain_id;
        let direct_primitive_before = retained.scene.primitive_scene.primitives[0].local_rect;
        let direct_clip_before = retained.scene.primitive_scene.clip_chains[primitive_clip_chain_id]
            .origin_clip_rect
            .unwrap();
        let direct_text_before = retained.scene.text_runs[0].local_rect;
        let plain_picture_id = retained
            .scene
            .pictures
            .iter()
            .position(|picture| picture.transform_id == 0)
            .unwrap();
        let plain_picture_before = retained.scene.pictures[plain_picture_id].local_rect;
        let plain_task_before = match &retained.scene.tasks[retained.scene.pictures[plain_picture_id].task_id].kind {
            makepad_compositor::MpBrowserTaskKind::Scene(task_scene) => task_scene.host_rect,
            other => panic!("expected scene task, got {other:?}"),
        };
        let transformed_picture_id = retained
            .scene
            .pictures
            .iter()
            .position(|picture| picture.transform_id != 0)
            .unwrap();
        let transformed_transform_id = retained.scene.pictures[transformed_picture_id].transform_id;
        let transformed_transform_before = retained.scene.primitive_scene.transforms[transformed_transform_id];

        let mut patched_document = document.clone();
        patched_document
            .scene
            .set_scroll_offset(scroll_id, dvec2(0.0, 30.0));
        let retained_scene_id = retained.retained_scene_id();
        renderer
            .patch_retained_document_scene(
                &mut retained,
                &patched_document,
                patched_document.scene.root_viewport_rect(),
            )
            .unwrap();

        assert_eq!(retained.retained_scene_id(), retained_scene_id);
        assert_eq!(
            retained.scene.primitive_scene.primitives[0].local_rect.pos.y,
            direct_primitive_before.pos.y - 30.0
        );
        assert_eq!(
            retained.scene.primitive_scene.clip_chains[primitive_clip_chain_id]
                .origin_clip_rect
                .unwrap()
                .pos
                .y,
            direct_clip_before.pos.y - 30.0
        );
        assert_eq!(retained.scene.text_runs[0].local_rect.pos.y, direct_text_before.pos.y - 30.0);
        assert_eq!(
            retained.scene.pictures[plain_picture_id].local_rect.pos.y,
            plain_picture_before.pos.y - 30.0
        );
        let plain_task_after = match &retained.scene.tasks[retained.scene.pictures[plain_picture_id].task_id].kind {
            makepad_compositor::MpBrowserTaskKind::Scene(task_scene) => task_scene.host_rect,
            other => panic!("expected scene task, got {other:?}"),
        };
        assert_eq!(plain_task_after.pos.y, plain_task_before.pos.y - 30.0);
        assert_ne!(
            retained.scene.primitive_scene.transforms[transformed_transform_id].scene_from_origin,
            transformed_transform_before.scene_from_origin,
        );
    }
}
