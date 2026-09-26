use crate::content::{Images, unhex};
use crate::selection_slot::SelectionSlot;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.MarkdownCode = #(MarkdownCode::register_widget(vm)) {
        width: Fill height: Fit
        draw_selection +: {color: #x3399ff55 pixel: fn() {return vec4(self.color.rgb * self.color.a, self.color.a)}}
        draw_bg +: {color: uniform(#xf3f5f4) pixel: fn() {return self.color}}
        draw_text +: {text_style: theme.font_code color: #x191919}
        code_style: theme.font_code
        prose_style: theme.font_regular
    }
    mod.widgets.MarkdownEmoji = #(MarkdownEmoji::register_widget(vm)) {
        draw_selection +: {color: #x3399ff55 pixel: fn() {return vec4(self.color.rgb * self.color.a, self.color.a)}}
        width: Fit height: Fit
        image: Image { width: Fit height: Fit fit: ImageFit.Stretch }
    }
    mod.widgets.MarkdownImage = #(MarkdownImage::register_widget(vm)) {
        width: Fit height: Fit
        image: Image { width: Fit height: Fit fit: ImageFit.Stretch }
    }
    mod.widgets.MarkdownCell = #(MarkdownCell::register_widget(vm)) {
        width: Fill height: Fit
        html: Html {
            selectable: true
            width: Fill height: Fit padding: 0
            rmath := MarkdownMath {} rimage := mod.widgets.MarkdownImage {} remoji := mod.widgets.MarkdownEmoji {}
        }
    }
}

/// Images borrowed by custom image widgets during one host draw.
/// Restore the previous value after drawing so nested hosts retain their resources.
#[derive(Default)]
pub struct DrawingImages(pub Images);

#[derive(Script, Widget)]
pub struct MarkdownImage {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    image: Image,
    #[rust]
    url: String,
    #[rust]
    payload: String,
    #[rust]
    dimensions: Option<(f64, f64)>,
}
impl ScriptHook for MarkdownImage {
    fn on_after_new_scoped(&mut self, _: &mut ScriptVm, scope: &mut Scope) {
        if let Some(doc) = scope.props.get::<makepad_widgets::makepad_html::HtmlDoc>() {
            let mut walker = doc.new_walker_with_index(scope.index + 1);
            while let Some((name, value)) = walker.while_attr_lc() {
                if name == live_id!(href) && makepad_markdown::render::valid_link(value) {
                    self.url = value.into();
                }
            }
        }
    }
}
impl Widget for MarkdownImage {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _: &mut Scope) {
        if self.url.is_empty() {
            return;
        }
        match event.hits(cx, self.image.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerUp(up) if up.is_over && up.is_primary_hit() && up.was_tap() => {
                cx.widget_action(
                    self.widget_uid(),
                    HtmlLinkAction::Clicked {
                        url: self.url.clone(),
                        key_modifiers: up.modifiers,
                    },
                );
            }
            _ => (),
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, mut walk: Walk) -> DrawStep {
        if let Some((width, height)) = self.dimensions {
            let ratio = (cx.turtle().inner_rect().size.x.max(1.0) / width).min(1.0);
            walk.width = Size::Fixed(width * ratio);
            walk.height = Size::Fixed(height * ratio);
            self.image.draw_walk(cx, scope, walk)
        } else {
            DrawStep::done()
        }
    }
    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        if self.payload == text {
            return;
        }
        self.dimensions = None;
        let fields: Vec<_> = text.split(':').collect();
        if fields.len() != 3 || !cx.has_global::<DrawingImages>() {
            return;
        }
        let image = cx
            .get_global::<DrawingImages>()
            .0
            .values()
            .find(|image| image.id == fields[0])
            .cloned();
        if let (Some(image), Ok(width), Ok(height)) =
            (image, fields[1].parse::<f64>(), fields[2].parse::<f64>())
        {
            if width > 0.0
                && height > 0.0
                && self.image.load_png_from_data(cx, &image.png, 0).is_ok()
            {
                self.dimensions = Some((width, height));
                self.payload = text.into();
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct MarkdownEmoji {
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
impl Widget for MarkdownEmoji {
    fn handle_event(&mut self, _: &mut Cx, _: &Event, _: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, mut walk: Walk) -> DrawStep {
        if let Some((width, height)) = self.dimensions {
            walk.width = Size::Fixed(width);
            walk.height = Size::Fixed(height);
            let step = self.image.draw_walk(cx, scope, walk);
            self.selection
                .register(scope, self.image.area(), &self.plain);
            if self.selection.selected(0, self.plain.len()) {
                self.draw_selection.draw_abs(cx, self.image.area().rect(cx));
            }
            step
        } else {
            DrawStep::done()
        }
    }
    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        if self.payload == text {
            return;
        }
        let Some((size, text)) = text.split_once(':') else {
            return;
        };
        if let (Ok(size), Some(emoji)) = (size.parse::<f32>(), unhex(text)) {
            self.plain = emoji.clone();
            if let Ok(image) = crate::emoji::render(&emoji, size) {
                if self.image.load_png_from_data(cx, &image.png, 0).is_ok() {
                    self.dimensions = Some((image.width as f64, image.height as f64));
                    self.payload = format!("{size}:{text}");
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct MarkdownCode {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_selection: DrawQuad,
    #[rust]
    selection: SelectionSlot,
    #[rust]
    plain: String,
    #[live]
    code_style: TextStyle,
    #[live]
    prose_style: TextStyle,
    #[rust]
    payload: String,
    #[rust]
    tokens: Option<crate::content::Tokens>,
    #[rust]
    size: f32,
}
impl Widget for MarkdownCode {
    fn handle_event(&mut self, _: &mut Cx, _: &Event, _: &mut Scope) {}
    fn set_text(&mut self, _: &mut Cx, text: &str) {
        if self.payload == text {
            return;
        }
        self.payload = text.into();
        let fields: Vec<_> = text.splitn(3, ':').collect();
        if fields.len() != 3 {
            return;
        }
        if let (Ok(size), Some(language), Some(source)) =
            (fields[0].parse(), unhex(fields[1]), unhex(fields[2]))
        {
            self.plain = source.clone();
            self.size = size;
            self.tokens = Some(crate::content::highlight(&language, &source));
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, mut walk: Walk) -> DrawStep {
        use unicode_segmentation::UnicodeSegmentation;
        let Some(tokens) = self.tokens.clone() else {
            return DrawStep::done();
        };
        let width = cx.turtle().inner_rect().size.x.max(25.0);
        let available = width - 24.0;
        let line_height = self.size as f64 * 1.333333 * 1.45;
        let (mut x, mut y) = (0.0, 0.0);
        let mut glyphs = Vec::new();
        let mut offset = 0;
        for (color, text) in tokens.iter() {
            for grapheme in text.graphemes(true) {
                if grapheme == "\n" {
                    offset += grapheme.len();
                    x = 0.0;
                    y += line_height;
                    continue;
                }
                let cjk = !grapheme.is_ascii();
                self.draw_text.text_style = if cjk {
                    self.prose_style.clone()
                } else {
                    self.code_style.clone()
                };
                self.draw_text.text_style.font_size = self.size;
                let layout =
                    self.draw_text
                        .layout(cx, 0.0, 0.0, None, false, Align::default(), grapheme);
                let advance = layout.size_in_lpxs.width as f64;
                if x > 0.0 && x + advance > available {
                    x = 0.0;
                    y += line_height;
                }
                glyphs.push((*color, grapheme.to_owned(), x, y, cjk, offset, advance));
                offset += grapheme.len();
                x += advance;
            }
        }
        walk.width = Size::Fixed(width);
        walk.height = Size::Fixed(y + if x > 0.0 { line_height } else { 0.0 } + 24.0);
        let rect = self.draw_bg.draw_walk(cx, walk);
        self.selection
            .register(scope, self.draw_bg.area(), &self.plain);
        for (color, text, x, y, cjk, offset, advance) in glyphs {
            if self.selection.selected(offset, offset + text.len()) {
                self.draw_selection.draw_abs(
                    cx,
                    Rect {
                        pos: rect.pos + dvec2(x + 12.0, y + 12.0),
                        size: dvec2(advance, line_height),
                    },
                );
            }
            self.draw_text.text_style = if cjk {
                self.prose_style.clone()
            } else {
                self.code_style.clone()
            };
            self.draw_text.text_style.font_size = self.size;
            self.draw_text.color = crate::color(color);
            self.draw_text
                .draw_abs(cx, rect.pos + dvec2(x + 12.0, y + 12.0), &text);
        }
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct MarkdownCell {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    html: WidgetRef,
    #[rust]
    payload: String,
    #[rust]
    align_x: f64,
}
impl Widget for MarkdownCell {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.html.handle_event(cx, event, scope);
    }
    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        if self.payload == text {
            return;
        }
        self.payload = text.into();
        if let Some((align, text)) = text.split_once(':') {
            if let (Ok(align), Some(html)) = (align.parse::<f64>(), unhex(text)) {
                self.align_x = align;
                self.html.set_text(cx, &html);
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let (Some(tf), Some(mut html)) = (
            scope.data.get_mut::<TextFlow>(),
            self.html.borrow_mut::<Html>(),
        ) {
            html.font_size = tf.font_size;
            html.font_color = tf.font_color;
            html.text_style_normal = if tf.bold.value() > 0 {
                tf.text_style_bold.clone()
            } else {
                tf.text_style_normal.clone()
            };
            html.text_style_bold = tf.text_style_bold.clone();
            html.text_style_italic = tf.text_style_italic.clone();
            html.text_style_fixed = tf.text_style_fixed.clone();
        }
        let available = cx.turtle().inner_rect().size.x;
        cx.begin_turtle(
            walk,
            Layout {
                align: Align {
                    x: self.align_x,
                    y: 0.0,
                },
                ..Default::default()
            },
        );
        let inner = Walk {
            width: Size::Fit {
                min: None,
                max: Some(FitBound::Abs(available)),
            },
            height: Size::fit(),
            ..Default::default()
        };
        let _ = self.html.draw_walk(cx, &mut Scope::empty(), inner);
        cx.end_turtle();
        if let Some(tf) = scope.data.get_mut::<TextFlow>() {
            tf.push_widget_text_for_selection(
                self.html.clone(),
                &format!("{}\t", self.html.selection_get_full_text()),
            );
        }
        DrawStep::done()
    }
}
