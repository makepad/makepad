//! Dividers: a rule between two things, with a word in it when the word
//! is what matters.
//!
//! `Hr` and `Vr` draw the theme's bevelled groove and are left alone; they
//! are the right line between two panels of chrome. Content wants a
//! quieter rule most of the time and a louder one now and then, and it
//! wants to say "or" between two sign-in paths, "today" above a run of
//! rows, "3 more" at the end of a list. That is this widget: one hairline
//! whose weight, style and inset are properties, which can carry a label
//! at its start, its middle or its end, and which lies either way.
//!
//! A label splits the rule into two segments. The segments are the same
//! quad drawn twice, so a dashed rule stays dashed on both sides of the
//! word and a brand rule stays brand. The segment lengths come from the
//! turtle: a `Fill` segment is a deferred fill the row settles once the
//! label has been measured, which is why a centred label lands in the
//! middle and an end label lands a short stub from the edge, whatever the
//! label says and whatever the font.
//!
//! The vertical form stretches to whatever its parent's cross axis gives
//! it, the way `Vr` does, so it can stand between two toolbar groups
//! without being told how tall the toolbar is. The inset keeps the rule off
//! the leading edge, or off both edges, so a list's separators line up with
//! the text after an avatar rather than cutting under it.

use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

/// Which way the rule lies.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum DividerAxis {
    #[pick]
    #[default]
    Horizontal,
    Vertical,
}

/// Where the label sits along the rule.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum DividerLabelAt {
    /// A short stub, the label, then the rest of the rule.
    Start,
    #[pick]
    #[default]
    Center,
    /// The rule, the label, then a short stub.
    End,
}

/// How the rule is drawn along its length.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum DividerStyle {
    #[pick]
    #[default]
    Solid,
    Dashed,
    Dotted,
}

/// How loud the rule is.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum DividerAppearance {
    /// The outline-variant colour: a rule you feel more than see.
    #[pick]
    #[default]
    Subtle,
    /// The outline colour.
    Strong,
    /// The accent colour.
    Brand,
}

/// Which ends are kept off the edge.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum DividerInset {
    #[pick]
    #[default]
    None,
    /// Off the leading edge only, for rules that line up with text after a leading column.
    Start,
    /// Off both edges.
    Middle,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawDividerBase = #(DrawDivider::script_component(vm))
    set_type_default() do #(DrawDivider::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.DividerAxis = #(DividerAxis::script_api(vm))
    mod.widgets.DividerLabelAt = #(DividerLabelAt::script_api(vm))
    mod.widgets.DividerStyle = #(DividerStyle::script_api(vm))
    mod.widgets.DividerAppearance = #(DividerAppearance::script_api(vm))
    mod.widgets.DividerInset = #(DividerInset::script_api(vm))

    mod.widgets.DividerBase = #(Divider::register_widget(vm))

    /** The divider: a hairline across its parent, with an optional label
     * that splits it in two. */
    mod.widgets.Divider = set_type_default() do mod.widgets.DividerBase{
        width: Fill
        height: Fit
        flow: Right
        /** gap between the rule and its label 0..24 step 1 */
        spacing: theme.space_2
        /** room above and below the rule */
        padding: Inset{top: theme.space_2, bottom: theme.space_2, left: 0., right: 0.}
        margin: 0.
        align: Align{x: 0.5, y: 0.5}

        /** the label; empty draws an unbroken rule. `text()` reports it */
        text: ""
        /** Horizontal or Vertical */
        axis: mod.widgets.DividerAxis.Horizontal
        /** where the label sits: Start, Center or End */
        label_at: mod.widgets.DividerLabelAt.Center
        /** Solid, Dashed or Dotted */
        style: mod.widgets.DividerStyle.Solid
        /** Subtle, Strong or Brand */
        appearance: mod.widgets.DividerAppearance.Subtle
        /** which ends stay off the edge: None, Start or Middle */
        inset: mod.widgets.DividerInset.None
        /** the rule's thickness in points 0.5..8 step 0.5 */
        thickness: theme.size_divider
        /** how far an inset end stays off the edge 0..64 step 1 */
        inset_size: 16.
        /** the stub beside a Start or End label 0..96 step 1 */
        stub_size: 24.

        /** The rule: a strip in the appearance's colour, cut into dashes
         * or dots along its length by the style. */
        draw_bg +: {
            /** the Subtle rule */
            color_subtle: uniform(theme.color_outline_variant)
            /** the Strong rule */
            color_strong: uniform(theme.color_outline)
            /** the Brand rule */
            color_brand: uniform(theme.color_makepad)
            /** dash length in points 1..24 step 1 */
            dash_size: uniform(6.0)
            /** gap between dashes in points 1..24 step 1 */
            gap_size: uniform(4.0)

            pixel: fn() {
                let mut color = self.color_subtle
                if self.appearance > 0.5 { color = self.color_strong }
                if self.appearance > 1.5 { color = self.color_brand }
                let p = self.pos * self.rect_size
                let mut along = p.x
                let mut thick = self.rect_size.y
                if self.axis > 0.5 {
                    along = p.y
                    thick = self.rect_size.x
                }
                let mut on = 1.0
                if self.style > 0.5 {
                    let period = self.dash_size + self.gap_size
                    if fract(along / period) * period > self.dash_size { on = 0.0 }
                }
                if self.style > 1.5 {
                    // Dots: one disc per two thicknesses, each as round as
                    // the rule is thick.
                    let sdf = Sdf2d.viewport(p)
                    let period = thick * 2.0
                    let cell = fract(along / period) * period
                    let r = thick * 0.5
                    if self.axis > 0.5 {
                        sdf.circle(r, p.y - cell + r, r)
                    } else {
                        sdf.circle(p.x - cell + r, r, r)
                    }
                    sdf.fill(color)
                    on = 0.0
                    color = sdf.result
                }
                // A solid or dashed rule is the quad itself, painted flat: a
                // distance field's edge band would fade a one-point rule to
                // half strength, and a subtle rule at half strength is gone.
                if on > 0.5 {
                    color = Pal.premul(color)
                } else {
                    if self.style < 1.5 { color = vec4(0.0, 0.0, 0.0, 0.0) }
                }
                return color
            }
        }

        /** The label's ink: a small bold word in the quiet text colour. */
        draw_text +: {
            /** the label typeface */
            text_style: theme.font_bold{
                /** label type size in points 6..32 step 0.5 */
                font_size: theme.type_label_m_size
            }
            /** label ink */
            color: theme.color_on_surface_variant
            /** center the ink, not the line box */
            ink_centered: true
        }
    }

    /** The vertical divider: stands as tall as its parent lets it. */
    mod.widgets.DividerVertical = mod.widgets.Divider{
        axis: mod.widgets.DividerAxis.Vertical
        width: Fit
        height: Fill
        flow: Down
        padding: Inset{top: 0., bottom: 0., left: theme.space_2, right: theme.space_2}
    }

    /** The labelled divider: an "or" in the middle of the rule. */
    mod.widgets.DividerLabelled = mod.widgets.Divider{
        text: "or"
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDivider {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    axis: f32,
    #[live]
    style: f32,
    #[live]
    appearance: f32,
}

/// A rule between two things, optionally carrying a label.
#[derive(Script, ScriptHook, Widget)]
pub struct Divider {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The whole widget, rule and label: what `/snap` and the pick see.
    #[redraw]
    #[area]
    area: Area,
    #[live]
    draw_bg: DrawDivider,
    #[live]
    draw_text: DrawText,

    #[live]
    pub text: String,
    #[live]
    pub axis: DividerAxis,
    #[live]
    pub label_at: DividerLabelAt,
    #[live]
    pub style: DividerStyle,
    #[live]
    pub appearance: DividerAppearance,
    #[live]
    pub inset: DividerInset,
    #[live(1.0)]
    pub thickness: f64,
    #[live(16.0)]
    pub inset_size: f64,
    #[live(24.0)]
    pub stub_size: f64,
}

impl Divider {
    fn horizontal(&self) -> bool {
        self.axis == DividerAxis::Horizontal
    }

    fn push_shader_state(&mut self) {
        self.draw_bg.axis = if self.horizontal() { 0.0 } else { 1.0 };
        self.draw_bg.style = match self.style {
            DividerStyle::Solid => 0.0,
            DividerStyle::Dashed => 1.0,
            DividerStyle::Dotted => 2.0,
        };
        self.draw_bg.appearance = match self.appearance {
            DividerAppearance::Subtle => 0.0,
            DividerAppearance::Strong => 1.0,
            DividerAppearance::Brand => 2.0,
        };
    }

    /// A segment's walk along the axis: `Fill` for the long part, a fixed
    /// stub beside a Start or End label.
    fn segment(&self, along: Size) -> Walk {
        let t = Size::Fixed(self.thickness.max(0.5));
        if self.horizontal() {
            Walk {
                width: along,
                height: t,
                ..Walk::default()
            }
        } else {
            Walk {
                width: t,
                height: along,
                ..Walk::default()
            }
        }
    }

    /// Draw one segment now, or defer it when it fills: a fill drawn at
    /// once would take the whole row and push the label off the edge.
    fn draw_segment(&mut self, cx: &mut Cx2d, walk: Walk) -> Option<DeferredWalk> {
        match cx.defer_walk_turtle(walk) {
            Some(deferred) => Some(deferred),
            None => {
                self.draw_bg.draw_walk(cx, walk);
                None
            }
        }
    }

    pub fn set_axis(&mut self, cx: &mut Cx, axis: DividerAxis) {
        if self.axis != axis {
            self.axis = axis;
            self.area.redraw(cx);
        }
    }

    pub fn set_style(&mut self, cx: &mut Cx, style: DividerStyle) {
        if self.style != style {
            self.style = style;
            self.area.redraw(cx);
        }
    }

    pub fn set_appearance(&mut self, cx: &mut Cx, appearance: DividerAppearance) {
        if self.appearance != appearance {
            self.appearance = appearance;
            self.area.redraw(cx);
        }
    }

    pub fn set_label_at(&mut self, cx: &mut Cx, label_at: DividerLabelAt) {
        if self.label_at != label_at {
            self.label_at = label_at;
            self.area.redraw(cx);
        }
    }

    pub fn set_inset(&mut self, cx: &mut Cx, inset: DividerInset) {
        if self.inset != inset {
            self.inset = inset;
            self.area.redraw(cx);
        }
    }
}

impl Widget for Divider {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.push_shader_state();
        let horizontal = self.horizontal();
        let mut layout = self.layout;
        layout.flow = if horizontal {
            Flow::right()
        } else {
            Flow::Down
        };
        // The inset is padding along the axis, so the segments and the
        // label share it without knowing about it.
        let (lead, trail) = match self.inset {
            DividerInset::None => (0.0, 0.0),
            DividerInset::Start => (self.inset_size, 0.0),
            DividerInset::Middle => (self.inset_size, self.inset_size),
        };
        if horizontal {
            layout.padding.left += lead;
            layout.padding.right += trail;
        } else {
            layout.padding.top += lead;
            layout.padding.bottom += trail;
        }
        cx.begin_turtle(walk, layout);
        if self.text.is_empty() {
            let segment = self.segment(Size::fill());
            let mut deferred = self.draw_segment(cx, segment);
            if let Some(deferred) = deferred.as_mut() {
                let resolved = deferred.resolve(cx);
                self.draw_bg.draw_walk(cx, resolved);
            }
        } else {
            let stub = Size::Fixed(self.stub_size.max(0.0));
            let (first, second) = match self.label_at {
                DividerLabelAt::Start => (stub, Size::fill()),
                DividerLabelAt::Center => (Size::fill(), Size::fill()),
                DividerLabelAt::End => (Size::fill(), stub),
            };
            let first = self.segment(first);
            let second = self.segment(second);
            let mut deferred_first = self.draw_segment(cx, first);
            self.draw_text
                .draw_walk(cx, Walk::fit(), Align::default(), &self.text);
            let mut deferred_second = self.draw_segment(cx, second);
            for deferred in [deferred_first.as_mut(), deferred_second.as_mut()] {
                if let Some(deferred) = deferred {
                    let resolved = deferred.resolve(cx);
                    self.draw_bg.draw_walk(cx, resolved);
                }
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text != v {
            self.text = v.to_string();
            self.area.redraw(cx);
        }
    }
}

impl DividerRef {
    pub fn set_axis(&self, cx: &mut Cx, axis: DividerAxis) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_axis(cx, axis);
        }
    }

    pub fn set_style(&self, cx: &mut Cx, style: DividerStyle) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_style(cx, style);
        }
    }

    pub fn set_appearance(&self, cx: &mut Cx, appearance: DividerAppearance) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_appearance(cx, appearance);
        }
    }

    pub fn set_label_at(&self, cx: &mut Cx, label_at: DividerLabelAt) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_label_at(cx, label_at);
        }
    }

    pub fn set_inset(&self, cx: &mut Cx, inset: DividerInset) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_inset(cx, inset);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn divider_is_registered_after_the_views() {
        let lib = include_str!("lib.rs");
        let divider = include_str!("divider.rs");
        assert!(lib.contains("pub mod divider;"));
        assert!(lib.contains("divider::*"));
        assert!(lib.contains("crate::divider::script_mod(vm);"));
        let views = lib.find("crate::view_ui::script_mod(vm);").unwrap();
        let here = lib.find("crate::divider::script_mod(vm);").unwrap();
        assert!(views < here, "Hr and Vr stay untouched and registered first");
        // Assembled at run time so this test's own text cannot satisfy them.
        let base = format!("mod.widgets.{} = #({}::register_widget(vm))", "DividerBase", "Divider");
        let one = format!("set_type_default() do mod.widgets.{}", "DividerBase");
        assert!(divider.contains(&base));
        assert_eq!(divider.matches(&one).count(), 1, "one type default per widget");
    }
}
