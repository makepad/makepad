use crate::*;
use makepad_draw::text::{
    font::Font,
    font_family::FontFamilyId,
    geom::Point as TextPoint,
    glyph_outline::Command as OutlineCommand,
    rasterizer::RasterizedGlyph,
};
use makepad_latex_math::{self as latex_math, LayoutGlyph, LayoutItem, LayoutRule, MathStyle};
use std::rc::Rc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.text.*
    
    mod.widgets.MathViewBase = #(MathView::register_widget(vm))

    mod.widgets.MathView = set_type_default() do mod.widgets.MathViewBase{
        width: Fit
        height: Fit
        color: #fff
        font_size: 11.0
        baseline_offset: -2.0
        sdf_fallback_font_size: 22.0
        draw_text +: {
            text_style: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("self:resources/NewCMMath-Regular.otf") asc: 0.0 desc: 0.0}
                }
                font_size: 11.0
                line_spacing: 1.2
            }
        }
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
    #[redraw]
    #[live]
    draw_text: DrawText,
    #[live]
    text: String,
    #[live]
    color: Vec4,
    #[live(11.0)]
    font_size: f64,
    #[live(-2.0)]
    baseline_offset: f64,
    #[live(12.0)]
    sdf_fallback_font_size: f64,
    #[rust]
    old_text: String,
    #[rust]
    old_font_size: f64,
    #[rust]
    old_font_family_id: Option<FontFamilyId>,
    #[rust]
    layout_font: Option<Rc<Font>>,
    #[rust]
    layout_cache: Option<latex_math::LayoutOutput>,
}

impl Widget for MathView {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, mut walk: Walk) -> DrawStep {
        self.compile_math(cx);

        let (layout, font) = match (&self.layout_cache, &self.layout_font) {
            (Some(layout), Some(font)) => (layout.clone(), font.clone()),
            _ => return DrawStep::done(),
        };

        let w = layout.width;
        let h = layout.height;
        walk.width = Size::Fixed(w as f64);
        walk.height = Size::Fixed(h as f64);
        walk.margin.top += self.baseline_offset;

        let rect = cx.walk_turtle(walk);
        let ox = rect.pos.x as f32;
        let oy = rect.pos.y as f32;
        let base_y = oy + layout.ascent;
        let scale = 1.0f32;
        let color = self.color;
        let sdf_threshold = self.sdf_fallback_font_size as f32;

        self.draw_vector.begin();
        self.draw_vector
            .set_color(color.x, color.y, color.z, color.w);

        let mut sdf_glyphs: Vec<(TextPoint<f32>, f32, RasterizedGlyph)> = Vec::new();

        for item in &layout.items {
            match item {
                LayoutItem::Glyph(glyph) => {
                    let glyph_font_size = glyph.size * scale;
                    if glyph_font_size <= sdf_threshold {
                        let dpxs_per_em = glyph_font_size * cx.current_dpi_factor() as f32;
                        if let Some(rasterized) = font.rasterize_glyph(glyph.glyph_id, dpxs_per_em)
                        {
                            let origin =
                                TextPoint::new(ox + glyph.x * scale, base_y + glyph.y * scale);
                            sdf_glyphs.push((origin, glyph_font_size, rasterized));
                        } else {
                            render_glyph_vector(
                                &mut self.draw_vector,
                                &font,
                                glyph,
                                ox,
                                base_y,
                                scale,
                            );
                        }
                    } else {
                        render_glyph_vector(&mut self.draw_vector, &font, glyph, ox, base_y, scale);
                    }
                }
                LayoutItem::Rule(rule) => {
                    render_rule(&mut self.draw_vector, rule, ox, base_y, scale);
                }
                LayoutItem::Rect(rect) => {
                    let rx = ox + rect.x * scale;
                    let ry = base_y + rect.y * scale;
                    let rw = rect.width * scale;
                    let rh = rect.height * scale;
                    self.draw_vector.rect(rx, ry, rw, rh);
                    self.draw_vector.fill();
                }
            }
        }

        self.draw_vector.end(cx);

        if !sdf_glyphs.is_empty() {
            self.draw_text.draw_rasterized_glyphs_abs(
                cx,
                &sdf_glyphs,
                vec4(color.x, color.y, color.z, color.w),
            );
        }

        DrawStep::done()
    }

    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        self.text = text.to_string();
        self.redraw(cx);
    }
}

impl MathView {
    fn compile_math(&mut self, cx: &mut Cx2d) {
        let font_family_id = self.draw_text.text_style.font_family_id();
        if self.text == self.old_text
            && self.font_size == self.old_font_size
            && self.old_font_family_id == Some(font_family_id)
        {
            return;
        }

        self.old_text = self.text.clone();
        self.old_font_size = self.font_size;
        self.old_font_family_id = Some(font_family_id);

        if self.text.is_empty() {
            self.layout_cache = None;
            self.layout_font = None;
            return;
        }

        let layout_font = {
            let mut fonts = cx.fonts.borrow_mut();
            let family = fonts.get_or_load_font_family(font_family_id);
            family.fonts().first().cloned()
        };

        let Some(layout_font) = layout_font else {
            self.layout_cache = None;
            self.layout_font = None;
            return;
        };

        let nodes = latex_math::parse(&self.text);
        let layout_size = self.font_size as f32 * 1.75;
        self.layout_cache = latex_math::layout(
            &nodes,
            layout_font.data().as_slice(),
            layout_size,
            MathStyle::Display,
        );
        self.layout_font = Some(layout_font);
    }
}

fn render_glyph_vector(
    dv: &mut DrawVector,
    font: &Font,
    glyph: &LayoutGlyph,
    ox: f32,
    base_y: f32,
    scale: f32,
) {
    let glyph_x = ox + glyph.x * scale;
    let glyph_y = base_y + glyph.y * scale;
    let font_scale = glyph.size / font.units_per_em() * scale;
    let fill_aa = (font_scale * 24.0).clamp(0.2, 1.0);

    let Some(outline) = font.glyph_outline(glyph.glyph_id) else {
        return;
    };

    for command in outline.commands() {
        match command {
            OutlineCommand::MoveTo(p) => {
                dv.move_to(glyph_x + p.x * font_scale, glyph_y - p.y * font_scale);
            }
            OutlineCommand::LineTo(p) => {
                dv.line_to(glyph_x + p.x * font_scale, glyph_y - p.y * font_scale);
            }
            OutlineCommand::QuadTo(c, p) => {
                dv.quad_to(
                    glyph_x + c.x * font_scale,
                    glyph_y - c.y * font_scale,
                    glyph_x + p.x * font_scale,
                    glyph_y - p.y * font_scale,
                );
            }
            OutlineCommand::CurveTo(c1, c2, p) => {
                dv.bezier_to(
                    glyph_x + c1.x * font_scale,
                    glyph_y - c1.y * font_scale,
                    glyph_x + c2.x * font_scale,
                    glyph_y - c2.y * font_scale,
                    glyph_x + p.x * font_scale,
                    glyph_y - p.y * font_scale,
                );
            }
            OutlineCommand::Close => {
                dv.close();
            }
        }
    }

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
