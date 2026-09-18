//! How many columns a width holds, and what each of them gets.
//!
//! Why: two containers ask this and have to be given the same answer.
//! `Masonry` packs its children into the columns and `TileList` lays a
//! virtualised set out in rows of them, and a page that puts the two side by
//! side shows the seam the moment they divide one width two ways. This is
//! that arithmetic once. It is a module of its own, and not a corner of
//! either widget, so that neither widget's file is the other's dependency
//! and the next container to lay things out in equal columns has somewhere
//! to get it from that is not a widget.
//!
//! The rule. A count that was asked for wins at any width. With none asked
//! for, the count is as many columns of the least width as the width will
//! hold. Whichever way the count was arrived at, the columns then share the
//! whole width equally — so the least width is a floor on the COUNT and not
//! the width a column ends up with.
//!
//! It is pure: lengths in, lengths out, no `Cx`. Every rule has a unit test
//! and none of them needs a draw pass.
use crate::makepad_draw::*;

/// A ceiling on the column count. A container can be momentarily very wide
/// while a window is being dragged, and `columns` is a number somebody
/// types; neither should be able to ask for an allocation the size of the
/// number that came out.
pub(crate) const MAX_COLUMNS: usize = 64;

/// A length the layout can work with: negatives and non-numbers are a
/// property somebody is in the middle of editing, not a request.
pub(crate) fn finite(value: f64) -> f64 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

/// How many columns there are: the fixed count when one is given, otherwise
/// as many columns of `min_width` as `width` will hold.
///
/// `width` is optional because a container inside a `Fit` parent has no
/// width to answer with. There is nothing to divide then, so it is one
/// column.
pub(crate) fn column_count(width: Option<f64>, fixed: usize, min_width: f64, gap: f64) -> usize {
    if fixed > 0 {
        return fixed.min(MAX_COLUMNS);
    }
    let Some(width) = width else {
        return 1;
    };
    if min_width <= 0.0 || !width.is_finite() {
        return 1;
    }
    // n columns need n * min_width + (n - 1) * gap, so the count that fits
    // is (width + gap) / (min_width + gap) rounded down.
    let count = ((width + gap) / (min_width + gap)).floor();
    if !count.is_finite() {
        return 1;
    }
    // A negative or huge float saturates rather than wrapping on the cast,
    // and the clamp takes it from there.
    (count as usize).clamp(1, MAX_COLUMNS)
}

/// What one column gets once the gaps are paid for.
pub(crate) fn column_width(width: Option<f64>, count: usize, gap: f64, min_width: f64) -> f64 {
    let count = count.max(1);
    match width {
        Some(width) => ((width - gap * (count - 1) as f64) / count as f64).max(0.0),
        // With no width to divide, the only length in the room is the
        // minimum that was asked for.
        None => min_width.max(0.0),
    }
}

/// An item is widened to its column: columns of different widths are not
/// what either caller means by the word. Its height is left exactly as
/// authored, which is the entire point of a masonry and the reason a tile
/// list's rows are free to differ.
pub(crate) fn item_walk(mut walk: Walk, column_width: f64) -> Walk {
    walk.abs_pos = None;
    walk.width = Size::Fixed((column_width - walk.margin.width()).max(0.0));
    walk
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_column_count_comes_from_the_width_when_no_count_is_given() {
        // Three columns of 180 with 20 between them need 580; four need 780.
        assert_eq!(column_count(Some(580.0), 0, 180.0, 20.0), 3);
        assert_eq!(column_count(Some(579.0), 0, 180.0, 20.0), 2);
        assert_eq!(column_count(Some(640.0), 0, 180.0, 20.0), 3);
        assert_eq!(column_count(Some(780.0), 0, 180.0, 20.0), 4);
        // Narrower than a single column is still a single column, not none.
        assert_eq!(column_count(Some(20.0), 0, 180.0, 20.0), 1);
        assert_eq!(column_count(Some(0.0), 0, 180.0, 20.0), 1);
        // A count that was asked for outright wins, and is capped.
        assert_eq!(column_count(Some(640.0), 2, 180.0, 20.0), 2);
        assert_eq!(column_count(Some(640.0), usize::MAX, 180.0, 20.0), MAX_COLUMNS);
        // Nothing to measure against, so nothing to derive from.
        assert_eq!(column_count(None, 0, 180.0, 20.0), 1);
        assert_eq!(column_count(Some(640.0), 0, 0.0, 20.0), 1);
        assert_eq!(column_count(Some(f64::NAN), 0, 180.0, 20.0), 1);
    }

    #[test]
    fn the_columns_share_whatever_the_gaps_leave() {
        assert_eq!(column_width(Some(640.0), 3, 20.0, 180.0), 200.0);
        assert_eq!(column_width(Some(100.0), 1, 20.0, 180.0), 100.0);
        // Gaps wider than the container do not make a negative column.
        assert_eq!(column_width(Some(10.0), 4, 40.0, 180.0), 0.0);
        // With no width to divide, a column is the minimum that was asked
        // for — which is why an indefinite width needs one.
        assert_eq!(column_width(None, 3, 20.0, 180.0), 180.0);
        assert_eq!(column_width(Some(100.0), 0, 0.0, 180.0), 100.0);
    }

    #[test]
    fn an_item_is_widened_to_its_column_and_keeps_its_height() {
        let mut authored = Walk::new(Size::fill(), Size::Fixed(140.0));
        authored.abs_pos = Some(dvec2(900.0, 900.0));
        authored.margin = Inset { left: 4.0, right: 6.0, top: 0.0, bottom: 0.0 };
        let placed = item_walk(authored, 200.0);
        assert_eq!(placed.abs_pos, None);
        assert_eq!(placed.width, Size::Fixed(190.0));
        assert_eq!(placed.height, Size::Fixed(140.0));
        // A column narrower than the item's own margins is not a negative
        // width.
        assert_eq!(item_walk(authored, 2.0).width, Size::Fixed(0.0));
    }

    #[test]
    fn a_length_nobody_can_lay_out_is_read_as_nothing() {
        assert_eq!(finite(12.0), 12.0);
        assert_eq!(finite(-4.0), 0.0);
        assert_eq!(finite(f64::NAN), 0.0);
        assert_eq!(finite(f64::INFINITY), 0.0);
    }
}
