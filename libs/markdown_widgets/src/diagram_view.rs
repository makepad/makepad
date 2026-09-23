use crate::selection_slot::SelectionSlot;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.MarkdownDiagram = #(MarkdownDiagram::register_widget(vm)) {
        draw_selection +: {color: #x3399ff55 pixel: fn() {return vec4(self.color.rgb * self.color.a, self.color.a)}}
        width: Fit height: Fit
        image: Image {width: Fit height: Fit fit: ImageFit.Stretch}
        fallback: Label {width: Fit height: Fit}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct MarkdownDiagram {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    image: Image,
    #[live]
    fallback: Label,
    #[live]
    draw_selection: DrawQuad,
    #[rust]
    selection: SelectionSlot,
    #[rust]
    plain: String,
    #[rust]
    payload: String,
    #[rust]
    dimensions: Option<(f64, f64)>,
}

impl Widget for MarkdownDiagram {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, mut walk: Walk) -> DrawStep {
        let step = if let Some((width, height)) = self.dimensions {
            let ratio = (cx.turtle().inner_rect().size.x.max(1.0) / width).min(1.0);
            walk.width = Size::Fixed(width * ratio);
            walk.height = Size::Fixed(height * ratio);
            self.image.draw_walk(cx, scope, walk)
        } else {
            self.fallback.draw_walk(cx, scope, walk)
        };
        let area = if self.dimensions.is_some() {
            self.image.area()
        } else {
            self.fallback.area()
        };
        self.selection.register(scope, area, &self.plain);
        if self.selection.selected(0, self.plain.len()) {
            self.draw_selection.draw_abs(cx, area.rect(cx));
        }
        step
    }
    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        if self.payload == text {
            return;
        }
        self.payload = text.to_owned();
        self.dimensions = None;
        self.plain.clear();
        self.fallback.set_text(cx, "Invalid diagram payload");
        if text.len() > 33 * 1024 {
            return;
        }
        let Some((language, source)) = text.split_once(':') else {
            return;
        };
        let (Some(_language), Some(source)) = (
            crate::content::unhex(language),
            crate::content::unhex(source),
        ) else {
            return;
        };
        self.plain = source.clone();
        self.fallback.set_text(cx, &source);
        #[cfg(feature = "diagrams")]
        match crate::diagram::render(&_language, &source) {
            Ok(image) => match self.image.load_png_from_data(cx, &image.png, 0) {
                Ok(_) => self.dimensions = Some((image.width as f64, image.height as f64)),
                Err(error) => self
                    .fallback
                    .set_text(cx, &format!("Diagram: {error:?}\n{source}")),
            },
            Err(error) => self
                .fallback
                .set_text(cx, &format!("Diagram: {error}\n{source}")),
        }
    }
}
