//! Breadcrumb — where you are, and every step back to the top.
//!
//! A trail has one rule that outranks everything else: **the last crumb is
//! where you are, and it must never be the one that disappears.** A trail
//! too long for its room has to drop something, and dropping the end tells
//! the reader where they came from while hiding where they got to, which is
//! the one thing the control exists to say.
//!
//! That rule is worth stating because both trails already in this repo break
//! it in different ways — one drops the leading ancestors with no mark at
//! all, so a deep path quietly lies about its own depth, and the other fills
//! from the root and stops at the first crumb that will not fit, so it is
//! the current folder that vanishes. The arithmetic below is one answer to
//! both, kept as a free function with its own tests because which end gets
//! dropped is the whole of the difference between a trail and a decoration.
//!
//! Note that `OverflowRow` (see `button_group.rs`) is NOT the helper for
//! this: it fills from the front and hides the tail, which is exactly
//! backwards here.

use crate::{badge::measure, makepad_derive_widget::*, makepad_draw::*, widget::*};

/// Which crumbs a trail shows when they do not all fit.
///
/// Read it as: draw `0..head`, then an ellipsis if `hidden` is not zero,
/// then `tail_from..len`. When everything fits, `head` is the whole length
/// and the other two say there is nothing to fold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BreadcrumbWindow {
    /// How many crumbs are shown at the start of the trail.
    pub head: usize,
    /// How many are folded into the ellipsis. Zero means no ellipsis.
    pub hidden: usize,
    /// The index the shown tail starts at.
    pub tail_from: usize,
}

/// Which crumbs of a trail fit, given each one's width, the `gap` between
/// them, the `room` there is, and how wide the ellipsis mark is.
///
/// The last crumb is always shown, even when it does not fit — a trail that
/// cannot say where you are has failed at its only job, and overrunning is
/// the honest way to fail. The root is kept when there is room for it,
/// because the top of the tree is the other end people navigate to; the
/// crumbs given up are the ones in the middle, which is what the ellipsis
/// then stands for.
pub fn breadcrumb_window(widths: &[f64], gap: f64, room: f64, ellipsis: f64) -> BreadcrumbWindow {
    let n = widths.len();
    if n == 0 {
        return BreadcrumbWindow { head: 0, hidden: 0, tail_from: 0 };
    }
    let gap = gap.max(0.0);
    let all: f64 = widths.iter().sum::<f64>() + gap * (n as f64 - 1.0);
    if all <= room || n == 1 {
        return BreadcrumbWindow { head: n, hidden: 0, tail_from: n };
    }

    // Start from where you are. That crumb is drawn whether or not it fits,
    // because a trail that cannot say where you are has failed at its only
    // job, and overrunning says so honestly.
    let last = n - 1;
    let ellipsis = ellipsis.max(0.0);
    let mut tail_from = last;
    let mut used = widths[last];

    // Keep the root only if it can be afforded alongside the mark and the
    // leaf. Squeezed harder than that it goes too — the top of the tree is
    // worth more than the middle, and less than where you are.
    let head = if n > 2 && used + gap + ellipsis + gap + widths[0] <= room {
        used += gap + ellipsis + gap + widths[0];
        1
    } else {
        used += gap + ellipsis;
        0
    };

    // Then fill backwards. Backwards is the whole point: after the current
    // crumb, the near ancestors are the ones worth the room.
    for i in (head..last).rev() {
        let step = widths[i] + gap;
        if used + step > room {
            break;
        }
        used += step;
        tail_from = i;
    }

    let hidden = tail_from.saturating_sub(head);
    if hidden == 0 {
        // Nothing folded after all — the room the ellipsis had reserved was
        // enough for the crumbs it would have stood for.
        return BreadcrumbWindow { head: n, hidden: 0, tail_from: n };
    }
    BreadcrumbWindow { head, hidden, tail_from }
}

/// The line box a crumb is drawn in — the same 14 points the tab strip
/// uses. A text run has no height of its own to centre by until it is laid
/// out, so the box is given one.
const LINE_HEIGHT: f64 = 14.0;

/// What a trail reports: the crumb that was picked, by its index in the
/// trail the host handed over.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum BreadcrumbAction {
    Picked(usize),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawCrumbGroundBase = #(DrawCrumbGround::script_component(vm))
    set_type_default() do #(DrawCrumbGround::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            // The trail's own rect, painted as nothing: every crumb is
            // drawn at an absolute position and so leaves the widget no
            // rect of its own to hover, to redraw, or to be found by.
            return vec4(0.0 0.0 0.0 0.0)
        }
    }

    mod.widgets.BreadcrumbBase = #(Breadcrumb::register_widget(vm))

    /** Where you are, and every step back to the top. Too long a trail
     * folds its middle into an ellipsis and never drops the end. */
    mod.widgets.Breadcrumb = set_type_default() do mod.widgets.BreadcrumbBase{
        width: Fit
        height: 22.
        /** what goes between two crumbs */
        separator: "\u{203a}"
        /** whether the crumb you are already on can be clicked 0..1 step 1 */
        current_interactive: false
        /** the room either side of a separator 0..24 step 1 */
        gap: 6.

        draw_text +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
        draw_text_current +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
        draw_text_hover +: {
            color: theme.color_text_hover
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCrumbGround {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Breadcrumb {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The trail's own rect, drawn as nothing, so hovering one crumb
    /// repaints the whole trail rather than the last word of it.
    #[redraw]
    #[live]
    draw_bg: DrawCrumbGround,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub draw_text_current: DrawText,
    #[live]
    pub draw_text_hover: DrawText,

    /// The trail, root first. The last one is where you are.
    #[live]
    pub trail: Vec<String>,
    #[live(">".to_string())]
    pub separator: String,
    /// Whether the crumb you are already on answers a click. Off by
    /// default: navigating to where you already are is not a thing to
    /// offer, and offering it is a bug two trails in this repo have.
    #[live]
    pub current_interactive: bool,
    #[live(6.0)]
    pub gap: f64,

    /// Where each shown crumb was drawn, and which trail index it is.
    #[rust]
    segments: Vec<(usize, Rect)>,
    #[rust]
    hover: Option<usize>,
    #[rust]
    area: Area,
}

impl Breadcrumb {
    pub fn set_trail(&mut self, cx: &mut Cx, trail: Vec<String>) {
        if self.trail != trail {
            self.trail = trail;
            self.hover = None;
            self.redraw(cx);
        }
    }

    /// Whether a crumb answers a click: every one but the current, unless
    /// the host has said otherwise.
    fn pickable(&self, index: usize) -> bool {
        self.current_interactive || index + 1 < self.trail.len()
    }
}

impl Widget for Breadcrumb {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let sep = format!(" {} ", self.separator);
        let sep_w = measure(&self.draw_text, cx, &sep);
        let widths: Vec<f64> = self
            .trail
            .iter()
            .map(|name| measure(&self.draw_text, cx, name))
            .collect();
        let ellipsis_w = measure(&self.draw_text, cx, "\u{2026}");

        // A Fit trail asks for exactly what it would draw uncollapsed, the
        // way SegmentedControl resolves its own Fit; the host decides
        // whether to grant it. apps/files depends on this: its crumb row is
        // Fit inside a Fill box precisely so a click in the leftover room
        // lands on the box and opens the editable path instead.
        let natural = widths.iter().sum::<f64>()
            + sep_w * (self.trail.len().saturating_sub(1)) as f64;
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(natural),
                other => other,
            },
            ..walk
        };
        self.draw_bg.begin(
            cx,
            walk,
            Layout {
                clip_x: true,
                ..self.layout
            },
        );
        let strip = cx.turtle().rect();
        // The room is whatever the turtle actually resolved to, which is the
        // only honest answer for a Fill trail.
        let window = breadcrumb_window(&widths, sep_w, strip.size.x, ellipsis_w);
        // A line box centred in the strip. The crumbs are placed absolutely,
        // and an absolute placement ignores a parent's `align`, so the trail
        // has to centre itself or it sits at the top of whatever it is given.
        let line = LINE_HEIGHT.min(strip.size.y);
        let text_y = strip.pos.y + (strip.size.y - line) * 0.5;
        let place = |x: f64, w: f64| Walk {
            abs_pos: Some(dvec2(x, text_y)),
            width: Size::Fixed(w),
            height: Size::Fixed(line),
            ..Walk::default()
        };
        let mid = Align { x: 0.0, y: 0.5 };
        self.segments.clear();

        let mut x = strip.pos.x;
        let mut first = true;
        let draw_one = |cx: &mut Cx2d, this: &mut Self, index: usize, x: &mut f64, first: &mut bool| {
            if !*first {
                this.draw_text.draw_walk(cx, place(*x, sep_w), mid, &sep);
                *x += sep_w;
            }
            *first = false;
            let name = this.trail[index].clone();
            let w = widths[index];
            let current = index + 1 == this.trail.len();
            let hovered = this.hover == Some(index) && this.pickable(index);
            let target = if hovered {
                &mut this.draw_text_hover
            } else if current {
                &mut this.draw_text_current
            } else {
                &mut this.draw_text
            };
            target.draw_walk(cx, place(*x, w), mid, &name);
            this.segments.push((index, Rect { pos: dvec2(*x, strip.pos.y), size: dvec2(w, strip.size.y) }));
            *x += w;
        };

        for i in 0..window.head.min(self.trail.len()) {
            draw_one(cx, self, i, &mut x, &mut first);
        }
        if window.hidden > 0 {
            if !first {
                self.draw_text.draw_walk(cx, place(x, sep_w), mid, &sep);
                x += sep_w;
            }
            first = false;
            self.draw_text.draw_walk(cx, place(x, ellipsis_w), mid, "\u{2026}");
            x += ellipsis_w;
        }
        for i in window.tail_from..self.trail.len() {
            draw_one(cx, self, i, &mut x, &mut first);
        }

        self.draw_bg.end(cx);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let at = self
                    .segments
                    .iter()
                    .find(|(i, r)| r.contains(fe.abs) && self.pickable(*i))
                    .map(|(i, _)| *i);
                if at != self.hover {
                    self.hover = at;
                    cx.set_cursor(if at.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Default
                    });
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(fe) => {
                if let Some((index, _)) = self
                    .segments
                    .iter()
                    .find(|(i, r)| r.contains(fe.abs) && self.pickable(*i))
                {
                    let (uid, index) = (self.uid, *index);
                    cx.widget_action(uid, BreadcrumbAction::Picked(index));
                }
            }
            _ => {}
        }
    }

    /// The trail as one line, so a test can read it without walking rects.
    fn text(&self) -> String {
        self.trail.join(&format!(" {} ", self.separator))
    }
}

impl BreadcrumbRef {
    pub fn set_trail(&self, cx: &mut Cx, trail: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_trail(cx, trail);
        }
    }

    /// Which crumb was picked this pass, by its index in the trail.
    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<BreadcrumbAction>() {
            BreadcrumbAction::Picked(index) => Some(index),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every crumb fits: nothing folds, and the window says so plainly.
    #[test]
    fn a_trail_that_fits_folds_nothing() {
        let w = breadcrumb_window(&[40.0, 40.0, 40.0], 10.0, 500.0, 12.0);
        assert_eq!(w, BreadcrumbWindow { head: 3, hidden: 0, tail_from: 3 });
    }

    /// One crumb is always the whole trail, however little room there is.
    #[test]
    fn one_crumb_never_folds() {
        let w = breadcrumb_window(&[400.0], 10.0, 10.0, 12.0);
        assert_eq!(w, BreadcrumbWindow { head: 1, hidden: 0, tail_from: 1 });
    }

    /// Nothing at all still answers.
    #[test]
    fn an_empty_trail_answers() {
        let w = breadcrumb_window(&[], 10.0, 100.0, 12.0);
        assert_eq!(w, BreadcrumbWindow { head: 0, hidden: 0, tail_from: 0 });
    }

    /// THE LAW: whatever is dropped, the crumb you are on is shown. This is
    /// the rule the treemap trail breaks today by filling from the root and
    /// stopping at the first crumb that will not fit.
    #[test]
    fn the_current_crumb_is_always_shown() {
        let widths = [60.0, 60.0, 60.0, 60.0, 60.0, 60.0, 60.0, 60.0];
        for room in [0.0, 10.0, 40.0, 80.0, 150.0, 300.0, 600.0, 5000.0] {
            let w = breadcrumb_window(&widths, 8.0, room, 12.0);
            let shown_tail = w.tail_from < widths.len();
            let shown_in_head = w.head >= widths.len();
            assert!(
                shown_tail || shown_in_head,
                "room {room}: the last crumb must be drawn, got {w:?}"
            );
        }
    }

    /// The middle is what goes, and the root stays while it can: a reader
    /// navigates to the top and to the near ancestors, not to the middle.
    #[test]
    fn the_middle_is_what_folds() {
        let widths = [50.0; 8];
        let w = breadcrumb_window(&widths, 8.0, 260.0, 12.0);
        assert_eq!(w.head, 1, "the root stays");
        assert!(w.hidden > 0, "something folded");
        assert!(w.tail_from > w.head, "the fold is in the middle, not at an end");
        assert!(w.tail_from < widths.len(), "and the near ancestors survive with it");
    }

    /// Squeezed to nothing, it gives up the root rather than the leaf.
    #[test]
    fn the_root_goes_before_the_leaf_does() {
        let widths = [50.0; 8];
        let w = breadcrumb_window(&widths, 8.0, 30.0, 12.0);
        assert_eq!(w.tail_from, 7, "only where you are is left");
        assert_eq!(w.head, 0, "even the root has gone");
        assert_eq!(w.hidden, 7);
    }

    /// With only a root and a leaf and no room for both, it is the leaf
    /// that stays: the root has nowhere to be but the ellipsis.
    #[test]
    fn a_two_crumb_trail_gives_up_the_root() {
        let w = breadcrumb_window(&[200.0, 200.0], 8.0, 100.0, 12.0);
        assert_eq!(w.head, 0, "no room to keep the root as well");
        assert_eq!(w.tail_from, 1, "the leaf is kept regardless");
        assert_eq!(w.hidden, 1);
    }
}
