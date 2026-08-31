//! Display geometry, and the policy that keeps a window inside it.

use crate::makepad_math::*;

/// The smallest window extent a fit ever produces. Small enough to leave a deliberately
/// compact tool window alone, large enough that the window still has a title bar to grab.
pub const MIN_WINDOW_SIZE: Vec2d = Vec2d { x: 200.0, y: 120.0 };

/// One display attached to the system.
///
/// The rectangles are in the same coordinate space as the platform's window-position API,
/// so a backend must build them from the same system calls it positions windows with:
/// physical pixels with a top-left origin on Windows and X11, points with Cocoa's
/// bottom-left origin on macOS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenGeom {
    /// The display's full extent.
    pub bounds: Rect,
    /// The extent left over once the system reserves its own space — the Windows taskbar,
    /// the macOS menu bar and Dock, X11 struts. Windows are placed inside this.
    pub work_area: Rect,
    /// Whether this is the system's primary display.
    pub is_primary: bool,
}

/// Area shared by two rectangles; zero when they do not overlap.
fn overlap_area(a: Rect, b: Rect) -> f64 {
    let w = (a.pos.x + a.size.x).min(b.pos.x + b.size.x) - a.pos.x.max(b.pos.x);
    let h = (a.pos.y + a.size.y).min(b.pos.y + b.size.y) - a.pos.y.max(b.pos.y);
    if w <= 0.0 || h <= 0.0 {
        0.0
    } else {
        w * h
    }
}

/// Squared distance between two rectangles' centres.
fn center_distance_sq(a: Rect, b: Rect) -> f64 {
    let d = a.center() - b.center();
    d.x * d.x + d.y * d.y
}

/// The parts of `a` that `b` does not cover, as up to four rectangles.
fn subtract(a: Rect, b: Rect) -> Vec<Rect> {
    if overlap_area(a, b) <= 0.0 {
        return vec![a];
    }
    let (ax0, ay0) = (a.pos.x, a.pos.y);
    let (ax1, ay1) = (a.pos.x + a.size.x, a.pos.y + a.size.y);
    let bx0 = b.pos.x.max(ax0);
    let by0 = b.pos.y.max(ay0);
    let bx1 = (b.pos.x + b.size.x).min(ax1);
    let by1 = (b.pos.y + b.size.y).min(ay1);
    let mut out = Vec::new();
    let mut push = |x0: f64, y0: f64, x1: f64, y1: f64| {
        if x1 > x0 && y1 > y0 {
            out.push(Rect {
                pos: dvec2(x0, y0),
                size: dvec2(x1 - x0, y1 - y0),
            });
        }
    };
    push(ax0, ay0, ax1, by0);
    push(ax0, by1, ax1, ay1);
    push(ax0, by0, bx0, by1);
    push(bx1, by0, ax1, by1);
    out
}

/// Whether `r` lies entirely within the union of `areas`, which may be several displays
/// covering it between them.
fn is_covered_by(areas: &[Rect], r: Rect) -> bool {
    if !is_usable(r) {
        return false;
    }
    let mut remaining = vec![r];
    for area in areas {
        let mut next = Vec::new();
        for piece in remaining.drain(..) {
            next.extend(subtract(piece, *area));
        }
        if next.is_empty() {
            return true;
        }
        remaining = next;
    }
    remaining.is_empty()
}

/// Whether a rectangle is usable as a destination: real numbers, and some area to put a
/// window in.
fn is_usable(r: Rect) -> bool {
    r.pos.x.is_finite()
        && r.pos.y.is_finite()
        && r.size.x.is_finite()
        && r.size.y.is_finite()
        && r.size.x > 0.0
        && r.size.y > 0.0
}

/// Fits a window's outer rectangle inside the work area of the display it belongs to.
///
/// Every window position an app restores has to survive a display layout that may have
/// changed completely since it was written: the display the window sat on can be gone, a
/// docked laptop can be back on a smaller built-in panel, and a state file saved while the
/// window was minimized holds coordinates no display ever had — Win32 reports position
/// `(-32000, -32000)` and a zero-sized client area for a minimized window, and an app that
/// persists that on shutdown restores a window it cannot see or grab on the next launch,
/// with no way back short of deleting the file. Fitting therefore applies to every
/// placement rather than only to values that look wrong.
///
/// A window that is already wholly on the desktop is returned untouched, including one
/// deliberately spanning two adjacent displays — the point is to rescue placements that
/// cannot be reached, not to enforce one window per display. Anything else moves onto the
/// display it overlaps most, or, when it overlaps none, the display nearest its centre; its
/// size is capped to that work area and floored at [`MIN_WINDOW_SIZE`], and its position is
/// pulled in until the whole window is visible.
///
/// An empty `screens` means the backend cannot enumerate displays — Wayland, where a client
/// is not allowed to know or choose where its windows go — and `window` is returned as-is.
pub fn fit_window_rect_to_screens(screens: &[ScreenGeom], window: Rect) -> Rect {
    let usable: Vec<Rect> = screens
        .iter()
        .map(|s| s.work_area)
        .filter(|r| is_usable(*r))
        .collect();
    let Some(&first) = usable.first() else {
        return window;
    };
    let primary = screens
        .iter()
        .find(|s| s.is_primary && is_usable(s.work_area))
        .map_or(first, |s| s.work_area);

    // Coordinates that are not real numbers cannot be compared or clamped, so they name no
    // display and get the primary's geometry to start from.
    let mut want = window;
    if !want.size.x.is_finite() || !want.size.y.is_finite() {
        want.size = primary.size * 0.5;
    }
    if !want.pos.x.is_finite() || !want.pos.y.is_finite() {
        want.pos = primary.pos;
    }

    // A window already wholly on the desktop is left exactly where it is, including one
    // deliberately spanning two adjacent displays. Fitting exists to rescue a placement that
    // cannot be reached, not to enforce one window per display.
    if is_covered_by(&usable, want) {
        return want;
    }

    let area = usable
        .iter()
        .copied()
        .max_by(|a, b| {
            let (oa, ob) = (overlap_area(*a, want), overlap_area(*b, want));
            oa.total_cmp(&ob).then_with(|| {
                // No overlap anywhere leaves every candidate tied at zero; nearest centre
                // breaks the tie, so a window off the right edge lands on the right display.
                center_distance_sq(*b, want).total_cmp(&center_distance_sq(*a, want))
            })
        })
        .unwrap_or(primary);

    let size = dvec2(
        want.size.x.clamp(MIN_WINDOW_SIZE.x.min(area.size.x), area.size.x),
        want.size.y.clamp(MIN_WINDOW_SIZE.y.min(area.size.y), area.size.y),
    );
    let pos = dvec2(
        want.pos.x.clamp(area.pos.x, area.pos.x + area.size.x - size.x),
        want.pos.y.clamp(area.pos.y, area.pos.y + area.size.y - size.y),
    );
    Rect { pos, size }
}

/// Clamps a point into the work area of the display nearest to it, leaving room for a
/// window of at least [`MIN_WINDOW_SIZE`] to be visible from there.
///
/// A window origin can break creation on its own, before there is a finished rectangle to
/// fit: a coordinate out of the platform's integer range saturates when it reaches the
/// system call, and the sizing that follows is done relative to wherever the window landed.
/// Backends pin the origin through here first and fit the finished rectangle afterwards.
///
/// An empty `screens` returns the point unchanged, for the same reason
/// [`fit_window_rect_to_screens`] does.
pub fn clamp_point_to_screens(screens: &[ScreenGeom], point: Vec2d) -> Vec2d {
    fit_window_rect_to_screens(
        screens,
        Rect {
            pos: point,
            size: dvec2(0.0, 0.0),
        },
    )
    .pos
}

/// The size a window falls back to when the requested one carries no usable information.
pub const DEFAULT_WINDOW_SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };

/// The largest extent a window may be given.
///
/// A floor alone is not enough: a state file holding a huge number reaches the backend just as
/// readily as a zero, and an extent no renderer can back is as unusable as an empty one. 16384 is
/// the point past which drivers stop being able to render the surface at all — it is the smallest
/// `GL_MAX_TEXTURE_SIZE` and maximum viewport dimension in practical use — and it is far larger
/// than any real arrangement of displays, so nothing legitimate is clipped by it.
pub const MAX_WINDOW_SIZE: Vec2d = Vec2d { x: 16384.0, y: 16384.0 };

/// Reduces a *resize* request to an extent a windowing system can act on, or `None` when the
/// request carries no usable size at all.
///
/// Creating a window and resizing one need the same guard but answer a bad request differently.
/// A create has to produce a window regardless, so [`sanitize_window_geom`] substitutes
/// [`DEFAULT_WINDOW_SIZE`]; a resize has an existing, working window to leave alone, so a request
/// that says nothing is refused outright rather than turned into a size the caller never asked
/// for. `None` means "keep the window as it is, and say why".
pub fn sanitize_resize(size: Vec2d) -> Option<Vec2d> {
    if !(size.x.is_finite() && size.y.is_finite() && size.x >= 1.0 && size.y >= 1.0) {
        return None;
    }
    Some(dvec2(
        size.x.clamp(MIN_WINDOW_SIZE.x, MAX_WINDOW_SIZE.x),
        size.y.clamp(MIN_WINDOW_SIZE.y, MAX_WINDOW_SIZE.y),
    ))
}

/// Reduces a requested window size and position to values a windowing system can act on,
/// without needing to know anything about the attached displays.
///
/// This is the guard that has to hold everywhere, including the backends
/// [`fit_window_rect_to_screens`] cannot help: Wayland enumerates no displays for a client
/// and passes the size straight to `wl_egl_window_create`, which rejects a non-positive one;
/// X11 encodes width and height as unsigned 16-bit and answers a zero with a protocol error
/// that terminates the process by default. A saved `0`, a negative, or a `NaN` — all of which
/// a JSON state file can hold, and which `as i32` quietly turns into `0` — must therefore
/// never leave this function. Position is dropped rather than corrected when it is not a real
/// number: `None` means "the system places this window", which is always a safe answer.
pub fn sanitize_window_geom(position: Option<Vec2d>, size: Vec2d) -> (Option<Vec2d>, Vec2d) {
    // A non-positive extent carries no information about how big the window should be — it is
    // what a zeroed, truncated or minimized-window state file holds — so it gets the default
    // rather than the floor, which would restore a technically-visible 200x120 sliver. A small
    // positive size is a real request and is only raised to something grabbable.
    let size = if size.x.is_finite() && size.y.is_finite() && size.x > 0.0 && size.y > 0.0 {
        // Capped as well as floored: a saved size far past what any renderer can back is not a
        // window the app can start with. On Windows, macOS and X11 the fit against the attached
        // displays would cut it down anyway, but Wayland enumerates no displays, so without this
        // the raw number reaches `wl_egl_window_create` and `eglCreateWindowSurface`.
        dvec2(
            size.x.clamp(MIN_WINDOW_SIZE.x, MAX_WINDOW_SIZE.x),
            size.y.clamp(MIN_WINDOW_SIZE.y, MAX_WINDOW_SIZE.y),
        )
    } else {
        DEFAULT_WINDOW_SIZE
    };
    let position = position.filter(|p| p.x.is_finite() && p.y.is_finite());
    (position, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(x: f64, y: f64, w: f64, h: f64, is_primary: bool) -> ScreenGeom {
        let bounds = rect(x, y, w, h);
        ScreenGeom {
            bounds,
            work_area: bounds,
            is_primary,
        }
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            pos: dvec2(x, y),
            size: dvec2(w, h),
        }
    }

    #[test]
    fn a_window_already_inside_a_display_is_left_alone() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0, true)];
        let want = rect(100.0, 100.0, 800.0, 600.0);
        assert_eq!(fit_window_rect_to_screens(&screens, want), want);
    }

    #[test]
    fn no_screens_leaves_the_request_untouched() {
        let want = rect(-32000.0, -32000.0, 0.0, 0.0);
        assert_eq!(fit_window_rect_to_screens(&[], want), want);
    }

    #[test]
    fn the_win32_minimized_sentinel_comes_back_onto_the_primary_display() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0, true)];
        let fitted = fit_window_rect_to_screens(&screens, rect(-32000.0, -32000.0, 0.0, 0.0));
        assert_eq!(fitted.pos, dvec2(0.0, 0.0));
        assert_eq!(fitted.size, MIN_WINDOW_SIZE);
    }

    #[test]
    fn a_window_past_the_right_edge_is_pulled_back_in() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0, true)];
        let fitted = fit_window_rect_to_screens(&screens, rect(1900.0, 50.0, 800.0, 600.0));
        assert_eq!(fitted, rect(1120.0, 50.0, 800.0, 600.0));
    }

    #[test]
    fn a_window_larger_than_the_work_area_is_capped_to_it() {
        let screens = [ScreenGeom {
            bounds: rect(0.0, 0.0, 1920.0, 1080.0),
            work_area: rect(0.0, 0.0, 1920.0, 1040.0),
            is_primary: true,
        }];
        let fitted = fit_window_rect_to_screens(&screens, rect(-500.0, -500.0, 4000.0, 4000.0));
        assert_eq!(fitted, rect(0.0, 0.0, 1920.0, 1040.0));
    }

    #[test]
    fn a_window_keeps_the_secondary_display_it_sits_on() {
        let screens = [
            screen(0.0, 0.0, 1920.0, 1080.0, true),
            screen(1920.0, 0.0, 2560.0, 1440.0, false),
        ];
        let want = rect(2000.0, 200.0, 800.0, 600.0);
        assert_eq!(fit_window_rect_to_screens(&screens, want), want);
    }

    #[test]
    fn a_window_on_a_display_that_is_gone_moves_to_the_nearest_one() {
        // The secondary display it was saved on is no longer attached.
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0, true)];
        let fitted = fit_window_rect_to_screens(&screens, rect(3000.0, 200.0, 800.0, 600.0));
        assert_eq!(fitted, rect(1120.0, 200.0, 800.0, 600.0));
    }

    #[test]
    fn a_window_spanning_two_adjacent_displays_is_left_alone() {
        let screens = [
            screen(0.0, 0.0, 1920.0, 1080.0, true),
            screen(1920.0, 0.0, 1920.0, 1080.0, false),
        ];
        // The window straddles the seam but every pixel of it is on a display.
        let want = rect(1720.0, 100.0, 800.0, 600.0);
        assert_eq!(fit_window_rect_to_screens(&screens, want), want);
    }

    #[test]
    fn a_window_over_a_gap_between_displays_moves_to_the_one_holding_most_of_it() {
        // Displays side by side with a gap between them, as a mismatched pair produces.
        let screens = [
            screen(0.0, 0.0, 1920.0, 1080.0, true),
            screen(2400.0, 0.0, 1920.0, 1080.0, false),
        ];
        let fitted = fit_window_rect_to_screens(&screens, rect(1800.0, 100.0, 800.0, 600.0));
        assert_eq!(fitted, rect(2400.0, 100.0, 800.0, 600.0));
    }

    #[test]
    fn a_window_hanging_off_the_end_of_the_arrangement_is_pulled_in() {
        let screens = [
            screen(0.0, 0.0, 1920.0, 1080.0, true),
            screen(1920.0, 0.0, 1920.0, 1080.0, false),
        ];
        let fitted = fit_window_rect_to_screens(&screens, rect(3600.0, 100.0, 800.0, 600.0));
        assert_eq!(fitted, rect(3040.0, 100.0, 800.0, 600.0));
    }

    #[test]
    fn a_window_spanning_displays_of_different_heights_is_not_left_hanging() {
        // The taller display sits lower, so the strip below the shorter one is off-desktop.
        let screens = [
            screen(0.0, 0.0, 1920.0, 1080.0, true),
            screen(1920.0, 0.0, 1920.0, 1440.0, false),
        ];
        let fitted = fit_window_rect_to_screens(&screens, rect(1600.0, 900.0, 800.0, 400.0));
        assert!(fitted != rect(1600.0, 900.0, 800.0, 400.0));
        assert!(screens.iter().any(|s| fitted.is_inside_of(s.work_area)));
    }

    #[test]
    fn non_finite_geometry_falls_back_to_the_primary_display() {
        let screens = [
            screen(-1920.0, 0.0, 1920.0, 1080.0, false),
            screen(0.0, 0.0, 1920.0, 1080.0, true),
        ];
        let fitted =
            fit_window_rect_to_screens(&screens, rect(f64::NAN, f64::INFINITY, f64::NAN, 600.0));
        assert_eq!(fitted, rect(0.0, 0.0, 960.0, 540.0));
    }

    #[test]
    fn cocoa_bottom_left_coordinates_fit_the_same_way() {
        // macOS reports the primary display at the origin with y growing upwards; a window
        // saved below the display comes back inside it.
        let screens = [ScreenGeom {
            bounds: rect(0.0, 0.0, 1728.0, 1117.0),
            work_area: rect(0.0, 76.0, 1728.0, 1004.0),
            is_primary: true,
        }];
        let fitted = fit_window_rect_to_screens(&screens, rect(20.0, -400.0, 900.0, 700.0));
        assert_eq!(fitted, rect(20.0, 76.0, 900.0, 700.0));
    }

    #[test]
    fn a_display_smaller_than_the_minimum_size_still_fits_a_window() {
        let screens = [screen(0.0, 0.0, 100.0, 60.0, true)];
        let fitted = fit_window_rect_to_screens(&screens, rect(500.0, 500.0, 800.0, 600.0));
        assert_eq!(fitted, rect(0.0, 0.0, 100.0, 60.0));
    }

    #[test]
    fn a_negative_size_is_raised_to_the_minimum() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0, true)];
        let fitted = fit_window_rect_to_screens(&screens, rect(10.0, 10.0, -800.0, -600.0));
        assert_eq!(fitted, rect(10.0, 10.0, MIN_WINDOW_SIZE.x, MIN_WINDOW_SIZE.y));
    }

    #[test]
    fn coordinates_far_outside_the_integer_range_land_on_a_display() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0, true)];
        for want in [
            rect(1e300, 1e300, 800.0, 600.0),
            rect(-1e300, -1e300, 800.0, 600.0),
            rect(f64::MAX, f64::MIN, f64::MAX, f64::MAX),
        ] {
            let fitted = fit_window_rect_to_screens(&screens, want);
            assert!(fitted.is_inside_of(screens[0].work_area), "{fitted:?}");
        }
    }

    #[test]
    fn every_fitted_rectangle_lies_within_some_work_area() {
        let screens = [
            screen(0.0, 0.0, 1920.0, 1080.0, true),
            screen(1920.0, -200.0, 2560.0, 1440.0, false),
        ];
        for want in [
            rect(-32000.0, -32000.0, 0.0, 0.0),
            rect(f64::NAN, f64::NAN, f64::NAN, f64::NAN),
            rect(f64::INFINITY, f64::NEG_INFINITY, 1e12, -1e12),
            rect(1e9, 1e9, 1e9, 1e9),
            rect(4400.0, 1100.0, 300.0, 200.0),
            rect(0.0, 0.0, 0.0, 0.0),
        ] {
            let fitted = fit_window_rect_to_screens(&screens, want);
            let areas: Vec<Rect> = screens.iter().map(|s| s.work_area).collect();
            assert!(is_covered_by(&areas, fitted), "{want:?} fitted to {fitted:?}");
            assert!(fitted.pos.x.is_finite() && fitted.pos.y.is_finite());
            assert!(fitted.size.x > 0.0 && fitted.size.y > 0.0);
        }
    }

    #[test]
    fn sanitizing_rejects_every_size_a_windowing_system_cannot_use() {
        for bad in [
            dvec2(0.0, 0.0),
            dvec2(-800.0, -600.0),
            dvec2(f64::NAN, f64::NAN),
            dvec2(f64::INFINITY, 600.0),
            dvec2(1.0, 1.0),
        ] {
            let (_, size) = sanitize_window_geom(None, bad);
            assert!(size.x >= MIN_WINDOW_SIZE.x && size.y >= MIN_WINDOW_SIZE.y, "{bad:?}");
            assert!(size.x.is_finite() && size.y.is_finite(), "{bad:?}");
        }
    }

    #[test]
    fn a_size_carrying_no_information_becomes_the_default_not_the_floor() {
        // Restoring a 200x120 sliver from a zeroed state file is visible but useless.
        for empty in [
            dvec2(0.0, 0.0),
            dvec2(-800.0, -600.0),
            dvec2(0.0, 800.0),
            dvec2(f64::NAN, f64::NAN),
        ] {
            assert_eq!(sanitize_window_geom(None, empty).1, DEFAULT_WINDOW_SIZE, "{empty:?}");
        }
        // A small but real request is only raised to something grabbable.
        assert_eq!(
            sanitize_window_geom(None, dvec2(50.0, 40.0)).1,
            MIN_WINDOW_SIZE
        );
    }

    #[test]
    fn a_size_no_renderer_can_back_is_capped_rather_than_passed_on() {
        // Wayland enumerates no displays, so this cap is the only thing between a state file
        // holding 1e9 and `eglCreateWindowSurface`.
        let (_, size) = sanitize_window_geom(None, dvec2(1e9, 1e9));
        assert_eq!(size, MAX_WINDOW_SIZE);
        let (_, size) = sanitize_window_geom(None, dvec2(1e9, 800.0));
        assert_eq!(size, dvec2(MAX_WINDOW_SIZE.x, 800.0));
    }

    #[test]
    fn a_resize_that_carries_no_usable_size_is_refused_not_substituted() {
        // Unlike a create, a resize has a working window to leave alone.
        for bad in [
            dvec2(0.0, 0.0),
            dvec2(-800.0, -600.0),
            dvec2(f64::NAN, f64::NAN),
            dvec2(f64::INFINITY, 600.0),
            dvec2(0.5, 600.0),
        ] {
            assert_eq!(sanitize_resize(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_resize_is_held_between_the_minimum_and_the_maximum() {
        assert_eq!(sanitize_resize(dvec2(900.0, 600.0)), Some(dvec2(900.0, 600.0)));
        assert_eq!(sanitize_resize(dvec2(1.0, 1.0)), Some(MIN_WINDOW_SIZE));
        assert_eq!(sanitize_resize(dvec2(1e9, 1e9)), Some(MAX_WINDOW_SIZE));
        assert_eq!(
            sanitize_resize(dvec2(100000.0, 100000.0)),
            Some(MAX_WINDOW_SIZE)
        );
    }

    #[test]
    fn sanitizing_keeps_a_usable_request_intact() {
        let (pos, size) = sanitize_window_geom(Some(dvec2(-1200.0, 40.0)), dvec2(1280.0, 800.0));
        // A position on a left-hand secondary display is legitimate and is not a size problem,
        // so it survives untouched; fitting to the displays is a separate, later step.
        assert_eq!(pos, Some(dvec2(-1200.0, 40.0)));
        assert_eq!(size, dvec2(1280.0, 800.0));
    }

    #[test]
    fn sanitizing_drops_a_position_that_is_not_a_real_number() {
        assert_eq!(
            sanitize_window_geom(Some(dvec2(f64::NAN, 0.0)), dvec2(800.0, 600.0)).0,
            None
        );
        assert_eq!(
            sanitize_window_geom(Some(dvec2(0.0, f64::INFINITY)), dvec2(800.0, 600.0)).0,
            None
        );
    }

    #[test]
    fn a_clamped_point_leaves_a_minimum_window_visible() {
        let screens = [ScreenGeom {
            bounds: rect(0.0, 0.0, 1920.0, 1080.0),
            work_area: rect(0.0, 0.0, 1920.0, 1040.0),
            is_primary: true,
        }];
        assert_eq!(
            clamp_point_to_screens(&screens, dvec2(-32000.0, -32000.0)),
            dvec2(0.0, 0.0)
        );
        assert_eq!(
            clamp_point_to_screens(&screens, dvec2(1e9, 1e9)),
            dvec2(1920.0 - MIN_WINDOW_SIZE.x, 1040.0 - MIN_WINDOW_SIZE.y)
        );
        assert_eq!(
            clamp_point_to_screens(&screens, dvec2(f64::NAN, 5.0)),
            dvec2(0.0, 0.0)
        );
        assert_eq!(clamp_point_to_screens(&screens, dvec2(40.0, 50.0)), dvec2(40.0, 50.0));
    }
}
