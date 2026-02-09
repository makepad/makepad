use crate::*;
use makepad_latex_math::{self as latex_math, LayoutGlyph, LayoutItem, LayoutRule, MathStyle};
use ttf_parser::{Face, GlyphId, OutlineBuilder};

const MATH_FONT_DATA: &[u8] = include_bytes!("../resources/NewCMMath-Regular.otf");

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw

    mod.widgets.MathViewBase = #(MathView::register_widget(vm))

    mod.widgets.MathView = set_type_default() do mod.widgets.MathViewBase{
        width: Fit
        height: Fit
        color: #fff
        font_size: 11.0
        baseline_offset: -2.0
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct MathView {
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_vector: DrawVector,
    #[live]
    text: String,
    #[live]
    color: Vec4,
    #[live(11.0)]
    font_size: f64,
    #[live(-2.0)]
    baseline_offset: f64,
    #[rust]
    old_text: String,
    #[rust]
    old_color: Vec4,
    #[rust]
    old_font_size: f64,
    #[rust]
    layout_cache: Option<latex_math::LayoutOutput>,
}

impl Widget for MathView {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, mut walk: Walk) -> DrawStep {
        self.compile_math();

        if let Some(layout) = &self.layout_cache {
            let w = layout.width;
            let h = layout.height;

            walk.width = Size::Fixed(w as f64);
            walk.height = Size::Fixed(h as f64);
            walk.margin.top += self.baseline_offset;

            let layout = layout.clone();
            let color = self.color;

            self.draw_vector.draw_walk(cx, walk, |dv, ox, oy| {
                render_layout(dv, &layout, ox, oy, color);
            });
        }
        DrawStep::done()
    }

    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        self.text = text.to_string();
        self.redraw(cx);
    }
}

impl MathView {
    fn compile_math(&mut self) {
        if self.text == self.old_text
            && self.color == self.old_color
            && self.font_size == self.old_font_size
        {
            return;
        }
        if self.text.is_empty() {
            self.layout_cache = None;
            return;
        }

        self.old_text = self.text.clone();
        self.old_color = self.color;
        self.old_font_size = self.font_size;

        let nodes = latex_math::parse(&self.text);
        let layout_size = self.font_size as f32 * 1.75;
        self.layout_cache =
            latex_math::layout(&nodes, MATH_FONT_DATA, layout_size, MathStyle::Display);
    }
}

fn render_layout(
    dv: &mut DrawVector,
    layout: &latex_math::LayoutOutput,
    ox: f32,
    oy: f32,
    color: Vec4,
) {
    let scale = 1.0f32;
    let base_y = oy + layout.ascent;

    dv.set_color(color.x, color.y, color.z, color.w);

    let face = Face::parse(MATH_FONT_DATA, 0).unwrap();
    let upem = face.units_per_em() as f32;

    for item in &layout.items {
        match item {
            LayoutItem::Glyph(glyph) => {
                render_glyph(dv, &face, upem, glyph, ox, base_y, scale);
            }
            LayoutItem::Rule(rule) => {
                render_rule(dv, rule, ox, base_y, scale);
            }
            LayoutItem::Rect(rect) => {
                let rx = ox + rect.x * scale;
                let ry = base_y + rect.y * scale;
                let rw = rect.width * scale;
                let rh = rect.height * scale;
                dv.rect(rx, ry, rw, rh);
                dv.fill();
            }
        }
    }
}

fn render_glyph(
    dv: &mut DrawVector,
    face: &Face,
    upem: f32,
    glyph: &LayoutGlyph,
    ox: f32,
    base_y: f32,
    scale: f32,
) {
    let glyph_x = ox + glyph.x * scale;
    let glyph_y = base_y + glyph.y * scale;
    let font_scale = glyph.size / upem * scale;

    let fill_aa = (font_scale * 24.0).clamp(0.2, 1.0);

    let id = GlyphId(glyph.glyph_id);
    let mut builder = VectorOutlineBuilder {
        dv,
        x: glyph_x,
        y: glyph_y,
        scale: font_scale,
    };
    let _ = face.outline_glyph(id, &mut builder);
    dv.fill_opts(crate::makepad_draw::vector::LineJoin::Miter, 4.0, fill_aa);
}

fn render_rule(dv: &mut DrawVector, rule: &LayoutRule, ox: f32, base_y: f32, scale: f32) {
    let rx = ox + rule.x * scale;
    let ry = base_y + rule.y * scale;
    let rw = rule.width * scale;
    let rh = rule.height * scale;
    dv.rect(rx, ry, rw, rh);
    dv.fill();
}

struct VectorOutlineBuilder<'a> {
    dv: &'a mut DrawVector,
    x: f32,
    y: f32,
    scale: f32,
}

impl OutlineBuilder for VectorOutlineBuilder<'_> {
    fn move_to(&mut self, px: f32, py: f32) {
        self.dv
            .move_to(self.x + px * self.scale, self.y - py * self.scale);
    }
    fn line_to(&mut self, px: f32, py: f32) {
        self.dv
            .line_to(self.x + px * self.scale, self.y - py * self.scale);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, px: f32, py: f32) {
        self.dv.quad_to(
            self.x + cx * self.scale,
            self.y - cy * self.scale,
            self.x + px * self.scale,
            self.y - py * self.scale,
        );
    }
    fn curve_to(&mut self, cx1: f32, cy1: f32, cx2: f32, cy2: f32, px: f32, py: f32) {
        self.dv.bezier_to(
            self.x + cx1 * self.scale,
            self.y - cy1 * self.scale,
            self.x + cx2 * self.scale,
            self.y - cy2 * self.scale,
            self.x + px * self.scale,
            self.y - py * self.scale,
        );
    }
    fn close(&mut self) {
        self.dv.close();
    }
}
