//! A numeric readout with stable digit advances: every digit is drawn in a
//! cell as wide as the widest digit, so a ticking stopwatch does not
//! jitter sideways (the shaper exposes no tabular figures). Separators
//! keep their natural width. Centred in its box on the ink.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.TabularLabelBase = #(TabularLabel::register_widget(vm))
    mod.widgets.TabularLabel = set_type_default() do mod.widgets.TabularLabelBase {
        width: Fill height: 68
        draw_text +: {text_style: theme.font_regular{font_size: 42} color: theme.color_text}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct TabularLabel {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    draw_text: DrawText,
    #[live]
    text: String,
    /// 0 = left, 0.5 = centred, 1 = right, within the box.
    #[live(0.5)]
    align_x: f64,
}

impl TabularLabel {
    pub fn set_text(&mut self, cx: &mut Cx, text: &str) {
        if self.text != text {
            self.text = text.to_string();
            self.redraw(cx);
        }
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    /// The ink the digits draw with (the theme's text role).
    pub fn ink(&self) -> Vec4f {
        self.draw_text.color
    }
}

impl Widget for TabularLabel {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        let text = self.text.clone();
        let Some(zero) = self.draw_text.prepare_single_line_run(cx, "0") else {
            cx.end_turtle_with_area(&mut self.area);
            return DrawStep::done();
        };
        let cell = zero.width_in_lpxs as f64;
        let ink = (zero.ascender_in_lpxs - zero.descender_in_lpxs) as f64;
        // Measure: digits take the cell, everything else its own width.
        let mut widths = Vec::with_capacity(text.chars().count());
        let mut total = 0.0;
        for ch in text.chars() {
            let w = if ch.is_ascii_digit() {
                cell
            } else {
                let s = ch.to_string();
                self.draw_text.prepare_single_line_run(cx, &s).map(|run| run.width_in_lpxs as f64).unwrap_or(0.0)
            };
            widths.push(w);
            total += w;
        }
        let mut x = r.pos.x + (r.size.x - total) * self.align_x;
        let y = r.pos.y + (r.size.y - ink) * 0.5;
        for (ch, w) in text.chars().zip(widths) {
            let s = ch.to_string();
            if ch.is_ascii_digit() {
                let glyph_w = self.draw_text.prepare_single_line_run(cx, &s).map(|run| run.width_in_lpxs as f64).unwrap_or(w);
                self.draw_text.draw_abs(cx, dvec2(x + (w - glyph_w) * 0.5, y), &s);
            } else {
                self.draw_text.draw_abs(cx, dvec2(x, y), &s);
            }
            x += w;
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
