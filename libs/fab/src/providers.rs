//! Trait surfaces between the reusable shell and loaded documents.

use crate::model::{
    Element, ElementId, Property, Quantity, Ray, RayHit, RenderBatch, Scene, SceneSnapshot,
    SceneState,
};
use std::sync::Arc;

/// A loader-neutral grouping axis exposed by a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentGroupKind {
    /// Floors, levels, assemblies, or another ordered spatial hierarchy.
    Spatial,
    /// Authoring or presentation layers.
    Layer,
    /// Any loader-defined collection that is neither spatial nor a layer.
    Collection,
}

/// A borrowed group descriptor. IDs are stable only within one loaded
/// document; consumers use the member element IDs for selection and filtering.
#[derive(Clone, Copy, Debug)]
pub struct DocumentGroup<'a> {
    pub id: u64,
    pub kind: DocumentGroupKind,
    pub name: &'a str,
    pub visible: bool,
    pub members: &'a [ElementId],
}

/// Hierarchy, properties, grouping, visibility and selection data consumed by
/// the outliner and properties editors.
pub trait DocumentProvider: Send + Sync {
    fn name(&self) -> &str;
    fn elements(&self) -> &[Element];
    fn group_count(&self) -> usize;
    fn group(&self, index: usize) -> Option<DocumentGroup<'_>>;
    fn properties(&self, element: ElementId) -> Option<(&[Property], &[Quantity])>;
    fn is_visible(&self, state: &SceneState, element: ElementId) -> bool;
}

/// Geometry, immutable ray-tracing snapshot and picking seam used by both
/// viewport backends.
pub trait SceneProvider: DocumentProvider {
    fn batches(&self) -> &[RenderBatch];
    fn snapshot(&self) -> Arc<SceneSnapshot>;
    fn pick(&self, ray: &Ray, state: &SceneState) -> Option<RayHit>;
}

impl DocumentProvider for Scene {
    fn name(&self) -> &str {
        &self.name
    }

    fn elements(&self) -> &[Element] {
        &self.elements
    }

    fn group_count(&self) -> usize {
        self.stories.len() + self.layers.len()
    }

    fn group(&self, index: usize) -> Option<DocumentGroup<'_>> {
        if let Some(story) = self.stories.get(index) {
            return Some(DocumentGroup {
                id: story.id.0 as u64,
                kind: DocumentGroupKind::Spatial,
                name: &story.name,
                visible: true,
                members: &story.elements,
            });
        }
        let layer = self.layers.get(index.checked_sub(self.stories.len())?)?;
        Some(DocumentGroup {
            id: (1_u64 << 32) | layer.id.0 as u64,
            kind: DocumentGroupKind::Layer,
            name: &layer.name,
            visible: layer.visible,
            members: &layer.elements,
        })
    }

    fn properties(&self, element: ElementId) -> Option<(&[Property], &[Quantity])> {
        let element = self.element(element)?;
        Some((&element.properties, &element.quantities))
    }

    fn is_visible(&self, state: &SceneState, element: ElementId) -> bool {
        state.is_visible(self, element)
    }
}

impl SceneProvider for Scene {
    fn batches(&self) -> &[RenderBatch] {
        &self.batches
    }

    fn snapshot(&self) -> Arc<SceneSnapshot> {
        Scene::snapshot(self)
    }

    fn pick(&self, ray: &Ray, state: &SceneState) -> Option<RayHit> {
        Scene::pick(self, ray, state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_groups_hide_storage_specific_ids() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        assert_eq!(scene.group_count(), scene.stories.len() + scene.layers.len());
        assert_eq!(scene.group(0).unwrap().kind, DocumentGroupKind::Spatial);
        assert_eq!(
            scene.group(scene.stories.len()).unwrap().kind,
            DocumentGroupKind::Layer
        );
    }
}
