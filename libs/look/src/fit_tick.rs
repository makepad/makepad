//! A one-line tick that keeps its whole word: a name wider than the width
//! it is given steps its font down half a point at a time, to its
//! `min_font_size` at most, instead of being cut. The console's knob names
//! (a card's dials, whose names the effects write) wear it.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    // StageTick's look, one line, its font fitted to its width.
    mod.widgets.StageFitTickBase = #(StageFitTick::register_widget(vm))
    mod.widgets.StageFitTick = set_type_default() do mod.widgets.StageFitTickBase{
        width: Fill
        height: Fit
        padding: Inset{left: 2}
        text_inset: 2.0
        max_lines: 1
        flow: Flow.Right{wrap: false}
        min_font_size: 5.5
        draw_text +: {
            color: stage.secondary
            text_style: theme.font_bold{font_size: 8}
        }
    }
}

/// The largest font size, from `base` down in half points to `min`, at
/// which `measure` (the text's width at a size) fits `room`; `min` when
/// none does. A room that is not positive (not laid out yet) keeps `base`.
pub fn fit_font_size(base: f32, min: f32, room: f32, mut measure: impl FnMut(f32) -> f32) -> f32 {
    if !(room > 0.0) {
        return base;
    }
    let mut size = base;
    while size > min && measure(size) > room {
        size = (size - 0.5).max(min);
    }
    size
}

#[derive(Script, ScriptHook, Widget)]
pub struct StageFitTick {
    #[deref]
    label: Label,
    /// The smallest the font steps down to: a nine-letter name (DIRECTION)
    /// takes 5.5 points in a channel card's knob cell.
    #[live(5.5)]
    min_font_size: f32,
    /// The left padding the text starts after (the label's own, which it
    /// keeps to itself).
    #[live(2.0)]
    text_inset: f32,
    /// The size the style gives, and the size last fitted: a style applied
    /// again (a theme change) shows as a size that is not the fitted one.
    #[rust]
    sizes: Option<(f32, f32)>,
}

impl Widget for StageFitTick {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let current = self.label.draw_text.text_style.font_size;
        let base = match self.sizes {
            Some((base, fitted)) if fitted == current => base,
            _ => current,
        };
        let room = cx.peek_walk_turtle(walk).size.x as f32 - self.text_inset;
        let text = self.label.text();
        let draw_text = &mut self.label.draw_text;
        let size = fit_font_size(base, self.min_font_size.min(base), room, |size| {
            draw_text.text_style.font_size = size;
            draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), &text).size_in_lpxs.width
        });
        self.label.draw_text.text_style.font_size = size;
        self.sizes = Some((base, size));
        self.label.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.label.handle_event(cx, event, scope);
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.label.set_text(cx, v);
    }

    fn text(&self) -> String {
        self.label.text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A width per point of font: 6 points of width per point, so a word
    /// is 48 wide at 8 points, 42 at 7, 36 at 6.
    fn word(size: f32) -> f32 {
        size * 6.0
    }

    /// A word that fits keeps the style's size; one that does not steps
    /// down half a point at a time until it fits.
    #[test]
    fn a_word_steps_down_until_it_fits() {
        assert_eq!(fit_font_size(8.0, 6.0, 48.0, word), 8.0);
        assert_eq!(fit_font_size(8.0, 6.0, 45.0, word), 7.5);
        assert_eq!(fit_font_size(8.0, 6.0, 42.0, word), 7.0);
        assert_eq!(fit_font_size(8.0, 6.0, 40.0, word), 6.5);
    }

    /// Never below the floor, and no room yet (not laid out) keeps the
    /// style's size.
    #[test]
    fn the_floor_holds_and_an_unlaid_label_keeps_its_size() {
        assert_eq!(fit_font_size(8.0, 6.0, 20.0, word), 6.0);
        assert_eq!(fit_font_size(8.0, 6.0, 0.0, word), 8.0);
        assert_eq!(fit_font_size(8.0, 6.0, f32::NAN, word), 8.0);
        assert_eq!(fit_font_size(8.0, 6.0, -3.0, word), 8.0);
    }
}
