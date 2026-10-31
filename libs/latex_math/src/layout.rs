/// Math layout engine. Takes a parsed AST and font metrics, produces positioned glyphs and rules.
///
/// This implements the core math typesetting algorithms from the OpenType MATH table spec
/// and TeX's math layout rules. It reads metrics from the font's MATH table via ttf-parser,
/// then produces a flat list of positioned items (glyphs, horizontal/vertical rules, rects).
use crate::parser::{self, AccentKind, Delimiter, MathNode, MatrixKind, SpaceWidth, StyleSwitch, Symbol};
use std::cell::RefCell;
use ttf_parser::{Face, GlyphId};

/// Display style vs inline (text) style, following TeX conventions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathStyle {
    Display,      // \displaystyle — large, centered fractions
    Text,         // \textstyle — inline, smaller fractions
    Script,       // Superscripts/subscripts
    ScriptScript, // Second-level scripts
}

impl MathStyle {
    fn script(self) -> Self {
        match self {
            MathStyle::Display | MathStyle::Text => MathStyle::Script,
            MathStyle::Script | MathStyle::ScriptScript => MathStyle::ScriptScript,
        }
    }

    fn scale(self) -> f32 {
        match self {
            MathStyle::Display | MathStyle::Text => 1.0,
            MathStyle::Script => 0.7,
            MathStyle::ScriptScript => 0.5,
        }
    }

    fn is_cramped(self) -> bool {
        matches!(self, MathStyle::Script | MathStyle::ScriptScript)
    }
}

/// Part id of an item that no formula node has claimed yet.
pub const NO_PART: u32 = u32::MAX;

/// A positioned glyph in the output
#[derive(Debug, Clone)]
pub struct LayoutGlyph {
    pub glyph_id: u16,
    pub x: f32,
    pub y: f32,
    pub size: f32, // font size in the local coordinate space
    /// The source character this glyph shows (the character the formula wrote,
    /// not the italic or size variant drawn); `'\0'` for a piece of an
    /// assembled delimiter.
    pub ch: char,
    /// Index into [`LayoutOutput::parts`].
    pub part: u32,
}

/// A horizontal or vertical rule (line)
#[derive(Debug, Clone)]
pub struct LayoutRule {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32, // thickness
    /// Index into [`LayoutOutput::parts`].
    pub part: u32,
}

/// A filled rectangle
#[derive(Debug, Clone)]
pub struct LayoutRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Index into [`LayoutOutput::parts`].
    pub part: u32,
}

/// All items that can appear in layout output
#[derive(Debug, Clone)]
pub enum LayoutItem {
    Glyph(LayoutGlyph),
    Rule(LayoutRule),
    Rect(LayoutRect),
}

impl LayoutItem {
    /// The index of the formula part this item belongs to.
    pub fn part(&self) -> u32 {
        match self {
            LayoutItem::Glyph(g) => g.part,
            LayoutItem::Rule(r) => r.part,
            LayoutItem::Rect(r) => r.part,
        }
    }

    fn part_mut(&mut self) -> &mut u32 {
        match self {
            LayoutItem::Glyph(g) => &mut g.part,
            LayoutItem::Rule(r) => &mut r.part,
            LayoutItem::Rect(r) => &mut r.part,
        }
    }

    fn shift(&mut self, dx: f32, dy: f32) {
        let (x, y) = match self {
            LayoutItem::Glyph(g) => (&mut g.x, &mut g.y),
            LayoutItem::Rule(r) => (&mut r.x, &mut r.y),
            LayoutItem::Rect(r) => (&mut r.x, &mut r.y),
        };
        *x += dx;
        *y += dy;
    }
}

/// One step of a part's path through the formula tree: a position in a
/// sequence (`role` empty, `index` set) or the named slot of a node
/// (`num`, `den`, `sup`, `sub`, `radicand`, `index`, `base`, `body`, and
/// `row`/`col` with their number for matrix cells).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartSeg {
    pub role: &'static str,
    pub index: Option<u16>,
}

/// What a group of layout items is in the formula: where it sits in the tree
/// and the names (`\id{name}{...}`) of the groups around it. Items of the
/// same part move together; a node's own marks (a fraction bar, a radical
/// sign, a growing delimiter) belong to the node's part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartInfo {
    pub path: Vec<PartSeg>,
    /// `\id` names around this part, outermost first.
    pub ids: Vec<String>,
}

impl PartInfo {
    /// The path as text: sequence positions as numbers, slots by name,
    /// joined with `/` (`1/num/0`, `2/sub/0`, `0/row:1/col:0/0`).
    pub fn path_string(&self) -> String {
        let mut s = String::new();
        for (i, seg) in self.path.iter().enumerate() {
            if i > 0 {
                s.push('/');
            }
            match (seg.role.is_empty(), seg.index) {
                (true, Some(n)) => s.push_str(&n.to_string()),
                (false, Some(n)) => {
                    s.push_str(seg.role);
                    s.push(':');
                    s.push_str(&n.to_string());
                }
                (_, None) => s.push_str(seg.role),
            }
        }
        s
    }
}

/// The output of the layout engine
#[derive(Debug, Clone)]
pub struct LayoutOutput {
    pub items: Vec<LayoutItem>,
    /// What each item's `part` index names.
    pub parts: Vec<PartInfo>,
    pub width: f32,
    pub height: f32,  // total height (ascent + descent)
    pub ascent: f32,  // distance from baseline to top
    pub descent: f32, // distance from baseline to bottom (positive = below)
}

/// Font metrics extracted from the OpenType MATH table and other font tables
struct FontMetrics<'a> {
    face: &'a Face<'a>,
    upem: f32,
    /// Base font size (in output units, e.g. pixels)
    font_size: f32,
}

impl<'a> FontMetrics<'a> {
    fn new(face: &'a Face<'a>, font_size: f32) -> Self {
        Self {
            face,
            upem: face.units_per_em() as f32,
            font_size,
        }
    }

    /// Convert font design units to output units at a given style scale
    fn to_output(&self, design_units: i16, style: MathStyle) -> f32 {
        design_units as f32 / self.upem * self.font_size * style.scale()
    }

    fn to_output_u16(&self, design_units: u16, style: MathStyle) -> f32 {
        design_units as f32 / self.upem * self.font_size * style.scale()
    }

    fn scaled_size(&self, style: MathStyle) -> f32 {
        self.font_size * style.scale()
    }

    fn glyph_id(&self, c: char) -> Option<GlyphId> {
        self.face.glyph_index(c)
    }

    fn glyph_advance(&self, glyph: GlyphId, style: MathStyle) -> f32 {
        let advance = self.face.glyph_hor_advance(glyph).unwrap_or(0);
        advance as f32 / self.upem * self.font_size * style.scale()
    }

    fn glyph_ascent(&self, style: MathStyle) -> f32 {
        let ascent = self.face.ascender() as f32;
        ascent / self.upem * self.font_size * style.scale()
    }

    fn glyph_descent(&self, style: MathStyle) -> f32 {
        let descent = self.face.descender() as f32;
        -descent / self.upem * self.font_size * style.scale() // make positive
    }

    fn glyph_bbox_height(&self, glyph: GlyphId, style: MathStyle) -> (f32, f32) {
        if let Some(bbox) = self.face.glyph_bounding_box(glyph) {
            let scale = self.font_size * style.scale() / self.upem;
            let ascent = bbox.y_max as f32 * scale;
            let descent = -(bbox.y_min as f32 * scale); // positive below baseline
            (ascent, descent)
        } else {
            (self.glyph_ascent(style), self.glyph_descent(style))
        }
    }

    // --- MATH table constants ---

    fn axis_height(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.axis_height().value)
            .unwrap_or(self.glyph_ascent(style) * 0.5)
    }

    fn fraction_rule_thickness(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.fraction_rule_thickness().value)
            .unwrap_or(self.scaled_size(style) * 0.04)
    }

    fn fraction_num_shift_up(&self, style: MathStyle) -> f32 {
        let f = if matches!(style, MathStyle::Display) {
            |c: &ttf_parser::math::Constants| c.fraction_numerator_display_style_shift_up().value
        } else {
            |c: &ttf_parser::math::Constants| c.fraction_numerator_shift_up().value
        };
        self.math_const(style, f)
            .unwrap_or(self.glyph_ascent(style) * 0.7)
    }

    fn fraction_den_shift_down(&self, style: MathStyle) -> f32 {
        let f = if matches!(style, MathStyle::Display) {
            |c: &ttf_parser::math::Constants| {
                c.fraction_denominator_display_style_shift_down().value
            }
        } else {
            |c: &ttf_parser::math::Constants| c.fraction_denominator_shift_down().value
        };
        self.math_const(style, f)
            .unwrap_or(self.glyph_descent(style) * 0.7)
    }

    fn fraction_num_gap_min(&self, style: MathStyle) -> f32 {
        let f = if matches!(style, MathStyle::Display) {
            |c: &ttf_parser::math::Constants| c.fraction_num_display_style_gap_min().value
        } else {
            |c: &ttf_parser::math::Constants| c.fraction_numerator_gap_min().value
        };
        self.math_const(style, f)
            .unwrap_or(self.scaled_size(style) * 0.05)
    }

    fn fraction_den_gap_min(&self, style: MathStyle) -> f32 {
        let f = if matches!(style, MathStyle::Display) {
            |c: &ttf_parser::math::Constants| c.fraction_denom_display_style_gap_min().value
        } else {
            |c: &ttf_parser::math::Constants| c.fraction_denominator_gap_min().value
        };
        self.math_const(style, f)
            .unwrap_or(self.scaled_size(style) * 0.05)
    }

    fn superscript_shift_up(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.superscript_shift_up().value)
            .unwrap_or(self.glyph_ascent(style) * 0.45)
    }

    fn superscript_shift_up_cramped(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.superscript_shift_up_cramped().value)
            .unwrap_or(self.glyph_ascent(style) * 0.35)
    }

    fn subscript_shift_down(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.subscript_shift_down().value)
            .unwrap_or(self.glyph_descent(style) * 0.3)
    }

    fn subscript_baseline_drop_min(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.subscript_baseline_drop_min().value)
            .unwrap_or(self.glyph_descent(style) * 0.5)
    }

    fn superscript_baseline_drop_max(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.superscript_baseline_drop_max().value)
            .unwrap_or(self.glyph_ascent(style) * 0.35)
    }

    fn superscript_bottom_min(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.superscript_bottom_min().value)
            .unwrap_or(self.glyph_ascent(style) * 0.25)
    }

    fn subscript_top_max(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.subscript_top_max().value)
            .unwrap_or(self.glyph_ascent(style) * 0.8)
    }

    fn sub_sup_gap_min(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.sub_superscript_gap_min().value)
            .unwrap_or(self.scaled_size(style) * 0.15)
    }

    fn space_after_script(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.space_after_script().value)
            .unwrap_or(self.scaled_size(style) * 0.05)
    }

    fn radical_vertical_gap(&self, style: MathStyle) -> f32 {
        let f = if matches!(style, MathStyle::Display) {
            |c: &ttf_parser::math::Constants| c.radical_display_style_vertical_gap().value
        } else {
            |c: &ttf_parser::math::Constants| c.radical_vertical_gap().value
        };
        self.math_const(style, f)
            .unwrap_or(self.scaled_size(style) * 0.05)
    }

    fn radical_rule_thickness(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.radical_rule_thickness().value)
            .unwrap_or(self.scaled_size(style) * 0.04)
    }

    fn radical_extra_ascender(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.radical_extra_ascender().value)
            .unwrap_or(self.scaled_size(style) * 0.04)
    }

    fn radical_kern_before_degree(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.radical_kern_before_degree().value)
            .unwrap_or(self.scaled_size(style) * 0.05)
    }

    fn radical_kern_after_degree(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.radical_kern_after_degree().value)
            .unwrap_or(-self.scaled_size(style) * 0.3)
    }

    fn radical_degree_bottom_raise_percent(&self, _style: MathStyle) -> f32 {
        if let Some(math) = self.face.tables().math {
            if let Some(constants) = &math.constants {
                return constants.radical_degree_bottom_raise_percent() as f32 / 100.0;
            }
        }
        0.6
    }

    fn upper_limit_gap_min(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.upper_limit_gap_min().value)
            .unwrap_or(self.scaled_size(style) * 0.1)
    }

    fn lower_limit_gap_min(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.lower_limit_gap_min().value)
            .unwrap_or(self.scaled_size(style) * 0.1)
    }

    fn upper_limit_baseline_rise_min(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.upper_limit_baseline_rise_min().value)
            .unwrap_or(self.scaled_size(style) * 0.3)
    }

    fn lower_limit_baseline_drop_min(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.lower_limit_baseline_drop_min().value)
            .unwrap_or(self.scaled_size(style) * 0.6)
    }

    fn overbar_vertical_gap(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.overbar_vertical_gap().value)
            .unwrap_or(self.scaled_size(style) * 0.15)
    }

    fn overbar_rule_thickness(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.overbar_rule_thickness().value)
            .unwrap_or(self.scaled_size(style) * 0.04)
    }

    fn overbar_extra_ascender(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.overbar_extra_ascender().value)
            .unwrap_or(self.scaled_size(style) * 0.04)
    }

    fn underbar_vertical_gap(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.underbar_vertical_gap().value)
            .unwrap_or(self.scaled_size(style) * 0.15)
    }

    fn underbar_rule_thickness(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.underbar_rule_thickness().value)
            .unwrap_or(self.scaled_size(style) * 0.04)
    }

    fn underbar_extra_descender(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.underbar_extra_descender().value)
            .unwrap_or(self.scaled_size(style) * 0.04)
    }

    fn accent_base_height(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.accent_base_height().value)
            .unwrap_or(self.glyph_ascent(style) * 0.75)
    }

    fn italic_correction(&self, glyph: GlyphId, style: MathStyle) -> f32 {
        if let Some(math) = self.face.tables().math {
            if let Some(gi) = &math.glyph_info {
                if let Some(ic) = &gi.italic_corrections {
                    if let Some(val) = ic.get(glyph) {
                        return self.to_output(val.value, style);
                    }
                }
            }
        }
        0.0
    }

    /// The font's math kern at one corner of `glyph` for a script reaching to
    /// `height` above the baseline (design: negative tucks the script in).
    fn math_kern(&self, glyph: GlyphId, corner: Corner, height: f32, style: MathStyle) -> f32 {
        let Some(math) = self.face.tables().math else { return 0.0 };
        let Some(info) = math.glyph_info.as_ref().and_then(|g| g.kern_infos.as_ref()).and_then(|k| k.get(glyph)) else {
            return 0.0;
        };
        let kern = match corner {
            Corner::TopRight => info.top_right,
            Corner::BottomRight => info.bottom_right,
        };
        let Some(kern) = kern else { return 0.0 };
        let h = height / (self.font_size * style.scale() / self.upem);
        let mut index = 0u16;
        while index < kern.count() {
            match kern.height(index) {
                Some(v) if h >= v.value as f32 => index += 1,
                _ => break,
            }
        }
        kern.kern(index).map_or(0.0, |v| self.to_output(v.value, style))
    }

    fn top_accent_attachment(&self, glyph: GlyphId, style: MathStyle) -> Option<f32> {
        if let Some(math) = self.face.tables().math {
            if let Some(gi) = &math.glyph_info {
                if let Some(ta) = &gi.top_accent_attachments {
                    if let Some(val) = ta.get(glyph) {
                        return Some(self.to_output(val.value, style));
                    }
                }
            }
        }
        None
    }

    /// A glyph grown to at least `min_height`: the first prepared variant that
    /// is tall enough, else an assembly of the font's parts (ends plus as many
    /// repeated extenders as it takes), else the largest variant. The box has the
    /// glyph's origin at its baseline; callers place it.
    fn stretch_vertical(
        &self,
        glyph: GlyphId,
        ch: char,
        min_height: f32,
        style: MathStyle,
    ) -> LayoutBox {
        if let Some(math) = self.face.tables().math {
            if let Some(variants) = &math.variants {
                if let Some(construction) = variants.vertical_constructions.get(glyph) {
                    let mut last = None;
                    for i in 0..construction.variants.len() {
                        if let Some(v) = construction.variants.get(i) {
                            if self.to_output_u16(v.advance_measurement, style) >= min_height {
                                return self.glyph_box(v.variant_glyph, ch, style);
                            }
                            last = Some(v.variant_glyph);
                        }
                    }
                    if let Some(assembly) = &construction.assembly {
                        if let Some(b) = self.assemble_vertical(
                            assembly,
                            variants.min_connector_overlap,
                            ch,
                            min_height,
                            style,
                        ) {
                            return b;
                        }
                    }
                    if let Some(g) = last {
                        return self.glyph_box(g, ch, style);
                    }
                }
            }
        }
        self.glyph_box(glyph, ch, style)
    }

    /// The glyph `stretch_vertical` would draw when it is a single glyph.
    fn metrics_variant(&self, glyph: GlyphId, ch: char, min_height: f32, style: MathStyle) -> GlyphId {
        match self.stretch_vertical(glyph, ch, min_height, style).items.first() {
            Some(LayoutItem::Glyph(g)) => GlyphId(g.glyph_id),
            _ => glyph,
        }
    }

    /// The least height a display-style big operator may have.
    fn display_operator_min_height(&self, style: MathStyle) -> f32 {
        self.math_const(style, |c| c.display_operator_min_height() as i16)
            .unwrap_or((self.glyph_ascent(style) + self.glyph_descent(style)) * 1.35)
    }

    fn glyph_box(&self, glyph: GlyphId, ch: char, style: MathStyle) -> LayoutBox {
        let (ascent, descent) = self.glyph_bbox_height(glyph, style);
        LayoutBox {
            items: vec![LayoutItem::Glyph(LayoutGlyph {
                glyph_id: glyph.0,
                x: 0.0,
                y: 0.0,
                size: self.scaled_size(style),
                ch,
                part: NO_PART,
            })],
            width: self.glyph_advance(glyph, style),
            ascent,
            descent,
            italic: self.italic_correction(glyph, style),
            kern_glyph: Some(glyph),
        }
    }

    /// Stacks the parts of a glyph assembly (listed bottom to top) so the stack is
    /// `min_height` tall: the fewest extender repeats whose loosest fit reaches it,
    /// then the connector overlap tightened as far as the font allows to hit the
    /// height exactly. Centered on the baseline.
    fn assemble_vertical(
        &self,
        assembly: &ttf_parser::math::GlyphAssembly,
        min_overlap: u16,
        ch: char,
        min_height: f32,
        style: MathStyle,
    ) -> Option<LayoutBox> {
        let parts: Vec<_> = assembly.parts.into_iter().collect();
        if parts.is_empty() {
            return None;
        }
        let k = self.scaled_size(style) / self.upem;
        let target = min_height / k;
        let min_overlap = min_overlap as f32;
        let has_extender = parts.iter().any(|p| p.part_flags.extender());
        let mut seq: Vec<usize> = Vec::new();
        for repeats in 0..=64usize {
            seq.clear();
            for (i, p) in parts.iter().enumerate() {
                let n = if p.part_flags.extender() { repeats } else { 1 };
                for _ in 0..n {
                    seq.push(i);
                }
            }
            if seq.is_empty() {
                continue;
            }
            let sum: f32 = seq.iter().map(|&i| parts[i].full_advance as f32).sum();
            let loosest = sum - (seq.len() - 1) as f32 * min_overlap;
            if loosest >= target || !has_extender {
                break;
            }
        }
        if seq.is_empty() {
            return None;
        }
        let sum: f32 = seq.iter().map(|&i| parts[i].full_advance as f32).sum();
        let joints = seq.len() - 1;
        let overlap = if joints == 0 {
            0.0
        } else {
            let max_overlap = seq
                .windows(2)
                .map(|w| {
                    (parts[w[0]].end_connector_length).min(parts[w[1]].start_connector_length)
                        as f32
                })
                .fold(f32::MAX, f32::min)
                .max(min_overlap);
            ((sum - target) / joints as f32).clamp(min_overlap, max_overlap)
        };
        let total = sum - joints as f32 * overlap;
        let width = seq
            .iter()
            .map(|&i| self.glyph_advance(parts[i].glyph_id, style))
            .fold(0.0f32, f32::max);
        let mut items = Vec::new();
        let mut bottom = 0.0f32;
        for &i in &seq {
            let p = &parts[i];
            let w = self.glyph_advance(p.glyph_id, style);
            // y grows downward; the stack is centered on the baseline
            let y_up = (bottom - total * 0.5) * k;
            items.push(LayoutItem::Glyph(LayoutGlyph {
                glyph_id: p.glyph_id.0,
                x: (width - w) * 0.5,
                y: -y_up,
                size: self.scaled_size(style),
                ch,
                part: NO_PART,
            }));
            bottom += p.full_advance as f32 - overlap;
        }
        Some(LayoutBox {
            items,
            width,
            ascent: total * 0.5 * k,
            descent: total * 0.5 * k,
            italic: assembly.italics_correction.value as f32 * k,
            kern_glyph: None,
        })
    }

    fn math_const<F: Fn(&ttf_parser::math::Constants) -> i16>(
        &self,
        style: MathStyle,
        f: F,
    ) -> Option<f32> {
        let math = self.face.tables().math?;
        let constants = math.constants.as_ref()?;
        Some(self.to_output(f(constants), style))
    }
}

#[derive(Clone, Copy)]
enum Corner {
    TopRight,
    BottomRight,
}

/// Internal layout box — tracks dimensions relative to baseline
#[derive(Debug, Clone)]
struct LayoutBox {
    items: Vec<LayoutItem>,
    width: f32,
    ascent: f32,  // above baseline (positive)
    descent: f32, // below baseline (positive)
    /// Italic correction of the last glyph (what a script attached to this box
    /// is pushed right by).
    italic: f32,
    /// The last glyph of the box, whose corners the font's math kerning
    /// describes (what a script attached to the box tucks against).
    kern_glyph: Option<GlyphId>,
}

impl LayoutBox {
    fn empty() -> Self {
        Self {
            items: Vec::new(),
            width: 0.0,
            ascent: 0.0,
            descent: 0.0,
            italic: 0.0,
            kern_glyph: None,
        }
    }

    /// Translate all items by (dx, dy)
    fn translate(&mut self, dx: f32, dy: f32) {
        for item in &mut self.items {
            item.shift(dx, dy);
        }
    }

    /// Append another box to the right
    fn append_right(&mut self, other: &LayoutBox) {
        let dx = self.width;
        for item in &other.items {
            let mut item = item.clone();
            item.shift(dx, 0.0);
            self.items.push(item);
        }
        self.width += other.width;
        self.ascent = self.ascent.max(other.ascent);
        self.descent = self.descent.max(other.descent);
        self.italic = other.italic;
        self.kern_glyph = other.kern_glyph;
    }
}

struct LayoutEngine<'a> {
    metrics: FontMetrics<'a>,
    /// The path of the node being laid out, for part keys.
    path: RefCell<Vec<PartSeg>>,
    /// The `\id` names around the node being laid out.
    ids: RefCell<Vec<String>>,
    parts: RefCell<Vec<PartInfo>>,
}

impl<'a> LayoutEngine<'a> {
    fn new(face: &'a Face<'a>, font_size: f32) -> Self {
        Self {
            metrics: FontMetrics::new(face, font_size),
            path: RefCell::new(Vec::new()),
            ids: RefCell::new(Vec::new()),
            parts: RefCell::new(Vec::new()),
        }
    }

    /// Lays out the nodes in the named slot of the node being laid out.
    fn sub(&self, role: &'static str, nodes: &[MathNode], style: MathStyle) -> LayoutBox {
        self.sub_at(role, None, nodes, style)
    }

    fn sub_at(
        &self,
        role: &'static str,
        index: Option<u16>,
        nodes: &[MathNode],
        style: MathStyle,
    ) -> LayoutBox {
        self.path.borrow_mut().push(PartSeg { role, index });
        let b = self.layout_sequence(nodes, style);
        self.path.borrow_mut().pop();
        b
    }

    /// Gives every item of `b` that no deeper node claimed to the node at the
    /// current path: a node's own marks (bar, radical sign, delimiters, accent).
    fn claim(&self, b: &mut LayoutBox) {
        if !b.items.iter().any(|i| i.part() == NO_PART) {
            return;
        }
        let id = {
            let mut parts = self.parts.borrow_mut();
            parts.push(PartInfo {
                path: self.path.borrow().clone(),
                ids: self.ids.borrow().clone(),
            });
            (parts.len() - 1) as u32
        };
        for item in &mut b.items {
            let part = item.part_mut();
            if *part == NO_PART {
                *part = id;
            }
        }
    }

    fn at_index<R>(&self, i: usize, f: impl FnOnce() -> R) -> R {
        self.path.borrow_mut().push(PartSeg { role: "", index: Some(i as u16) });
        let r = f();
        self.path.borrow_mut().pop();
        r
    }

    fn layout_sequence(&self, nodes: &[MathNode], style: MathStyle) -> LayoutBox {
        // (box, how scripts attach to it, whether it is one glyph)
        let mut chunks: Vec<(LayoutBox, BaseKind, bool)> = Vec::new();
        // `\\displaystyle` and its kin switch the style of what follows.
        let mut style = style;

        let mut prev: Option<MathClass> = None;
        for (i, node) in nodes.iter().enumerate() {
            let class = effective_class(node_class(node), prev, nodes.get(i + 1).map(node_class));
            prev = Some(class);
            if let MathNode::Style(switch) = node {
                style = match switch {
                    StyleSwitch::Display => MathStyle::Display,
                    StyleSwitch::Text => MathStyle::Text,
                    StyleSwitch::Script => MathStyle::Script,
                    StyleSwitch::ScriptScript => MathStyle::ScriptScript,
                };
                continue;
            }
            self.at_index(i, || match node {
                MathNode::Superscript(sup) => {
                    if let Some((base, base_kind, simple)) = chunks.pop() {
                        let mut attached =
                            self.layout_attached_superscript(base, sup, style, base_kind, simple);
                        self.claim(&mut attached);
                        chunks.push((attached, BaseKind::Normal, false));
                    } else {
                        let mut b = self.layout_superscript(sup, style);
                        self.claim(&mut b);
                        chunks.push((b, BaseKind::Normal, false));
                    }
                }
                MathNode::Subscript(sub) => {
                    if let Some((base, base_kind, _)) = chunks.pop() {
                        let mut attached =
                            self.layout_attached_subscript(base, sub, style, base_kind);
                        self.claim(&mut attached);
                        chunks.push((attached, BaseKind::Normal, false));
                    } else {
                        let mut b = self.layout_subscript(sub, style);
                        self.claim(&mut b);
                        chunks.push((b, BaseKind::Normal, false));
                    }
                }
                MathNode::SubSuperscript(sub, sup) => {
                    if let Some((base, base_kind, simple)) = chunks.pop() {
                        let mut attached =
                            self.layout_attached_sub_superscript(base, sub, sup, style, base_kind, simple);
                        self.claim(&mut attached);
                        chunks.push((attached, BaseKind::Normal, false));
                    } else {
                        let mut b = self.layout_sub_superscript(sub, sup, style);
                        self.claim(&mut b);
                        chunks.push((b, BaseKind::Normal, false));
                    }
                }
                _ => {
                    let mut box_node = match node {
                        MathNode::Char(c) => self.layout_char_impl(*c, style, true, class),
                        MathNode::Symbol(sym) => self.layout_symbol(*sym, style, class),
                        _ => self.layout_node(node, style),
                    };
                    self.claim(&mut box_node);
                    let base_kind = base_kind_for_node(node, style);
                    chunks.push((box_node, base_kind, is_single_glyph(node)));
                }
            });
        }

        let mut result = LayoutBox::empty();
        for (b, _, _) in chunks {
            result.append_right(&b);
        }
        result
    }

    fn layout_node(&self, node: &MathNode, style: MathStyle) -> LayoutBox {
        match node {
            MathNode::Char(c) => self.layout_char_impl(*c, style, true, char_math_class(*c)),
            MathNode::Symbol(sym) => self.layout_symbol(*sym, style, symbol_math_class(*sym)),
            MathNode::Frac(num, den) => self.layout_frac(num, den, style),
            MathNode::Sqrt(index, content) => self.layout_sqrt(index.as_deref(), content, style),
            MathNode::Superscript(sup) => self.layout_superscript(sup, style),
            MathNode::Subscript(sub) => self.layout_subscript(sub, style),
            MathNode::SubSuperscript(sub, sup) => self.layout_sub_superscript(sub, sup, style),
            MathNode::Accent(kind, content) => self.layout_accent(*kind, content, style),
            MathNode::Group(children) => self.layout_sequence(children, style),
            MathNode::LeftRight(left, content, right) => {
                self.layout_left_right(*left, content, *right, style)
            }
            MathNode::Matrix(kind, rows) => self.layout_matrix(*kind, rows, style),
            MathNode::Space(width) => self.layout_space(*width, style),
            MathNode::Text(text) => self.layout_text(text, style),
            MathNode::OperatorName(name) => self.layout_operator(name, style),
            MathNode::Overline(content) => self.layout_overline(content, style),
            MathNode::Underline(content) => self.layout_underline(content, style),
            MathNode::Overbrace(content) => self.layout_overline(content, style), // simplified
            MathNode::Underbrace(content) => self.layout_underline(content, style), // simplified
            MathNode::MathVariant(_variant, content) => {
                // For now, just layout content normally
                // A full implementation would switch font variant
                self.sub("body", content, style)
            }
            MathNode::Id(name, content) => {
                self.ids.borrow_mut().push(name.clone());
                let b = self.sub("id", content, style);
                self.ids.borrow_mut().pop();
                b
            }
            MathNode::Color(_color, content) => {
                // Layout content, color is handled at render time
                self.sub("body", content, style)
            }
            MathNode::Limits(_, content) => self.layout_sequence(content, style),
            // A style switch outside a sequence has nothing after it to style.
            MathNode::Style(_) => LayoutBox::empty(),
        }
    }

    /// The space a large operator keeps after its glyph (not part of the sign).
    fn operator_right_space(&self, style: MathStyle) -> f32 {
        spacing_for_class(MathClass::LargeOperator, self.metrics.scaled_size(style), style).1
    }

    /// How far right of the base's advance a superscript starts: the italic
    /// correction and the font's top-right kern at the script's bottom.
    fn sup_offset(&self, base: &LayoutBox, bottom_height: f32, style: MathStyle, kind: BaseKind) -> f32 {
        if kind == BaseKind::Integral {
            return -self.operator_right_space(style);
        }
        base.italic
            + base
                .kern_glyph
                .map_or(0.0, |g| self.metrics.math_kern(g, Corner::TopRight, bottom_height, style))
    }

    /// The same for a subscript (bottom-right kern at the script's top).
    fn sub_offset(&self, base: &LayoutBox, top_height: f32, style: MathStyle, kind: BaseKind) -> f32 {
        if kind == BaseKind::Integral {
            return -self.operator_right_space(style) - base.italic;
        }
        base.kern_glyph
            .map_or(0.0, |g| self.metrics.math_kern(g, Corner::BottomRight, top_height, style))
    }

    /// The least a superscript is raised: its bottom at least
    /// SuperscriptBottomMin above the baseline, and on a built base (not one
    /// glyph) its baseline at most SuperscriptBaselineDropMax below the base's
    /// top. A single letter's ascender (b, d, f) does not lift its script.
    fn sup_min_shift(&self, base: &LayoutBox, sup_box: &LayoutBox, style: MathStyle, simple: bool) -> f32 {
        let bottom = self.metrics.superscript_bottom_min(style) + sup_box.descent;
        let drop = if simple { 0.0 } else { base.ascent - self.metrics.superscript_baseline_drop_max(style) };
        bottom.max(drop).max(0.0)
    }

    fn layout_attached_superscript(
        &self,
        base: LayoutBox,
        sup: &[MathNode],
        style: MathStyle,
        base_kind: BaseKind,
        simple: bool,
    ) -> LayoutBox {
        if base_kind == BaseKind::DisplayBigOperator {
            return self.layout_limit_superscript(base, sup, style);
        }
        let simple = simple && base_kind != BaseKind::Integral;

        let script_style = style.script();
        let sup_box = self.sub("sup", sup, script_style);
        let shift = if style.is_cramped() {
            self.metrics.superscript_shift_up_cramped(style)
        } else {
            self.metrics.superscript_shift_up(style)
        };
        let final_shift = shift.max(self.sup_min_shift(&base, &sup_box, style, simple));
        let after = self.metrics.space_after_script(style);

        let mut result = base.clone();
        let mut sup_items = sup_box.clone();
        let sup_x = base.width + self.sup_offset(&base, final_shift - sup_box.descent, style, base_kind);
        sup_items.translate(sup_x, -final_shift);
        result.items.extend(sup_items.items);

        result.width = sup_x + sup_box.width + after;
        result.italic = 0.0;
        result.kern_glyph = None;
        result.ascent = base.ascent.max(final_shift + sup_box.ascent);
        result.descent = base.descent.max((sup_box.descent - final_shift).max(0.0));
        result
    }

    fn layout_attached_subscript(
        &self,
        base: LayoutBox,
        sub: &[MathNode],
        style: MathStyle,
        base_kind: BaseKind,
    ) -> LayoutBox {
        if base_kind == BaseKind::DisplayBigOperator {
            return self.layout_limit_subscript(base, sub, style);
        }

        let script_style = style.script();
        let sub_box = self.sub("sub", sub, script_style);
        let shift = self.metrics.subscript_shift_down(style);
        let min_shift = sub_box.ascent - self.metrics.subscript_top_max(style);
        let mut final_shift = shift.max(min_shift.max(0.0));
        if base_kind == BaseKind::Integral {
            final_shift = final_shift.max(base.descent + self.metrics.subscript_baseline_drop_min(style));
        }
        let after = self.metrics.space_after_script(style);

        let mut result = base.clone();
        let mut sub_items = sub_box.clone();
        let sub_x = base.width + self.sub_offset(&base, sub_box.ascent - final_shift, style, base_kind);
        sub_items.translate(sub_x, final_shift);
        result.items.extend(sub_items.items);

        result.width = sub_x + sub_box.width + after;
        result.italic = 0.0;
        result.kern_glyph = None;
        result.ascent = base.ascent.max((sub_box.ascent - final_shift).max(0.0));
        result.descent = base.descent.max(final_shift + sub_box.descent);
        result
    }

    fn layout_attached_sub_superscript(
        &self,
        base: LayoutBox,
        sub: &[MathNode],
        sup: &[MathNode],
        style: MathStyle,
        base_kind: BaseKind,
        simple: bool,
    ) -> LayoutBox {
        if base_kind == BaseKind::DisplayBigOperator {
            return self.layout_limit_sub_superscript(base, sub, sup, style);
        }
        let simple = simple && base_kind != BaseKind::Integral;

        let script_style = style.script();
        let sup_box = self.sub("sup", sup, script_style);
        let sub_box = self.sub("sub", sub, script_style);

        let sup_shift = if style.is_cramped() {
            self.metrics.superscript_shift_up_cramped(style)
        } else {
            self.metrics.superscript_shift_up(style)
        };
        let mut final_sup_shift = sup_shift.max(self.sup_min_shift(&base, &sup_box, style, simple));

        let sub_shift = self.metrics.subscript_shift_down(style);
        let sub_min_shift = sub_box.ascent - self.metrics.subscript_top_max(style);
        let mut final_sub_shift = sub_shift.max(sub_min_shift.max(0.0));
        if base_kind == BaseKind::Integral {
            final_sub_shift = final_sub_shift.max(base.descent + self.metrics.subscript_baseline_drop_min(style));
        }

        let min_gap = self.metrics.sub_sup_gap_min(style);
        let sup_bottom = -final_sup_shift + sup_box.descent;
        let sub_top = final_sub_shift - sub_box.ascent;
        let gap = sub_top - sup_bottom;
        if gap < min_gap {
            let extra = (min_gap - gap) * 0.5;
            final_sup_shift += extra;
            final_sub_shift += extra;
        }

        let after = self.metrics.space_after_script(style);
        let sup_x = base.width + self.sup_offset(&base, final_sup_shift - sup_box.descent, style, base_kind);
        let sub_x = base.width + self.sub_offset(&base, sub_box.ascent - final_sub_shift, style, base_kind);

        let mut result = base.clone();

        let mut sup_items = sup_box.clone();
        sup_items.translate(sup_x, -final_sup_shift);
        result.items.extend(sup_items.items);

        let mut sub_items = sub_box.clone();
        sub_items.translate(sub_x, final_sub_shift);
        result.items.extend(sub_items.items);

        result.width = (sup_x + sup_box.width).max(sub_x + sub_box.width) + after;
        result.italic = 0.0;
        result.kern_glyph = None;
        result.ascent = base.ascent.max(final_sup_shift + sup_box.ascent);
        result.descent = base.descent.max(final_sub_shift + sub_box.descent);
        result
    }

    fn layout_limit_superscript(
        &self,
        base: LayoutBox,
        sup: &[MathNode],
        style: MathStyle,
    ) -> LayoutBox {
        let script_style = style.script();
        let sup_box = self.sub("sup", sup, script_style);
        let gap = self.metrics.upper_limit_gap_min(style);
        let rise_min = self.metrics.upper_limit_baseline_rise_min(style);
        let shift = rise_min.max(base.ascent + gap + sup_box.descent);

        let total_width = base.width.max(sup_box.width);
        let base_x = (total_width - base.width) * 0.5;
        let sup_x = (total_width - sup_box.width) * 0.5;

        let mut result = LayoutBox::empty();
        let mut base_items = base.clone();
        base_items.translate(base_x, 0.0);
        result.items.extend(base_items.items);

        let mut sup_items = sup_box.clone();
        sup_items.translate(sup_x, -shift);
        result.items.extend(sup_items.items);

        result.width = total_width;
        result.ascent = base.ascent.max(shift + sup_box.ascent);
        result.descent = base.descent.max((sup_box.descent - shift).max(0.0));
        result
    }

    fn layout_limit_subscript(
        &self,
        base: LayoutBox,
        sub: &[MathNode],
        style: MathStyle,
    ) -> LayoutBox {
        let script_style = style.script();
        let sub_box = self.sub("sub", sub, script_style);
        let gap = self.metrics.lower_limit_gap_min(style);
        let drop_min = self.metrics.lower_limit_baseline_drop_min(style);
        let shift = drop_min.max(base.descent + gap + sub_box.ascent);

        let total_width = base.width.max(sub_box.width);
        let base_x = (total_width - base.width) * 0.5;
        let sub_x = (total_width - sub_box.width) * 0.5;

        let mut result = LayoutBox::empty();
        let mut base_items = base.clone();
        base_items.translate(base_x, 0.0);
        result.items.extend(base_items.items);

        let mut sub_items = sub_box.clone();
        sub_items.translate(sub_x, shift);
        result.items.extend(sub_items.items);

        result.width = total_width;
        result.ascent = base.ascent.max((sub_box.ascent - shift).max(0.0));
        result.descent = base.descent.max(shift + sub_box.descent);
        result
    }

    fn layout_limit_sub_superscript(
        &self,
        base: LayoutBox,
        sub: &[MathNode],
        sup: &[MathNode],
        style: MathStyle,
    ) -> LayoutBox {
        let script_style = style.script();
        let sub_box = self.sub("sub", sub, script_style);
        let sup_box = self.sub("sup", sup, script_style);

        let upper_gap = self.metrics.upper_limit_gap_min(style);
        let lower_gap = self.metrics.lower_limit_gap_min(style);
        let upper_rise_min = self.metrics.upper_limit_baseline_rise_min(style);
        let lower_drop_min = self.metrics.lower_limit_baseline_drop_min(style);

        let sup_shift = upper_rise_min.max(base.ascent + upper_gap + sup_box.descent);
        let sub_shift = lower_drop_min.max(base.descent + lower_gap + sub_box.ascent);

        let total_width = base.width.max(sub_box.width.max(sup_box.width));
        let base_x = (total_width - base.width) * 0.5;
        let sup_x = (total_width - sup_box.width) * 0.5;
        let sub_x = (total_width - sub_box.width) * 0.5;

        let mut result = LayoutBox::empty();

        let mut base_items = base.clone();
        base_items.translate(base_x, 0.0);
        result.items.extend(base_items.items);

        let mut sup_items = sup_box.clone();
        sup_items.translate(sup_x, -sup_shift);
        result.items.extend(sup_items.items);

        let mut sub_items = sub_box.clone();
        sub_items.translate(sub_x, sub_shift);
        result.items.extend(sub_items.items);

        result.width = total_width;
        result.ascent = base.ascent.max(sup_shift + sup_box.ascent);
        result.descent = base.descent.max(sub_shift + sub_box.descent);
        result
    }

    fn layout_upright_char(&self, c: char, style: MathStyle) -> LayoutBox {
        self.layout_char_impl(c, style, false, char_math_class(c))
    }

    fn layout_char_impl(&self, c: char, style: MathStyle, use_math_italic: bool, class: MathClass) -> LayoutBox {
        let mapped = if use_math_italic {
            map_math_italic_latin(c)
                .and_then(|mc| self.metrics.glyph_id(mc).map(|_| mc))
                .unwrap_or(c)
        } else {
            c
        };
        // The hyphen is the minus sign in math.
        let mapped = if mapped == '-' && self.metrics.glyph_id('\u{2212}').is_some() { '\u{2212}' } else { mapped };

        if let Some(glyph_id) = self.metrics.glyph_id(mapped) {
            let advance = self.metrics.glyph_advance(glyph_id, style);
            let (ascent, descent) = self.metrics.glyph_bbox_height(glyph_id, style);
            let (left_space, right_space) = spacing_for_class(class, self.metrics.scaled_size(style), style);
            let item = LayoutItem::Glyph(LayoutGlyph {
                glyph_id: glyph_id.0,
                x: left_space,
                y: 0.0,
                size: self.metrics.scaled_size(style),
                ch: c,
                part: NO_PART,
            });
            LayoutBox {
                items: vec![item],
                width: left_space + advance + right_space,
                ascent,
                descent,
                italic: if use_math_italic {
                    self.metrics.italic_correction(glyph_id, style)
                } else {
                    0.0
                },
                kern_glyph: Some(glyph_id),
            }
        } else {
            // Unknown character — leave a space
            let space = self.metrics.scaled_size(style) * 0.3;
            LayoutBox {
                width: space,
                ..LayoutBox::empty()
            }
        }
    }

    fn layout_symbol(&self, sym: Symbol, style: MathStyle, class: MathClass) -> LayoutBox {
        let c = parser::symbol_to_char(sym);
        if let Some(mut glyph_id) = self.metrics.glyph_id(c) {
            let (left_space, right_space) = spacing_for_class(class, self.metrics.scaled_size(style), style);
            // Display-style big operators (sum/integral/...) should use larger
            // variants when available and sit centered around the math axis.
            if is_big_operator(sym) && matches!(style, MathStyle::Display) {
                let target = self.metrics.display_operator_min_height(style);
                glyph_id = self.metrics.metrics_variant(glyph_id, c, target, style);
            }

            let advance = self.metrics.glyph_advance(glyph_id, style);
            let (mut ascent, mut descent) = self.metrics.glyph_bbox_height(glyph_id, style);
            let mut y = 0.0f32;

            if is_big_operator(sym) {
                let axis = self.metrics.axis_height(style);
                let glyph_center = (ascent - descent) * 0.5;
                let shift = axis - glyph_center;
                y = -shift;
                ascent += shift;
                descent -= shift;
            }

            let width = left_space + advance + right_space;

            return LayoutBox {
                items: vec![LayoutItem::Glyph(LayoutGlyph {
                    glyph_id: glyph_id.0,
                    x: left_space,
                    y,
                    size: self.metrics.scaled_size(style),
                    ch: c,
                    part: NO_PART,
                })],
                width,
                ascent: ascent.max(0.0),
                descent: descent.max(0.0),
                italic: self.metrics.italic_correction(glyph_id, style),
                kern_glyph: Some(glyph_id),
            };
        }

        // Missing symbol glyph.
        LayoutBox {
            width: self.metrics.scaled_size(style) * 0.3,
            ..LayoutBox::empty()
        }
    }

    fn layout_frac(&self, num: &[MathNode], den: &[MathNode], style: MathStyle) -> LayoutBox {
        let num_style = match style {
            MathStyle::Display => MathStyle::Text,
            _ => style.script(),
        };
        let den_style = num_style;

        let num_box = self.sub("num", num, num_style);
        let den_box = self.sub("den", den, den_style);

        let rule_thickness = self.metrics.fraction_rule_thickness(style);
        let axis = self.metrics.axis_height(style);
        let num_shift = self.metrics.fraction_num_shift_up(style);
        let den_shift = self.metrics.fraction_den_shift_down(style);
        let num_gap_min = self.metrics.fraction_num_gap_min(style);
        let den_gap_min = self.metrics.fraction_den_gap_min(style);

        let total_width = num_box.width.max(den_box.width);
        let padding = self.metrics.scaled_size(style) * 0.1;
        let frac_width = total_width + padding * 2.0;

        // Position numerator centered above the rule
        let mut result = LayoutBox::empty();

        // Numerator: shift up so its bottom is num_gap_min above the rule
        let rule_y = axis; // rule sits on the math axis
        let num_bottom = rule_y + rule_thickness / 2.0 + num_gap_min;
        let num_y = -(num_shift.max(num_bottom + num_box.descent));
        let num_x = padding + (total_width - num_box.width) / 2.0;

        let mut num_items = num_box.clone();
        num_items.translate(num_x, num_y);
        result.items.extend(num_items.items);

        // Fraction rule
        result.items.push(LayoutItem::Rule(LayoutRule {
            x: 0.0,
            y: -axis - rule_thickness / 2.0,
            width: frac_width,
            height: rule_thickness,
            part: NO_PART,
        }));

        // Denominator: shift down so its top is den_gap_min below the rule
        let den_top = rule_thickness / 2.0 + den_gap_min - rule_y;
        let den_y = den_shift.max(den_top + den_box.ascent);
        let den_x = padding + (total_width - den_box.width) / 2.0;

        let mut den_items = den_box.clone();
        den_items.translate(den_x, den_y);
        result.items.extend(den_items.items);

        result.width = frac_width;
        result.ascent = (-num_y) + num_box.ascent;
        result.descent = den_y + den_box.descent;

        result
    }

    fn layout_superscript(&self, sup: &[MathNode], style: MathStyle) -> LayoutBox {
        let script_style = style.script();
        let sup_box = self.sub("sup", sup, script_style);

        let shift = if style.is_cramped() {
            self.metrics.superscript_shift_up_cramped(style)
        } else {
            self.metrics.superscript_shift_up(style)
        };

        let space_after = self.metrics.space_after_script(style);

        let mut result = LayoutBox::empty();
        let mut sup_items = sup_box.clone();
        sup_items.translate(0.0, -shift);
        result.items.extend(sup_items.items);
        result.width = sup_box.width + space_after;
        result.ascent = (shift + sup_box.ascent).max(0.0);
        result.descent = (sup_box.descent - shift).max(0.0);

        result
    }

    fn layout_subscript(&self, sub: &[MathNode], style: MathStyle) -> LayoutBox {
        let script_style = style.script();
        let sub_box = self.sub("sub", sub, script_style);

        let shift = self.metrics.subscript_shift_down(style);
        let space_after = self.metrics.space_after_script(style);

        let mut result = LayoutBox::empty();
        let mut sub_items = sub_box.clone();
        sub_items.translate(0.0, shift);
        result.items.extend(sub_items.items);
        result.width = sub_box.width + space_after;
        result.ascent = (sub_box.ascent - shift).max(0.0);
        result.descent = (shift + sub_box.descent).max(0.0);

        result
    }

    fn layout_sub_superscript(
        &self,
        sub: &[MathNode],
        sup: &[MathNode],
        style: MathStyle,
    ) -> LayoutBox {
        let script_style = style.script();
        let sup_box = self.sub("sup", sup, script_style);
        let sub_box = self.sub("sub", sub, script_style);

        let sup_shift = if style.is_cramped() {
            self.metrics.superscript_shift_up_cramped(style)
        } else {
            self.metrics.superscript_shift_up(style)
        };
        let sub_shift = self.metrics.subscript_shift_down(style);

        // Ensure minimum gap between sub top and sup bottom
        let gap = (sup_shift - sup_box.descent) - (sub_box.ascent - sub_shift);
        let min_gap = self.metrics.sub_sup_gap_min(style);
        let extra = if gap < min_gap {
            (min_gap - gap) / 2.0
        } else {
            0.0
        };

        let final_sup_shift = sup_shift + extra;
        let final_sub_shift = sub_shift + extra;

        let space_after = self.metrics.space_after_script(style);
        let width = sup_box.width.max(sub_box.width) + space_after;

        let mut result = LayoutBox::empty();

        // Superscript
        let mut sup_items = sup_box.clone();
        sup_items.translate(0.0, -final_sup_shift);
        result.items.extend(sup_items.items);

        // Subscript
        let mut sub_items = sub_box.clone();
        sub_items.translate(0.0, final_sub_shift);
        result.items.extend(sub_items.items);

        result.width = width;
        result.ascent = final_sup_shift + sup_box.ascent;
        result.descent = final_sub_shift + sub_box.descent;

        result
    }

    fn layout_sqrt(
        &self,
        index: Option<&[MathNode]>,
        content: &[MathNode],
        style: MathStyle,
    ) -> LayoutBox {
        let content_box = self.sub("radicand", content, style);

        let gap = self.metrics.radical_vertical_gap(style);
        let rule_thickness = self.metrics.radical_rule_thickness(style);
        let extra_ascender = self.metrics.radical_extra_ascender(style);

        // Total height the radical sign needs to cover
        let inner_height = content_box.ascent + content_box.descent + gap + rule_thickness;

        // Get radical glyph (√), grown (variant or assembly) to cover the radicand
        let radical_glyph = self.metrics.glyph_id('\u{221A}').unwrap_or(GlyphId(0));
        let radical = self.metrics.stretch_vertical(radical_glyph, '\u{221A}', inner_height, style);
        let radical_advance = radical.width;
        let radical_ascent = radical.ascent;
        let radical_descent = radical.descent;
        let radical_height = radical_ascent + radical_descent;

        // If the radical is taller than needed, add extra gap
        let actual_gap = if radical_height > inner_height {
            gap + (radical_height - inner_height)
        } else {
            gap
        };

        let mut result = LayoutBox::empty();

        // Place radical so its bottom aligns with the content's descent.
        // This keeps the radical hook attached and avoids visual "split" from the overbar.
        let radical_y = content_box.descent - radical_descent;
        let mut radical_items = radical;
        radical_items.translate(0.0, radical_y);
        result.items.extend(radical_items.items);

        // Horizontal rule above content
        let rule_y = -(content_box.ascent + actual_gap + rule_thickness);
        let rule_overlap = (self.metrics.scaled_size(style) * 0.03).max(rule_thickness * 0.75);
        let rule_x = (radical_advance - rule_overlap).max(0.0);
        result.items.push(LayoutItem::Rule(LayoutRule {
            x: rule_x,
            y: rule_y,
            width: content_box.width + (radical_advance - rule_x),
            height: rule_thickness,
            part: NO_PART,
        }));

        // Content
        let mut content_items = content_box.clone();
        content_items.translate(radical_advance, 0.0);
        result.items.extend(content_items.items);

        result.width = radical_advance + content_box.width;
        let radical_top = radical_y - radical_ascent;
        let radical_bottom = radical_y + radical_descent;
        result.ascent =
            (-radical_top).max(content_box.ascent + actual_gap + rule_thickness) + extra_ascender;
        result.descent = radical_bottom.max(content_box.descent);

        // Optional index (nth root)
        if let Some(index_nodes) = index {
            let index_style = MathStyle::ScriptScript;
            let index_box = self.sub("index", index_nodes, index_style);
            let raise_percent = self.metrics.radical_degree_bottom_raise_percent(style);
            let kern_before = self.metrics.radical_kern_before_degree(style);
            let kern_after = self.metrics.radical_kern_after_degree(style);

            let raise = (result.ascent + result.descent) * raise_percent - result.descent;

            let mut index_items = index_box.clone();
            index_items.translate(kern_before, -(raise + index_box.descent));
            let index_width = kern_before + index_box.width + kern_after;

            // Shift everything right to make room for index
            if index_width > 0.0 {
                result.translate(index_width.max(0.0), 0.0);
                result.width += index_width.max(0.0);
            }

            result.items.extend(index_items.items);
            result.ascent = result
                .ascent
                .max(raise + index_box.ascent + index_box.descent);
        }

        result
    }

    fn layout_accent(&self, kind: AccentKind, content: &[MathNode], style: MathStyle) -> LayoutBox {
        let content_box = self.sub("base", content, style);

        let accent_char = match kind {
            AccentKind::Hat => '\u{0302}',   // combining circumflex
            AccentKind::Bar => '\u{0304}',   // combining macron
            AccentKind::Tilde => '\u{0303}', // combining tilde
            AccentKind::Vec => '\u{20D7}',   // combining right arrow above
            AccentKind::Dot => '\u{0307}',   // combining dot above
            AccentKind::Ddot => '\u{0308}',  // combining diaeresis
            AccentKind::Acute => '\u{0301}', // combining acute
            AccentKind::Grave => '\u{0300}', // combining grave
            AccentKind::Check => '\u{030C}', // combining caron
            AccentKind::Breve => '\u{0306}', // combining breve
            AccentKind::WideHat => '\u{0302}',
            AccentKind::WideTilde => '\u{0303}',
        };

        // Try to find the accent glyph; if not found, use ^ as fallback
        let accent_glyph = self
            .metrics
            .glyph_id(accent_char)
            .or_else(|| self.metrics.glyph_id('^'));

        let mut result = content_box.clone();

        if let Some(glyph) = accent_glyph {
            let k = self.metrics.scaled_size(style) / self.metrics.upem;
            // The accent's ink is centered on the base's accent attachment point
            // (a lone letter's own, else the middle of the base).
            let ink = self
                .metrics
                .face
                .glyph_bounding_box(glyph)
                .map(|b| (b.x_min as f32 + b.x_max as f32) * 0.5 * k)
                .unwrap_or(0.0);
            let attach = match content {
                [MathNode::Char(c)] => map_math_italic_latin(*c)
                    .and_then(|mc| self.metrics.glyph_id(mc))
                    .or_else(|| self.metrics.glyph_id(*c))
                    .and_then(|g| self.metrics.top_accent_attachment(g, style)),
                _ => None,
            }
            .unwrap_or(content_box.width * 0.5);
            let accent_x = attach - ink;

            // The accent glyph is drawn for a base as tall as the font's accent base
            // height; a taller base lifts it.
            let lift = (content_box.ascent - self.metrics.accent_base_height(style)).max(0.0);
            let (accent_top, _) = self.metrics.glyph_bbox_height(glyph, style);

            result.items.push(LayoutItem::Glyph(LayoutGlyph {
                glyph_id: glyph.0,
                x: accent_x,
                y: -lift,
                size: self.metrics.scaled_size(style),
                ch: accent_char,
                part: NO_PART,
            }));

            result.ascent = content_box.ascent.max(accent_top + lift);
        }

        result
    }

    fn layout_left_right(
        &self,
        left: Delimiter,
        content: &[MathNode],
        right: Delimiter,
        style: MathStyle,
    ) -> LayoutBox {
        let content_box = self.sub("body", content, style);
        let axis = self.metrics.axis_height(style);
        let reach = (content_box.ascent - axis).max(content_box.descent + axis);
        // Delimiter size: cover 90.1 % of twice the reach about the axis, never
        // less than the content's height minus a small shortfall.
        let size = self.metrics.scaled_size(style);
        let target_height = (reach * 2.0 * 0.901).max(reach * 2.0 - size * 0.5);

        let left_box = self.layout_delimiter(left, target_height, true, style);
        let right_box = self.layout_delimiter(right, target_height, false, style);

        let mut result = LayoutBox::empty();
        result.append_right(&left_box);
        result.append_right(&content_box);
        result.append_right(&right_box);

        result
    }

    fn layout_delimiter(
        &self,
        delim: Delimiter,
        target_height: f32,
        is_left: bool,
        style: MathStyle,
    ) -> LayoutBox {
        if matches!(delim, Delimiter::None) {
            return LayoutBox::empty();
        }

        let c = delimiter_char(delim, is_left);
        if let Some(base_glyph) = self.metrics.glyph_id(c) {
            let mut grown = self.metrics.stretch_vertical(base_glyph, c, target_height, style);

            // Center the delimiter vertically around the math axis
            let axis = self.metrics.axis_height(style);
            let glyph_center = (grown.ascent - grown.descent) / 2.0;
            let shift = axis - glyph_center;
            grown.translate(0.0, -shift);
            grown.ascent += shift;
            grown.descent -= shift;
            grown
        } else {
            LayoutBox::empty()
        }
    }

    fn layout_matrix(
        &self,
        kind: MatrixKind,
        rows: &[Vec<Vec<MathNode>>],
        style: MathStyle,
    ) -> LayoutBox {
        let inner_style = match (style, kind) {
            (MathStyle::Display, MatrixKind::Aligned) => MathStyle::Display,
            (MathStyle::Display, _) => MathStyle::Text,
            (s, _) => s,
        };

        if rows.is_empty() {
            return LayoutBox::empty();
        }

        let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if num_cols == 0 {
            return LayoutBox::empty();
        }

        let aligned = kind == MatrixKind::Aligned;
        let outer_pad_x = if aligned { 0.0 } else { self.metrics.scaled_size(style) * 0.18 };
        // The space after each column: an aligned block packs its columns in
        // (right, left) pairs and spaces the pairs apart.
        let col_gaps: Vec<f32> = (0..num_cols)
            .map(|c| match (aligned, c % 2) {
                (true, 0) => 0.0,
                (true, _) => self.metrics.scaled_size(style),
                _ => self.metrics.scaled_size(style) * 0.7,
            })
            .collect();
        // How a cell sits in its column: 0 left, 0.5 centered, 1 right.
        let align_of = |c: usize| -> f32 {
            match kind {
                MatrixKind::Cases => 0.0,
                MatrixKind::Aligned => {
                    if c % 2 == 0 {
                        1.0
                    } else {
                        0.0
                    }
                }
                _ => 0.5,
            }
        };
        let outer_pad_y = self.metrics.scaled_size(style) * 0.18;
        let row_gap_y = self.metrics.scaled_size(style) * 0.32;

        // Layout all cells
        let mut cell_boxes: Vec<Vec<LayoutBox>> = Vec::new();
        let mut col_widths = vec![0.0f32; num_cols];
        let mut row_ascents = Vec::new();
        let mut row_descents = Vec::new();

        for (row_idx, row) in rows.iter().enumerate() {
            let mut row_boxes = Vec::new();
            let mut max_ascent = 0.0f32;
            let mut max_descent = 0.0f32;

            for (col_idx, cell) in row.iter().enumerate() {
                self.path.borrow_mut().push(PartSeg { role: "row", index: Some(row_idx as u16) });
                let cell_box = self.sub_at("col", Some(col_idx as u16), cell, inner_style);
                self.path.borrow_mut().pop();
                col_widths[col_idx] = col_widths[col_idx].max(cell_box.width);
                max_ascent = max_ascent.max(cell_box.ascent);
                max_descent = max_descent.max(cell_box.descent);
                row_boxes.push(cell_box);
            }

            // Pad with empty cells
            while row_boxes.len() < num_cols {
                row_boxes.push(LayoutBox::empty());
            }

            row_ascents.push(max_ascent);
            row_descents.push(max_descent);
            cell_boxes.push(row_boxes);
        }

        // Compute total dimensions
        let total_width: f32 = col_widths.iter().sum::<f32>()
            + outer_pad_x * 2.0
            + col_gaps.iter().take(num_cols.saturating_sub(1)).sum::<f32>();
        let total_height: f32 = row_ascents
            .iter()
            .zip(row_descents.iter())
            .map(|(a, d)| a + d)
            .sum::<f32>()
            + outer_pad_y * 2.0
            + row_gap_y * (rows.len().saturating_sub(1) as f32);

        // Center vertically on math axis
        let axis = self.metrics.axis_height(style);
        let matrix_ascent = total_height / 2.0 + axis;
        let matrix_descent = total_height / 2.0 - axis;

        let mut result = LayoutBox::empty();
        let mut y_offset = -matrix_ascent + outer_pad_y;

        for (row_idx, row_boxes) in cell_boxes.iter().enumerate() {
            let row_ascent = row_ascents[row_idx];
            let row_descent = row_descents[row_idx];
            y_offset += row_ascent;

            let mut x_offset = outer_pad_x;
            for (col_idx, cell_box) in row_boxes.iter().enumerate() {
                // Center cell within its column
                let cell_x = x_offset + (col_widths[col_idx] - cell_box.width) * align_of(col_idx);

                let mut cell_items = cell_box.clone();
                cell_items.translate(cell_x, y_offset);
                result.items.extend(cell_items.items);

                x_offset += col_widths[col_idx] + col_gaps[col_idx];
            }

            y_offset += row_descent + row_gap_y;
        }

        result.width = total_width;
        result.ascent = matrix_ascent;
        result.descent = matrix_descent;

        // Add delimiters for matrix types that have them
        let (left_delim, right_delim) = match kind {
            MatrixKind::Paren => (Delimiter::Paren, Delimiter::Paren),
            MatrixKind::Bracket => (Delimiter::Bracket, Delimiter::Bracket),
            MatrixKind::Brace => (Delimiter::Brace, Delimiter::Brace),
            MatrixKind::Vert => (Delimiter::Vert, Delimiter::Vert),
            MatrixKind::DoubleVert => (Delimiter::DoubleVert, Delimiter::DoubleVert),
            MatrixKind::Cases => (Delimiter::Brace, Delimiter::None),
            MatrixKind::Plain | MatrixKind::Aligned => return result,
        };

        let target = result.ascent + result.descent;
        let left_box = self.layout_delimiter(left_delim, target, true, style);
        let right_box = self.layout_delimiter(right_delim, target, false, style);

        // Shift matrix right by left delimiter width
        result.translate(left_box.width, 0.0);
        result.width += left_box.width;

        let mut final_result = LayoutBox::empty();
        final_result.items.extend(left_box.items);
        final_result.items.extend(result.items);

        // Right delimiter
        let mut right_items = right_box.clone();
        right_items.translate(result.width, 0.0);
        final_result.items.extend(right_items.items);

        final_result.width = result.width + right_box.width;
        final_result.ascent = result.ascent.max(left_box.ascent).max(right_box.ascent);
        final_result.descent = result.descent.max(left_box.descent).max(right_box.descent);

        final_result
    }

    fn layout_space(&self, width: SpaceWidth, style: MathStyle) -> LayoutBox {
        let size = self.metrics.scaled_size(style);
        let w = match width {
            SpaceWidth::Thin => size * 3.0 / 18.0,
            SpaceWidth::Medium => size * 4.0 / 18.0,
            SpaceWidth::Thick => size * 5.0 / 18.0,
            SpaceWidth::Quad => size,
            SpaceWidth::QQuad => size * 2.0,
            SpaceWidth::NegThin => -size * 3.0 / 18.0,
        };
        LayoutBox {
            width: w,
            ..LayoutBox::empty()
        }
    }

    fn layout_text(&self, text: &str, style: MathStyle) -> LayoutBox {
        let mut result = LayoutBox::empty();
        for c in text.chars() {
            let b = self.layout_upright_char(c, style);
            result.append_right(&b);
        }
        result
    }

    fn layout_operator(&self, name: &str, style: MathStyle) -> LayoutBox {
        // Operators are rendered in upright (roman) style with thin spacing on each side
        let thin = self.metrics.scaled_size(style) * 3.0 / 18.0;
        let mut result = LayoutBox {
            width: thin,
            ..LayoutBox::empty()
        };
        for c in name.chars() {
            let b = self.layout_upright_char(c, style);
            result.append_right(&b);
        }
        result.width += thin;
        result
    }

    fn layout_overline(&self, content: &[MathNode], style: MathStyle) -> LayoutBox {
        let content_box = self.sub("body", content, style);
        let gap = self.metrics.overbar_vertical_gap(style);
        let thickness = self.metrics.overbar_rule_thickness(style);
        let extra = self.metrics.overbar_extra_ascender(style);

        let mut result = content_box.clone();
        let rule_y = -(content_box.ascent + gap + thickness);

        result.items.push(LayoutItem::Rule(LayoutRule {
            x: 0.0,
            y: rule_y,
            width: content_box.width,
            height: thickness,
            part: NO_PART,
        }));

        result.ascent = content_box.ascent + gap + thickness + extra;
        result
    }

    fn layout_underline(&self, content: &[MathNode], style: MathStyle) -> LayoutBox {
        let content_box = self.sub("body", content, style);
        let gap = self.metrics.underbar_vertical_gap(style);
        let thickness = self.metrics.underbar_rule_thickness(style);
        let extra = self.metrics.underbar_extra_descender(style);

        let mut result = content_box.clone();
        let rule_y = content_box.descent + gap;

        result.items.push(LayoutItem::Rule(LayoutRule {
            x: 0.0,
            y: rule_y,
            width: content_box.width,
            height: thickness,
            part: NO_PART,
        }));

        result.descent = content_box.descent + gap + thickness + extra;
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BaseKind {
    Normal,
    /// An integral sign: scripts sit beside it, the sup at the top right and
    /// the sub tucked back by the italic correction.
    Integral,
    DisplayBigOperator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MathClass {
    Ordinary,
    Open,
    Close,
    Binary,
    Relation,
    Punctuation,
    LargeOperator,
}

fn base_kind_for_node(node: &MathNode, style: MathStyle) -> BaseKind {
    if let MathNode::Symbol(sym) = node {
        if is_integral(*sym) {
            return BaseKind::Integral;
        }
    }
    if matches!(style, MathStyle::Display) {
        match node {
            // Integrals take their limits beside them; the other big operators
            // take them above and below in display style.
            MathNode::Symbol(sym) if is_big_operator(*sym) && !is_integral(*sym) => {
                return BaseKind::DisplayBigOperator;
            }
            _ => {}
        }
    }
    if let MathNode::Id(_, content) = node {
        if content.len() == 1 {
            return base_kind_for_node(&content[0], style);
        }
    }
    // `\lim\limits`: above and below in any style; `\sum\nolimits`: beside.
    if let MathNode::Limits(limits, content) = node {
        return match (limits, content.first()) {
            (true, _) => BaseKind::DisplayBigOperator,
            (false, Some(first)) if base_kind_for_node(first, style) == BaseKind::Integral => BaseKind::Integral,
            (false, _) => BaseKind::Normal,
        };
    }
    // Operator names that take limits (lim, max, sup, det, ...): under and
    // over in display style, as the big operators do.
    if matches!(style, MathStyle::Display) {
        if let MathNode::OperatorName(name) = node {
            if matches!(name.as_str(), "lim" | "lim sup" | "lim inf" | "max" | "min" | "sup" | "inf" | "det" | "gcd" | "Pr") {
                return BaseKind::DisplayBigOperator;
            }
        }
    }
    BaseKind::Normal
}

/// Whether a script's base is one glyph (its ascender does not lift the script).
fn is_single_glyph(node: &MathNode) -> bool {
    match node {
        MathNode::Char(_) | MathNode::Symbol(_) => true,
        MathNode::Id(_, content) | MathNode::Group(content) | MathNode::MathVariant(_, content) | MathNode::Color(_, content) => content.len() == 1 && is_single_glyph(&content[0]),
        _ => false,
    }
}

fn is_integral(sym: Symbol) -> bool {
    matches!(sym, Symbol::Int | Symbol::Iint | Symbol::Iiint | Symbol::Oint)
}

fn is_big_operator(sym: Symbol) -> bool {
    matches!(
        sym,
        Symbol::Sum
            | Symbol::Prod
            | Symbol::Coprod
            | Symbol::Int
            | Symbol::Iint
            | Symbol::Iiint
            | Symbol::Oint
            | Symbol::Bigcup
            | Symbol::Bigcap
            | Symbol::Bigsqcup
            | Symbol::Bigvee
            | Symbol::Bigwedge
            | Symbol::Bigoplus
            | Symbol::Bigotimes
            | Symbol::Bigodot
    )
}

fn char_math_class(c: char) -> MathClass {
    match c {
        '(' | '[' => MathClass::Open,
        ')' | ']' => MathClass::Close,
        '=' | '<' | '>' => MathClass::Relation,
        '+' | '-' | '*' | '/' => MathClass::Binary,
        ',' | ';' | ':' => MathClass::Punctuation,
        _ => MathClass::Ordinary,
    }
}

fn symbol_math_class(sym: Symbol) -> MathClass {
    if is_big_operator(sym) {
        return MathClass::LargeOperator;
    }

    match sym {
        Symbol::Plus
        | Symbol::Minus
        | Symbol::Times
        | Symbol::Div
        | Symbol::Cdot
        | Symbol::Star
        | Symbol::Circ
        | Symbol::Bullet
        | Symbol::Diamond
        | Symbol::Pm
        | Symbol::Mp
        | Symbol::Ast
        | Symbol::Dagger
        | Symbol::Ddagger
        | Symbol::Setminus
        | Symbol::Wr
        | Symbol::Cap
        | Symbol::Cup
        | Symbol::Wedge
        | Symbol::Vee => MathClass::Binary,

        Symbol::Eq
        | Symbol::Neq
        | Symbol::Lt
        | Symbol::Gt
        | Symbol::Le
        | Symbol::Ge
        | Symbol::Leq
        | Symbol::Geq
        | Symbol::Ll
        | Symbol::Gg
        | Symbol::Prec
        | Symbol::Succ
        | Symbol::Preceq
        | Symbol::Succeq
        | Symbol::Sim
        | Symbol::Simeq
        | Symbol::Approx
        | Symbol::Cong
        | Symbol::Equiv
        | Symbol::Subset
        | Symbol::Supset
        | Symbol::Subseteq
        | Symbol::Supseteq
        | Symbol::In
        | Symbol::Ni
        | Symbol::Notin
        | Symbol::Propto
        | Symbol::Parallel
        | Symbol::Perp
        | Symbol::Mid
        | Symbol::Vdash
        | Symbol::Dashv
        | Symbol::Models
        | Symbol::LeftArrow
        | Symbol::RightArrow
        | Symbol::LeftRightArrow
        | Symbol::Uparrow
        | Symbol::Downarrow
        | Symbol::DoubleLeftArrow
        | Symbol::DoubleRightArrow
        | Symbol::DoubleLeftRightArrow
        | Symbol::Mapsto
        | Symbol::LongRightArrow
        | Symbol::LongLeftArrow
        | Symbol::LongLeftRightArrow
        | Symbol::Hookrightarrow
        | Symbol::Hookleftarrow
        | Symbol::Nearrow
        | Symbol::Searrow
        | Symbol::Nwarrow
        | Symbol::Swarrow => MathClass::Relation,

        Symbol::Comma | Symbol::Semicolon | Symbol::Colon => MathClass::Punctuation,
        _ => MathClass::Ordinary,
    }
}

fn spacing_for_class(class: MathClass, font_size: f32, style: MathStyle) -> (f32, f32) {
    let mu = font_size / 18.0;
    let script = matches!(style, MathStyle::Script | MathStyle::ScriptScript);
    match class {
        MathClass::Ordinary | MathClass::Open | MathClass::Close => (0.0, 0.0),
        // medium, thick and punctuation spaces are not set in script styles
        MathClass::Binary | MathClass::Relation | MathClass::Punctuation if script => (0.0, 0.0),
        MathClass::Binary => (4.0 * mu, 4.0 * mu),
        MathClass::Relation => (5.0 * mu, 5.0 * mu),
        MathClass::Punctuation => (0.0, 3.0 * mu),
        MathClass::LargeOperator => (2.0 * mu, 2.0 * mu),
    }
}

/// The class an atom is set in given its neighbours: a binary operator with
/// nothing to operate on to its left (list start, or after a binary operator,
/// relation, opening delimiter, punctuation or large operator) or nothing to
/// its right (list end, relation, closing delimiter, punctuation) is ordinary.
fn effective_class(class: MathClass, prev: Option<MathClass>, next: Option<MathClass>) -> MathClass {
    use MathClass::*;
    if class != Binary {
        return class;
    }
    let no_left = matches!(prev, None | Some(Binary | Relation | Open | Punctuation | LargeOperator));
    let no_right = matches!(next, None | Some(Relation | Close | Punctuation));
    if no_left || no_right {
        Ordinary
    } else {
        class
    }
}

/// The class of a node as an atom of a list; scripts and composite nodes are
/// ordinary.
fn node_class(node: &MathNode) -> MathClass {
    match node {
        MathNode::Char(c) => char_math_class(*c),
        MathNode::Symbol(sym) => symbol_math_class(*sym),
        MathNode::Id(_, content) | MathNode::Limits(_, content) if content.len() == 1 => node_class(&content[0]),
        _ => MathClass::Ordinary,
    }
}

fn delimiter_char(delim: Delimiter, is_left: bool) -> char {
    match (delim, is_left) {
        (Delimiter::Paren, true) => '(',
        (Delimiter::Paren, false) => ')',
        (Delimiter::Bracket, true) => '[',
        (Delimiter::Bracket, false) => ']',
        (Delimiter::Brace, true) => '{',
        (Delimiter::Brace, false) => '}',
        (Delimiter::Vert, _) => '|',
        (Delimiter::DoubleVert, _) => '\u{2016}',
        (Delimiter::Angle, true) => '\u{27E8}',
        (Delimiter::Angle, false) => '\u{27E9}',
        (Delimiter::Floor, true) => '\u{230A}',
        (Delimiter::Floor, false) => '\u{230B}',
        (Delimiter::Ceil, true) => '\u{2308}',
        (Delimiter::Ceil, false) => '\u{2309}',
        (Delimiter::None, _) => ' ', // invisible
    }
}

fn map_math_italic_latin(c: char) -> Option<char> {
    if c.is_ascii_uppercase() {
        let cp = 0x1D434 + (c as u32 - 'A' as u32);
        return char::from_u32(cp);
    }
    if c.is_ascii_lowercase() {
        if c == 'h' {
            // Unicode has no U+1D455; use Planck constant symbol as math italic h.
            return Some('\u{210E}');
        }
        let cp = 0x1D44E + (c as u32 - 'a' as u32);
        return char::from_u32(cp);
    }
    None
}

/// Layout a parsed LaTeX math expression using the given font.
///
/// Returns a `LayoutOutput` with positioned glyphs and rules.
/// The font must contain an OpenType MATH table for best results;
/// without one, fallback heuristics are used.
///
/// # Arguments
/// * `nodes` - Parsed AST from `parser::parse()`
/// * `font_data` - Raw font file bytes (OTF/TTF)
/// * `font_size` - Desired font size in output units
/// * `style` - Display or text style
pub fn layout(
    nodes: &[MathNode],
    font_data: &[u8],
    font_size: f32,
    style: MathStyle,
) -> Option<LayoutOutput> {
    let face = Face::parse(font_data, 0).ok()?;
    let engine = LayoutEngine::new(&face, font_size);
    let result = engine.layout_sequence(nodes, style);
    let mut result = result;
    // Whatever no node claimed (nothing, for a parsed formula) is the formula's own.
    engine.claim(&mut result);

    Some(LayoutOutput {
        items: result.items,
        parts: engine.parts.into_inner(),
        width: result.width,
        height: result.ascent + result.descent,
        ascent: result.ascent,
        descent: result.descent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    // Use the same font that math_view embeds
    const MATH_FONT: &[u8] = include_bytes!("../fonts/NewCMMath-Regular.otf");

    fn layout_expr(latex: &str) -> LayoutOutput {
        let nodes = parse(latex);
        layout(&nodes, MATH_FONT, 20.0, MathStyle::Display).expect("layout failed")
    }

    fn has_glyphs(output: &LayoutOutput) -> bool {
        output
            .items
            .iter()
            .any(|i| matches!(i, LayoutItem::Glyph(_)))
    }

    fn has_rules(output: &LayoutOutput) -> bool {
        output
            .items
            .iter()
            .any(|i| matches!(i, LayoutItem::Rule(_)))
    }

    fn count_glyphs(output: &LayoutOutput) -> usize {
        output
            .items
            .iter()
            .filter(|i| matches!(i, LayoutItem::Glyph(_)))
            .count()
    }

    #[test]
    fn test_simple_char() {
        let out = layout_expr("x");
        assert!(out.width > 0.0);
        assert!(out.ascent > 0.0);
        assert_eq!(count_glyphs(&out), 1);
    }

    #[test]
    fn test_multiple_chars() {
        let out = layout_expr("x+y");
        assert!(out.width > 0.0);
        assert_eq!(count_glyphs(&out), 3);
    }

    #[test]
    fn test_fraction_has_rule() {
        let out = layout_expr(r"\frac{a}{b}");
        assert!(has_rules(&out), "fraction should produce a rule (line)");
        assert!(has_glyphs(&out), "fraction should produce glyphs");
        assert!(out.ascent > 0.0);
        assert!(out.descent > 0.0);
    }

    #[test]
    fn test_fraction_dimensions() {
        let single = layout_expr("a");
        let frac = layout_expr(r"\frac{a}{b}");
        // Fraction should be taller than a single char
        assert!(
            frac.height > single.height,
            "frac height {} should be > single height {}",
            frac.height,
            single.height
        );
    }

    #[test]
    fn test_superscript_raises() {
        let out = layout_expr("x^2");
        assert!(count_glyphs(&out) >= 2, "should have at least x and 2");
        // The superscript glyph should be positioned above baseline
        let glyphs: Vec<_> = out
            .items
            .iter()
            .filter_map(|i| {
                if let LayoutItem::Glyph(g) = i {
                    Some(g)
                } else {
                    None
                }
            })
            .collect();
        // At least one glyph should have negative y (above baseline)
        assert!(
            glyphs.iter().any(|g| g.y < 0.0),
            "superscript should be above baseline"
        );
    }

    #[test]
    fn test_subscript_lowers() {
        let out = layout_expr("x_i");
        let glyphs: Vec<_> = out
            .items
            .iter()
            .filter_map(|i| {
                if let LayoutItem::Glyph(g) = i {
                    Some(g)
                } else {
                    None
                }
            })
            .collect();
        // At least one glyph should have positive y (below baseline)
        assert!(
            glyphs.iter().any(|g| g.y > 0.0),
            "subscript should be below baseline"
        );
    }

    #[test]
    fn test_sqrt() {
        let out = layout_expr(r"\sqrt{x}");
        assert!(has_glyphs(&out));
        assert!(has_rules(&out), "sqrt should have a horizontal rule");
    }

    #[test]
    fn test_greek_symbols() {
        let out = layout_expr(r"\alpha + \beta");
        assert!(count_glyphs(&out) >= 3);
        assert!(out.width > 0.0);
    }

    #[test]
    fn test_left_right_parens() {
        let inner = layout_expr("x");
        let delimited = layout_expr(r"\left(x\right)");
        assert!(
            delimited.width > inner.width,
            "delimited should be wider than content alone"
        );
    }

    #[test]
    fn test_matrix() {
        let out = layout_expr(r"\begin{pmatrix}a & b \\ c & d\end{pmatrix}");
        assert!(has_glyphs(&out));
        // Should have at least 4 content glyphs + 2 delimiter glyphs
        assert!(
            count_glyphs(&out) >= 4,
            "matrix should have at least 4 glyphs, got {}",
            count_glyphs(&out)
        );
    }

    #[test]
    fn test_spacing() {
        let no_space = layout_expr("ab");
        let with_space = layout_expr(r"a\quad b");
        assert!(
            with_space.width > no_space.width,
            "\\quad should add significant space"
        );
    }

    #[test]
    fn test_overline() {
        let plain = layout_expr("x");
        let overlined = layout_expr(r"\overline{x}");
        assert!(
            overlined.ascent > plain.ascent,
            "overline should increase ascent"
        );
        assert!(has_rules(&overlined));
    }

    #[test]
    fn test_nested_fractions() {
        let out = layout_expr(r"\frac{\frac{a}{b}}{c}");
        assert!(has_glyphs(&out));
        assert!(has_rules(&out));
        // Should have at least 2 rules (one for each fraction)
        let rule_count = out
            .items
            .iter()
            .filter(|i| matches!(i, LayoutItem::Rule(_)))
            .count();
        assert!(
            rule_count >= 2,
            "nested fractions should have at least 2 rules, got {}",
            rule_count
        );
    }

    #[test]
    fn test_complex_expression() {
        // Euler's identity: e^{i\pi} + 1 = 0
        let out = layout_expr(r"e^{i\pi} + 1 = 0");
        assert!(has_glyphs(&out));
        assert!(out.width > 0.0);
    }

    #[test]
    fn test_sum_with_limits() {
        let out = layout_expr(r"\sum_{i=1}^{n} x_i");
        assert!(has_glyphs(&out));
        assert!(out.width > 0.0);
    }

    #[test]
    fn test_display_vs_text_style() {
        let nodes = parse(r"\frac{a}{b}");
        let display = layout(&nodes, MATH_FONT, 20.0, MathStyle::Display).unwrap();
        let text = layout(&nodes, MATH_FONT, 20.0, MathStyle::Text).unwrap();
        // Display style fractions should be taller
        assert!(
            display.height >= text.height,
            "display height {} should be >= text height {}",
            display.height,
            text.height
        );
    }

    #[test]
    fn test_zero_width_for_empty() {
        let out = layout_expr("");
        assert_eq!(out.width, 0.0);
        assert_eq!(count_glyphs(&out), 0);
    }

    #[test]
    fn test_operator_name() {
        let out = layout_expr(r"\sin x");
        assert!(has_glyphs(&out));
        // sin produces 3 glyphs (s, i, n) + 1 for x, but with spacing
        assert!(
            count_glyphs(&out) >= 4,
            "should have at least 4 glyphs for sin x, got {}",
            count_glyphs(&out)
        );
    }

    #[test]
    fn test_accent_hat() {
        let plain = layout_expr("x");
        let accented = layout_expr(r"\hat{x}");
        assert!(
            accented.ascent >= plain.ascent,
            "accent should increase ascent"
        );
    }

    #[test]
    fn test_glyph_positions_monotonic_x() {
        // For a horizontal expression, glyph x positions should generally increase
        let out = layout_expr("abcdef");
        let glyphs: Vec<_> = out
            .items
            .iter()
            .filter_map(|i| {
                if let LayoutItem::Glyph(g) = i {
                    Some(g.x)
                } else {
                    None
                }
            })
            .collect();
        for i in 1..glyphs.len() {
            assert!(
                glyphs[i] > glyphs[i - 1],
                "glyph positions should increase: {} should be > {}",
                glyphs[i],
                glyphs[i - 1]
            );
        }
    }

    #[test]
    fn test_sub_superscript() {
        let out = layout_expr("x_i^2");
        let glyphs: Vec<_> = out
            .items
            .iter()
            .filter_map(|i| {
                if let LayoutItem::Glyph(g) = i {
                    Some(g)
                } else {
                    None
                }
            })
            .collect();
        // Should have x, i, 2 (at minimum)
        assert!(
            glyphs.len() >= 3,
            "should have >= 3 glyphs, got {}",
            glyphs.len()
        );
        // Should have both above and below baseline
        let has_above = glyphs.iter().any(|g| g.y < -0.1);
        let has_below = glyphs.iter().any(|g| g.y > 0.1);
        assert!(has_above, "should have glyph above baseline (superscript)");
        assert!(has_below, "should have glyph below baseline (subscript)");
    }

    #[test]
    fn test_integral() {
        let out = layout_expr(r"\int_0^1 f(x) dx");
        assert!(has_glyphs(&out));
        assert!(out.width > 0.0);
    }
}

/// One drawing command of a glyph outline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutlineSeg {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    /// Control point, end point.
    QuadTo(f32, f32, f32, f32),
    /// Two control points, end point.
    CurveTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// The outline of glyph `glyph_id` at `size` (the em, in the units of the
/// layout), y down, with the glyph origin at (0, 0): what a [`LayoutGlyph`]
/// is drawn as, placed at its `x`, `y`. `None` for a glyph without outline
/// (a space) or a font that does not parse.
pub fn glyph_outline(font_data: &[u8], glyph_id: u16, size: f32) -> Option<Vec<OutlineSeg>> {
    struct Builder {
        k: f32,
        out: Vec<OutlineSeg>,
    }
    impl ttf_parser::OutlineBuilder for Builder {
        fn move_to(&mut self, x: f32, y: f32) {
            self.out.push(OutlineSeg::MoveTo(x * self.k, -y * self.k));
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.out.push(OutlineSeg::LineTo(x * self.k, -y * self.k));
        }
        fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
            let k = self.k;
            self.out.push(OutlineSeg::QuadTo(x1 * k, -y1 * k, x * k, -y * k));
        }
        fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
            let k = self.k;
            self.out.push(OutlineSeg::CurveTo(x1 * k, -y1 * k, x2 * k, -y2 * k, x * k, -y * k));
        }
        fn close(&mut self) {
            self.out.push(OutlineSeg::Close);
        }
    }
    let face = Face::parse(font_data, 0).ok()?;
    let mut b = Builder { k: size / face.units_per_em() as f32, out: Vec::new() };
    face.outline_glyph(GlyphId(glyph_id), &mut b)?;
    Some(b.out)
}
