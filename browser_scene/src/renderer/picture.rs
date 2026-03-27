use std::collections::HashMap;

use makepad_widgets::{dvec2, Rect};

use makepad_compositor::{
    MpBlendMode as MpCompositorBlendMode, MpBrowserPicture,
    MpBrowserScene as MpCompositorBrowserScene, MpBrowserTask, MpBrowserTaskKind,
};

use crate::{
    effect::MpEffectId,
    embed::MpEmbed,
    resource::{MpGlyphRunKey, MpGlyphRunResource, ResourceRegistry},
    scene::{MpDocument, MpScene},
    MpBlendMode,
};

use super::{
    clip::lower_clip_chain,
    geom::{outset_rect, resolve_embed_rect, resolve_primitive_rect, union_rects},
    retained_patch,
    traversal::SceneItemRef,
    MpRenderError, MpRendererStats, MpSceneLowerer,
};

impl MpSceneLowerer<'_> {
    pub(super) fn lower_effect_picture(
        &mut self,
        document: Option<&MpDocument>,
        scene: &MpScene,
        registry: &ResourceRegistry,
        glyph_runs: &HashMap<MpGlyphRunKey, MpGlyphRunResource>,
        lowered: &mut MpCompositorBrowserScene,
        patch_builder: &mut retained_patch::ScenePatchBuilder,
        effect_id: MpEffectId,
        items: &[SceneItemRef<'_>],
        stats: &mut MpRendererStats,
    ) -> Result<(), MpRenderError> {
        let effect = &scene.effects[effect_id.0];
        let transform_spatial_id = scene.find_transform_ancestor(effect.spatial_id);
        let bounds = effect_run_bounds(scene, effect_id, items, transform_spatial_id)?;
        let (nested_scene, nested_isolated_count, nested_patch) = self.lower_scene_items_with_patch(
            document,
            scene,
            registry,
            glyph_runs,
            items,
            Some(effect_id),
            bounds,
        )?;
        let scene_task_id = lowered.push_task(MpBrowserTask {
            size: bounds.size,
            cache_key: None,
            kind: MpBrowserTaskKind::Scene(Box::new(nested_scene)),
        });
        patch_builder.record_task(
            scene_task_id,
            retained_patch::RetainedTaskSource::Scene {
                source: retained_patch::RetainedTaskSceneSource::Effect {
                    effect_id,
                    origin_spatial_id: transform_spatial_id,
                    items: items
                        .iter()
                        .map(|item| match item {
                            SceneItemRef::Primitive(primitive) => {
                                retained_patch::RetainedSceneItemSource::Primitive(primitive.id)
                            }
                            SceneItemRef::Embed(embed) => {
                                retained_patch::RetainedSceneItemSource::Embed(embed.pipeline_id)
                            }
                        })
                        .collect(),
                },
                patch: Box::new(nested_patch),
            },
        );
        let task_id = effect.filters.iter().fold(scene_task_id, |input, filter| match filter {
            crate::MpFilter::Blur(radius) if *radius > 0.01 => {
                let blur_task_id = lowered.push_task(MpBrowserTask {
                    size: bounds.size,
                    cache_key: None,
                    kind: MpBrowserTaskKind::Blur {
                        input,
                        radius: *radius,
                    },
                });
                patch_builder.record_task(
                    blur_task_id,
                    retained_patch::RetainedTaskSource::Blur { input_task_id: input },
                );
                blur_task_id
            }
            crate::MpFilter::Named(name) => {
                super::log_unsupported_skip_once(format!("unsupported effect filter {name}"));
                input
            }
            _ => input,
        });
        let transform_id =
            self.ensure_transform(lowered, patch_builder, scene, transform_spatial_id)?;
        let clip_chain = lower_clip_chain(
            scene,
            effect.spatial_id,
            effect.clip_chain_id,
            transform_spatial_id,
        )?;
        let clip_chain_id = self.ensure_clip_chain(
            lowered,
            patch_builder,
            clip_chain,
            retained_patch::RetainedClipChainSource {
                spatial_id: effect.spatial_id,
                clip_chain_id: effect.clip_chain_id,
                origin_spatial_id: transform_spatial_id,
            },
        );
        let picture_id = lowered.push_picture(MpBrowserPicture {
            local_rect: bounds,
            transform_id,
            clip_chain_id,
            task_id,
            opacity: effect.opacity * effect_filter_opacity(&effect.filters),
            blend_mode: match &effect.blend_mode {
                MpBlendMode::Normal => MpCompositorBlendMode::Normal,
                MpBlendMode::Named(name) => MpCompositorBlendMode::Named(name.clone()),
            },
        });
        patch_builder.record_picture(
            picture_id,
            retained_patch::RetainedPictureSource::Effect {
                effect_id,
                origin_spatial_id: transform_spatial_id,
                items: items
                    .iter()
                    .map(|item| match item {
                        SceneItemRef::Primitive(primitive) => {
                            retained_patch::RetainedSceneItemSource::Primitive(primitive.id)
                        }
                        SceneItemRef::Embed(embed) => {
                            retained_patch::RetainedSceneItemSource::Embed(embed.pipeline_id)
                        }
                    })
                    .collect(),
            },
        );
        stats.isolated_boundary_count += 1 + nested_isolated_count;
        stats.isolated_primitive_count += items
            .iter()
            .filter(|item| matches!(item, SceneItemRef::Primitive(_)))
            .count();
        stats.compositor_surface_count += 1;
        Ok(())
    }

    pub(super) fn lower_embed_picture(
        &mut self,
        document: Option<&MpDocument>,
        scene: &MpScene,
        registry: &ResourceRegistry,
        lowered: &mut MpCompositorBrowserScene,
        patch_builder: &mut retained_patch::ScenePatchBuilder,
        embed: &MpEmbed,
        stats: &mut MpRendererStats,
    ) -> Result<(), MpRenderError> {
        let Some(document) = document else {
            return Err(MpRenderError::MissingEmbedDocument(embed.pipeline_id));
        };
        let child_document = document
            .child_document(embed.pipeline_id)
            .ok_or(MpRenderError::MissingEmbedDocument(embed.pipeline_id))?;
        let child_viewport = Rect {
            pos: dvec2(0.0, 0.0),
            size: embed.bounds.size,
        };
        let (child_scene, _, child_patch) = self.lower_scene_with_patch(
            Some(child_document),
            &child_document.scene,
            registry,
            &child_document.glyph_runs,
            child_viewport,
        )?;
        let transform_spatial_id = scene.find_transform_ancestor(embed.spatial_id);
        let bounds = resolve_embed_rect(scene, embed, transform_spatial_id)?;
        let transform_id =
            self.ensure_transform(lowered, patch_builder, scene, transform_spatial_id)?;
        let clip_chain = lower_clip_chain(
            scene,
            embed.spatial_id,
            embed.clip_chain_id,
            transform_spatial_id,
        )?;
        let clip_chain_id = self.ensure_clip_chain(
            lowered,
            patch_builder,
            clip_chain,
            retained_patch::RetainedClipChainSource {
                spatial_id: embed.spatial_id,
                clip_chain_id: embed.clip_chain_id,
                origin_spatial_id: transform_spatial_id,
            },
        );
        let task_id = lowered.push_task(MpBrowserTask {
            size: embed.bounds.size,
            cache_key: None,
            kind: MpBrowserTaskKind::Scene(Box::new(child_scene)),
        });
        patch_builder.record_task(
            task_id,
            retained_patch::RetainedTaskSource::Scene {
                source: retained_patch::RetainedTaskSceneSource::Embed {
                    pipeline_id: embed.pipeline_id,
                },
                patch: Box::new(child_patch),
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
            retained_patch::RetainedPictureSource::Embed {
                pipeline_id: embed.pipeline_id,
                origin_spatial_id: transform_spatial_id,
            },
        );
        stats.compositor_surface_count += 1;
        Ok(())
    }
}

pub(super) fn effect_run_bounds(
    scene: &MpScene,
    effect_id: MpEffectId,
    items: &[SceneItemRef<'_>],
    origin_spatial_id: Option<crate::MpSpatialId>,
) -> Result<Rect, MpRenderError> {
    let effect = &scene.effects[effect_id.0];
    let mut bounds = None;
    for item in items {
        bounds = union_rects(
            bounds,
            outset_rect(
                resolve_item_bounds(scene, *item, origin_spatial_id)?,
                effect_filter_outset(&effect.filters),
            ),
        );
    }
    bounds.ok_or(MpRenderError::UnsupportedEffect(effect_id))
}

pub(super) fn resolve_item_bounds(
    scene: &MpScene,
    item: SceneItemRef<'_>,
    origin_spatial_id: Option<crate::MpSpatialId>,
) -> Result<Rect, MpRenderError> {
    match item {
        SceneItemRef::Primitive(primitive) => {
            resolve_primitive_rect(scene, primitive, origin_spatial_id)
        }
        SceneItemRef::Embed(embed) => resolve_embed_rect(scene, embed, origin_spatial_id),
    }
}

fn effect_filter_opacity(filters: &[crate::MpFilter]) -> f32 {
    filters.iter().fold(1.0, |opacity, filter| match filter {
        crate::MpFilter::Opacity(value) => opacity * *value,
        _ => opacity,
    })
}

fn effect_filter_outset(filters: &[crate::MpFilter]) -> f64 {
    filters.iter().fold(0.0, |outset, filter| match filter {
        crate::MpFilter::Blur(radius) => outset + (*radius as f64 * 3.0),
        _ => outset,
    })
}

#[cfg(test)]
mod tests {
    use makepad_widgets::{dvec2, vec4, Cx, Rect};

    use std::sync::Arc;

    use crate::{
        effect::{MpEffectNode, MpIsolation},
        embed::MpEmbed,
        primitive::MpPrimitive,
        resource::{
            MpFontKey, MpFontResource, MpGlyphRunKey, MpGlyphRunResource, MpPositionedGlyph,
            MpResourceStore, ResourceRegistry,
        },
        MpBrowserRenderer, MpChildDocument, MpDocument, MpDocumentId, MpPipelineId, MpSceneId,
    };

    use super::*;

    #[test]
    fn lower_scene_keeps_transformed_effect_pictures() {
        let mut scene = MpScene::new(
            crate::MpSceneId(6),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let transform_id = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(scene.root_spatial_id),
            kind: crate::MpSpatialKind::ReferenceFrame(crate::MpReferenceFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(200.0, 120.0),
                },
                placement_origin: dvec2(40.0, 20.0),
                transform: Some(makepad_widgets::Mat4f {
                    v: [
                        1.5, 0.0, 0.0, 0.0, 0.0, 1.5, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
                        0.0, 0.0, 1.0,
                    ],
                }),
                perspective: None,
                transform_style: crate::MpTransformStyle::Flat,
                backface_visibility: crate::MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        });
        let effect_id = scene.push_effect(MpEffectNode {
            spatial_id: transform_id,
            clip_chain_id: scene.root_clip_chain_id,
            opacity: 0.5,
            filters: vec![crate::MpFilter::Blur(2.0)],
            blend_mode: crate::MpBlendMode::Normal,
            isolation: MpIsolation::Isolate,
            mask: None,
        });
        let mut primitive = MpPrimitive::solid_rect(
            crate::MpPrimitiveId(0),
            transform_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(10.0, 12.0),
                size: dvec2(60.0, 24.0),
            },
            vec4(1.0, 0.0, 0.0, 1.0),
        );
        primitive.effect_id = Some(effect_id);
        scene.push_primitive(primitive);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);
        let resources = MpResourceStore::default();
        let registry = ResourceRegistry::from(&resources);

        let (lowered, _) = renderer
            .lower_scene(None, &scene, &registry, &resources.glyph_runs, scene.root_viewport_rect())
            .unwrap();

        assert_eq!(lowered.pictures.len(), 1);
        assert_ne!(lowered.pictures[0].transform_id, 0);
    }

    #[test]
    fn lower_scene_keeps_stable_text_run_ids_in_iframe_child_documents() {
        let mut child_scene = MpScene::new(
            MpSceneId(20),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(120.0, 80.0),
            },
        );
        child_scene.push_primitive(MpPrimitive::text_run(
            crate::MpPrimitiveId(0),
            child_scene.root_spatial_id,
            child_scene.root_clip_chain_id,
            Rect {
                pos: dvec2(10.0, 12.0),
                size: dvec2(90.0, 24.0),
            },
            MpGlyphRunKey(1),
            vec4(1.0, 1.0, 1.0, 1.0),
        ));
        let child_document = MpDocument {
            id: MpDocumentId(2),
            epoch: 0,
            scene: child_scene,
            glyph_runs: std::iter::once((
                MpGlyphRunKey(1),
                MpGlyphRunResource {
                    text: "iframe".to_string(),
                    font_keys: vec![MpFontKey(1)],
                    glyphs: vec![MpPositionedGlyph {
                        glyph_id: 7,
                        font_size_px: 14.0,
                        origin: dvec2(5.0, 12.0),
                        font_slot: 0,
                    }],
                    metrics: Default::default(),
                    decorations: Default::default(),
                },
            ))
            .collect(),
            child_documents: Vec::new(),
        };

        let mut root_scene = MpScene::new(
            MpSceneId(21),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(240.0, 180.0),
            },
        );
        root_scene.push_embed(MpEmbed {
            scene_id: child_document.scene.id,
            pipeline_id: MpPipelineId(77),
            spatial_id: root_scene.root_spatial_id,
            clip_chain_id: root_scene.root_clip_chain_id,
            effect_id: None,
            bounds: Rect {
                pos: dvec2(20.0, 30.0),
                size: dvec2(120.0, 80.0),
            },
            hit_test_tag: None,
        });
        let root_document = MpDocument {
            id: MpDocumentId(1),
            epoch: 0,
            scene: root_scene,
            glyph_runs: Default::default(),
            child_documents: vec![MpChildDocument {
                pipeline_id: MpPipelineId(77),
                document: Box::new(child_document),
            }],
        };

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);
        let mut resources = MpResourceStore::default();
        resources.fonts.insert(
            MpFontKey(1),
            MpFontResource {
                bytes: Arc::from(vec![1, 2, 3]),
                face_index: 0,
            },
        );
        let registry = ResourceRegistry::from(&resources);

        let (lowered, _) = renderer
            .lower_scene(
                Some(&root_document),
                &root_document.scene,
                &registry,
                &root_document.glyph_runs,
                root_document.scene.root_viewport_rect(),
            )
            .unwrap();

        assert_eq!(lowered.pictures.len(), 1);
        let picture = &lowered.pictures[0];
        let task = &lowered.tasks[picture.task_id];
        let child_scene = match &task.kind {
            MpBrowserTaskKind::Scene(scene) => scene,
            other => panic!("expected child iframe scene task, got {other:?}"),
        };
        assert_eq!(child_scene.retained_scene_id, lowered.retained_scene_id);
        assert_eq!(child_scene.text_runs.len(), 1);
        assert!(child_scene.text_runs[0].stable_id > 0);
    }
}
