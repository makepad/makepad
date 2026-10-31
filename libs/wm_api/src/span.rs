//! Pure screen-span model: resolving a request to span one, several
//! adjacent, or all screens into an index range and a bounding rect, with
//! no platform or `Cx` dependency so it unit-tests without a display.
//!
//! `WmRequest::SetFullscreenSpan` and `WmEvent::Screens` (see `lib.rs`) carry
//! [`ScreenSpan`] and [`WmScreen`] values over the wire; this module only
//! resolves them against a screen list.

use makepad_widgets_core::makepad_micro_serde::*;

/// Which screens an app is asking to span.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum ScreenSpan {
    /// Connector names; must resolve to a contiguous run of `screens`,
    /// left to right (duplicates in the list are fine).
    Screens(Vec<String>),
    /// Every live screen.
    All,
    /// The screen the window is already on.
    Current,
}

/// One screen, as the window manager reports it. The rect is in whatever
/// coordinate space the caller agreed on (desktop space, or a receiving
/// window's local space after [`screens_in_window`]).
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct WmScreen {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub primary: bool,
}

/// Why a [`ScreenSpan`] could not be resolved against a screen list.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum SpanError {
    /// Nothing to span: an empty `Screens` list, no screens for `All`, or
    /// no current screen for `Current` -- including a `current` index
    /// that is out of range for `screens`, which is folded into this same
    /// variant rather than treated as a distinct error (`screens` lists
    /// the live screens; an out-of-range index is equally "no current
    /// screen among them").
    Empty,
    /// A name in `Screens` is not in `screens`.
    UnknownScreen(String),
    /// The resolved screens are not a contiguous run in `screens`' order.
    NotAdjacent,
    /// A screen inside the resolved span has non-finite (`NaN`/`±Infinity`)
    /// or negative-size (`w < 0` or `h < 0`) geometry, so no meaningful
    /// bounding box exists. Screens outside the resolved span are not
    /// checked -- only the ones the caller is actually asking to span.
    BadGeometry,
}

/// True when `s`'s rect can contribute to a bounding box: every
/// coordinate finite and both sizes non-negative.
fn geometry_is_valid(s: &WmScreen) -> bool {
    s.x.is_finite() && s.y.is_finite() && s.w.is_finite() && s.h.is_finite() && s.w >= 0.0 && s.h >= 0.0
}

/// Resolve `span` against `screens` (left to right). Returns the
/// half-open index range it covers and the bounding box of those
/// screens' rects (their union, including any dead space between
/// different-height screens). `current` is the window's own screen
/// index, used only by [`ScreenSpan::Current`]: `None` and an
/// out-of-range index both resolve to [`SpanError::Empty`] (see that
/// variant's doc) rather than a distinct error.
pub fn span_rect(
    screens: &[WmScreen],
    span: &ScreenSpan,
    current: Option<usize>,
) -> Result<(std::ops::Range<usize>, (f64, f64, f64, f64)), SpanError> {
    let range = match span {
        ScreenSpan::Screens(names) => {
            if names.is_empty() {
                return Err(SpanError::Empty);
            }
            let mut indices = Vec::new();
            for name in names {
                let idx = screens
                    .iter()
                    .position(|s| &s.name == name)
                    .ok_or_else(|| SpanError::UnknownScreen(name.clone()))?;
                if !indices.contains(&idx) {
                    indices.push(idx);
                }
            }
            indices.sort_unstable();
            let lo = *indices.first().unwrap();
            let hi = *indices.last().unwrap();
            if indices.len() != hi - lo + 1 {
                return Err(SpanError::NotAdjacent);
            }
            lo..hi + 1
        }
        ScreenSpan::All => {
            if screens.is_empty() {
                return Err(SpanError::Empty);
            }
            0..screens.len()
        }
        ScreenSpan::Current => {
            // `None` (no current screen) and an out-of-range index both
            // land here as `Empty`, documented on that variant.
            let idx = current.filter(|&idx| idx < screens.len()).ok_or(SpanError::Empty)?;
            idx..idx + 1
        }
    };
    if screens[range.clone()].iter().any(|s| !geometry_is_valid(s)) {
        return Err(SpanError::BadGeometry);
    }
    let rect = bounding_box(&screens[range.clone()]);
    Ok((range, rect))
}

/// The union bounding box of `screens`' rects: `(x, y, w, h)`. Callers
/// must have already rejected non-finite/negative-size geometry (see
/// `span_rect`'s `BadGeometry` check) -- `f64::min`/`max` silently ignore
/// a `NaN` operand, so this alone would not catch it.
fn bounding_box(screens: &[WmScreen]) -> (f64, f64, f64, f64) {
    let x0 = screens.iter().map(|s| s.x).fold(f64::INFINITY, f64::min);
    let y0 = screens.iter().map(|s| s.y).fold(f64::INFINITY, f64::min);
    let x1 = screens
        .iter()
        .map(|s| s.x + s.w)
        .fold(f64::NEG_INFINITY, f64::max);
    let y1 = screens
        .iter()
        .map(|s| s.y + s.h)
        .fold(f64::NEG_INFINITY, f64::max);
    (x0, y0, x1 - x0, y1 - y0)
}

/// `screens` translated so a window sitting at `(window_x, window_y)` in
/// the same space `screens` are given in sees its own position as the
/// origin: each rect shifts by `(-window_x, -window_y)`, sizes unchanged.
///
/// Guards non-finite input rather than propagating `NaN`/`Infinity`
/// downstream: a screen whose own geometry is not finite or has a
/// negative size (see `span_rect`'s `BadGeometry`) is skipped and left
/// out of the result; if `window_x`/`window_y` themselves are not
/// finite, there is no meaningful origin to translate by, so every
/// screen is skipped and an empty `Vec` is returned.
pub fn screens_in_window(screens: &[WmScreen], window_x: f64, window_y: f64) -> Vec<WmScreen> {
    if !window_x.is_finite() || !window_y.is_finite() {
        return Vec::new();
    }
    screens
        .iter()
        .filter(|s| geometry_is_valid(s))
        .map(|s| WmScreen {
            name: s.name.clone(),
            x: s.x - window_x,
            y: s.y - window_y,
            w: s.w,
            h: s.h,
            primary: s.primary,
        })
        .collect()
}

/// The screen a standalone app should treat as "current" for
/// [`ScreenSpan::Current`]: there is no WM-tracked window position, so the
/// primary screen stands in for it, or the first (left-most) screen when
/// none is marked primary. `None` when `screens` is empty.
pub fn primary_or_first_index(screens: &[WmScreen]) -> Option<usize> {
    if screens.is_empty() {
        return None;
    }
    Some(screens.iter().position(|s| s.primary).unwrap_or(0))
}

/// Resolve a `set_fullscreen_span` request against the live `screens` for
/// a standalone app with no WM to answer it -- the pure core of the
/// direct backend's `set_fullscreen_span`, and testable without a live
/// desktop. Returns the span to record from now on (its screens' names,
/// left to right) and whether the request was honoured, mirroring the
/// WM's own rule: a span that cannot be resolved changes nothing, so the
/// span already `recorded` is kept.
pub fn resolve_standalone_span(
    screens: &[WmScreen],
    current: Option<usize>,
    recorded: &Option<Vec<String>>,
    span: Option<ScreenSpan>,
) -> (Option<Vec<String>>, bool) {
    let Some(span) = span else {
        return (None, true);
    };
    match span_rect(screens, &span, current) {
        Ok((range, _)) => (
            Some(screens[range].iter().map(|s| s.name.clone()).collect()),
            true,
        ),
        Err(_) => (recorded.clone(), false),
    }
}

/// Re-resolve a standalone app's recorded span against the live `screens`
/// on read: a hotplug/reorder since it was set can make `recorded` no
/// longer a live, adjacent run, in which case it has lapsed (`None`) the
/// same way the WM drops a span whose screens are no longer live.
pub fn resolve_standalone_current_span(
    screens: &[WmScreen],
    recorded: &Option<Vec<String>>,
) -> Option<Vec<String>> {
    let names = recorded.clone()?;
    let span = ScreenSpan::Screens(names);
    span_rect(screens, &span, None)
        .ok()
        .map(|(range, _)| screens[range].iter().map(|s| s.name.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(name: &str, x: f64, y: f64, w: f64, h: f64) -> WmScreen {
        WmScreen {
            name: name.into(),
            x,
            y,
            w,
            h,
            primary: false,
        }
    }

    /// Three same-height screens left to right: a (0..1920), b
    /// (1920..3840), c (3840..5760).
    fn three() -> Vec<WmScreen> {
        vec![
            screen("a", 0.0, 0.0, 1920.0, 1080.0),
            screen("b", 1920.0, 0.0, 1920.0, 1080.0),
            screen("c", 3840.0, 0.0, 1920.0, 1080.0),
        ]
    }

    #[test]
    fn adjacent_subset_resolves() {
        let screens = three();
        let span = ScreenSpan::Screens(vec!["b".into(), "c".into()]);
        let (range, rect) = span_rect(&screens, &span, None).unwrap();
        assert_eq!(range, 1..3);
        assert_eq!(rect, (1920.0, 0.0, 3840.0, 1080.0));
    }

    #[test]
    fn non_adjacent_subset_errors() {
        let screens = three();
        let span = ScreenSpan::Screens(vec!["a".into(), "c".into()]);
        assert_eq!(span_rect(&screens, &span, None), Err(SpanError::NotAdjacent));
    }

    #[test]
    fn unknown_name_errors() {
        let screens = three();
        let span = ScreenSpan::Screens(vec!["z".into()]);
        assert_eq!(
            span_rect(&screens, &span, None),
            Err(SpanError::UnknownScreen("z".into()))
        );
    }

    #[test]
    fn empty_screens_list_errors() {
        let screens = three();
        let span = ScreenSpan::Screens(vec![]);
        assert_eq!(span_rect(&screens, &span, None), Err(SpanError::Empty));
    }

    #[test]
    fn current_without_an_index_errors() {
        let screens = three();
        assert_eq!(
            span_rect(&screens, &ScreenSpan::Current, None),
            Err(SpanError::Empty)
        );
    }

    #[test]
    fn current_out_of_range_errors() {
        let screens = three();
        assert_eq!(
            span_rect(&screens, &ScreenSpan::Current, Some(9)),
            Err(SpanError::Empty)
        );
    }

    #[test]
    fn current_resolves_its_own_screen() {
        let screens = three();
        let (range, rect) = span_rect(&screens, &ScreenSpan::Current, Some(1)).unwrap();
        assert_eq!(range, 1..2);
        assert_eq!(rect, (1920.0, 0.0, 1920.0, 1080.0));
    }

    #[test]
    fn all_spans_every_screen() {
        let screens = three();
        let (range, rect) = span_rect(&screens, &ScreenSpan::All, None).unwrap();
        assert_eq!(range, 0..3);
        assert_eq!(rect, (0.0, 0.0, 5760.0, 1080.0));
    }

    #[test]
    fn all_with_no_screens_errors() {
        assert_eq!(span_rect(&[], &ScreenSpan::All, None), Err(SpanError::Empty));
    }

    #[test]
    fn mixed_heights_bounding_box_includes_dead_space() {
        let screens = vec![
            screen("a", 0.0, 0.0, 1920.0, 1080.0),
            screen("b", 1920.0, 200.0, 1280.0, 720.0),
        ];
        let span = ScreenSpan::Screens(vec!["a".into(), "b".into()]);
        let (range, rect) = span_rect(&screens, &span, None).unwrap();
        assert_eq!(range, 0..2);
        // Union: left/top from a, right from b's far edge, bottom is the
        // lower of the two far edges (a's, since b sits higher and ends
        // at 200+720=920 < 1080).
        assert_eq!(rect, (0.0, 0.0, 3200.0, 1080.0));
    }

    #[test]
    fn duplicate_names_tolerated() {
        let screens = three();
        let span = ScreenSpan::Screens(vec!["b".into(), "b".into(), "c".into()]);
        let (range, rect) = span_rect(&screens, &span, None).unwrap();
        assert_eq!(range, 1..3);
        assert_eq!(rect, (1920.0, 0.0, 3840.0, 1080.0));
    }

    #[test]
    fn screens_in_window_translates_to_window_local_coordinates() {
        let screens = three();
        let local = screens_in_window(&screens, 1920.0, 10.0);
        assert_eq!(local[0].x, -1920.0);
        assert_eq!(local[0].y, -10.0);
        assert_eq!(local[1].x, 0.0);
        assert_eq!(local[2].x, 1920.0);
        // Names, sizes and primary flag are untouched by translation.
        assert_eq!(local[1].name, "b");
        assert_eq!(local[1].w, 1920.0);
        assert_eq!(local[1].h, 1080.0);
    }

    #[test]
    fn nan_coordinate_errors() {
        let screens = vec![screen("a", f64::NAN, 0.0, 1920.0, 1080.0)];
        let span = ScreenSpan::Screens(vec!["a".into()]);
        assert_eq!(span_rect(&screens, &span, None), Err(SpanError::BadGeometry));
    }

    #[test]
    fn infinite_size_errors() {
        let screens = vec![screen("a", 0.0, 0.0, f64::INFINITY, 1080.0)];
        assert_eq!(
            span_rect(&screens, &ScreenSpan::All, None),
            Err(SpanError::BadGeometry)
        );
    }

    #[test]
    fn negative_size_errors() {
        let screens = vec![screen("a", 0.0, 0.0, -100.0, 1080.0)];
        assert_eq!(
            span_rect(&screens, &ScreenSpan::All, None),
            Err(SpanError::BadGeometry)
        );
    }

    #[test]
    fn bad_geometry_outside_the_resolved_span_is_ignored() {
        // "bad" is not part of the "a","b" span, so its NaN x must not
        // fail the call -- only screens inside the resolved range count.
        let mut screens = three();
        screens.push(screen("bad", f64::NAN, 0.0, 1.0, 1.0));
        let span = ScreenSpan::Screens(vec!["a".into(), "b".into()]);
        let (range, rect) = span_rect(&screens, &span, None).unwrap();
        assert_eq!(range, 0..2);
        assert_eq!(rect, (0.0, 0.0, 3840.0, 1080.0));
    }

    #[test]
    fn screens_in_window_skips_non_finite_screens() {
        let screens = vec![
            screen("a", 0.0, 0.0, 1920.0, 1080.0),
            screen("nan-x", f64::NAN, 0.0, 1920.0, 1080.0),
            screen("inf-w", 0.0, 0.0, f64::INFINITY, 1080.0),
            screen("negative-h", 0.0, 0.0, 1920.0, -1.0),
        ];
        let local = screens_in_window(&screens, 0.0, 0.0);
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].name, "a");
    }

    #[test]
    fn screens_in_window_guards_non_finite_origin() {
        let screens = three();
        assert_eq!(screens_in_window(&screens, f64::NAN, 0.0), Vec::new());
        assert_eq!(
            screens_in_window(&screens, 0.0, f64::NEG_INFINITY),
            Vec::new()
        );
    }

    #[test]
    fn screen_span_json_round_trip() {
        for span in [
            ScreenSpan::All,
            ScreenSpan::Current,
            ScreenSpan::Screens(vec!["a".into(), "b".into()]),
        ] {
            let json = span.serialize_json();
            let parsed = ScreenSpan::deserialize_json(&json).unwrap();
            assert_eq!(parsed, span);
        }
    }

    #[test]
    fn wm_screen_json_round_trip() {
        let s = screen("a", 1.5, 2.5, 3.5, 4.5);
        let json = s.serialize_json();
        let parsed = WmScreen::deserialize_json(&json).unwrap();
        assert_eq!(parsed, s);
    }

    #[test]
    fn span_error_json_round_trip() {
        for err in [
            SpanError::Empty,
            SpanError::NotAdjacent,
            SpanError::UnknownScreen("x".into()),
            SpanError::BadGeometry,
        ] {
            let json = err.serialize_json();
            let parsed = SpanError::deserialize_json(&json).unwrap();
            assert_eq!(parsed, err);
        }
    }

    // ---- standalone span resolution (direct mode, no WM to answer) ----

    #[test]
    fn primary_or_first_index_picks_the_primary_screen() {
        let mut screens = three();
        screens[2].primary = true;
        assert_eq!(primary_or_first_index(&screens), Some(2));
    }

    #[test]
    fn primary_or_first_index_falls_back_to_the_first_screen() {
        let screens = three();
        assert_eq!(primary_or_first_index(&screens), Some(0));
    }

    #[test]
    fn primary_or_first_index_empty_is_none() {
        assert_eq!(primary_or_first_index(&[]), None);
    }

    #[test]
    fn resolve_standalone_span_none_leaves_fullscreen() {
        let screens = three();
        assert_eq!(
            resolve_standalone_span(&screens, Some(0), &Some(vec!["a".into()]), None),
            (None, true)
        );
    }

    #[test]
    fn resolve_standalone_span_resolves_a_subset() {
        let screens = three();
        let span = ScreenSpan::Screens(vec!["b".into(), "c".into()]);
        let (recorded, ok) = resolve_standalone_span(&screens, None, &None, Some(span));
        assert!(ok);
        assert_eq!(recorded, Some(vec!["b".into(), "c".into()]));
    }

    #[test]
    fn resolve_standalone_span_resolves_all() {
        let screens = three();
        let (recorded, ok) = resolve_standalone_span(&screens, None, &None, Some(ScreenSpan::All));
        assert!(ok);
        assert_eq!(recorded, Some(vec!["a".into(), "b".into(), "c".into()]));
    }

    #[test]
    fn resolve_standalone_span_resolves_current_via_the_given_index() {
        let screens = three();
        let (recorded, ok) =
            resolve_standalone_span(&screens, Some(1), &None, Some(ScreenSpan::Current));
        assert!(ok);
        assert_eq!(recorded, Some(vec!["b".into()]));
    }

    #[test]
    fn resolve_standalone_span_unresolvable_keeps_the_record() {
        let screens = three();
        let span = ScreenSpan::Screens(vec!["a".into(), "c".into()]); // not adjacent
        let before = Some(vec!["b".into()]);
        let (recorded, ok) = resolve_standalone_span(&screens, None, &before, Some(span.clone()));
        assert!(!ok);
        assert_eq!(recorded, before);
        // Nothing recorded stays nothing.
        assert_eq!(resolve_standalone_span(&screens, None, &None, Some(span)), (None, false));
    }

    #[test]
    fn resolve_standalone_span_unknown_name_keeps_the_record() {
        let screens = three();
        let span = ScreenSpan::Screens(vec!["z".into()]);
        let before = Some(vec!["a".into(), "b".into()]);
        let (recorded, ok) = resolve_standalone_span(&screens, None, &before, Some(span));
        assert!(!ok);
        assert_eq!(recorded, before);
    }

    #[test]
    fn resolve_standalone_current_span_reresolves_live_names() {
        let screens = three();
        let recorded = Some(vec!["b".into(), "c".into()]);
        assert_eq!(
            resolve_standalone_current_span(&screens, &recorded),
            Some(vec!["b".into(), "c".into()])
        );
    }

    #[test]
    fn resolve_standalone_current_span_none_when_unset() {
        let screens = three();
        assert_eq!(resolve_standalone_current_span(&screens, &None), None);
    }

    #[test]
    fn resolve_standalone_current_span_drops_on_hotplug_removal() {
        // "b" was spanned, then unplugged: only "a" and "c" remain, no
        // longer adjacent under that name -- the span has lapsed.
        let screens = vec![
            screen("a", 0.0, 0.0, 1920.0, 1080.0),
            screen("c", 1920.0, 0.0, 1920.0, 1080.0),
        ];
        let recorded = Some(vec!["b".into(), "c".into()]);
        assert_eq!(resolve_standalone_current_span(&screens, &recorded), None);
    }
}
