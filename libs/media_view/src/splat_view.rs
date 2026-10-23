//! A byte-backed Gaussian-splat viewer with the XR desktop orbit controls.

use crate::{media_kind, MediaFit, MediaKind, MediaViewAction};
use makepad_widgets::*;
use makepad_xr::obj::ViewSplat;
use makepad_xr::scene::XrSceneView;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SplatViewBase = #(SplatView::register_widget(vm))
    mod.widgets.SplatView = set_type_default() do mod.widgets.SplatViewBase{
        width: Fill
        height: Fill
        scene := XrSceneView{
            width: Fill
            height: Fill
            camera.distance: 6.0
            camera.distance_min: 0.03
            splat := ViewSplat{
                scale: vec3(1.0, 1.0, 1.0)
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct SplatView {
    #[deref]
    view: View,
    #[rust]
    fit: MediaFit,
    #[rust]
    loaded: bool,
}

impl SplatView {
    /// Load a PLY/splat payload. The bytes go to `ViewSplat` through its
    /// typed setter: this widget may live in a host isolate, where a script
    /// apply from the main VM is not allowed.
    pub fn load_bytes(
        &mut self,
        cx: &mut Cx,
        bytes: &[u8],
        content_type: &str,
    ) -> Result<(), String> {
        if media_kind(content_type, bytes) != MediaKind::Splat {
            let error = format!("unsupported splat content type: {content_type}");
            cx.widget_action(self.widget_uid(), MediaViewAction::Failed(error.clone()));
            return Err(error);
        }
        if bytes.is_empty() {
            let error = "empty splat payload".to_string();
            cx.widget_action(self.widget_uid(), MediaViewAction::Failed(error.clone()));
            return Err(error);
        }
        let splat_ref = self.view.widget(cx, ids!(splat));
        let Some(mut splat) = splat_ref.borrow_mut::<ViewSplat>() else {
            let error = "splat scene widget is missing".to_string();
            cx.widget_action(self.widget_uid(), MediaViewAction::Failed(error.clone()));
            return Err(error);
        };
        splat.set_scene_bytes(bytes.to_vec());
        splat.set_scale(vec3f(1.0, 1.0, 1.0));
        drop(splat);
        self.loaded = true;
        self.view.view(cx, ids!(scene)).set_visible(cx, true);
        self.apply_fit(cx);
        cx.widget_action(self.widget_uid(), MediaViewAction::Loaded(MediaKind::Splat));
        self.view.redraw(cx);
        Ok(())
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.loaded = false;
        self.view.view(cx, ids!(scene)).set_visible(cx, false);
        self.view.redraw(cx);
    }

    pub fn set_fit(&mut self, cx: &mut Cx, fit: MediaFit) {
        self.fit = fit;
        self.apply_fit(cx);
    }

    pub fn set_size(&mut self, cx: &mut Cx, width: Size, height: Size) {
        self.view.walk.width = width;
        self.view.walk.height = height;
        self.view.redraw(cx);
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    fn apply_fit(&mut self, cx: &mut Cx) {
        if let Some(mut scene) = self
            .view
            .widget(cx, ids!(scene))
            .borrow_mut::<XrSceneView>()
        {
            let camera = scene.camera_mut();
            camera.distance = match self.fit {
                MediaFit::Contain => 6.0,
                MediaFit::Cover => 4.2,
                MediaFit::Stretch => 5.0,
            };
        }
        self.view.redraw(cx);
    }
}

impl Widget for SplatView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}
