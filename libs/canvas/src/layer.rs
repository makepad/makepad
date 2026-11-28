use crate::Camera;
use makepad_widgets_core::*;

/// A retained draw list for content laid out in camera-local units.
pub struct CanvasLayer {
    list: DrawList2d,
}

impl CanvasLayer {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            list: DrawList2d::new(cx),
        }
    }

    /// Begin drawing inside the visible intersection in window coordinates.
    pub fn begin<'a>(
        &'a mut self,
        cx: &mut Cx2d,
        camera: &Camera,
        screen_clip: Rect,
    ) -> CanvasLayerScope<'a> {
        self.list.begin_always(cx);
        // The align pass clips before the GPU applies the view transform.
        // Start a fresh clip context and give it the visible rect in local units.
        let side = camera.origin() * 2.0;
        cx.begin_unclipped_root_turtle(dvec2(side, side), Layout::flow_overlay());
        cx.push_clip_rect(Rect {
            pos: camera.screen_to_local(screen_clip.pos),
            size: screen_clip.size / camera.scale,
        });
        CanvasLayerScope {
            list: &mut self.list,
            transform: camera.matrix(),
        }
    }
}

#[must_use = "end the layer after drawing its contents"]
pub struct CanvasLayerScope<'a> {
    list: &'a mut DrawList2d,
    transform: Mat4f,
}

impl CanvasLayerScope<'_> {
    pub fn end(self, cx: &mut Cx2d) {
        cx.pop_clip_rect();
        cx.end_pass_sized_turtle_no_clip();
        self.list.end(cx);
        // Descendant draw lists inherit this transform once they exist.
        self.list.set_view_transform(cx, &self.transform);
    }
}
