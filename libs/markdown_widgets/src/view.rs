use crate::content::{native_html, Images, NativeRenderer};
use crate::content_view::DrawingImages;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.MarkdownView = #(MarkdownView::register_widget(vm)) {
        width: Fill height: Fit
        font_size: theme.font_size_p
        font_color: theme.color_label_inner
        html: Html {
            width: Fill height: Fit
            selectable: true
            rcode := MarkdownCode {}
            rmath := MarkdownMath {}
            rimage := MarkdownImage {}
            remoji := MarkdownEmoji {}
            rdiagram := MarkdownDiagram {}
            rcell := MarkdownCell {}
        }
    }
}

/// Native Markdown with host-supplied resources and ordinary `HtmlLinkAction`
/// navigation. Source is retained exactly; rendering never changes the document.
/// Place this widget inside a scroll view when displaying a long document.
#[derive(Script, Widget)]
pub struct MarkdownView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[find]
    #[live]
    html: WidgetRef,
    #[live]
    text: String,
    #[live(14.0)]
    font_size: f32,
    #[live]
    font_color: Vec4f,
    #[rust]
    images: Images,
    #[rust]
    dirty: bool,
}

impl ScriptHook for MarkdownView {
    fn on_after_apply(&mut self, vm: &mut ScriptVm, _: &Apply, _: &mut Scope, _: ScriptValue) {
        self.dirty = true;
        // Html owns dynamic text-flow children. Keep its own UID in the tree
        // so changes raised by that flow reach selection and inspection APIs.
        vm.cx_mut()
            .widget_tree_insert_child_deep(self.uid, id!(html), self.html.clone());
    }
}

impl MarkdownView {
    /// Replace the immutable resource snapshot. Keys are the image URLs in the
    /// source. Loading/network policy and update scheduling belong to the host.
    pub fn set_images(&mut self, cx: &mut Cx, images: Images) {
        if !std::sync::Arc::ptr_eq(&self.images, &images) {
            self.images = images;
            self.dirty = true;
            self.redraw(cx);
        }
    }

    pub fn set_style(&mut self, cx: &mut Cx, font_size: f32, font_color: Vec4f) {
        if self.font_size != font_size || self.font_color != font_color {
            self.font_size = font_size;
            self.font_color = font_color;
            self.dirty = true;
            self.redraw(cx);
        }
    }

    fn refresh(&mut self, cx: &mut Cx) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let size = if self.font_size.is_finite() {
            self.font_size.clamp(8.0, 48.0)
        } else {
            14.0
        };
        let channel = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
        let ink = channel(self.font_color.x) << 16
            | channel(self.font_color.y) << 8
            | channel(self.font_color.z);
        let mut renderer = NativeRenderer {
            images: &self.images,
            size,
            ink,
        };
        let blocks = makepad_markdown::markdown_render::render(&self.text, &mut renderer);
        let html = blocks
            .iter()
            .map(|block| native_html(&block.html))
            .collect::<String>();
        if let Some(mut body) = self.html.borrow_mut::<Html>() {
            body.font_size = size;
            body.font_color = self.font_color;
            body.set_text(cx, &html);
        }
    }
}

impl Widget for MarkdownView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.html.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.refresh(cx);
        let previous = std::mem::replace(&mut cx.global::<DrawingImages>().0, self.images.clone());
        let step = self.html.draw_walk(cx, scope, walk);
        cx.global::<DrawingImages>().0 = previous;
        step
    }

    fn text(&self) -> String {
        self.text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        if self.text != text {
            self.text = text.to_owned();
            self.dirty = true;
            self.redraw(cx);
        }
    }
}

impl MarkdownViewRef {
    pub fn set_images(&self, cx: &mut Cx, images: Images) {
        if let Some(mut view) = self.borrow_mut() {
            view.set_images(cx, images);
        }
    }

    pub fn set_style(&self, cx: &mut Cx, font_size: f32, font_color: Vec4f) {
        if let Some(mut view) = self.borrow_mut() {
            view.set_style(cx, font_size, font_color);
        }
    }
}
