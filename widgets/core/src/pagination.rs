//! The arithmetic behind a numbered pagination strip, ahead of the widget.
//!
//! A strip that shows every page number stops being readable once there are
//! more than a handful of them, and one that only shows "prev / next" loses
//! the reader's place in a list they were scanning. The answer both problems
//! share is the same free function every "numbered with ellipses" pager
//! needs and none of this repo's hand-rolled pagers have: always show a few
//! pages at each end, always show a few pages around the one the reader is
//! on, and fold whatever is left into a single mark rather than either
//! spelling it out or hiding where it went.
//!
//! This is a clean-room design from the observed behaviour of that kind of
//! control, not a port: no code or constants are taken from any reference
//! implementation.

/// One thing a numbered strip draws: a page, or a gap standing in for the
/// pages between two shown numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSlot {
    Page(usize),
    Ellipsis,
}

/// Which pages a numbered strip shows for a `total`-page list centred on
/// `current`, keeping `boundaries` pages fixed at each end and `siblings`
/// pages on each side of the current one.
///
/// Pages are 1-indexed; `total` below 1 and `current` outside `1..=total`
/// are both clamped rather than treated as errors, so a caller mid-load
/// (total not known yet, or a stale current from a list that just shrank)
/// still gets a sane single-page strip instead of an empty one or a panic.
///
/// A gap of exactly one hidden page is filled in rather than folded: an
/// ellipsis standing in for one page costs the same room as the page would
/// have and hides a real number for no gain. A gap of two or more folds to
/// one `Ellipsis`, however wide it is — the strip says "there is more here",
/// not how much.
pub fn page_window(total: usize, current: usize, siblings: usize, boundaries: usize) -> Vec<PageSlot> {
    let total = total.max(1);
    let current = current.clamp(1, total);

    // Every page this strip wants shown, for whatever reason: a boundary
    // cap, or the window around the current page. The same page can be
    // wanted twice (a small `total` makes every range overlap); sorting and
    // deduplicating below is what makes that harmless.
    let mut wanted = Vec::new();
    wanted.extend(1..=boundaries.min(total));
    let lo = current.saturating_sub(siblings).max(1);
    let hi = (current + siblings).min(total);
    wanted.extend(lo..=hi);
    wanted.extend((total.saturating_sub(boundaries.min(total)) + 1)..=total);
    wanted.sort_unstable();
    wanted.dedup();

    // Fill a single-page gap before folding: a page costs no more room than
    // the ellipsis that would have hidden it alone.
    let mut widened = Vec::with_capacity(wanted.len());
    for (i, &p) in wanted.iter().enumerate() {
        if i > 0 && p == wanted[i - 1] + 2 {
            widened.push(p - 1);
        }
        widened.push(p);
    }

    let mut slots = Vec::with_capacity(widened.len() + 2);
    for (i, &p) in widened.iter().enumerate() {
        if i > 0 && p > widened[i - 1] + 1 {
            slots.push(PageSlot::Ellipsis);
        }
        slots.push(PageSlot::Page(p));
    }
    slots
}


use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

/// What a strip reports: the page now being asked for, 1-indexed, the same
/// way `page_window` counts.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum PaginationAction {
    Changed(usize),
    #[default]
    None,
}

/// What one drawn rect answers to.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cell {
    Page(usize),
    /// The fold. Drawn, and never answers.
    Gap,
    Prev,
    Next,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawPageCellBase = #(DrawPageCell::script_component(vm))
    set_type_default() do #(DrawPageCell::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: #00000000
        radius: 4.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5 0.5 self.rect_size.x - 1.0 self.rect_size.y - 1.0 self.radius)
            sdf.fill(self.color)
            return sdf.result
        }
    }

    mod.widgets.PaginationBase = #(Pagination::register_widget(vm))

    /** A numbered strip for a list too long to show at once: the pages
     * around the one being read, the ends, and one mark for the rest. */
    mod.widgets.Pagination = set_type_default() do mod.widgets.PaginationBase{
        width: Fit
        height: 30.
        /** how many pages there are 1..9999 step 1 */
        total: 1
        /** which page is being read, counting from one 1..9999 step 1 */
        page: 1
        /** how many neighbours to show each side of it 0..4 step 1 */
        siblings: 1
        /** how many to keep pinned at each end 0..3 step 1 */
        boundaries: 1
        /** offer the two step marks at the ends 0..1 step 1 */
        can_step: true
        /** how wide one cell is 18..64 step 2 */
        cell_size: 30.
        /** the room between two cells 0..12 step 1 */
        cell_gap: 2.

        draw_cell +: {color: #00000000}
        draw_cell_hover +: {color: theme.color_surface_container_high}
        draw_cell_current +: {color: theme.color_primary}
        draw_text +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
        draw_text_current +: {
            color: theme.color_on_primary
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPageCell {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    radius: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Pagination {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The strip's own rect, so the widget has something to be hovered by
    /// and redrawn by: every cell is placed absolutely and leaves none.
    #[redraw]
    #[live]
    draw_bg: DrawPageCell,
    #[live]
    draw_cell: DrawPageCell,
    #[live]
    draw_cell_hover: DrawPageCell,
    #[live]
    draw_cell_current: DrawPageCell,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub draw_text_current: DrawText,

    #[live(1)]
    pub total: usize,
    #[live(1)]
    pub page: usize,
    #[live(1)]
    pub siblings: usize,
    #[live(1)]
    pub boundaries: usize,
    #[live(true)]
    pub can_step: bool,
    #[live(30.0)]
    pub cell_size: f64,
    #[live(2.0)]
    pub cell_gap: f64,

    #[rust]
    cells: Vec<(Rect, Cell)>,
    #[rust]
    hover: Option<Cell>,
    #[rust]
    area: Area,
}

impl Pagination {
    /// The page this strip is on, counting from one.
    pub fn page(&self) -> usize {
        self.page.clamp(1, self.total.max(1))
    }

    /// Turn to a page, if it is a different one and one that exists.
    pub fn set_page(&mut self, cx: &mut Cx, page: usize) {
        let page = page.clamp(1, self.total.max(1));
        if page != self.page {
            self.page = page;
            self.hover = None;
            self.redraw(cx);
        }
    }

    /// The page a cell answers with, or nothing for the fold and for a step
    /// that has nowhere left to go.
    fn answer(&self, cell: Cell) -> Option<usize> {
        let page = self.page();
        match cell {
            Cell::Page(p) => Some(p),
            Cell::Gap => None,
            Cell::Prev => (page > 1).then(|| page - 1),
            Cell::Next => (page < self.total.max(1)).then(|| page + 1),
        }
    }
}

impl Widget for Pagination {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let slots = page_window(self.total, self.page, self.siblings, self.boundaries);
        // Every cell is one size, so the strip's own width is known before
        // anything is drawn and a Fit strip can ask for exactly it.
        let count = slots.len() + if self.can_step { 2 } else { 0 };
        let step = self.cell_size + self.cell_gap;
        let natural = (count as f64 * step - self.cell_gap).max(0.0);
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(natural),
                other => other,
            },
            ..walk
        };
        self.draw_bg.begin(cx, walk, self.layout);
        let strip = cx.turtle().rect();
        self.cells.clear();

        let mut order: Vec<Cell> = Vec::with_capacity(count);
        if self.can_step {
            order.push(Cell::Prev);
        }
        order.extend(slots.iter().map(|slot| match slot {
            PageSlot::Page(p) => Cell::Page(*p),
            PageSlot::Ellipsis => Cell::Gap,
        }));
        if self.can_step {
            order.push(Cell::Next);
        }

        let page = self.page();
        let line = 14.0_f64.min(strip.size.y);
        let mut x = strip.pos.x;
        for cell in order {
            let rect = Rect {
                pos: dvec2(x, strip.pos.y),
                size: dvec2(self.cell_size, strip.size.y),
            };
            let current = matches!(cell, Cell::Page(p) if p == page);
            let live = self.answer(cell).is_some();
            if current {
                self.draw_cell_current.draw_abs(cx, rect);
            } else if live && self.hover == Some(cell) {
                self.draw_cell_hover.draw_abs(cx, rect);
            } else {
                self.draw_cell.draw_abs(cx, rect);
            }
            let label = match cell {
                Cell::Page(p) => p.to_string(),
                Cell::Gap => "\u{2026}".to_string(),
                Cell::Prev => "\u{2039}".to_string(),
                Cell::Next => "\u{203a}".to_string(),
            };
            let text_walk = Walk {
                abs_pos: Some(dvec2(x, strip.pos.y + (strip.size.y - line) * 0.5)),
                width: Size::Fixed(self.cell_size),
                height: Size::Fixed(line),
                ..Walk::default()
            };
            let mid = Align { x: 0.5, y: 0.5 };
            if current {
                self.draw_text_current.draw_walk(cx, text_walk, mid, &label);
            } else {
                self.draw_text.draw_walk(cx, text_walk, mid, &label);
            }
            self.cells.push((rect, cell));
            x += step;
        }

        self.draw_bg.end(cx);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let at = self
                    .cells
                    .iter()
                    .find(|(r, c)| r.contains(fe.abs) && self.answer(*c).is_some())
                    .map(|(_, c)| *c);
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
                let hit = self
                    .cells
                    .iter()
                    .find(|(r, _)| r.contains(fe.abs))
                    .map(|(_, c)| *c);
                if let Some(page) = hit.and_then(|cell| self.answer(cell)) {
                    let uid = self.uid;
                    self.set_page(cx, page);
                    cx.widget_action(uid, PaginationAction::Changed(page));
                }
            }
            _ => {}
        }
    }

    /// Which page of how many, so a test can read the strip in one line.
    fn text(&self) -> String {
        format!("{} of {}", self.page(), self.total.max(1))
    }
}

impl PaginationRef {
    pub fn page(&self) -> usize {
        self.borrow().map(|inner| inner.page()).unwrap_or(1)
    }

    pub fn set_page(&self, cx: &mut Cx, page: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_page(cx, page);
        }
    }

    pub fn set_total(&self, cx: &mut Cx, total: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.total = total.max(1);
            let page = inner.page();
            inner.set_page(cx, page);
            inner.redraw(cx);
        }
    }

    /// The page asked for this pass, if one was.
    pub fn changed(&self, actions: &Actions) -> Option<usize> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<PaginationAction>() {
            PaginationAction::Changed(page) => Some(page),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PageSlot::{Ellipsis, Page};

    /// Small enough that the boundaries and the window already cover every
    /// page between them: no ellipsis has anything to hide.
    #[test]
    fn a_short_list_shows_every_page() {
        assert_eq!(
            page_window(5, 3, 1, 1),
            vec![Page(1), Page(2), Page(3), Page(4), Page(5)]
        );
    }

    /// Near the start, the left boundary and the window overlap, so only
    /// the right side folds.
    #[test]
    fn near_the_start_only_the_far_side_folds() {
        assert_eq!(page_window(20, 1, 1, 1), vec![Page(1), Page(2), Ellipsis, Page(20)]);
    }

    /// Symmetric at the end.
    #[test]
    fn near_the_end_only_the_near_side_folds() {
        assert_eq!(page_window(20, 20, 1, 1), vec![Page(1), Ellipsis, Page(19), Page(20)]);
    }

    /// In the middle, both sides fold, and the current page sits in the
    /// centre of its own window.
    #[test]
    fn in_the_middle_both_sides_fold_around_the_current_page() {
        assert_eq!(
            page_window(20, 10, 1, 1),
            vec![Page(1), Ellipsis, Page(9), Page(10), Page(11), Ellipsis, Page(20)]
        );
    }

    /// A gap of exactly one page is filled in rather than folded: showing
    /// page 2 costs the same room as an ellipsis standing in for it alone.
    #[test]
    fn a_single_page_gap_is_shown_rather_than_folded() {
        assert_eq!(
            page_window(10, 4, 1, 1),
            vec![Page(1), Page(2), Page(3), Page(4), Page(5), Ellipsis, Page(10)]
        );
    }

    /// A zero or out-of-range total is clamped to a single sane page rather
    /// than treated as an error, since a caller mid-load has nothing better
    /// to hand it.
    #[test]
    fn a_degenerate_total_still_answers() {
        assert_eq!(page_window(0, 0, 1, 1), vec![Page(1)]);
        assert_eq!(page_window(1, 1, 1, 1), vec![Page(1)]);
    }

    /// A current page outside the list clamps into range instead of
    /// producing a window centred on a page that does not exist.
    #[test]
    fn a_current_page_past_the_end_clamps_into_range() {
        assert_eq!(page_window(5, 99, 1, 1), page_window(5, 5, 1, 1));
    }

    /// Zero siblings and zero boundaries still folds the whole middle into
    /// one ellipsis rather than a page's worth of nothing.
    #[test]
    fn zero_siblings_and_boundaries_still_fold_the_middle() {
        assert_eq!(page_window(10, 5, 0, 0), vec![Page(5)]);
    }
}
