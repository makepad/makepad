//! How the dock fits its items along its axis: a pure function, so the
//! overlap guarantee is a unit test and not a look at the screen.
//!
//! The dock holds the launcher slot first, then one slot per app. When the
//! natural pitch does not fit, icons and spacing shrink together by one
//! factor, down to `MIN_SCALE`. Past that the apps that do not fit give way
//! to one "more" slot at the end, which opens a menu of the rest.

/// The smallest share of the theme's icon size the dock shrinks to.
pub const MIN_SCALE: f64 = 0.6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockPlan {
    /// The one factor applied to icon size and spacing alike (1.0 = theme size).
    pub scale: f64,
    /// Distance between slot origins along the axis: the natural pitch times `scale`.
    pub pitch: f64,
    /// Apps drawn as icons; the rest are behind the "more" slot.
    pub shown: usize,
    /// Apps behind the "more" slot.
    pub overflow: usize,
}

impl DockPlan {
    /// The launcher slot, the shown apps, and the "more" slot when apps overflow.
    pub fn slots(&self) -> usize {
        1 + self.shown + usize::from(self.overflow > 0)
    }
    /// Slot index (0 is the launcher) the app at `index` of the dock's app list
    /// is drawn in; an overflowed app is reached through the "more" slot.
    pub fn slot_of_app(&self, index: usize) -> usize {
        if index < self.shown { index + 1 } else { self.shown + 1 }
    }
}

/// `avail` is the length the slots may fill; `apps` the app count (the
/// launcher slot is extra); `natural_pitch` the theme's slot length.
pub fn plan(avail: f64, apps: usize, natural_pitch: f64) -> DockPlan {
    let slots = apps + 1;
    let natural = slots as f64 * natural_pitch;
    if natural <= avail {
        return DockPlan { scale: 1.0, pitch: natural_pitch, shown: apps, overflow: 0 };
    }
    if avail >= natural * MIN_SCALE {
        let scale = avail / natural;
        return DockPlan { scale, pitch: natural_pitch * scale, shown: apps, overflow: 0 };
    }
    let pitch = natural_pitch * MIN_SCALE;
    let capacity = (avail / pitch).floor() as usize;
    let shown = capacity.saturating_sub(2).min(apps);
    DockPlan { scale: MIN_SCALE, pitch, shown, overflow: apps - shown }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICON: f64 = 54.0;
    const PITCH: f64 = 62.0;
    const PAD: f64 = 44.0;

    /// The slot origins, the icon rects (at full magnification) and the dock length.
    fn extent(p: &DockPlan) -> (Vec<(f64, f64)>, f64) {
        let icon = (ICON + 8.0) * p.scale;
        let spans = (0..p.slots())
            .map(|i| {
                let x = i as f64 * p.pitch;
                (x + (p.pitch - icon) * 0.5, x + (p.pitch + icon) * 0.5)
            })
            .collect();
        (spans, p.slots() as f64 * p.pitch)
    }

    #[test]
    fn many_items_never_overlap_and_stay_inside_at_every_display_width() {
        for width in [1280.0, 1920.0, 2560.0, 3440.0] {
            let avail = width - PAD;
            for apps in 0..200 {
                let p = plan(avail, apps, PITCH);
                let (spans, len) = extent(&p);
                assert!(len <= avail + 1e-9, "{apps} apps at {width}: {len} > {avail}");
                for w in spans.windows(2) {
                    assert!(w[0].1 <= w[1].0 + 1e-9, "{apps} apps at {width}: icons overlap");
                }
                assert!(spans.first().unwrap().0 >= -1e-9);
                assert!(spans.last().unwrap().1 <= len + 1e-9);
                assert!(p.scale >= MIN_SCALE - 1e-12 && p.scale <= 1.0);
                assert_eq!(p.shown + p.overflow, apps);
                assert!((p.pitch - PITCH * p.scale).abs() < 1e-9, "icon and spacing share one factor");
            }
        }
    }

    #[test]
    fn a_dock_that_fits_keeps_theme_size_and_shrinks_only_when_it_must() {
        let p = plan(1280.0 - PAD, 10, PITCH);
        assert_eq!((p.scale, p.overflow, p.shown), (1.0, 0, 10));
        // 1236 / 62 = 19 slots fit at theme size; the 20th shrinks everything.
        let p = plan(1280.0 - PAD, 18, PITCH);
        assert_eq!(p.scale, 1.0);
        let p = plan(1280.0 - PAD, 19, PITCH);
        assert!(p.scale < 1.0 && p.scale > MIN_SCALE && p.overflow == 0);
        assert!((p.slots() as f64 * p.pitch - (1280.0 - PAD)).abs() < 1e-9, "fills the dock exactly");
    }

    #[test]
    fn past_the_minimum_the_rest_goes_behind_one_more_slot() {
        // 1236 / (62 * 0.6) = 33 slots at the minimum: launcher + 31 apps + more.
        let p = plan(1280.0 - PAD, 40, PITCH);
        assert_eq!(p.scale, MIN_SCALE);
        assert_eq!((p.shown, p.overflow, p.slots()), (31, 9, 33));
        assert_eq!(p.slot_of_app(0), 1);
        assert_eq!(p.slot_of_app(30), 31);
        assert_eq!(p.slot_of_app(31), 32, "an overflowed app is reached through the more slot");
        assert_eq!(p.slot_of_app(39), 32);
        // Exactly the capacity of apps needs no more slot.
        let p = plan(1280.0 - PAD, 32, PITCH);
        assert_eq!((p.shown, p.overflow), (32, 0));
        // Wider displays hold more before overflowing.
        assert!(plan(1280.0 - PAD, 80, PITCH).overflow > 0);
        assert_eq!(plan(3440.0 - PAD, 80, PITCH).overflow, 0);
    }
}
