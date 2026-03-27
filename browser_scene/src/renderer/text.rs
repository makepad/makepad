use makepad_widgets::{dvec2, Rect};

use makepad_compositor::{
    MpBlendMode as MpCompositorBlendMode, MpBrowserGlyphInstance, MpBrowserPicture,
    MpBrowserScene as MpCompositorBrowserScene, MpBrowserTask, MpBrowserTaskKind,
    MpBrowserTextDecorations, MpBrowserTextMetrics,
    MpBrowserTextRun as MpCompositorBrowserTextRun, MpBrowserTextShadow,
    MpBrowserFontResource,
};

use std::collections::HashMap;

use crate::{
    primitive::{MpPrimitive, MpPrimitiveKind, MpTextRun},
    resource::{MpGlyphRunKey, MpGlyphRunResource, ResourceRegistry},
    scene::MpScene,
};

use super::{
    clip::lower_clip_chain,
    geom::resolve_primitive_rect,
    retained_patch, MpRenderError, MpRendererStats, MpSceneLowerer,
};

impl MpSceneLowerer<'_> {
    fn alloc_stable_text_run_id(&mut self) -> makepad_compositor::MpBrowserStableTextRunId {
        let id = self.next_stable_text_run_id;
        self.next_stable_text_run_id = self.next_stable_text_run_id.wrapping_add(1);
        id
    }

    /// Direct text path.
    ///
    /// This path is used only for root untransformed text when the lowered clip
    /// chain has no mask entries. `local_rect` is resolved in origin space via
    /// `resolve_primitive_rect()`. Glyph origins stay primitive-local and are
    /// passed through untouched.
    pub(super) fn lower_direct_text(
        &mut self,
        scene: &MpScene,
        registry: &ResourceRegistry,
        glyph_runs: &HashMap<MpGlyphRunKey, MpGlyphRunResource>,
        lowered: &mut MpCompositorBrowserScene,
        patch_builder: &mut retained_patch::ScenePatchBuilder,
        primitive: &MpPrimitive,
    ) -> Result<bool, MpRenderError> {
        let transform_spatial_id = scene.find_transform_ancestor(primitive.spatial_id);
        if transform_spatial_id.is_some() {
            return Ok(false);
        }
        let clip_chain = lower_clip_chain(
            scene,
            primitive.spatial_id,
            primitive.clip_chain_id,
            transform_spatial_id,
        )?;
        if !clip_chain.entries.is_empty() {
            return Ok(false);
        }
        let transform_id =
            self.ensure_transform(lowered, patch_builder, scene, transform_spatial_id)?;
        let clip_chain_id = self.ensure_clip_chain(
            lowered,
            patch_builder,
            clip_chain,
            retained_patch::RetainedClipChainSource {
                spatial_id: primitive.spatial_id,
                clip_chain_id: primitive.clip_chain_id,
                origin_spatial_id: transform_spatial_id,
            },
        );
        let local_rect = resolve_primitive_rect(scene, primitive, transform_spatial_id)?;
        let Some(text_run) = self.build_text_run(
            registry,
            glyph_runs,
            primitive,
            local_rect,
            transform_id,
            clip_chain_id,
        )?
        else {
            return Ok(false);
        };
        let text_run_id = lowered.push_text_run(text_run);
        patch_builder.record_text_run(
            text_run_id,
            retained_patch::RetainedTextRunSource::Direct {
                primitive_id: primitive.id,
                origin_spatial_id: transform_spatial_id,
            },
        );
        Ok(true)
    }

    /// Picture fallback text path.
    ///
    /// This path is used for transformed or complexly clipped text. It creates
    /// a nested task scene rooted at `(0, 0)` with `size = resolved_bounds.size`.
    /// The nested text run gets `local_rect.pos = (0, 0)` because it is local
    /// to the task surface. The outer picture owns placement while the inner
    /// text run stays task-local.
    pub(super) fn lower_text_picture(
        &mut self,
        scene: &MpScene,
        registry: &ResourceRegistry,
        glyph_runs: &HashMap<MpGlyphRunKey, MpGlyphRunResource>,
        lowered: &mut MpCompositorBrowserScene,
        patch_builder: &mut retained_patch::ScenePatchBuilder,
        primitive: &MpPrimitive,
        stats: &mut MpRendererStats,
    ) -> Result<bool, MpRenderError> {
        let transform_spatial_id = scene.find_transform_ancestor(primitive.spatial_id);
        let clip_chain = lower_clip_chain(
            scene,
            primitive.spatial_id,
            primitive.clip_chain_id,
            transform_spatial_id,
        )?;
        let needs_picture = transform_spatial_id.is_some() || !clip_chain.entries.is_empty();
        if !needs_picture {
            return Ok(false);
        }
        let bounds = resolve_primitive_rect(scene, primitive, transform_spatial_id)?;
        let Some(text_run) = self.build_text_run(
            registry,
            glyph_runs,
            primitive,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: bounds.size,
            },
            0,
            0,
        )? else {
            return Ok(false);
        };
        debug_assert_eq!(text_run.local_rect.pos, dvec2(0.0, 0.0));
        let mut task_scene = MpCompositorBrowserScene::new_with_retained_scene_id(
            self.retained_scene_id,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: bounds.size,
            },
        );
        let text_run_id = task_scene.push_text_run(text_run);
        let mut task_patch_builder = retained_patch::ScenePatchBuilder::new();
        task_patch_builder.record_text_run(
            text_run_id,
            retained_patch::RetainedTextRunSource::TaskLocal {
                primitive_id: primitive.id,
                origin_spatial_id: transform_spatial_id,
            },
        );
        let task_patch = task_patch_builder.finish();
        let task_id = lowered.push_task(MpBrowserTask {
            size: bounds.size,
            cache_key: None,
            kind: MpBrowserTaskKind::Scene(Box::new(task_scene)),
        });
        patch_builder.record_task(
            task_id,
            retained_patch::RetainedTaskSource::Scene {
                source: retained_patch::RetainedTaskSceneSource::TextPicture {
                    primitive_id: primitive.id,
                    origin_spatial_id: transform_spatial_id,
                },
                patch: Box::new(task_patch),
            },
        );
        let transform_id =
            self.ensure_transform(lowered, patch_builder, scene, transform_spatial_id)?;
        let clip_chain_id = self.ensure_clip_chain(
            lowered,
            patch_builder,
            clip_chain,
            retained_patch::RetainedClipChainSource {
                spatial_id: primitive.spatial_id,
                clip_chain_id: primitive.clip_chain_id,
                origin_spatial_id: transform_spatial_id,
            },
        );
        let picture_id = lowered.push_picture(MpBrowserPicture {
            local_rect: bounds,
            transform_id,
            clip_chain_id,
            task_id,
            opacity: 1.0,
            blend_mode: MpCompositorBlendMode::Normal,
        });
        patch_builder.record_picture(
            picture_id,
            retained_patch::RetainedPictureSource::TextPicture {
                primitive_id: primitive.id,
                origin_spatial_id: transform_spatial_id,
            },
        );
        stats.compositor_surface_count += 1;
        Ok(true)
    }

    /// Builds a compositor text run by copying glyph origins unchanged from
    /// the resource layer.
    ///
    /// `local_rect` is the resolved text run bounds in origin space. No host or
    /// widget offsets are applied here. Glyph origins remain primitive-local
    /// throughout.
    fn build_text_run(
        &mut self,
        registry: &ResourceRegistry,
        glyph_runs: &HashMap<MpGlyphRunKey, MpGlyphRunResource>,
        primitive: &MpPrimitive,
        local_rect: Rect,
        transform_id: usize,
        clip_chain_id: usize,
    ) -> Result<Option<MpCompositorBrowserTextRun>, MpRenderError> {
        let MpPrimitiveKind::TextRun(MpTextRun {
            glyph_run_key,
            color,
        }) = &primitive.kind else {
            return Ok(None);
        };
        let Some(glyph_run) = glyph_runs.get(glyph_run_key) else {
            return Err(MpRenderError::MissingGlyphRunResource(primitive.id));
        };
        let mut fonts = Vec::with_capacity(glyph_run.font_keys.len());
        for font_key in &glyph_run.font_keys {
            let Some(font) = registry.fonts.get(font_key) else {
                return Err(MpRenderError::MissingFontResource(primitive.id));
            };
            fonts.push(MpBrowserFontResource {
                key: font_key.0,
                bytes: font.bytes.clone(),
                face_index: font.face_index,
            });
        }
        Ok(Some(MpCompositorBrowserTextRun {
            stable_id: self.alloc_stable_text_run_id(),
            local_rect,
            transform_id,
            clip_chain_id,
            color: *color,
            fonts,
            glyphs: glyph_run
                .glyphs
                .iter()
                .map(|glyph| MpBrowserGlyphInstance {
                    glyph_id: glyph.glyph_id,
                    font_size_px: glyph.font_size_px,
                    origin: glyph.origin,
                    font_slot: glyph.font_slot,
                })
                .collect(),
            metrics: MpBrowserTextMetrics {
                advance_width_px: glyph_run.metrics.advance_width_px,
                baseline_ascent_px: glyph_run.metrics.baseline_ascent_px,
                underline_offset_px: glyph_run.metrics.underline_offset_px,
                underline_thickness_px: glyph_run.metrics.underline_thickness_px,
                strikeout_offset_px: glyph_run.metrics.strikeout_offset_px,
                strikeout_thickness_px: glyph_run.metrics.strikeout_thickness_px,
            },
            decorations: MpBrowserTextDecorations {
                background_color: glyph_run.decorations.background_color,
                decoration_color: glyph_run.decorations.decoration_color,
                underline: glyph_run.decorations.underline,
                overline: glyph_run.decorations.overline,
                line_through: glyph_run.decorations.line_through,
                shadows: glyph_run
                    .decorations
                    .shadows
                    .iter()
                    .map(|shadow| MpBrowserTextShadow {
                        offset: shadow.offset,
                        blur_radius_px: shadow.blur_radius_px,
                        color: shadow.color,
                    })
                    .collect(),
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use makepad_widgets::{dvec2, vec4, Cx, Rect};
    use std::sync::Arc;

    use makepad_compositor::{MpBrowserScene as MpCompositorBrowserScene, MpBrowserTaskKind};

    use crate::{
        resource::{
            MpFontKey, MpFontResource, MpGlyphRunKey, MpGlyphRunMetrics, MpGlyphRunResource,
            MpPositionedGlyph, MpResourceStore, ResourceRegistry,
        },
        MpBrowserRenderer, MpClipChain, MpClipKind, MpClipNode, MpPrimitive, MpReferenceFrame,
        MpScene, MpSceneId, MpSpatialKind, MpSpatialNode, MpTextDecorations, MpTextShadow,
        MpTransformStyle,
    };

    use super::*;

    fn test_resources() -> MpResourceStore {
        let mut resources = MpResourceStore::default();
        resources.fonts.insert(
            MpFontKey(1),
            MpFontResource {
                bytes: Arc::from(vec![1, 2, 3]),
                face_index: 0,
            },
        );
        resources.glyph_runs.insert(
            MpGlyphRunKey(1),
            MpGlyphRunResource {
                text: "hi".to_string(),
                font_keys: vec![MpFontKey(1)],
                glyphs: vec![
                    MpPositionedGlyph {
                        glyph_id: 7,
                        font_size_px: 14.0,
                        origin: dvec2(5.0, 12.0),
                        font_slot: 0,
                    },
                    MpPositionedGlyph {
                        glyph_id: 8,
                        font_size_px: 14.0,
                        origin: dvec2(15.0, 12.0),
                        font_slot: 0,
                    },
                ],
                metrics: MpGlyphRunMetrics::default(),
                decorations: MpTextDecorations::default(),
            },
        );
        resources
    }

    fn test_registry(resources: &MpResourceStore) -> ResourceRegistry {
        ResourceRegistry::from(resources)
    }

    fn affine_transformed_spatial(scene: &mut MpScene) -> crate::MpSpatialId {
        scene.push_spatial_node(MpSpatialNode {
            parent: Some(scene.root_spatial_id),
            kind: MpSpatialKind::ReferenceFrame(MpReferenceFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(200.0, 120.0),
                },
                placement_origin: dvec2(40.0, 20.0),
                transform: Some(makepad_widgets::Mat4f::rotation(makepad_widgets::vec3(
                    0.0, 0.0, 0.2,
                ))),
                perspective: None,
                transform_style: MpTransformStyle::Flat,
                backface_visibility: crate::MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        })
    }

    fn non_affine_transformed_spatial(scene: &mut MpScene) -> crate::MpSpatialId {
        scene.push_spatial_node(MpSpatialNode {
            parent: Some(scene.root_spatial_id),
            kind: MpSpatialKind::ReferenceFrame(MpReferenceFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(200.0, 120.0),
                },
                placement_origin: dvec2(40.0, 20.0),
                transform: Some(makepad_widgets::Mat4f {
                    v: [
                        1.0, 0.0, 0.2, 0.0,
                        0.0, 1.0, 0.0, 0.0,
                        0.0, 0.0, 1.0, 0.0,
                        0.0, 0.0, 0.0, 1.0,
                    ],
                }),
                perspective: None,
                transform_style: MpTransformStyle::Flat,
                backface_visibility: crate::MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        })
    }

    #[test]
    fn direct_text_lowering_requires_local_rect_clip_only() {
        let mut scene = MpScene::new(
            MpSceneId(2),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let clip = scene.push_clip(MpClipNode {
            spatial_id: scene.root_spatial_id,
            kind: MpClipKind::RoundedRect {
                rect: Rect {
                    pos: dvec2(10.0, 10.0),
                    size: dvec2(100.0, 40.0),
                },
                radius: crate::MpPerCornerRadius::uniform(8.0),
            },
        });
        let chain = scene.push_clip_chain(MpClipChain {
            parent: Some(scene.root_clip_chain_id),
            clips: vec![clip],
        });
        let primitive = MpPrimitive::text_run(
            crate::MpPrimitiveId(0),
            scene.root_spatial_id,
            chain,
            Rect {
                pos: dvec2(10.0, 10.0),
                size: dvec2(80.0, 20.0),
            },
            MpGlyphRunKey(1),
            vec4(1.0, 1.0, 1.0, 1.0),
        );
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);
        let mut lowered = MpCompositorBrowserScene::new(scene.root_viewport_rect());

        let resources = MpResourceStore::default();
        let registry = test_registry(&resources);
        let mut patch_builder = retained_patch::ScenePatchBuilder::new();
        let direct = MpSceneLowerer {
            image_textures: &mut renderer.image_textures,
            retained_scene_id: 1,
            next_stable_text_run_id: 1,
        }
        .lower_direct_text(
            &scene,
            &registry,
            &resources.glyph_runs,
            &mut lowered,
            &mut patch_builder,
            &primitive,
        )
        .unwrap();

        assert!(!direct);
        assert!(lowered.text_runs.is_empty());
    }

    #[test]
    fn lower_scene_routes_clipped_text_through_picture_fallback() {
        let mut scene = MpScene::new(
            MpSceneId(7),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let clip = scene.push_clip(MpClipNode {
            spatial_id: scene.root_spatial_id,
            kind: MpClipKind::RoundedRect {
                rect: Rect {
                    pos: dvec2(10.0, 10.0),
                    size: dvec2(100.0, 40.0),
                },
                radius: crate::MpPerCornerRadius::uniform(8.0),
            },
        });
        let chain = scene.push_clip_chain(MpClipChain {
            parent: Some(scene.root_clip_chain_id),
            clips: vec![clip],
        });
        scene.push_primitive(MpPrimitive::text_run(
            crate::MpPrimitiveId(0),
            scene.root_spatial_id,
            chain,
            Rect {
                pos: dvec2(10.0, 10.0),
                size: dvec2(80.0, 20.0),
            },
            MpGlyphRunKey(1),
            vec4(1.0, 1.0, 1.0, 1.0),
        ));
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);
        let resources = test_resources();
        let registry = test_registry(&resources);

        let (lowered, _) = renderer
            .lower_scene(None, &scene, &registry, &resources.glyph_runs, scene.root_viewport_rect())
            .unwrap();

        assert_eq!(lowered.text_runs.len(), 0);
        assert_eq!(lowered.pictures.len(), 1);
    }

    #[test]
    fn lower_scene_routes_affine_transformed_text_through_picture_fallback() {
        let mut scene = MpScene::new(
            MpSceneId(4),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let transform_id = affine_transformed_spatial(&mut scene);
        scene.push_primitive(MpPrimitive::text_run(
            crate::MpPrimitiveId(0),
            transform_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(10.0, 12.0),
                size: dvec2(120.0, 32.0),
            },
            MpGlyphRunKey(1),
            vec4(1.0, 1.0, 1.0, 1.0),
        ));
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);
        let resources = test_resources();
        let registry = test_registry(&resources);

        let (lowered, _) = renderer
            .lower_scene(None, &scene, &registry, &resources.glyph_runs, scene.root_viewport_rect())
            .unwrap();

        assert_eq!(lowered.text_runs.len(), 0);
        assert_eq!(lowered.pictures.len(), 1);
    }

    #[test]
    fn direct_text_preserves_primitive_local_glyph_origins() {
        let mut scene = MpScene::new(
            MpSceneId(9),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        scene.push_primitive(MpPrimitive::text_run(
            crate::MpPrimitiveId(0),
            scene.root_spatial_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(50.0, 30.0),
                size: dvec2(100.0, 20.0),
            },
            MpGlyphRunKey(1),
            vec4(1.0, 1.0, 1.0, 1.0),
        ));
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);
        let resources = test_resources();
        let registry = test_registry(&resources);

        let (lowered, _) = renderer
            .lower_scene(None, &scene, &registry, &resources.glyph_runs, scene.root_viewport_rect())
            .unwrap();

        assert_eq!(lowered.text_runs.len(), 1);
        assert!(lowered.text_runs[0].stable_id > 0);
        assert_eq!(lowered.text_runs[0].local_rect.pos, dvec2(50.0, 30.0));
        assert_eq!(lowered.text_runs[0].glyphs.len(), 2);
        assert_eq!(lowered.text_runs[0].glyphs[0].origin, dvec2(5.0, 12.0));
        assert_eq!(lowered.text_runs[0].glyphs[1].origin, dvec2(15.0, 12.0));
    }

    #[test]
    fn text_picture_fallback_uses_zero_origin_local_rect() {
        let mut scene = MpScene::new(
            MpSceneId(10),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let transform_id = non_affine_transformed_spatial(&mut scene);
        scene.push_primitive(MpPrimitive::text_run(
            crate::MpPrimitiveId(0),
            transform_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(10.0, 12.0),
                size: dvec2(120.0, 32.0),
            },
            MpGlyphRunKey(1),
            vec4(1.0, 1.0, 1.0, 1.0),
        ));
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);
        let resources = test_resources();
        let registry = test_registry(&resources);

        let (lowered, _) = renderer
            .lower_scene(None, &scene, &registry, &resources.glyph_runs, scene.root_viewport_rect())
            .unwrap();

        assert_eq!(lowered.pictures.len(), 1);
        let picture = &lowered.pictures[0];
        let task = &lowered.tasks[picture.task_id];
        let nested_scene = match &task.kind {
            MpBrowserTaskKind::Scene(scene) => scene,
            other => panic!("expected scene task, got {other:?}"),
        };
        assert_eq!(nested_scene.host_rect.pos, dvec2(0.0, 0.0));
        assert_eq!(nested_scene.host_rect.size, picture.local_rect.size);
        assert_eq!(nested_scene.retained_scene_id, lowered.retained_scene_id);
        assert_eq!(nested_scene.text_runs.len(), 1);
        assert!(nested_scene.text_runs[0].stable_id > 0);
        assert_eq!(nested_scene.text_runs[0].local_rect.pos, dvec2(0.0, 0.0));
        assert_eq!(nested_scene.text_runs[0].glyphs[0].origin, dvec2(5.0, 12.0));
        assert_eq!(nested_scene.text_runs[0].glyphs[1].origin, dvec2(15.0, 12.0));
    }

    #[test]
    fn text_at_two_positions_shares_glyph_origins() {
        let mut scene = MpScene::new(
            MpSceneId(11),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(500.0, 400.0),
            },
        );
        scene.push_primitive(MpPrimitive::text_run(
            crate::MpPrimitiveId(0),
            scene.root_spatial_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(10.0, 20.0),
                size: dvec2(100.0, 20.0),
            },
            MpGlyphRunKey(1),
            vec4(1.0, 1.0, 1.0, 1.0),
        ));
        scene.push_primitive(MpPrimitive::text_run(
            crate::MpPrimitiveId(1),
            scene.root_spatial_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(200.0, 300.0),
                size: dvec2(100.0, 20.0),
            },
            MpGlyphRunKey(1),
            vec4(1.0, 1.0, 1.0, 1.0),
        ));
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);
        let resources = test_resources();
        let registry = test_registry(&resources);

        let (lowered, _) = renderer
            .lower_scene(None, &scene, &registry, &resources.glyph_runs, scene.root_viewport_rect())
            .unwrap();

        assert_eq!(lowered.text_runs.len(), 2);
        assert_ne!(lowered.text_runs[0].stable_id, lowered.text_runs[1].stable_id);
        assert_eq!(lowered.text_runs[0].local_rect.pos, dvec2(10.0, 20.0));
        assert_eq!(lowered.text_runs[1].local_rect.pos, dvec2(200.0, 300.0));
        assert_ne!(lowered.text_runs[0].local_rect.pos, lowered.text_runs[1].local_rect.pos);
        assert_eq!(lowered.text_runs[0].glyphs[0].origin, lowered.text_runs[1].glyphs[0].origin);
        assert_eq!(lowered.text_runs[0].glyphs[1].origin, lowered.text_runs[1].glyphs[1].origin);
    }

    #[test]
    fn text_decorations_preserved_through_lowering() {
        let mut scene = MpScene::new(
            MpSceneId(12),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        scene.push_primitive(MpPrimitive::text_run(
            crate::MpPrimitiveId(0),
            scene.root_spatial_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(25.0, 35.0),
                size: dvec2(140.0, 24.0),
            },
            MpGlyphRunKey(1),
            vec4(0.9, 0.9, 0.9, 1.0),
        ));
        let mut resources = test_resources();
        resources.glyph_runs.insert(
            MpGlyphRunKey(1),
            MpGlyphRunResource {
                text: "hi".to_string(),
                font_keys: vec![MpFontKey(1)],
                glyphs: vec![MpPositionedGlyph {
                    glyph_id: 7,
                    font_size_px: 14.0,
                    origin: dvec2(5.0, 12.0),
                    font_slot: 0,
                }],
                metrics: MpGlyphRunMetrics {
                    advance_width_px: 22.0,
                    baseline_ascent_px: 11.0,
                    underline_offset_px: 2.0,
                    underline_thickness_px: 1.5,
                    strikeout_offset_px: 4.0,
                    strikeout_thickness_px: 1.25,
                },
                decorations: MpTextDecorations {
                    background_color: Some(vec4(1.0, 0.0, 0.0, 1.0)),
                    decoration_color: Some(vec4(0.0, 1.0, 0.0, 1.0)),
                    underline: true,
                    overline: false,
                    line_through: true,
                    shadows: vec![MpTextShadow {
                        offset: dvec2(1.0, 2.0),
                        blur_radius_px: 3.0,
                        color: vec4(0.0, 0.0, 0.0, 0.5),
                    }],
                },
            },
        );
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);
        let registry = test_registry(&resources);

        let (lowered, _) = renderer
            .lower_scene(None, &scene, &registry, &resources.glyph_runs, scene.root_viewport_rect())
            .unwrap();

        assert_eq!(lowered.text_runs.len(), 1);
        let text_run = &lowered.text_runs[0];
        assert_eq!(text_run.decorations.background_color, Some(vec4(1.0, 0.0, 0.0, 1.0)));
        assert_eq!(text_run.decorations.decoration_color, Some(vec4(0.0, 1.0, 0.0, 1.0)));
        assert!(text_run.decorations.underline);
        assert!(text_run.decorations.line_through);
        assert!(!text_run.decorations.overline);
        assert_eq!(text_run.decorations.shadows.len(), 1);
        assert_eq!(text_run.metrics.advance_width_px, 22.0);
        assert_eq!(text_run.metrics.baseline_ascent_px, 11.0);
        assert_eq!(text_run.metrics.underline_offset_px, 2.0);
        assert_eq!(text_run.metrics.underline_thickness_px, 1.5);
        assert_eq!(text_run.metrics.strikeout_offset_px, 4.0);
        assert_eq!(text_run.metrics.strikeout_thickness_px, 1.25);
    }
}
