use std::collections::{HashMap, HashSet};

use makepad_widgets::Rect;

use makepad_compositor::MpBrowserScene as MpCompositorBrowserScene;

use crate::{
    embed::MpEmbed,
    primitive::MpPrimitive,
    resource::{MpGlyphRunKey, MpGlyphRunResource, ResourceRegistry},
    scene::{MpDocument, MpScene, MpSceneItem},
    MpEffectId,
};

use super::{
    retained_patch, unsupported_primitive_skip, MpRenderError, MpRendererStats, MpSceneLowerer,
};

#[derive(Clone, Copy)]
pub(super) enum SceneItemRef<'a> {
    Primitive(&'a MpPrimitive),
    Embed(&'a MpEmbed),
}

impl<'a> SceneItemRef<'a> {
    pub(super) fn effect_id(self) -> Option<MpEffectId> {
        match self {
            Self::Primitive(primitive) => primitive.effect_id,
            Self::Embed(embed) => embed.effect_id,
        }
    }
}

fn scene_item_ref(scene: &MpScene, item: MpSceneItem) -> SceneItemRef<'_> {
    match item {
        MpSceneItem::Primitive(primitive_id) => {
            SceneItemRef::Primitive(&scene.primitives[primitive_id.0])
        }
        MpSceneItem::Embed(embed_id) => SceneItemRef::Embed(&scene.embeds[embed_id.0]),
    }
}

impl MpSceneLowerer<'_> {
    pub(super) fn lower_scene(
        &mut self,
        document: Option<&MpDocument>,
        scene: &MpScene,
        registry: &ResourceRegistry,
        glyph_runs: &HashMap<MpGlyphRunKey, MpGlyphRunResource>,
        viewport: Rect,
    ) -> Result<(MpCompositorBrowserScene, MpRendererStats), MpRenderError> {
        let (lowered, stats, _) = self.lower_scene_with_patch(
            document,
            scene,
            registry,
            glyph_runs,
            viewport,
        )?;
        Ok((lowered, stats))
    }

    pub(super) fn lower_scene_with_patch(
        &mut self,
        document: Option<&MpDocument>,
        scene: &MpScene,
        registry: &ResourceRegistry,
        glyph_runs: &HashMap<MpGlyphRunKey, MpGlyphRunResource>,
        viewport: Rect,
    ) -> Result<
        (
            MpCompositorBrowserScene,
            MpRendererStats,
            retained_patch::RetainedScenePatch,
        ),
        MpRenderError,
    > {
        let isolated_effects: HashSet<MpEffectId> = scene
            .effects
            .iter()
            .enumerate()
            .filter_map(|(index, effect)| effect.requires_isolation().then_some(MpEffectId(index)))
            .collect();
        let mut lowered = MpCompositorBrowserScene::new_with_retained_scene_id(
            self.retained_scene_id,
            viewport,
        );
        let mut stats = MpRendererStats::default();
        let mut patch_builder = retained_patch::ScenePatchBuilder::new();
        let mut index = 0;
        while index < scene.items.len() {
            let item = scene_item_ref(scene, scene.items[index]);
            if let Some(effect_id) = item
                .effect_id()
                .filter(|effect_id| isolated_effects.contains(effect_id))
            {
                let (items, end) =
                    collect_isolated_effect_run(scene, index, effect_id, &isolated_effects);
                self.lower_effect_picture(
                    document,
                    scene,
                    registry,
                    glyph_runs,
                    &mut lowered,
                    &mut patch_builder,
                    effect_id,
                    &items,
                    &mut stats,
                )?;
                index = end;
                continue;
            }
            self.lower_scene_item(
                document,
                scene,
                registry,
                glyph_runs,
                &mut lowered,
                &mut patch_builder,
                item,
                None,
                &mut stats,
            )?;
            index += 1;
        }
        Ok((lowered, stats, patch_builder.finish()))
    }

    pub(super) fn lower_scene_items_with_patch(
        &mut self,
        document: Option<&MpDocument>,
        scene: &MpScene,
        registry: &ResourceRegistry,
        glyph_runs: &HashMap<MpGlyphRunKey, MpGlyphRunResource>,
        items: &[SceneItemRef<'_>],
        excluded_effect_id: Option<MpEffectId>,
        viewport: Rect,
    ) -> Result<
        (
            MpCompositorBrowserScene,
            usize,
            retained_patch::RetainedScenePatch,
        ),
        MpRenderError,
    > {
        let isolated_effects: HashSet<MpEffectId> = scene
            .effects
            .iter()
            .enumerate()
            .filter_map(|(index, effect)| effect.requires_isolation().then_some(MpEffectId(index)))
            .collect();
        let mut lowered = MpCompositorBrowserScene::new_with_retained_scene_id(
            self.retained_scene_id,
            viewport,
        );
        let mut isolated_boundary_count = 0;
        let mut patch_builder = retained_patch::ScenePatchBuilder::new();
        let mut index = 0;
        while index < items.len() {
            let item = items[index];
            if let Some(effect_id) = item
                .effect_id()
                .filter(|effect_id| Some(*effect_id) != excluded_effect_id)
                .filter(|effect_id| isolated_effects.contains(effect_id))
            {
                let (run_items, end) =
                    collect_isolated_effect_run_from_slice(items, index, effect_id);
                let mut nested_stats = MpRendererStats::default();
                self.lower_effect_picture(
                    document,
                    scene,
                    registry,
                    glyph_runs,
                    &mut lowered,
                    &mut patch_builder,
                    effect_id,
                    &run_items,
                    &mut nested_stats,
                )?;
                isolated_boundary_count += 1 + nested_stats.isolated_boundary_count;
                index = end;
                continue;
            }
            self.lower_scene_item(
                document,
                scene,
                registry,
                glyph_runs,
                &mut lowered,
                &mut patch_builder,
                item,
                excluded_effect_id,
                &mut MpRendererStats::default(),
            )?;
            index += 1;
        }
        Ok((lowered, isolated_boundary_count, patch_builder.finish()))
    }

    fn lower_scene_item(
        &mut self,
        document: Option<&MpDocument>,
        scene: &MpScene,
        registry: &ResourceRegistry,
        glyph_runs: &HashMap<MpGlyphRunKey, MpGlyphRunResource>,
        lowered: &mut MpCompositorBrowserScene,
        patch_builder: &mut retained_patch::ScenePatchBuilder,
        item: SceneItemRef<'_>,
        allowed_effect_id: Option<MpEffectId>,
        stats: &mut MpRendererStats,
    ) -> Result<(), MpRenderError> {
        match item {
            SceneItemRef::Primitive(primitive) => {
                if primitive.effect_id != allowed_effect_id && primitive.effect_id.is_some() {
                    return unsupported_primitive_skip(primitive, "unsupported primitive");
                }
                if self.lower_direct_text(
                    scene,
                    registry,
                    glyph_runs,
                    lowered,
                    patch_builder,
                    primitive,
                )? {
                    stats.direct_primitive_count += 1;
                    return Ok(());
                }
                if self.lower_text_picture(
                    scene,
                    registry,
                    glyph_runs,
                    lowered,
                    patch_builder,
                    primitive,
                    stats,
                )? {
                    return Ok(());
                }
                if self.lower_direct_primitive(scene, registry, lowered, patch_builder, primitive)? {
                    stats.direct_primitive_count += 1;
                    return Ok(());
                }
                unsupported_primitive_skip(primitive, "unsupported primitive")
            }
            SceneItemRef::Embed(embed) => self.lower_embed_picture(
                document,
                scene,
                registry,
                lowered,
                patch_builder,
                embed,
                stats,
            ),
        }
    }
}

fn collect_isolated_effect_run<'a>(
    scene: &'a MpScene,
    start: usize,
    effect_id: MpEffectId,
    isolated_effects: &HashSet<MpEffectId>,
) -> (Vec<SceneItemRef<'a>>, usize) {
    let mut end = start;
    let mut items = Vec::new();
    while end < scene.items.len() {
        let item = scene_item_ref(scene, scene.items[end]);
        if item.effect_id().filter(|id| isolated_effects.contains(id)) != Some(effect_id) {
            break;
        }
        items.push(item);
        end += 1;
    }
    (items, end)
}

fn collect_isolated_effect_run_from_slice<'a>(
    items: &[SceneItemRef<'a>],
    start: usize,
    effect_id: MpEffectId,
) -> (Vec<SceneItemRef<'a>>, usize) {
    let mut end = start;
    let mut out = Vec::new();
    while end < items.len() {
        let item = items[end];
        if item.effect_id() != Some(effect_id) {
            break;
        }
        out.push(item);
        end += 1;
    }
    (out, end)
}

#[cfg(test)]
mod tests {
    use makepad_widgets::{dvec2, vec4, Cx, Rect};

    use crate::{
        effect::{MpEffectNode, MpIsolation},
        primitive::MpPrimitive,
        resource::{MpResourceStore, ResourceRegistry},
        MpBlendMode, MpBrowserRenderer, MpSceneId,
    };

    use super::*;

    #[test]
    fn lower_scene_interleaves_direct_and_effect_pictures() {
        let mut scene = MpScene::new(
            MpSceneId(1),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let effect_id = scene.push_effect(MpEffectNode {
            spatial_id: scene.root_spatial_id,
            clip_chain_id: scene.root_clip_chain_id,
            opacity: 0.5,
            filters: vec![crate::MpFilter::Blur(4.0)],
            blend_mode: MpBlendMode::Normal,
            isolation: MpIsolation::Isolate,
            mask: None,
        });
        scene.push_primitive(MpPrimitive::solid_rect(
            crate::MpPrimitiveId(0),
            scene.root_spatial_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(20.0, 20.0),
            },
            vec4(1.0, 0.0, 0.0, 1.0),
        ));
        let mut isolated = MpPrimitive::solid_rect(
            crate::MpPrimitiveId(1),
            scene.root_spatial_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(30.0, 30.0),
                size: dvec2(20.0, 20.0),
            },
            vec4(0.0, 1.0, 0.0, 1.0),
        );
        isolated.effect_id = Some(effect_id);
        scene.push_primitive(isolated);
        scene.push_primitive(MpPrimitive::solid_rect(
            crate::MpPrimitiveId(2),
            scene.root_spatial_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(60.0, 0.0),
                size: dvec2(20.0, 20.0),
            },
            vec4(0.0, 0.0, 1.0, 1.0),
        ));

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);
        let resources = MpResourceStore::default();
        let registry = ResourceRegistry::from(&resources);
        let (lowered, stats) = renderer
            .lower_scene(None, &scene, &registry, &resources.glyph_runs, scene.root_viewport_rect())
            .unwrap();

        assert_eq!(stats.direct_primitive_count, 2);
        assert_eq!(stats.isolated_boundary_count, 1);
        assert_eq!(stats.isolated_primitive_count, 1);
        assert_eq!(stats.compositor_surface_count, 1);
        assert_eq!(lowered.draw_order.len(), 3);
    }
}
