//! Shake to periphery. Shaking a window by its title tidies every other
//! window of that display out to the side zones; a second shake of the same
//! window brings back the ones nobody has touched since.
//!
//! The detector reads the pointer samples `drag_move` already sees (a pure
//! fn, unit-tested below). With spacewarp off the others minimize to the dock
//! instead, through the same `minimized` flag the title button sets.
use makepad_app_host::ClientId;
use crate::layout::LRect;
use crate::spacewarp::{pose_in, slot_at};
use crate::App;
use makepad_widgets::*;

/// Direction reversals that make a shake.
pub const REVERSALS: usize = 3;
/// Each swing between two reversals is longer than this (logical px).
pub const MIN_SWING: f64 = 40.0;
/// All of them fall inside this window (seconds).
pub const WINDOW: f64 = 0.6;

/// Does the horizontal track `(time, x)` (oldest first) end in a shake: at
/// least [`REVERSALS`] reversals, every swing over [`MIN_SWING`], within
/// [`WINDOW`] of the last sample?
pub fn detect(samples: &[(f64, f64)]) -> bool {
    let Some(&(t_end, _)) = samples.last() else { return false };
    let mut dir = 0i8;
    let mut extreme = 0.0;
    let mut reversals = 0;
    let mut first = true;
    for &(_, x) in samples.iter().filter(|(t, _)| t_end - *t <= WINDOW) {
        if first {
            extreme = x;
            first = false;
            continue;
        }
        match dir {
            0 => {
                if (x - extreme).abs() > MIN_SWING {
                    dir = if x > extreme { 1 } else { -1 };
                    extreme = x;
                }
            }
            d => {
                if (x - extreme) * d as f64 > 0.0 {
                    extreme = x;
                } else if (extreme - x).abs() > MIN_SWING {
                    reversals += 1;
                    dir = -d;
                    extreme = x;
                }
            }
        }
    }
    reversals >= REVERSALS
}

/// The arrangement a shake replaced, for the second shake.
struct Saved {
    client: ClientId,
    screen: usize,
    /// Per moved window: its rect and parked flag before, its rect after.
    parked: Vec<(ClientId, LRect, bool, LRect)>,
    /// Windows the spacewarp-off path minimized.
    minimized: Vec<ClientId>,
}

#[derive(Default)]
pub struct Shake {
    samples: Vec<(f64, f64)>,
    saved: Option<Saved>,
}

impl Shake {
    pub fn reset(&mut self) {
        self.samples.clear();
    }

    /// Add a drag sample; true when it completes a shake (the track starts
    /// over, so the next shake needs its own swings).
    pub fn push(&mut self, t: f64, x: f64) -> bool {
        self.samples.push((t, x));
        self.samples.retain(|(s, _)| t - *s <= WINDOW);
        if detect(&self.samples) {
            self.samples.clear();
            return true;
        }
        false
    }
}

fn same(a: LRect, b: LRect) -> bool {
    (a.x - b.x).abs() < 0.5 && (a.y - b.y).abs() < 0.5 && (a.w - b.w).abs() < 0.5 && (a.h - b.h).abs() < 0.5
}

impl App {
    /// `drag_move` of a floating window: feed the shake detector.
    pub(crate) fn shake_sample(&mut self, cx: &mut Cx, screen: usize, client: ClientId, abs: Vec2d) {
        if self.state_mut().shake.push(crate::host::now(), abs.x) {
            self.shake_fire(cx, screen, client);
        }
    }

    fn shake_fire(&mut self, cx: &mut Cx, screen: usize, client: ClientId) {
        let saved = self.state_mut().shake.saved.take();
        match saved {
            Some(s) if s.client == client && s.screen == screen => self.shake_restore(cx, s),
            _ => self.shake_park(cx, screen, client),
        }
        self.redraw_all(cx);
    }

    fn shake_park(&mut self, cx: &mut Cx, screen: usize, client: ClientId) {
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        let zones = self.state_mut().warp.zones_for(area);
        let layout = self.state_mut().layout_at(screen);
        let others: Vec<(ClientId, LRect)> = layout
            .rects(area, gap)
            .into_iter()
            .filter(|(c, _)| *c != client && layout.is_float(*c) && !layout.is_client_fullscreen(*c))
            .collect();
        let mut saved = Saved { client, screen, parked: Vec::new(), minimized: Vec::new() };
        match zones {
            Some(z) => {
                let mid = (z.left + z.right) * 0.5;
                let band: Vec<(ClientId, LRect)> =
                    others.iter().copied().filter(|(_, r)| !pose_in(&z, *r).warped()).collect();
                if band.is_empty() {
                    return;
                }
                for (c, r) in &others {
                    saved.parked.push((*c, *r, pose_in(&z, *r).warped(), *r));
                }
                for (c, r) in band {
                    let side = if r.center().0 < mid { -1 } else { 1 };
                    let slot = slot_at(&z, side, 0.0, r.w, r.h);
                    self.state_mut().layout_at_mut(screen).set_float_rect(c, LRect::new(slot.x, r.y, r.w, r.h));
                    self.warp_flag(c, true);
                }
                self.warp_settle(cx, screen, Some(client));
                let layout = self.state_mut().layout_at(screen);
                for p in saved.parked.iter_mut() {
                    if let Some(r) = layout.rect_of(p.0, area, gap) {
                        p.3 = r;
                    }
                }
            }
            None => {
                if others.is_empty() {
                    return;
                }
                let layout = self.state_mut().layout_at_mut(screen);
                for (c, _) in &others {
                    if let Some(w) = layout.desktop.get_mut(*c) {
                        w.minimized = true;
                        saved.minimized.push(*c);
                    }
                }
                let layout = self.state_mut().layout_at_mut(screen);
                layout.workspaces[layout.active].focus = Some(client);
            }
        }
        self.state_mut().shake.saved = Some(saved);
    }

    fn shake_restore(&mut self, cx: &mut Cx, saved: Saved) {
        let area = self.screen_area(cx, saved.screen);
        let gap = self.state_mut().gap;
        for (c, before, parked, after) in saved.parked {
            let layout = self.state_mut().layout_at_mut(saved.screen);
            if layout.rect_of(c, area, gap).is_some_and(|r| same(r, after)) {
                layout.set_float_rect(c, before);
                self.warp_flag(c, parked);
            }
        }
        for c in saved.minimized {
            if let Some(w) = self.state_mut().layout_at_mut(saved.screen).desktop.get_mut(c) {
                w.minimized = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
        points.to_vec()
    }

    /// A zigzag of `legs` swings of `amp` px, sampled every 10 ms at `speed` px/ms.
    fn zigzag(legs: usize, amp: f64, step_ms: f64) -> Vec<(f64, f64)> {
        let per = (amp / 10.0) as usize;
        let mut out = vec![(0.0, 500.0)];
        let (mut t, mut x, mut d) = (0.0, 500.0, 1.0);
        for _ in 0..legs {
            for _ in 0..per {
                t += step_ms / 1000.0;
                x += 10.0 * d;
                out.push((t, x));
            }
            d = -d;
        }
        out
    }

    #[test]
    fn four_fast_swings_are_a_shake() {
        // Four legs of 60 px = three reversals, 12 x 15 ms = 180 ms.
        assert!(detect(&zigzag(4, 60.0, 15.0)));
    }

    #[test]
    fn three_swings_are_not() {
        assert!(!detect(&zigzag(3, 60.0, 15.0)));
    }

    #[test]
    fn short_swings_are_a_wobble() {
        assert!(!detect(&zigzag(8, 30.0, 10.0)));
    }

    #[test]
    fn slow_swings_are_a_drag() {
        // 24 samples x 40 ms: about a second, beyond the 600 ms window.
        assert!(!detect(&zigzag(4, 60.0, 40.0)));
    }

    #[test]
    fn a_plain_drag_is_not() {
        let line: Vec<(f64, f64)> = (0..60).map(|i| (i as f64 * 0.01, i as f64 * 8.0)).collect();
        assert!(!detect(&line));
        assert!(!detect(&track(&[])));
    }

    #[test]
    fn jitter_on_the_way_is_not_a_reversal() {
        let mut v = zigzag(2, 100.0, 15.0);
        // Small back-steps (under the swing length) inside a long run.
        for i in (3..v.len()).step_by(3) {
            v[i].1 -= 8.0;
        }
        assert!(!detect(&v));
    }

    #[test]
    fn push_fires_once_then_starts_over() {
        let mut s = Shake::default();
        let fired: Vec<bool> = zigzag(4, 60.0, 15.0).into_iter().map(|(t, x)| s.push(t, x)).collect();
        assert_eq!(fired.iter().filter(|f| **f).count(), 1);
        assert!(!s.push(10.0, 500.0));
    }
}
