use crate::selection_slot::SelectionSlot;
use makepad_markdown::math::Formula;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.MarkdownMath = #(MarkdownMath::register_widget(vm)) {
        draw_selection +: {color: #x3399ff55 pixel: fn() {return vec4(self.color.rgb * self.color.a, self.color.a)}}
        width: Fit height: Fit
        image: Image {width: Fit height: Fit fit: ImageFit.Stretch}
        fallback: Label {width: Fit height: Fit}
    }
}

#[cfg(not(feature = "math"))]
pub fn formula_html(formula: &Formula, _: f32, _: u32) -> String {
    makepad_markdown::math::fallback(formula)
}

#[cfg(feature = "math")]
pub fn formula_html(formula: &Formula, size: f32, ink: u32) -> String {
    // The native payload and typesetter are bounded. Preserve readable TeX
    // when a formula cannot enter the widget, including with math disabled.
    if formula.tex.len() > 2048 {
        return makepad_markdown::math::fallback(formula);
    }
    let tex = crate::content::hex(&formula.tex);
    let payload =
        makepad_markdown::escape(&format!("{}:{size}:{ink}:{tex}", u8::from(formula.display)));
    let widget = format!("<rmath>{payload}</rmath>");
    if formula.display {
        format!("<p>{widget}</p>")
    } else {
        widget
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct MarkdownMath {
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

impl Widget for MarkdownMath {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, mut walk: Walk) -> DrawStep {
        let step = if let Some((width, height)) = self.dimensions {
            let available = cx.turtle().inner_rect().size.x.max(1.0);
            let ratio = (available / width).min(1.0);
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
        let fields: Vec<_> = text.splitn(4, ':').collect();
        if fields.len() != 4 || fields[3].len() > 4096 {
            return;
        }
        let bytes: Result<Vec<_>, _> = fields[3]
            .as_bytes()
            .chunks(2)
            .map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
                    .ok_or(())
            })
            .collect();
        let Some(tex) = bytes.ok().and_then(|b| String::from_utf8(b).ok()) else {
            return;
        };
        self.plain = tex.clone();
        self.fallback.set_text(cx, &tex);
        #[cfg(feature = "math")]
        let (Ok(size), Ok(ink)) = (fields[1].parse(), fields[2].parse()) else {
            return;
        };
        #[cfg(feature = "math")]
        let formula = Formula {
            tex,
            display: fields[0] == "1",
        };
        #[cfg(feature = "math")]
        if let Ok(image) = crate::math::render(&formula, size, ink) {
            if self.image.load_png_from_data(cx, &image.png, 0).is_ok() {
                self.dimensions = Some((image.width as f64, image.height as f64));
            }
        }
    }
}
