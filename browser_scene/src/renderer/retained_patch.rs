use std::collections::HashMap;

use makepad_widgets::{dvec2, Rect};

use makepad_compositor::{MpBrowserScene as MpCompositorBrowserScene, MpBrowserTaskKind};

use crate::{
    clip::MpClipChainId,
    embed::MpPipelineId,
    primitive::MpPrimitiveId,
    scene::{MpDocument, MpScene},
    MpEffectId, MpSpatialId,
};

use super::{
    clip::lower_clip_chain,
    geom::{resolve_embed_rect, resolve_primitive_rect},
    picture::effect_run_bounds,
    transform::lower_direct_transform,
    traversal::SceneItemRef,
    MpRenderError,
};

#[derive(Clone, Debug, Default)]
pub(super) struct RetainedScenePatch {
    pub(super) transforms: Vec<RetainedTransformPatch>,
    pub(super) clip_chains: Vec<RetainedClipChainPatch>,
    pub(super) primitives: Vec<RetainedPrimitivePatch>,
    pub(super) text_runs: Vec<RetainedTextRunPatch>,
    pub(super) pictures: Vec<RetainedPicturePatch>,
    pub(super) tasks: Vec<RetainedTaskPatch>,
}

#[derive(Default)]
pub(super) struct ScenePatchBuilder {
    transform_sources: HashMap<RetainedTransformSource, usize>,
    clip_chain_sources: HashMap<RetainedClipChainSource, usize>,
    patch: RetainedScenePatch,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct RetainedTransformSource {
    pub(super) transform_spatial_id: Option<MpSpatialId>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct RetainedClipChainSource {
    pub(super) spatial_id: MpSpatialId,
    pub(super) clip_chain_id: MpClipChainId,
    pub(super) origin_spatial_id: Option<MpSpatialId>,
}

#[derive(Clone, Debug)]
pub(super) struct RetainedTransformPatch {
    pub(super) transform_id: usize,
    pub(super) source: RetainedTransformSource,
}

#[derive(Clone, Debug)]
pub(super) struct RetainedClipChainPatch {
    pub(super) clip_chain_id: usize,
    pub(super) source: RetainedClipChainSource,
}

#[derive(Clone, Debug)]
pub(super) struct RetainedPrimitivePatch {
    pub(super) primitive_id: usize,
    pub(super) source_primitive_id: MpPrimitiveId,
    pub(super) origin_spatial_id: Option<MpSpatialId>,
}

#[derive(Clone, Debug)]
pub(super) struct RetainedTextRunPatch {
    pub(super) text_run_id: usize,
    pub(super) source: RetainedTextRunSource,
}

#[derive(Clone, Debug)]
pub(super) enum RetainedTextRunSource {
    Direct {
        primitive_id: MpPrimitiveId,
        origin_spatial_id: Option<MpSpatialId>,
    },
    TaskLocal {
        primitive_id: MpPrimitiveId,
        origin_spatial_id: Option<MpSpatialId>,
    },
}

#[derive(Clone, Debug)]
pub(super) struct RetainedPicturePatch {
    pub(super) picture_id: usize,
    pub(super) source: RetainedPictureSource,
}

#[derive(Clone, Debug)]
pub(super) enum RetainedPictureSource {
    TextPicture {
        primitive_id: MpPrimitiveId,
        origin_spatial_id: Option<MpSpatialId>,
    },
    Effect {
        effect_id: MpEffectId,
        origin_spatial_id: Option<MpSpatialId>,
        items: Vec<RetainedSceneItemSource>,
    },
    Embed {
        pipeline_id: MpPipelineId,
        origin_spatial_id: Option<MpSpatialId>,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum RetainedSceneItemSource {
    Primitive(MpPrimitiveId),
    Embed(MpPipelineId),
}

#[derive(Clone, Debug)]
pub(super) struct RetainedTaskPatch {
    pub(super) task_id: usize,
    pub(super) source: RetainedTaskSource,
}

#[derive(Clone, Debug)]
pub(super) enum RetainedTaskSource {
    Scene {
        source: RetainedTaskSceneSource,
        patch: Box<RetainedScenePatch>,
    },
    Blur {
        input_task_id: usize,
    },
}

#[derive(Clone, Debug)]
pub(super) enum RetainedTaskSceneSource {
    TextPicture {
        primitive_id: MpPrimitiveId,
        origin_spatial_id: Option<MpSpatialId>,
    },
    Effect {
        effect_id: MpEffectId,
        origin_spatial_id: Option<MpSpatialId>,
        items: Vec<RetainedSceneItemSource>,
    },
    Embed {
        pipeline_id: MpPipelineId,
    },
}

impl ScenePatchBuilder {
    pub(super) fn new() -> Self {
        let mut builder = Self::default();
        let root_source = RetainedTransformSource {
            transform_spatial_id: None,
        };
        builder.transform_sources.insert(root_source, 0);
        builder.patch.transforms.push(RetainedTransformPatch {
            transform_id: 0,
            source: root_source,
        });
        builder
    }

    pub(super) fn transform_id_for_source(&mut self, source: RetainedTransformSource) -> Option<usize> {
        self.transform_sources.get(&source).copied()
    }

    pub(super) fn record_transform(&mut self, transform_id: usize, source: RetainedTransformSource) {
        self.transform_sources.insert(source, transform_id);
        self.patch.transforms.push(RetainedTransformPatch { transform_id, source });
    }

    pub(super) fn clip_chain_id_for_source(&mut self, source: RetainedClipChainSource) -> Option<usize> {
        self.clip_chain_sources.get(&source).copied()
    }

    pub(super) fn record_clip_chain(&mut self, clip_chain_id: usize, source: RetainedClipChainSource) {
        self.clip_chain_sources.insert(source, clip_chain_id);
        self.patch.clip_chains.push(RetainedClipChainPatch { clip_chain_id, source });
    }

    pub(super) fn record_primitive(
        &mut self,
        primitive_id: usize,
        source_primitive_id: MpPrimitiveId,
        origin_spatial_id: Option<MpSpatialId>,
    ) {
        self.patch.primitives.push(RetainedPrimitivePatch {
            primitive_id,
            source_primitive_id,
            origin_spatial_id,
        });
    }

    pub(super) fn record_text_run(&mut self, text_run_id: usize, source: RetainedTextRunSource) {
        self.patch.text_runs.push(RetainedTextRunPatch { text_run_id, source });
    }

    pub(super) fn record_picture(&mut self, picture_id: usize, source: RetainedPictureSource) {
        self.patch.pictures.push(RetainedPicturePatch { picture_id, source });
    }

    pub(super) fn record_task(&mut self, task_id: usize, source: RetainedTaskSource) {
        self.patch.tasks.push(RetainedTaskPatch { task_id, source });
    }

    pub(super) fn finish(self) -> RetainedScenePatch {
        self.patch
    }
}

#[derive(Clone, Copy)]
struct ScenePatchContext<'a> {
    document: &'a MpDocument,
    scene: &'a MpScene,
}

pub(super) fn patch_scene_from_document(
    lowered: &mut MpCompositorBrowserScene,
    patch: &RetainedScenePatch,
    document: &MpDocument,
) -> Result<(), MpRenderError> {
    patch_scene(
        lowered,
        patch,
        ScenePatchContext {
            document,
            scene: &document.scene,
        },
    )
}

fn patch_scene(
    lowered: &mut MpCompositorBrowserScene,
    patch: &RetainedScenePatch,
    context: ScenePatchContext<'_>,
) -> Result<(), MpRenderError> {
    lowered.primitive_scene.host_rect = lowered.host_rect;

    for transform_patch in &patch.transforms {
        let Some(transform) = lowered
            .primitive_scene
            .transforms
            .get_mut(transform_patch.transform_id)
        else {
            continue;
        };
        *transform = lower_direct_transform(
            context.scene,
            lowered.host_rect,
            transform_patch.source.transform_spatial_id,
        )?;
    }

    for clip_patch in &patch.clip_chains {
        let Some(clip_chain) = lowered
            .primitive_scene
            .clip_chains
            .get_mut(clip_patch.clip_chain_id)
        else {
            continue;
        };
        *clip_chain = lower_clip_chain(
            context.scene,
            clip_patch.source.spatial_id,
            clip_patch.source.clip_chain_id,
            clip_patch.source.origin_spatial_id,
        )?;
    }

    for primitive_patch in &patch.primitives {
        let Some(lowered_primitive) = lowered
            .primitive_scene
            .primitives
            .get_mut(primitive_patch.primitive_id)
        else {
            continue;
        };
        let source_primitive = context
            .scene
            .primitives
            .get(primitive_patch.source_primitive_id.0)
            .ok_or(MpRenderError::UnsupportedPrimitive(primitive_patch.source_primitive_id))?;
        lowered_primitive.local_rect =
            resolve_primitive_rect(context.scene, source_primitive, primitive_patch.origin_spatial_id)?;
    }

    for text_run_patch in &patch.text_runs {
        let Some(text_run) = lowered.text_runs.get_mut(text_run_patch.text_run_id) else {
            continue;
        };
        text_run.local_rect = resolve_text_run_rect(context.scene, &text_run_patch.source)?;
    }

    for picture_patch in &patch.pictures {
        let Some(picture) = lowered.pictures.get_mut(picture_patch.picture_id) else {
            continue;
        };
        picture.local_rect = resolve_picture_rect(context, &picture_patch.source)?;
    }

    for task_patch in &patch.tasks {
        let blur_input_size = match &task_patch.source {
            RetainedTaskSource::Blur { input_task_id } => {
                lowered.tasks.get(*input_task_id).map(|task| task.size)
            }
            RetainedTaskSource::Scene { .. } => None,
        };
        let Some(task) = lowered.tasks.get_mut(task_patch.task_id) else {
            continue;
        };
        match (&mut task.kind, &task_patch.source) {
            (
                MpBrowserTaskKind::Scene(task_scene),
                RetainedTaskSource::Scene { source, patch },
            ) => {
                let nested_context = nested_scene_context(context, source)?;
                let host_rect = resolve_task_scene_host_rect(nested_context, source)?;
                task.size = host_rect.size;
                task_scene.host_rect = host_rect;
                task_scene.primitive_scene.host_rect = host_rect;
                patch_scene(task_scene, patch, nested_context)?;
            }
            (MpBrowserTaskKind::Blur { .. }, RetainedTaskSource::Blur { .. }) => {
                if let Some(size) = blur_input_size {
                    task.size = size;
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn resolve_text_run_rect(
    scene: &MpScene,
    source: &RetainedTextRunSource,
) -> Result<Rect, MpRenderError> {
    match source {
        RetainedTextRunSource::Direct {
            primitive_id,
            origin_spatial_id,
        } => {
            let primitive = scene
                .primitives
                .get(primitive_id.0)
                .ok_or(MpRenderError::UnsupportedPrimitive(*primitive_id))?;
            resolve_primitive_rect(scene, primitive, *origin_spatial_id)
        }
        RetainedTextRunSource::TaskLocal {
            primitive_id,
            origin_spatial_id,
        } => {
            let primitive = scene
                .primitives
                .get(primitive_id.0)
                .ok_or(MpRenderError::UnsupportedPrimitive(*primitive_id))?;
            let bounds = resolve_primitive_rect(scene, primitive, *origin_spatial_id)?;
            Ok(Rect {
                pos: dvec2(0.0, 0.0),
                size: bounds.size,
            })
        }
    }
}

fn resolve_picture_rect(
    context: ScenePatchContext<'_>,
    source: &RetainedPictureSource,
) -> Result<Rect, MpRenderError> {
    match source {
        RetainedPictureSource::TextPicture {
            primitive_id,
            origin_spatial_id,
        } => {
            let primitive = context
                .scene
                .primitives
                .get(primitive_id.0)
                .ok_or(MpRenderError::UnsupportedPrimitive(*primitive_id))?;
            resolve_primitive_rect(context.scene, primitive, *origin_spatial_id)
        }
        RetainedPictureSource::Effect {
            effect_id,
            origin_spatial_id,
            items,
        } => effect_run_bounds(
            context.scene,
            *effect_id,
            &resolve_scene_item_sources(context.scene, items)?,
            *origin_spatial_id,
        ),
        RetainedPictureSource::Embed {
            pipeline_id,
            origin_spatial_id,
        } => {
            let embed = context
                .scene
                .embeds
                .iter()
                .find(|embed| embed.pipeline_id == *pipeline_id)
                .ok_or(MpRenderError::MissingEmbedDocument(*pipeline_id))?;
            resolve_embed_rect(context.scene, embed, *origin_spatial_id)
        }
    }
}

fn nested_scene_context<'a>(
    context: ScenePatchContext<'a>,
    source: &RetainedTaskSceneSource,
) -> Result<ScenePatchContext<'a>, MpRenderError> {
    match source {
        RetainedTaskSceneSource::Embed { pipeline_id } => {
            let child_document = context
                .document
                .child_document(*pipeline_id)
                .ok_or(MpRenderError::MissingEmbedDocument(*pipeline_id))?;
            Ok(ScenePatchContext {
                document: child_document,
                scene: &child_document.scene,
            })
        }
        RetainedTaskSceneSource::TextPicture { .. } | RetainedTaskSceneSource::Effect { .. } => {
            Ok(context)
        }
    }
}

fn resolve_task_scene_host_rect(
    context: ScenePatchContext<'_>,
    source: &RetainedTaskSceneSource,
) -> Result<Rect, MpRenderError> {
    match source {
        RetainedTaskSceneSource::TextPicture {
            primitive_id,
            origin_spatial_id,
        } => {
            let primitive = context
                .scene
                .primitives
                .get(primitive_id.0)
                .ok_or(MpRenderError::UnsupportedPrimitive(*primitive_id))?;
            let bounds = resolve_primitive_rect(context.scene, primitive, *origin_spatial_id)?;
            Ok(Rect {
                pos: dvec2(0.0, 0.0),
                size: bounds.size,
            })
        }
        RetainedTaskSceneSource::Effect {
            effect_id,
            origin_spatial_id,
            items,
        } => effect_run_bounds(
            context.scene,
            *effect_id,
            &resolve_scene_item_sources(context.scene, items)?,
            *origin_spatial_id,
        ),
        RetainedTaskSceneSource::Embed { .. } => Ok(context.scene.root_viewport_rect()),
    }
}

fn resolve_scene_item_sources<'a>(
    scene: &'a MpScene,
    items: &[RetainedSceneItemSource],
) -> Result<Vec<SceneItemRef<'a>>, MpRenderError> {
    items
        .iter()
        .map(|item| match item {
            RetainedSceneItemSource::Primitive(id) => scene
                .primitives
                .get(id.0)
                .map(SceneItemRef::Primitive)
                .ok_or(MpRenderError::UnsupportedPrimitive(*id)),
            RetainedSceneItemSource::Embed(pipeline_id) => scene
                .embeds
                .iter()
                .find(|embed| embed.pipeline_id == *pipeline_id)
                .map(SceneItemRef::Embed)
                .ok_or(MpRenderError::MissingEmbedDocument(*pipeline_id)),
        })
        .collect()
}

