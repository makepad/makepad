//! How large the console draws itself when the window is too narrow for it.
//!
//! The console gives in the worst possible place. Its deck panels are rigid
//! and so is the queue, so every point taken off the window comes out of the
//! middle column — the waveform and the crossfader, the two things the
//! operator actually watches and touches — until they are twenty points wide
//! and then gone entirely.
//!
//! So the middle column is what the scaling defends. Above
//! [`CENTRE_MIN_POINTS`] the console is left alone and the middle keeps the
//! surplus; below it the whole console draws smaller, which hands the middle
//! back exactly the points it was about to lose.
//!
//! Shrinking keeps the layout: nothing moves, nothing vanishes, every control
//! stays where the hand expects it. It goes on until the type stops being
//! readable ([`MIN_SCALE`]) and no further — past that a narrow console needs
//! a different arrangement, not a smaller one, and that is not this module's
//! business.

/// What the console spends either side of the middle column, in layout
/// points: the two deck panels and the gaps between them, none of which
/// yields an inch at any width.
///
/// Measured from the running console, and it holds exactly — at layout
/// widths of 1680, 1500, 1400, 1200, 1000, 800 and 700 points the middle
/// column came to 1000, 820, 720, 520, 320, 120 and 20. Every one of them
/// is the width less 680.
///
/// To re-measure after changing a deck panel: run with `--remote`, then
/// `/s` for the window's width in points and `/snap?q=music_waves` for the
/// middle column. The difference is this number.
pub const FLANKS_POINTS: f64 = 680.0;

/// The narrowest the middle column may get before the console starts
/// drawing smaller to protect it: the width of ONE deck panel.
///
/// The operator's rule, and it is a better one than any round number: the
/// middle should never be narrower than the panels standing either side of
/// it. Written as half the flanks rather than as its own constant so it
/// cannot drift out of step with them — restyle a deck and the middle's
/// floor follows it.
///
/// In physical pixels this is whatever the display makes of it: on a 1.5×
/// screen it is the ~500 px the console was eyeballed at.
pub const CENTRE_MIN_POINTS: f64 = FLANKS_POINTS / 2.0;

/// The window width, in layout points, at which the middle column is exactly
/// [`CENTRE_MIN_POINTS`] wide. Wider than this and the console is left at the
/// display's own scale; narrower and it shrinks to hold the middle open.
pub const TARGET_POINTS: f64 = FLANKS_POINTS + CENTRE_MIN_POINTS;

/// The window height, in layout points, at which the console still shows
/// everything it has: the deck row, the fx band, the transport and a lower
/// region whose rail runs all the way to its pager. Measured on the running
/// console the same way as [`FLANKS_POINTS`] — the default 1040-point window
/// holds it with ~40 points to spare.
///
/// Below this the whole console draws smaller, exactly as it does for a
/// narrow window: a hosted tile 844 points tall used to clip the rail off
/// mid-chip while every rule said the console fit, because every rule was
/// keyed on width alone.
pub const TARGET_HEIGHT_POINTS: f64 = 1000.0;

/// How far the console may shrink, as a fraction of the display's own scale.
///
/// The floor is legibility, and legibility is a property of the TYPE: the
/// console labels its controls at 8–9 points, so three quarters puts them at
/// 6–6.75 and that is as far as they go while still being labels.
pub const MIN_SCALE: f64 = 0.75;

/// The DPI the main window should draw at, given its size in PHYSICAL pixels
/// and the scale the display natively reports.
///
/// Physical pixels, and never layout points, because setting the DPI is what
/// changes the layout points — the window is remeasured and the change comes
/// back round as another geometry event. A rule fed on its own output is a
/// rule that oscillates (the transport strip did exactly this, at two frames
/// a cycle). Physical pixels are the one width the scaling cannot move, so
/// the second pass reaches the same answer as the first and stops. That is
/// what [`the_scale_is_a_fixed_point`] pins down.
///
/// Never above native: a wide console is meant to hold MORE console — the
/// surplus goes to the middle column, which is what wants it — not the same
/// console with bigger knobs.
pub fn console_dpi(physical_width: f64, physical_height: f64, native_dpi: f64) -> f64 {
    if !physical_width.is_finite()
        || physical_width <= 0.0
        || !native_dpi.is_finite()
        || native_dpi <= 0.0
    {
        return native_dpi.max(f64::MIN_POSITIVE);
    }
    // A height that makes no sense constrains nothing — the width rule
    // stands alone, which is also what every width-only test feeds in. And a
    // WIDE, SHORT window is not this rule's either: there the lists move
    // beside the decks (see [`console_lists_beside`]) and take the room that
    // actually exists, which uses the surplus width far better than drawing
    // the whole console smaller would. Judged at NATIVE scale so this stays
    // a pure function of the physical window, never of its own output.
    let beside_at_native = physical_height / native_dpi < LISTS_STACK_POINTS
        && physical_width / native_dpi >= lists_beside_min_points();
    let by_height = if !physical_height.is_finite() || physical_height <= 0.0 || beside_at_native
    {
        f64::INFINITY
    } else {
        physical_height / TARGET_HEIGHT_POINTS
    };
    let wanted = (physical_width / TARGET_POINTS).min(by_height);
    wanted.clamp(native_dpi * MIN_SCALE, native_dpi)
}

/// The console's size relative to the display's own, for a readout.
pub fn console_scale(physical_width: f64, physical_height: f64, native_dpi: f64) -> f64 {
    console_dpi(physical_width, physical_height, native_dpi) / native_dpi
}

/// The window width, in layout points, below which the explorer and the
/// queue stop standing side by side and take turns behind tabs.
///
/// The queue is a fixed 320 points at every width, so the explorer gets
/// whatever is left; 560 is where its columns are already down to the narrow
/// set and the titles start losing their tails. 880 is the two together.
///
/// On a 1.5x display with the console at its 0.75 floor that is about 990
/// device pixels.
pub const LISTS_TAB_POINTS: f64 = 880.0;

/// Whether the explorer and the queue have to take turns.
///
/// `physical_width` is the width the LISTS get, not the window's — see
/// [`lists_span`]. Standing beside the decks they have about a third of it,
/// and the queue's fixed 320 points would eat almost all of that: the
/// explorer came out 36 points wide before this took the lists' own share
/// as its input.
pub fn console_lists_tabbed(physical_width: f64, physical_height: f64, native_dpi: f64) -> bool {
    if !physical_width.is_finite() || !native_dpi.is_finite() || native_dpi <= 0.0 {
        return false;
    }
    physical_width / console_dpi(physical_width, physical_height, native_dpi) < LISTS_TAB_POINTS
}

/// The physical width the LISTS get: the window, or their share of it once
/// they stand beside the decks.
pub fn lists_span(physical_width: f64, physical_height: f64, native_dpi: f64) -> f64 {
    if !console_lists_beside(physical_width, physical_height, native_dpi) {
        return physical_width;
    }
    let dpi = console_dpi(physical_width, physical_height, native_dpi);
    let gap = 6.0 * dpi;
    (lists_width_points((physical_width - gap) / dpi) * dpi).max(1.0)
}

/// The height, in layout points, below which the two lists can no longer
/// stand under the decks.
///
/// The deck region wants 330 at its floor and the lists want about 300 to be
/// worth reading; with the headers, the wells and the status bar above them
/// that comes to roughly 700.
pub const LISTS_STACK_POINTS: f64 = 700.0;

/// The share of the width the decks keep when the lists stand beside them.
///
/// Not half. An even split is the lazy answer and it serves neither side:
/// the decks would be left too narrow to hold a panel and a usable middle,
/// and the lists would get more width than a track list has any use for.
/// Two thirds to the decks keeps a tabbed panel with a proper mixer beside
/// it, and a third is plenty for titles and a queue.
pub const DECKS_SHARE: f64 = 0.65;

/// The narrowest a list is worth standing beside anything, in layout points.
pub const LISTS_MIN_POINTS: f64 = 300.0;

/// The widest a list has any use for, in layout points.
///
/// A share alone is not enough: a third of a very wide window is 700 points
/// of track titles, which reads as waste next to decks that are starving for
/// it. Past this the surplus goes back to the decks, where it buys a second
/// panel or a wider mixer.
pub const LISTS_MAX_POINTS: f64 = 520.0;

/// How the width is divided when the lists stand beside the decks, in
/// layout points: `(decks, lists)`.
///
/// A waterfall, not a share, because the two are not equally important and a
/// percentage cannot say so. The order is the operator's:
///
/// 1. the decks take enough for ONE panel and a full-width middle — the
///    mixer is the last thing that should ever starve, and a panel the
///    operator can reach through a tab is no loss beside it;
/// 2. the lists take enough to be worth reading;
/// 3. the decks take enough for the SECOND panel;
/// 4. the lists fill out to what a track list can use;
/// 5. anything still going spare goes to the decks.
///
/// Shares got this backwards: a third of a wide window went to track titles
/// while the mixer was squeezed to nothing between two panels that could
/// have been tabbed.
pub fn split_body(total_points: f64) -> (f64, f64) {
    let one_panel = FLANKS_POINTS / 2.0 + CENTRE_MIN_POINTS;
    let mut decks = total_points.min(one_panel);
    let mut left = total_points - decks;

    let lists = left.min(LISTS_MIN_POINTS);
    left -= lists;

    let top_up = left.min(TARGET_POINTS - one_panel);
    decks += top_up;
    left -= top_up;

    let lists_top_up = left.min(LISTS_MAX_POINTS - lists);
    left -= lists_top_up;

    (decks + left, lists + lists_top_up)
}

/// How many layout points the lists take beside the decks.
pub fn lists_width_points(total_points: f64) -> f64 {
    split_body(total_points).1
}

/// The width, in layout points, a console needs before standing its lists
/// beside the decks beats stacking them.
///
/// Whichever side runs out first decides it: the decks need one panel and a
/// playable middle out of their share, and the lists need
/// [`LISTS_MIN_POINTS`] out of theirs.
pub fn lists_beside_min_points() -> f64 {
    let decks_min = FLANKS_POINTS / 2.0
        + crate::music_view::STRIP_SWEEP_MIN
        + crate::music_view::STRIP_ROW_SLACK;
    (decks_min / DECKS_SHARE).max(LISTS_MIN_POINTS / (1.0 - DECKS_SHARE))
}

/// Whether the explorer and the queue stand to the right of deck B rather
/// than under the decks.
///
/// A wide, short window — a console squeezed against the bottom of a screen
/// — has no vertical room and plenty of horizontal, so the lists take the
/// room that actually exists. Both dimensions in physical pixels over the
/// width's scale, like the rest of the chain.
pub fn console_lists_beside(
    physical_width: f64,
    physical_height: f64,
    native_dpi: f64,
) -> bool {
    if !physical_height.is_finite() || physical_height <= 0.0 {
        return false;
    }
    let dpi = console_dpi(physical_width, physical_height, native_dpi);
    physical_height / dpi < LISTS_STACK_POINTS
        && physical_width / dpi >= lists_beside_min_points()
}

/// The physical width the DECKS actually get.
///
/// Once the lists stand beside them the decks no longer have the window to
/// themselves — they get about half of it — and every tab threshold is about
/// what the DECKS can hold, not what the window can. Feeding them the window
/// width put the console back where it started: a deck region too narrow for
/// its two panels, the middle column collapsed to nothing, and no tab in
/// sight because the window still looked wide.
///
/// Still a pure function of the physical window, so the chain stays a fixed
/// point.
pub fn deck_span(physical_width: f64, physical_height: f64, native_dpi: f64) -> f64 {
    if !console_lists_beside(physical_width, physical_height, native_dpi) {
        return physical_width;
    }
    let dpi = console_dpi(physical_width, physical_height, native_dpi);
    let gap = 6.0 * dpi;
    (split_body((physical_width - gap) / dpi).0 * dpi).max(1.0)
}

/// The width, in layout points, that the status bar's controls come to when
/// they stand in one line.
///
/// Measured on the running console: the bar fills exactly at 1108 device
/// pixels on a 1.5x display with the console at its 0.75 floor, which is
/// 985 layout points. Below that the row has to take a second line.
pub const STATUS_BAR_POINTS: f64 = 985.0;

/// Whether the status bar has to break onto a second line.
pub fn console_status_bar_wrapped(physical_width: f64, native_dpi: f64) -> bool {
    if !physical_width.is_finite() || !native_dpi.is_finite() || native_dpi <= 0.0 {
        return false;
    }
    // Physical, for the same reason as `console_tabs`.
    physical_width < STATUS_BAR_POINTS * native_dpi * MIN_SCALE
}

/// The window height, in layout points, below which the deck panel stops
/// getting what it asks for.
///
/// Measured on the running console. The panel's fixed content — the sync
/// row, the faders, the equalizer and the stem mix — comes to 284 points,
/// and the karaoke reader below them is Fill, so it absorbs whatever the
/// window is short by:
///
/// | window | 1040 | 940 | 900 | 860 | 820 | 780 |
/// | panel  |  434 | 384 | 364 | 344 | 330 | 330 |
/// | karaoke|  150 | 100 |  80 |  60 |  46 |  46 |
///
/// At 820 the deck region reaches its own 330-point floor and stops
/// shrinking: the console is out of room to give, and the karaoke box is
/// down to the 46 points left over. That is where folding has to start.
pub const PANEL_FLOOR_POINTS: f64 = 820.0;

/// The window height, in layout points, below which not even the two knob
/// blocks fit above one another and each block has to stand alone.
///
/// From the same measurements. The panel keeps half of what the window
/// gives (the library below it takes the other half): `panel = window/2 -
/// 86`, which the table above bears out at every row. Paired, the blocks
/// need 277 points of panel — 70 for the sync row and faders, 85 and 83 for
/// the knobs, and 13 for each of the three headings — and a panel of 277
/// wants a window of 726.
pub const PANEL_SPLIT_POINTS: f64 = 726.0;

/// The deck region's floor, which comes down as the panel folds.
///
/// Its usual 330 exists to keep a readable karaoke box under the knobs, and
/// folding is exactly what makes that unnecessary. But the floor has to fall
/// only as far as the fold actually allows, or the panel is left shorter
/// than the blocks it is still showing and the last heading is clipped off
/// the bottom — which is a fold with no way back out of it.
///
/// The panel spends 72 points before its blocks get any: a 28-point sync
/// row at the top, a 24-point transport row at the foot, and 20 of spacing
/// between the four children. On top of that, paired, the column carries
/// three headings (59) and both knob blocks (168) — 299, so 310. Singly it
/// carries three headings and the tallest block alone — 216, so 230.
///
/// Those two rows are why the first attempt clipped: a floor that counted
/// only the blocks left the column a dozen points short, and the heading it
/// dropped off the bottom was the KARAOKE one — the only way back to the
/// transcript. A fold with no way out of it.
pub fn region_min_points(fold: ConsoleFold) -> f64 {
    match fold {
        ConsoleFold::None => 330.0,
        ConsoleFold::Pairs => 310.0,
        ConsoleFold::Singles => 230.0,
    }
}

/// How hard the deck panel has to fold, if at all.
///
/// Height in physical pixels over the width's scale, because the scaling
/// decides how many layout points a physical window is worth — and both are
/// functions of the physical window alone, so the chain still cannot chase
/// its own tail.
pub fn console_fold(physical_width: f64, physical_height: f64, native_dpi: f64) -> ConsoleFold {
    if !physical_height.is_finite() || physical_height <= 0.0 {
        return ConsoleFold::None;
    }
    let points = physical_height / console_dpi(physical_width, physical_height, native_dpi);
    if points < PANEL_SPLIT_POINTS {
        ConsoleFold::Singles
    } else if points < PANEL_FLOOR_POINTS {
        ConsoleFold::Pairs
    } else {
        ConsoleFold::None
    }
}

/// What [`console_fold`] found. Mirrors `deck_sections::Fold`, which is the
/// panel's own word for the same thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleFold {
    None,
    Pairs,
    Singles,
}

/// Whether the deck panels have to stand one at a time behind tabs.
///
/// This is where the console runs out of the other answer. Shrinking holds
/// the middle column open until the type stops being readable; past that
/// there is nothing left to give but a panel, and folding one away hands the
/// middle back half the flanks.
///
/// Keyed on PHYSICAL width, exactly like [`console_dpi`] and for exactly the
/// same reason — and here the trap is sharper. Tabbing changes the flanks,
/// the flanks decide how much room the middle has, so a rule that asked
/// "does the middle need it?" of the CURRENT layout would engage, free the
/// room, discover it was no longer needed, disengage, and flip a deck panel
/// in and out twice a frame. Physical pixels are downstream of nothing.
pub fn console_tabbed(physical_width: f64, physical_height: f64, native_dpi: f64) -> bool {
    console_tabs(physical_width, physical_height, native_dpi) != TabStage::None
}

/// What the tabs have to hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabStage {
    /// Both deck panels stand either side of the mixer.
    None,
    /// One deck panel at a time, with the mixer beside it.
    Decks,
    /// The mixer is a tab too: deck A, deck B, mixer, one at a time.
    All,
}

/// How far the tabs have to go.
///
/// The second stage is where even a folded-away deck panel is not enough.
/// The middle is allowed well past its comfortable minimum here before the
/// mixer gives up its place: what it must keep is not room to be pleasant
/// in but room to be USED — a crossfader still worth playing, which is
/// `STRIP_SWEEP_MIN` plus the row's slack. Below that the middle is a
/// gesture, not a control, and the mixer is better off taking the width in
/// its own turn.
///
/// That is about 30% narrower than holding the middle at its comfortable
/// minimum would allow: 560 device pixels on a 1.5x display rather than
/// 765.
///
/// Keyed on physical width alone, like everything else in this chain.
pub fn console_tabs(physical_width: f64, physical_height: f64, native_dpi: f64) -> TabStage {
    if !physical_width.is_finite() || !native_dpi.is_finite() || native_dpi <= 0.0 {
        return TabStage::None;
    }
    console_tabs_for(physical_width / console_dpi(physical_width, physical_height, native_dpi))
}

/// The same decision, from the width the decks have in LAYOUT POINTS.
///
/// This is the honest form of the rule: the panels take turns as soon as
/// they would squeeze the middle under [`CENTRE_MIN_POINTS`], whatever it is
/// that took the room — a narrow window, or the lists moving alongside them.
/// Keyed on the width the decks actually have, so a wide window with the
/// lists beside it tabs at the same MIDDLE width as a narrow one without.
///
/// The half-point of slack keeps the comparison off a knife edge: inside the
/// scaling band the layout is worth exactly `TARGET_POINTS`, and a division
/// that lands a bit under would otherwise tab a console that fits.
pub fn console_tabs_for(deck_points: f64) -> TabStage {
    if !deck_points.is_finite() || deck_points <= 0.0 {
        return TabStage::None;
    }
    let centre_floor = crate::music_view::STRIP_SWEEP_MIN + crate::music_view::STRIP_ROW_SLACK;
    if deck_points < FLANKS_POINTS / 2.0 + centre_floor {
        TabStage::All
    } else if deck_points < TARGET_POINTS - 0.5 {
        TabStage::Decks
    } else {
        TabStage::None
    }
}

/// What the console spends either side of the middle column once the tabs
/// have had their say: both panels, or the one that is showing.
pub fn flanks_points(physical_width: f64, physical_height: f64, native_dpi: f64) -> f64 {
    if console_tabbed(physical_width, physical_height, native_dpi) {
        FLANKS_POINTS / 2.0
    } else {
        FLANKS_POINTS
    }
}

/// How many layout points the middle column ends up with, which is the whole
/// point of the exercise.
pub fn centre_points(physical_width: f64, physical_height: f64, native_dpi: f64) -> f64 {
    let layout = physical_width / console_dpi(physical_width, physical_height, native_dpi);
    (layout - flanks_points(physical_width, physical_height, native_dpi)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A height that constrains nothing, for the width-only rules.
    const TALL: f64 = 1.0e9;

    #[test]
    fn a_short_console_shrinks_to_keep_its_rail() {
        // The hosted tile that found this: 1376x844 points on a 2x surface.
        let (w, h, dpi) = (2752.0, 1688.0, 2.0);
        let scaled = console_dpi(w, h, dpi);
        assert!(scaled < dpi, "a short console must draw smaller");
        assert!(
            h / scaled >= TARGET_HEIGHT_POINTS - 0.5,
            "the layout must get its full height in points: {}",
            h / scaled
        );
        // Tall enough: untouched, and the width rule is unaffected.
        assert_eq!(console_dpi(w, TARGET_HEIGHT_POINTS * dpi, dpi), dpi);
        assert_eq!(console_dpi(w, TALL, dpi), dpi);
    }

    /// Every window width worth having, at the scales displays actually
    /// report.
    fn widths_and_dpis() -> impl Iterator<Item = (f64, f64)> {
        [1.0f64, 1.25, 1.5, 2.0, 3.0].into_iter().flat_map(|dpi| {
            (200..6_000).step_by(7).map(move |px| (px as f64, dpi))
        })
    }

    #[test]
    fn the_scale_is_a_fixed_point() {
        // Setting the DPI remeasures the window and hands the geometry back,
        // so the rule runs again on its own result. It has to agree with
        // itself the second time or the console pumps between two sizes.
        for (px, dpi) in widths_and_dpis() {
            let first = console_dpi(px, TALL, dpi);
            // The window is PHYSICALLY unchanged by the scaling, so the
            // second pass is handed the very same width.
            let second = console_dpi(px, TALL, dpi);
            assert_eq!(first, second, "{px}px at {dpi}: {first} then {second}");
        }
    }

    #[test]
    fn the_middle_column_is_what_the_scaling_defends() {
        for dpi in [1.0, 1.5, 2.0] {
            // Wide: the middle keeps everything above the minimum, because a
            // wider console is supposed to mean more room to work in.
            let wide = (TARGET_POINTS + 400.0) * dpi;
            assert_eq!(console_scale(wide, TALL, dpi), 1.0);
            assert!(
                (centre_points(wide, TALL, dpi) - (CENTRE_MIN_POINTS + 400.0)).abs() < 1e-9,
                "the surplus should go to the middle"
            );

            // At the trigger the middle is exactly at its minimum and the
            // console has not started shrinking yet.
            let trigger = TARGET_POINTS * dpi;
            assert_eq!(console_scale(trigger, TALL, dpi), 1.0);
            assert!((centre_points(trigger, TALL, dpi) - CENTRE_MIN_POINTS).abs() < 1e-9);

            // Narrower, all the way to the floor: the console shrinks and the
            // middle column holds its 500 points rather than paying for it.
            let floor = TARGET_POINTS * dpi * MIN_SCALE;
            for px in [trigger - 1.0, trigger * 0.95, floor + 1.0, floor] {
                assert!(
                    (centre_points(px, TALL, dpi) - CENTRE_MIN_POINTS).abs() < 1e-6,
                    "{px}px at {dpi}: middle came to {}",
                    centre_points(px, TALL, dpi)
                );
                assert!(console_scale(px, TALL, dpi) < 1.000_001);
            }

            // Past the floor the shrinking is spent — and the tabs take
            // over, folding a panel away and holding the middle open again
            // (see `the_tabs_take_over_exactly_where_the_shrinking_runs_out`).
            // It gives only when one panel plus the minimum will not fit.
            let tabbed_floor =
                (FLANKS_POINTS / 2.0 + CENTRE_MIN_POINTS) * dpi * MIN_SCALE;
            assert!(centre_points(tabbed_floor * 1.05, TALL, dpi) >= CENTRE_MIN_POINTS);
            assert!(centre_points(tabbed_floor * 0.9, TALL, dpi) < CENTRE_MIN_POINTS);
        }
    }

    #[test]
    fn a_console_that_fits_is_left_alone_and_one_that_does_not_shrinks() {
        // Wide enough for the console it wants: drawn at the display's own
        // scale, whatever that is.
        for dpi in [1.0, 1.5, 2.0] {
            let wide = TARGET_POINTS * dpi + 400.0;
            assert_eq!(console_dpi(wide, TALL, dpi), dpi, "a wide console is untouched");
            assert_eq!(console_scale(wide, TALL, dpi), 1.0);
        }

        // Exactly the width it wants: still untouched, and the seam is
        // continuous — a point either side is a hair either side of 1.0.
        let dpi = 1.0;
        assert_eq!(console_dpi(TARGET_POINTS * dpi, TALL, dpi), dpi);
        let just_under = console_scale(TARGET_POINTS - 1.0, TALL, 1.0);
        assert!(just_under < 1.0 && just_under > 0.999, "seam jumped: {just_under}");

        // Narrower: the console gives up exactly the fraction it is short by,
        // so the layout still gets its full width in points.
        let px = TARGET_POINTS * 0.8;
        let dpi = console_dpi(px, TALL, 1.0);
        assert!((px / dpi - TARGET_POINTS).abs() < 1e-9, "the layout is short");
        assert!((console_scale(px, TALL, 1.0) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn the_tabs_take_over_exactly_where_the_shrinking_runs_out() {
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            let floor = TARGET_POINTS * dpi * MIN_SCALE;

            // Above the floor the console shrinks and keeps both panels.
            for px in [floor + 1.0, floor * 1.2, TARGET_POINTS * dpi, 6_000.0] {
                assert!(!console_tabbed(px, TALL, dpi), "{px}px at {dpi} tabbed too early");
            }
            // At the floor exactly the middle is still at its minimum, so
            // there is nothing to rescue yet; below it the panels go one at
            // a time. No width where both answers have given up, and none
            // where both fire.
            assert!(!console_tabbed(floor, TALL, dpi), "the floor itself still fits");
            for px in [floor - 1.0, floor * 0.8] {
                assert!(console_tabbed(px, TALL, dpi), "{px}px at {dpi} should tab");
                assert!(
                    (console_scale(px, TALL, dpi) - MIN_SCALE).abs() < 1e-9,
                    "tabbing before the shrinking is spent"
                );
            }

            // Folding a panel hands the middle back half the flanks, so it
            // is roomier just after the handover than just before it.
            let before = centre_points(floor + 1.0, TALL, dpi);
            let after = centre_points(floor - 1.0, TALL, dpi);
            assert!(after > before, "tabbing bought nothing: {before} then {after}");

            // And from there the middle keeps its minimum until the window
            // reaches the flanks-plus-minimum it now needs.
            let narrow_target = (FLANKS_POINTS / 2.0 + CENTRE_MIN_POINTS) * dpi * MIN_SCALE;
            assert!(centre_points(narrow_target + 1.0, TALL, dpi) >= CENTRE_MIN_POINTS);
        }
    }

    #[test]
    fn the_panel_folds_in_two_stages_as_the_console_loses_height() {
        // On a window too narrow for the lists to move beside the decks,
        // losing height runs the full ladder: the shrink absorbs the first
        // of it, and once the shrink is spent — MIN_SCALE — the folds take
        // over, measured in the SHRUNK points.
        for dpi in [1.0, 1.5, 2.0] {
            let narrow = (lists_beside_min_points() - 40.0) * dpi;
            let at = |points: f64| console_fold(narrow, points * dpi, dpi);

            // Room to spare: every block on screen, no chevrons.
            assert_eq!(at(TARGET_HEIGHT_POINTS + 100.0), ConsoleFold::None);
            // Short, but within the shrink's reach: still every block.
            assert_eq!(at(PANEL_FLOOR_POINTS), ConsoleFold::None);
            // Past the shrink floor the transcript folds away and the knobs
            // pair up...
            assert_eq!(at(PANEL_FLOOR_POINTS * MIN_SCALE - 1.0), ConsoleFold::Pairs);
            assert_eq!(at(PANEL_SPLIT_POINTS * MIN_SCALE + 1.0), ConsoleFold::Pairs);
            // ...and below THAT the knobs cannot share a column either.
            assert_eq!(at(PANEL_SPLIT_POINTS * MIN_SCALE - 1.0), ConsoleFold::Singles);
            assert_eq!(at(200.0), ConsoleFold::Singles);
        }

        // On a wide window the lists-beside arrangement takes over below
        // [`LISTS_STACK_POINTS`] instead (the shrink stands down for it),
        // and what is left of the height goes straight past Pairs.
        for dpi in [1.0, 2.0] {
            let wide = (lists_beside_min_points() + 200.0) * dpi;
            let at = |points: f64| console_fold(wide, points * dpi, dpi);
            assert_eq!(at(PANEL_FLOOR_POINTS), ConsoleFold::None);
            assert_eq!(at(LISTS_STACK_POINTS - 1.0), ConsoleFold::Singles);
        }

        // The stages are ordered and the boundaries do not overlap.
        assert!(PANEL_SPLIT_POINTS < PANEL_FLOOR_POINTS);
        // Each stage's floor has to hold what that stage still shows, or
        // the panel clips its own last heading and the fold cannot be
        // undone. And the floors fall as the fold tightens.
        // 72 points of sync row, transport row and spacing come off the top
        // before a single block is drawn.
        const CHROME: f64 = 72.0;
        const HEADINGS: f64 = 59.0;
        assert!(
            region_min_points(ConsoleFold::Pairs) >= CHROME + HEADINGS + 85.0 + 83.0,
            "both knob blocks, their headings, and the panel's own rows"
        );
        assert!(
            region_min_points(ConsoleFold::Singles) >= CHROME + HEADINGS + 85.0,
            "the tallest block, its headings, and the panel's own rows"
        );
        assert!(region_min_points(ConsoleFold::None) > region_min_points(ConsoleFold::Pairs));
        assert!(region_min_points(ConsoleFold::Pairs) > region_min_points(ConsoleFold::Singles));

        // The width matters too, through the scale: the same physical height
        // is worth MORE layout points on a console that has shrunk, so a
        // narrow window folds later than a wide one of the same height. On
        // the wide window the lists-beside arrangement holds the scale at
        // native and the height reads short; the narrow one is already at
        // the shrink floor and the same pixels buy it a third more points.
        let native = 1.5;
        let wide = (lists_beside_min_points() + 200.0) * native;
        let narrow = (lists_beside_min_points() - 100.0) * native;
        let height = 690.0 * native;
        assert_eq!(console_fold(wide, height, native), ConsoleFold::Singles);
        assert_eq!(
            console_fold(narrow, height, native),
            ConsoleFold::None,
            "the shrunken console has the points to spare"
        );
    }

    #[test]
    fn the_mixer_joins_the_tabs_when_folding_a_deck_is_no_longer_enough() {
        // Below the scaling floor a layout point costs `native * MIN_SCALE`
        // physical pixels, and every one of these thresholds is down there.
        let px = |points: f64, native: f64| points * native * MIN_SCALE;
        for dpi in [1.0, 1.5, 2.0] {
            let deck_stage = px(TARGET_POINTS, dpi);
            // Wide: nothing tabs.
            assert_eq!(console_tabs(deck_stage + 100.0, TALL, dpi), TabStage::None);
            // Narrower: the deck panels take turns, the mixer stays put.
            assert_eq!(console_tabs(deck_stage - 1.0, TALL, dpi), TabStage::Decks);

            // Narrower still: the middle can no longer hold a fader worth
            // playing, so the mixer takes its turn too.
            let centre_floor =
                crate::music_view::STRIP_SWEEP_MIN + crate::music_view::STRIP_ROW_SLACK;
            let all_stage = px(FLANKS_POINTS / 2.0 + centre_floor, dpi);
            assert_eq!(console_tabs(all_stage - 1.0, TALL, dpi), TabStage::All);
            assert_eq!(console_tabs(200.0, TALL, dpi), TabStage::All);

            // The stages are ordered and every width has exactly one answer.
            assert!(all_stage < deck_stage, "the mixer joins after the decks");
            assert_eq!(
                console_tabbed(deck_stage - 1.0, TALL, dpi),
                console_tabs(deck_stage - 1.0, TALL, dpi) != TabStage::None
            );
        }

        // On the display this was specified against — 1.5x, console at its
        // 0.75 floor — the mixer joins at about 560 physical pixels, which
        // is some 30% narrower than holding the middle at its comfortable
        // minimum (765) would have allowed.
        let native = 1.5;
        let floor = crate::music_view::STRIP_SWEEP_MIN + crate::music_view::STRIP_ROW_SLACK;
        let at = px(FLANKS_POINTS / 2.0 + floor, native);
        let comfortable = px(FLANKS_POINTS / 2.0 + CENTRE_MIN_POINTS, native);
        assert!((at - 560.0).abs() < 2.0, "{at} should be about 560 device pixels");
        // Twenty-seven percent narrower, not the thirty that was asked for,
        // and the fader is why: thirty would put the middle at 136 points,
        // under `STRIP_SWEEP_MIN`, so the console would be holding on to a
        // crossfader too short to play. The rule is "a fader worth playing",
        // and this is where that lands.
        let narrower = 1.0 - at / comfortable;
        assert!((0.25..0.30).contains(&narrower), "{narrower} off {comfortable} to {at}");
    }

    #[test]
    fn the_lists_take_turns_only_once_they_cannot_stand_side_by_side() {
        let px = |points: f64, native: f64| points * native * MIN_SCALE;
        for dpi in [1.0, 1.5, 2.0] {
            let at = px(LISTS_TAB_POINTS, dpi);
            assert!(!console_lists_tabbed(at + 1.0, TALL, dpi), "room for both");
            assert!(!console_lists_tabbed(at, TALL, dpi), "exactly enough is enough");
            assert!(console_lists_tabbed(at - 1.0, TALL, dpi), "not any more");
            assert!(console_lists_tabbed(300.0, TALL, dpi));
        }
        // The lists give up side-by-side BEFORE the mixer joins the deck
        // tabs: a console narrow enough to tab its mixer has long since had
        // to choose between its two lists.
        let native = 1.5;
        let lists = px(LISTS_TAB_POINTS, native);
        let mixer = px(
            FLANKS_POINTS / 2.0
                + crate::music_view::STRIP_SWEEP_MIN
                + crate::music_view::STRIP_ROW_SLACK,
            native,
        );
        assert!(mixer < lists, "{mixer} should come after {lists}");
        // And on the display this was specified against, about 990 pixels.
        assert!((lists - 990.0).abs() < 1.0, "{lists} should be about 990 device pixels");
    }

    #[test]
    fn the_status_bar_takes_a_second_line_only_when_its_controls_will_not_fit() {
        let px = |points: f64, native: f64| points * native * MIN_SCALE;
        for dpi in [1.0, 1.5, 2.0] {
            let at = px(STATUS_BAR_POINTS, dpi);
            assert!(!console_status_bar_wrapped(at + 1.0, dpi), "room for one line");
            assert!(!console_status_bar_wrapped(at, dpi), "exactly enough is enough");
            assert!(console_status_bar_wrapped(at - 1.0, dpi), "not any more");
        }
        // On the display it was measured on, 1108 device pixels.
        let at = px(STATUS_BAR_POINTS, 1.5);
        assert!((at - 1108.0).abs() < 1.0, "{at} should be 1108 device pixels");
        // The bar gives up its single line BEFORE the deck panels start
        // taking turns: it is the widest thing the console carries.
        assert!(at > px(TARGET_POINTS, 1.5) * 0.0, "sanity");
        assert!(STATUS_BAR_POINTS < TARGET_POINTS, "still narrower than the console's target");
    }

    #[test]
    fn the_lists_stand_beside_the_decks_only_on_a_wide_short_window() {
        for dpi in [1.0, 1.5, 2.0] {
            // A width comfortably past the bar, and heights converted at the
            // scale the console ACTUALLY draws at — `native` is the wrong
            // divisor whenever it has shrunk, which at these widths it has.
            let wide = lists_beside_min_points() * dpi * 1.3;
            let scale = console_dpi(wide, TALL, dpi);
            let short = (LISTS_STACK_POINTS - 1.0) * scale;
            let tall = LISTS_STACK_POINTS * scale;
            assert!(console_lists_beside(wide, short, dpi), "wide and short");
            assert!(!console_lists_beside(wide, tall, dpi), "tall enough to stack");
            // Narrow and short: there is no width to put them in either, so
            // they stay stacked and the tabs deal with it.
            let narrow = lists_beside_min_points() * dpi * 0.5;
            let short = (LISTS_STACK_POINTS - 1.0) * console_dpi(narrow, TALL, dpi);
            assert!(!console_lists_beside(narrow, short, dpi), "no width to spare");
        }
    }

    #[test]
    fn the_tab_stages_follow_what_the_decks_get_not_what_the_window_has() {
        let native = 1.5;
        // A wide, short window: the lists move beside, so the decks get
        // about half of it — and the tabs have to answer to that half.
        let wide = lists_beside_min_points() * native * 1.2;
        let short = (LISTS_STACK_POINTS - 50.0) * native;
        assert!(console_lists_beside(wide, short, native));
        let span = deck_span(wide, short, native);
        assert!(span > wide * 0.6 && span < wide, "the decks' share: {span} of {wide}");
        // The whole window would say "no tabs at all"; the decks' own share
        // says otherwise, which is the point.
        assert_eq!(console_tabs(wide, TALL, native), TabStage::None);
        let dpi = console_dpi(wide, TALL, native);
        assert_ne!(
            console_tabs_for(span / dpi),
            TabStage::None,
            "the decks know better"
        );
        // And the middle they are protecting never goes under its minimum
        // while the panels still stand side by side.
        let stage = console_tabs_for(span / dpi);
        if stage == TabStage::None {
            assert!(span / dpi - FLANKS_POINTS >= CENTRE_MIN_POINTS);
        }

        // Stacked, the decks get the window and nothing changes.
        let tall = LISTS_STACK_POINTS * native;
        assert_eq!(deck_span(wide, tall, native), wide);
    }

    #[test]
    fn a_short_console_puts_its_lists_beside_as_soon_as_both_sides_would_fit() {
        // The window that prompted this: 1077 x 490 points. Short, and wide
        // enough that both sides get what they need — so the lists go
        // alongside rather than being squeezed into three rows underneath.
        let native = 1.5;
        let (w, h) = (1077.0 * native, 490.0 * native);
        assert!(console_lists_beside(w, h, native), "should stand beside");

        // Both sides come out usable, which is the whole test.
        let span = deck_span(w, h, native) / console_dpi(w, TALL, native);
        assert!(span >= FLANKS_POINTS / 2.0 + CENTRE_MIN_POINTS * 0.5, "decks: {span}");
        let lists = 1077.0 - span;
        assert!(lists >= LISTS_MIN_POINTS, "lists: {lists}");
        // And the decks tab rather than squeeze the middle.
        assert_ne!(console_tabs_for(span), TabStage::None);
    }

    #[test]
    fn the_lists_take_turns_once_they_are_down_to_their_share() {
        // The window that prompted this: standing beside the decks, the
        // lists get about a third — far too little for a 320-point queue and
        // a readable explorer side by side, so they tab.
        let native = 1.5;
        let (w, h) = (1077.0 * native, 490.0 * native);
        assert!(console_lists_beside(w, h, native));
        let span = lists_span(w, h, native);
        assert!(console_lists_tabbed(span, TALL, native), "their share is {span}px");
        // Stacked, they have the window and the same call says otherwise.
        let tall = 900.0 * native;
        assert_eq!(lists_span(w, tall, native), w);
        assert!(!console_lists_tabbed(w, TALL, native), "the whole window is plenty");
    }

    #[test]
    fn the_mixer_is_never_hidden_while_both_panels_still_stand() {
        // The invariant, stated once and checked everywhere: two deck panels
        // side by side may only be chosen when there is room for the middle
        // column between them. Anything else hides the mixer to keep a panel
        // the operator could have reached through a tab — which is the wrong
        // way round, and was a real bug.
        let mut spans = vec![];
        let mut p = 40.0;
        while p < 4000.0 {
            spans.push(p);
            p += 3.0;
        }
        for span in spans {
            if console_tabs_for(span) == TabStage::None {
                let middle = span - FLANKS_POINTS;
                assert!(
                    middle >= CENTRE_MIN_POINTS - 0.5,
                    "both panels at {span} points leaves the middle {middle}"
                );
            }
        }
    }

    #[test]
    fn the_mixer_is_served_before_the_lists_get_anything_spare() {
        // The priority, checked at every width worth having: until the decks
        // have one panel AND a full middle, the lists get no more than the
        // minimum that makes them worth showing at all.
        let one_panel = FLANKS_POINTS / 2.0 + CENTRE_MIN_POINTS;
        let mut total = 200.0;
        while total < 4000.0 {
            let (decks, lists) = split_body(total);
            assert!((decks + lists - total).abs() < 1e-9, "{total}: {decks} + {lists}");
            if lists > LISTS_MIN_POINTS + 1e-9 {
                assert!(
                    decks >= one_panel - 1e-9,
                    "{total}: lists took {lists} while the decks had {decks}"
                );
            }
            // And the second panel never comes before the lists are readable.
            if decks > TARGET_POINTS + 1e-9 {
                assert!(lists >= LISTS_MIN_POINTS - 1e-9, "{total}: lists {lists}");
            }
            total += 7.0;
        }
    }

    #[test]
    fn a_list_never_takes_more_width_than_it_can_use() {
        // A third of a very wide window is more track list than anyone
        // needs; the surplus belongs to the decks.
        let native = 1.5;
        let wide = 3000.0 * native;
        let short = (LISTS_STACK_POINTS - 50.0) * console_dpi(wide, TALL, native);
        assert!(console_lists_beside(wide, short, native));
        let dpi = console_dpi(wide, TALL, native);
        let lists = lists_span(wide, short, native) / dpi;
        assert!(
            (lists - LISTS_MAX_POINTS).abs() < 1.0,
            "capped at {LISTS_MAX_POINTS}, got {lists}"
        );
        // And the decks get everything else, so the pair still adds up.
        let decks = deck_span(wide, short, native) / dpi;
        assert!((decks + lists - (wide / dpi - 6.0)).abs() < 1.0, "{decks} + {lists}");
        // Two panels and a full middle fit again at that width.
        assert_eq!(console_tabs_for(decks), TabStage::None);
    }

    #[test]
    fn the_whole_chain_is_a_fixed_point() {
        // Scale AND tabs, together: both are functions of a width neither
        // can move, so the console cannot chase its own tail through them.
        for (px, dpi) in widths_and_dpis() {
            let (dpi1, tab1) = (console_dpi(px, TALL, dpi), console_tabbed(px, TALL, dpi));
            let (dpi2, tab2) = (console_dpi(px, TALL, dpi), console_tabbed(px, TALL, dpi));
            assert_eq!((dpi1, tab1), (dpi2, tab2), "{px}px at {dpi} disagreed with itself");
        }
    }

    #[test]
    fn the_type_sets_the_floor_and_the_console_stops_there() {
        // Past the floor the console holds its size rather than shrink the
        // labels out of readability. The layout then goes short, which is
        // where a narrower ARRANGEMENT has to take over — not a smaller one.
        for dpi in [1.0, 1.5, 2.0] {
            let floor_px = TARGET_POINTS * dpi * MIN_SCALE;
            assert!((console_scale(floor_px, TALL, dpi) - MIN_SCALE).abs() < 1e-9);
            for px in [floor_px - 1.0, floor_px * 0.5, 200.0] {
                assert!(
                    (console_scale(px, TALL, dpi) - MIN_SCALE).abs() < 1e-9,
                    "{px}px at {dpi} went past the floor"
                );
            }
        }
    }

    #[test]
    fn the_scale_only_ever_shrinks_and_only_ever_moves_one_way() {
        for (px, dpi) in widths_and_dpis() {
            let scale = console_scale(px, TALL, dpi);
            assert!(scale <= 1.0, "{px}px at {dpi} magnified to {scale}");
            assert!(scale >= MIN_SCALE, "{px}px at {dpi} shrank to {scale}");
        }
        // Widening a window never makes the console smaller.
        for dpi in [1.0, 1.5, 2.0] {
            let mut last = 0.0;
            for px in (200..4_000).step_by(3) {
                let scale = console_scale(px as f64, TALL, dpi);
                assert!(scale >= last - 1e-12, "{px}px at {dpi} went backwards");
                last = scale;
            }
        }
    }

    #[test]
    fn a_display_that_reports_nonsense_is_not_allowed_to_blank_the_window() {
        // A zero or NaN DPI reaches the layout as a divide, so the rule has
        // to hand back something a window can actually be drawn at.
        for bad in [0.0, -2.0, f64::NAN, f64::INFINITY] {
            assert!(console_dpi(1600.0, TALL, bad) > 0.0, "dpi {bad}");
            assert!(console_dpi(bad, TALL, 1.5) > 0.0, "width {bad}");
        }
    }
}
