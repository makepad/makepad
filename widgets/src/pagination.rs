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
