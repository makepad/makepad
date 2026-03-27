use makepad_widgets::vec2;

use makepad_compositor::{
    MpBrowserGradientStop, MpBrowserPrimitive, MpBrowserPrimitiveKind,
    MpBrowserScene as MpCompositorBrowserScene,
};

use crate::{
    primitive::{
        MpBorder, MpBoxShadow, MpImage, MpLineDecoration, MpPrimitive, MpPrimitiveKind,
        MpRepeatingImage, MpRoundedRect, MpSolidRect,
    },
    resource::ResourceRegistry,
    scene::MpScene,
};

use super::{
    clip::lower_clip_chain,
    geom::{radius_to_vec4, resolve_primitive_rect},
    retained_patch, MpRenderError, MpSceneLowerer,
};

impl MpSceneLowerer<'_> {
    pub(super) fn lower_direct_primitive(
        &mut self,
        scene: &MpScene,
        registry: &ResourceRegistry,
        lowered: &mut MpCompositorBrowserScene,
        patch_builder: &mut retained_patch::ScenePatchBuilder,
        primitive: &MpPrimitive,
    ) -> Result<bool, MpRenderError> {
        let Some(lowered_kind) = self.lower_direct_primitive_kind(registry, primitive)? else {
            return Ok(false);
        };
        let transform_spatial_id = scene.find_transform_ancestor(primitive.spatial_id);
        let transform_id =
            self.ensure_transform(lowered, patch_builder, scene, transform_spatial_id)?;
        let clip_chain = lower_clip_chain(
            scene,
            primitive.spatial_id,
            primitive.clip_chain_id,
            transform_spatial_id,
        )?;
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
        let primitive_id = lowered.push_primitive(MpBrowserPrimitive {
            local_rect,
            transform_id,
            clip_chain_id,
            kind: lowered_kind,
        });
        patch_builder.record_primitive(primitive_id, primitive.id, transform_spatial_id);
        Ok(true)
    }

    pub(super) fn lower_direct_primitive_kind(
        &mut self,
        registry: &ResourceRegistry,
        primitive: &MpPrimitive,
    ) -> Result<Option<MpBrowserPrimitiveKind>, MpRenderError> {
        Ok(Some(match &primitive.kind {
            MpPrimitiveKind::SolidRect(MpSolidRect { color }) => {
                MpBrowserPrimitiveKind::SolidRect { color: *color }
            }
            MpPrimitiveKind::RoundedRect(MpRoundedRect { color, radius }) => {
                MpBrowserPrimitiveKind::RoundedRect {
                    color: *color,
                    radius: radius_to_vec4(*radius),
                }
            }
            MpPrimitiveKind::Border(MpBorder {
                color,
                width,
                radius,
            }) => MpBrowserPrimitiveKind::Border {
                color: *color,
                width: *width,
                radius: radius_to_vec4(*radius),
            },
            MpPrimitiveKind::TextRun(_) => return Ok(None),
            MpPrimitiveKind::Image(MpImage { image_key }) => {
                let Some(image) = registry.images.get(image_key) else {
                    return Err(MpRenderError::MissingImageResource(primitive.id));
                };
                MpBrowserPrimitiveKind::Image {
                    texture: self.ensure_image_texture(*image_key, image),
                }
            }
            MpPrimitiveKind::RepeatingImage(MpRepeatingImage { image_key }) => {
                let Some(image) = registry.images.get(image_key) else {
                    return Err(MpRenderError::MissingImageResource(primitive.id));
                };
                MpBrowserPrimitiveKind::RepeatingImage {
                    texture: self.ensure_image_texture(*image_key, image),
                    tile_size: vec2(image.size.x as f32, image.size.y as f32),
                }
            }
            MpPrimitiveKind::LinearGradient(gradient) => MpBrowserPrimitiveKind::LinearGradient {
                start: gradient.start,
                end: gradient.end,
                repeating: gradient.repeating,
                stops: gradient
                    .stops
                    .iter()
                    .map(|stop| MpBrowserGradientStop {
                        offset: stop.offset,
                        color: stop.color,
                    })
                    .collect(),
            },
            MpPrimitiveKind::RadialGradient(gradient) => MpBrowserPrimitiveKind::RadialGradient {
                center: gradient.center,
                radius: gradient.radius,
                repeating: gradient.repeating,
                stops: gradient
                    .stops
                    .iter()
                    .map(|stop| MpBrowserGradientStop {
                        offset: stop.offset,
                        color: stop.color,
                    })
                    .collect(),
            },
            MpPrimitiveKind::ConicGradient(gradient) => MpBrowserPrimitiveKind::ConicGradient {
                center: gradient.center,
                start_angle_rad: gradient.start_angle_rad,
                repeating: gradient.repeating,
                stops: gradient
                    .stops
                    .iter()
                    .map(|stop| MpBrowserGradientStop {
                        offset: stop.offset,
                        color: stop.color,
                    })
                    .collect(),
            },
            MpPrimitiveKind::BoxShadow(MpBoxShadow {
                color,
                box_offset,
                box_size,
                sigma,
                corner_radius_px,
                inset,
            }) => MpBrowserPrimitiveKind::BoxShadow {
                color: *color,
                box_offset: *box_offset,
                box_size: *box_size,
                sigma: *sigma,
                corner_radius_px: *corner_radius_px,
                inset: *inset,
            },
            MpPrimitiveKind::LineDecoration(MpLineDecoration { color, .. }) => {
                MpBrowserPrimitiveKind::SolidRect { color: *color }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use makepad_widgets::{dvec2, vec3, vec4, Cx, Mat4f, Rect};
    use std::sync::Arc;

    use crate::{
        primitive::{MpLineDecoration, MpPrimitive},
        resource::{MpResourceStore, ResourceRegistry},
        MpBrowserRenderer, MpRepeatingImage, MpScene, MpSceneId,
    };

    use super::*;

    #[test]
    fn lower_scene_maps_repeating_image_to_compositor_repeating_image() {
        let mut scene = MpScene::new(
            MpSceneId(5),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        scene.push_primitive(MpPrimitive {
            id: crate::MpPrimitiveId(0),
            spatial_id: scene.root_spatial_id,
            clip_chain_id: scene.root_clip_chain_id,
            effect_id: None,
            bounds: Rect {
                pos: dvec2(20.0, 30.0),
                size: dvec2(120.0, 90.0),
            },
            kind: MpPrimitiveKind::RepeatingImage(MpRepeatingImage {
                image_key: crate::MpImageKey(1),
            }),
            hit_test_tag: None,
        });
        let mut resources = MpResourceStore::default();
        resources.images.insert(
            crate::MpImageKey(1),
            crate::MpImageResource {
                size: dvec2(16.0, 24.0),
                rgba8: Arc::from(vec![255; 16 * 24 * 4]),
            },
        );
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);

        let registry = ResourceRegistry::from(&resources);
        let (lowered, _) = renderer
            .lower_scene(None, &scene, &registry, &resources.glyph_runs, scene.root_viewport_rect())
            .unwrap();

        match &lowered.primitive_scene.primitives[0].kind {
            MpBrowserPrimitiveKind::RepeatingImage { tile_size, .. } => {
                assert_eq!(*tile_size, vec2(16.0, 24.0));
            }
            other => panic!("expected repeating image, got {other:?}"),
        }
    }

    #[test]
    fn lower_scene_maps_line_decoration_to_solid_rect() {
        let mut scene = MpScene::new(
            MpSceneId(8),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        scene.push_primitive(MpPrimitive {
            id: crate::MpPrimitiveId(0),
            spatial_id: scene.root_spatial_id,
            clip_chain_id: scene.root_clip_chain_id,
            effect_id: None,
            bounds: Rect {
                pos: dvec2(20.0, 30.0),
                size: dvec2(120.0, 2.0),
            },
            kind: MpPrimitiveKind::LineDecoration(MpLineDecoration {
                color: vec4(1.0, 0.0, 0.0, 1.0),
                thickness: 2.0,
            }),
            hit_test_tag: None,
        });
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);

        let resources = MpResourceStore::default();
        let registry = ResourceRegistry::from(&resources);
        let (lowered, _) = renderer
            .lower_scene(None, &scene, &registry, &resources.glyph_runs, scene.root_viewport_rect())
            .unwrap();

        match &lowered.primitive_scene.primitives[0].kind {
            MpBrowserPrimitiveKind::SolidRect { color } => {
                assert_eq!(*color, vec4(1.0, 0.0, 0.0, 1.0));
            }
            other => panic!("expected solid rect, got {other:?}"),
        }
    }

    #[test]
    fn lower_scene_applies_viewport_origin_to_root_primitive_transform() {
        let mut scene = MpScene::new(
            MpSceneId(9),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        scene.push_primitive(MpPrimitive {
            id: crate::MpPrimitiveId(0),
            spatial_id: scene.root_spatial_id,
            clip_chain_id: scene.root_clip_chain_id,
            effect_id: None,
            bounds: Rect {
                pos: dvec2(20.0, 30.0),
                size: dvec2(120.0, 90.0),
            },
            kind: MpPrimitiveKind::SolidRect(crate::MpSolidRect {
                color: vec4(0.0, 1.0, 0.0, 1.0),
            }),
            hit_test_tag: None,
        });
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = MpBrowserRenderer::new(&mut cx);

        let resources = MpResourceStore::default();
        let registry = ResourceRegistry::from(&resources);
        let viewport = Rect {
            pos: dvec2(46.0, 268.0),
            size: dvec2(400.0, 300.0),
        };
        let (lowered, _) = renderer
            .lower_scene(None, &scene, &registry, &resources.glyph_runs, viewport)
            .unwrap();
        let primitive = &lowered.primitive_scene.primitives[0];
        let transform = lowered.primitive_scene.transforms[primitive.transform_id].scene_from_origin;

        assert_eq!(lowered.host_rect, viewport);
        assert_eq!(
            transform,
            Mat4f::translation(vec3(viewport.pos.x as f32, viewport.pos.y as f32, 0.0))
        );
    }
}
