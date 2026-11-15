use crate::*;
use crate::math_outline::{Outline, Slab};
use makepad_draw::{
    glyph_capture::{OutlineGlyph, PicturedText},
    shader::draw_glyph::GlyphShapeId,
    text::{font::Font, font_family::FontFamilyId},
};
use makepad_latex_math::{self as latex_math, Joint, LayoutGlyph, LayoutItem, LayoutRule, MathStyle};
use std::{collections::HashSet, rc::Rc};

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
        source_style: theme.font_regular
        draw_glyph +: {
            aa_pad_px: 1.0
        }
        draw_text +: {
            text_style: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{res: crate_resource("makepad_widgets:resources/NewCMMath-Regular.otf") asc: 0.0 desc: 0.0}
                }
                font_size: 11.0
                line_spacing: 1.2
            }
        }
    }
}

#[derive(Clone, Debug)]
struct MathComponent {
    shape_id: GlyphShapeId,
    origin: Vec2f,
    size: Vec2f,
    /// The glyphs it fills, for a document writer: one, the pieces of an
    /// assembled glyph, or none for rules and boxes.
    glyphs: Vec<ComponentGlyph>,
}

/// A glyph component as a document writer records it, from the formula's top left.
#[derive(Clone, Debug)]
struct ComponentGlyph {
    id: u16,
    x: f32,
    baseline: f32,
    size: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct MathView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_glyph: DrawGlyph,
    #[live]
    draw_text: DrawText,
    #[live]
    text: String,
    #[live]
    color: Vec4,
    #[live(11.0)]
    font_size: f64,
    /// Lay out in TeX's text style: smaller fractions and limits beside the
    /// operator for inline math. Off uses display style.
    #[live(false)]
    inline: bool,
    #[live(-2.0)]
    baseline_offset: f64,
    /// The face a document writer sets the formula's source in, invisible over the
    /// formula, for a reader to copy and search; it takes the size of the text around
    /// the formula.
    #[live]
    source_style: TextStyle,
    #[rust]
    old_text: String,
    #[rust]
    old_font_size: f64,
    #[rust]
    old_inline: bool,
    /// The scale of the text around the formula against the flow's base
    /// size (a heading's), set by the host; `font_size` is at scale 1.
    #[rust(1.0)]
    text_scale: f64,
    #[rust]
    old_text_scale: f64,
    #[rust]
    old_font_family_id: Option<FontFamilyId>,
    #[rust]
    layout_cache: Option<latex_math::LayoutOutput>,
    #[rust]
    components: Vec<MathComponent>,
    /// The drawing (`Cx::drawing_id`) the view last drew in. A view that draws
    /// another formula in the same drawing (a host that lays out several rows
    /// with one item each) adds that formula's shapes after the first's, which
    /// its earlier draws still read; the next drawing starts the shapes over.
    #[rust]
    drawn: Option<u64>,
    /// The shapes hold other formulas' too, drawn earlier in `drawn`.
    #[rust]
    shared_shapes: bool,
    /// The face the formula is set in, for a document writer.
    #[rust]
    layout_font: Option<Rc<Font>>,
    /// The rect the view took: where a press or a host finds the formula.
    #[rust]
    #[area]
    area: Area,
}

impl Widget for MathView {
    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        let _ = cx;
        node.role = a11y_tree::Role::Image;
        makepad_platform::a11y::set_string(&mut node.name, &self.text());
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, mut walk: Walk) -> DrawStep {
        self.compile_math(cx);

        let Some(layout) = self.layout_cache.as_ref() else {
            self.draw_text
                .draw_walk(cx, walk, Align::default(), &self.text);
            return DrawStep::done();
        };

        let w = layout.width;
        let h = layout.height;
        // A Fill width centres the formula in it (display math); otherwise
        // the view is as wide as the formula.
        let centred = walk.width.is_fill();
        if !centred {
            walk.width = Size::Fixed(w as f64);
        }
        walk.height = Size::Fixed(h as f64);
        walk.margin.top += self.baseline_offset;

        let mut bounds = cx.walk_turtle_with_area(&mut self.area, walk);
        self.drawn = Some(cx.drawing_id());
        if centred {
            bounds.pos.x += ((bounds.size.x - w as f64) / 2.0).max(0.0);
        }
        let color = self.color;
        // While a document writer captures text, a glyph whose face it embeds is drawn
        // by the writer as a shape instead, under the formula's source (without the lines
        // around it) set invisible on its baseline; rules and boxes stay in the picture.
        let mut capture = cx.glyph_capture.take();
        let row = match (&mut capture, &self.layout_font) {
            (Some(capture), Some(layout_font)) if capture.records() => {
                let source_font = self.source_font(cx);
                capture.begin_outline_row(cx, rect(bounds.pos.x, bounds.pos.y, w as f64, h as f64), PicturedText {
                    text: self.text.trim(),
                    font: source_font.as_ref().unwrap_or(layout_font),
                    size: self.text_size(),
                    x: bounds.pos.x as f32,
                    baseline: (bounds.pos.y + layout.ascent as f64) as f32,
                    width: w,
                })
            }
            _ => None,
        };

        for component in &self.components {
            if let (Some(capture), Some(font)) = (&mut capture, &self.layout_font) {
                // A face is set as text for all its glyphs or none.
                let mut written = !component.glyphs.is_empty();
                for glyph in &component.glyphs {
                    written &= capture.outline_glyph(row, font, OutlineGlyph {
                        id: glyph.id,
                        x: (bounds.pos.x + glyph.x as f64) as f32,
                        baseline: (bounds.pos.y + glyph.baseline as f64) as f32,
                        size: glyph.size,
                        color: [color.x, color.y, color.z, color.w],
                    });
                }
                if written {
                    continue;
                }
            }
            let mut layers = {
                let Some(shape) = self.draw_glyph.shape(component.shape_id) else {
                    continue;
                };
                shape.layers.clone()
            };
            for layer in &mut layers {
                layer.color = vec4(color.x, color.y, color.z, color.w * layer.color.w);
            }
            self.draw_glyph.draw_layers_abs(
                cx,
                rect(
                    bounds.pos.x + component.origin.x as f64,
                    bounds.pos.y + component.origin.y as f64,
                    component.size.x as f64,
                    component.size.y as f64,
                ),
                &layers,
            );
        }
        cx.glyph_capture = capture;

        DrawStep::done()
    }

    /// Redraws only for a new formula: a host sets the text on every draw,
    /// and a redraw asked while drawing records again in the next frame.
    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        if self.text != text {
            self.text = text.to_string();
            self.redraw(cx);
        }
    }
}

impl MathView {
    /// Sizes the formula for text `scale` times the base size (a heading's
    /// inline math); 1 draws it at `font_size`.
    pub fn set_text_scale(&mut self, cx: &mut Cx, scale: f64) {
        if self.text_scale != scale {
            self.text_scale = scale;
            self.redraw(cx);
        }
    }

    /// The em size the formula is laid out at, in logical units.
    fn layout_size(&self) -> f32 {
        (self.font_size * self.text_scale) as f32 * 1.75
    }

    /// The em size of the text around the formula, in logical units: a font size is in
    /// points, 72 to the 96 logical pixels of an inch.
    fn text_size(&self) -> f32 {
        (self.font_size * self.text_scale) as f32 * 96.0 / 72.0
    }

    /// The face of `source_style`, or `None` when it has none.
    fn source_font(&self, cx: &mut Cx2d) -> Option<Rc<Font>> {
        self.source_style.ensure_fonts_loaded(cx.cx.cx);
        let mut fonts = cx.fonts.borrow_mut();
        let family = fonts.get_or_load_font_family(self.source_style.font_family_id());
        family.fonts().first().cloned()
    }

    fn compile_math(&mut self, cx: &mut Cx2d) {
        let font_family_id = self.draw_text.text_style.font_family_id();
        self.draw_text.text_style.ensure_fonts_loaded(cx.cx.cx);
        if self.text == self.old_text
            && self.font_size == self.old_font_size
            && self.inline == self.old_inline
            && self.text_scale == self.old_text_scale
            && self.old_font_family_id == Some(font_family_id)
            && (self.layout_cache.is_some() || self.text.is_empty())
            && !(self.shared_shapes && self.drawn != Some(cx.drawing_id()))
        {
            return;
        }
        // Shapes an earlier draw of this drawing reads stay where they are.
        let keep = self.drawn == Some(cx.drawing_id());
        self.shared_shapes = keep;

        self.old_text = self.text.clone();
        self.old_font_size = self.font_size;
        self.old_inline = self.inline;
        self.old_text_scale = self.text_scale;
        self.old_font_family_id = Some(font_family_id);

        if self.text.is_empty() {
            self.layout_cache = None;
            self.components.clear();
            if !keep {
                self.draw_glyph.clear_shapes();
            }
            return;
        }

        let layout_font = {
            let mut fonts = cx.fonts.borrow_mut();
            let family = fonts.get_or_load_font_family(font_family_id);
            family.fonts().first().cloned()
        };

        let Some(layout_font) = layout_font else {
            self.layout_cache = None;
            self.components.clear();
            if !keep {
                self.draw_glyph.clear_shapes();
            }
            return;
        };

        let nodes = latex_math::parse(&self.text);
        let layout_size = self.layout_size();
        let Some(layout) = latex_math::layout(
            &nodes,
            layout_font.data().as_slice(),
            layout_size,
            if self.inline {
                MathStyle::Text
            } else {
                MathStyle::Display
            },
        ) else {
            self.layout_cache = None;
            self.components.clear();
            if !keep {
                self.draw_glyph.clear_shapes();
            }
            return;
        };

        if !keep {
            self.draw_glyph.clear_shapes();
        }
        let mut components = Vec::new();
        let mut items = layout.items.iter().peekable();
        while let Some(item) = items.next() {
            match item {
                LayoutItem::Glyph(glyph) => {
                    // An assembled glyph (a grown delimiter or arrow) fills as one
                    // shape, its pieces cut where they meet, so their overlaps
                    // cover once instead of adding up to brighter seams.
                    let mut pieces = vec![glyph];
                    while let Some(LayoutItem::Glyph(next)) = items.peek() {
                        if next.joint.is_none() {
                            break;
                        }
                        pieces.push(next);
                        items.next();
                    }
                    if let Some(shape_id) = build_glyph_shape(
                        &mut self.draw_glyph,
                        layout_font.as_ref(),
                        &pieces,
                        layout.ascent,
                    ) {
                        let glyphs = pieces
                            .iter()
                            .map(|glyph| ComponentGlyph {
                                id: glyph.glyph_id,
                                x: glyph.x,
                                baseline: layout.ascent + glyph.y,
                                size: glyph.size,
                            })
                            .collect();
                        push_component(&self.draw_glyph, shape_id, glyphs, &mut components);
                    }
                }
                LayoutItem::Rule(rule) => {
                    if let Some(shape_id) =
                        build_rule_shape(&mut self.draw_glyph, rule, layout.ascent)
                    {
                        push_component(&self.draw_glyph, shape_id, Vec::new(), &mut components);
                    }
                }
                LayoutItem::Rect(rect) => {
                    if let Some(shape_id) = build_rect_shape(
                        &mut self.draw_glyph,
                        rect.x,
                        layout.ascent + rect.y,
                        rect.width,
                        rect.height,
                    ) {
                        push_component(&self.draw_glyph, shape_id, Vec::new(), &mut components);
                    }
                }
            }
        }

        self.layout_cache = Some(layout);
        self.components = components;
        self.layout_font = Some(layout_font);
    }
}

/// Unknown commands, each once in first-seen order; empty means the formula typesets.
pub fn unknown_math_commands(text: &str) -> Vec<String> {
    let (_, mut unknown) = latex_math::parse_checked(text);
    let mut seen = HashSet::new();
    unknown.retain(|command| seen.insert(command.clone()));
    unknown
}

/// The glyphs filled as one shape: one glyph, or the pieces of an assembled
/// glyph, each cut at its joints and joined there (`math_outline`).
fn build_glyph_shape(
    dg: &mut DrawGlyph,
    font: &Font,
    glyphs: &[&LayoutGlyph],
    layout_ascent: f32,
) -> Option<GlyphShapeId> {
    // Where a piece meets the one before it, as (axis, coordinate).
    let cut = |g: &LayoutGlyph| match g.joint? {
        Joint::Right(d) => Some((0, g.x + d)),
        Joint::Up(d) => Some((1, layout_ascent + g.y - d)),
    };
    let mut outline = Outline::default();
    for (i, glyph) in glyphs.iter().enumerate() {
        let Some(glyph_outline) = font.glyph_outline(glyph.glyph_id) else {
            continue;
        };
        // A piece starts at its own joint and ends at the next piece's; y
        // grows downward, so a vertical piece starts at its bottom.
        let (start, end) = (cut(glyph), glyphs.get(i + 1).and_then(|g| cut(g)));
        let slab = start.or(end).map(|(axis, _)| {
            let (before, after) = if axis == 0 { (start, end) } else { (end, start) };
            Slab {
                axis,
                lo: before.map_or(f32::NEG_INFINITY, |c| c.1),
                hi: after.map_or(f32::INFINITY, |c| c.1),
            }
        });
        outline.add_glyph(
            glyph_outline.commands(),
            glyph.x,
            layout_ascent + glyph.y,
            glyph.size / font.units_per_em(),
            slab,
        );
    }
    for (axis, c) in glyphs.iter().filter_map(|g| cut(g)) {
        outline.join(axis, c);
    }
    dg.begin_shape();
    dg.set_color(1.0, 1.0, 1.0, 1.0);
    outline.fill(dg);
    dg.fill_layer();
    dg.commit_shape(None)
}

fn build_rule_shape(
    dg: &mut DrawGlyph,
    rule: &LayoutRule,
    layout_ascent: f32,
) -> Option<GlyphShapeId> {
    build_rect_shape(dg, rule.x, layout_ascent + rule.y, rule.width, rule.height)
}

fn build_rect_shape(dg: &mut DrawGlyph, x: f32, y: f32, w: f32, h: f32) -> Option<GlyphShapeId> {
    dg.begin_shape();
    dg.set_color(1.0, 1.0, 1.0, 1.0);
    dg.rect(x, y, w, h);
    dg.fill_layer();
    dg.commit_shape(None)
}

fn push_component(dg: &DrawGlyph, shape_id: GlyphShapeId, glyphs: Vec<ComponentGlyph>, components: &mut Vec<MathComponent>) {
    let Some(shape) = dg.shape(shape_id) else {
        return;
    };
    components.push(MathComponent {
        shape_id,
        origin: shape.origin,
        size: shape.size,
        glyphs,
    });
}
