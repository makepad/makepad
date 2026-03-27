use crate::{MpBrowserRenderer, MpDocument, MpRenderError, MpRendererStats, MpResourceStore};
use makepad_widgets::{Cx2d, Rect};

#[derive(Clone)]
pub struct MpExampleDocument {
    pub document: MpDocument,
    pub resources: MpResourceStore,
}

impl MpExampleDocument {
    pub fn new(document: MpDocument, resources: MpResourceStore) -> Self {
        Self {
            document,
            resources,
        }
    }

    pub fn draw(
        &self,
        renderer: &mut MpBrowserRenderer,
        cx: &mut Cx2d,
        viewport: Rect,
    ) -> Result<MpRendererStats, MpRenderError> {
        renderer.register_resource_store(&self.resources);
        renderer.draw_document(cx, &self.document, viewport)
    }
}
