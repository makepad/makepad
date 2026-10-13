//! The slide between spaces. A display's slide is one spring over the
//! fractional INDEX of the space list: at `pos = 2.3` space 2 is shifted left
//! by 0.3 screens and space 3 enters from the right. Retargeting keeps the
//! speed (`crate::motion`), so a second press mid-slide bends the motion
//! instead of restarting it. What is shown during a slide are cached captures
//! of the two spaces (`desk.rs` captures them once); no client is asked to
//! draw while it runs.
use crate::layout::LRect;
use crate::motion::Spring;
use std::collections::HashMap;

/// Index units are big next to pixels: the spring rests when it is within
/// 0.3 thousandths of a screen (under half a point on a 1280 pt display), so
/// the handover from the slide to the live windows is below a device pixel.
const UNIT: f64 = 0.006;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slide {
    pub pos: Spring,
    /// The space shown when the slide began, and the one it heads for.
    pub from: usize,
    pub to: usize,
}

/// The running slides, by display index.
#[derive(Default, Debug)]
pub struct Slides {
    per: HashMap<usize, Slide>,
    /// A slide ended since the desk last looked: the windows that appear now
    /// were there all along and must not pop in.
    ended: bool,
}

impl Slides {
    /// Start sliding display `screen` from space `from` to `to`, or bend a
    /// slide already running to the new target.
    pub fn begin(&mut self, screen: usize, from: usize, to: usize) {
        match self.per.get_mut(&screen) {
            Some(s) => {
                s.pos.retarget(to as f64);
                s.to = to;
            }
            None => {
                let mut pos = Spring::at(from as f64);
                pos.retarget(to as f64);
                self.per.insert(screen, Slide { pos, from, to });
            }
        }
    }

    pub fn get(&self, screen: usize) -> Option<&Slide> {
        self.per.get(&screen)
    }

    pub fn active(&self, screen: usize) -> bool {
        self.per.contains_key(&screen)
    }

    pub fn any(&self) -> bool {
        !self.per.is_empty()
    }

    /// Advance every slide; true while any is still running. A finished one
    /// is removed.
    pub fn step(&mut self, dt: f64) -> bool {
        let mut done = false;
        for s in self.per.values_mut() {
            if !s.pos.step_scaled(dt, UNIT) {
                done = true;
            }
        }
        let before = self.per.len();
        self.per.retain(|_, s| !s.pos.settled());
        if self.per.len() != before || done {
            self.ended = true;
        }
        self.any()
    }

    /// True once after a slide ended.
    pub fn take_ended(&mut self) -> bool {
        std::mem::take(&mut self.ended)
    }

    /// A display's space list changed under a slide (a space was removed):
    /// the slide is over.
    pub fn cancel(&mut self, screen: usize) {
        if self.per.remove(&screen).is_some() {
            self.ended = true;
        }
    }
}

impl Slide {
    /// The spaces in view and the x offset (screens, 0 = in place) of each:
    /// space `i` sits `i - pos` screens from its place.
    pub fn views(&self, spaces: usize) -> Vec<(usize, f64)> {
        let p = self.pos.x;
        let lo = p.floor().max(0.0) as usize;
        let hi = (p.ceil().max(0.0) as usize).min(spaces.saturating_sub(1));
        let mut out = vec![(lo.min(spaces.saturating_sub(1)), lo as f64 - p)];
        if hi != lo {
            out.push((hi, hi as f64 - p));
        }
        out
    }

    /// Every space whose capture the slide needs: those in view and the one
    /// it heads for, so the first frame already has both.
    pub fn needed(&self, spaces: usize) -> Vec<usize> {
        let mut v: Vec<usize> = self.views(spaces).into_iter().map(|(i, _)| i).collect();
        for i in [self.from, self.to] {
            if i < spaces && !v.contains(&i) {
                v.push(i);
            }
        }
        v
    }
}

/// Where a capture of the whole display shifted by `offset` screens lands:
/// the part of `screen` it covers and the matching span of the capture
/// (u0, u1 in 0..1). None when nothing of it is on the display.
pub fn crop(screen: LRect, offset: f64) -> Option<(LRect, f64, f64)> {
    let dx = offset * screen.w;
    let x0 = (screen.x + dx).max(screen.x);
    let x1 = (screen.x + screen.w + dx).min(screen.x + screen.w);
    if x1 - x0 < 0.5 {
        return None;
    }
    let u0 = (x0 - (screen.x + dx)) / screen.w;
    let u1 = (x1 - (screen.x + dx)) / screen.w;
    Some((LRect::new(x0, screen.y, x1 - x0, screen.h), u0, u1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_neighbours_share_the_screen_and_meet_at_the_seam() {
        let mut s = Slides::default();
        s.begin(0, 1, 2);
        let slide = s.get(0).unwrap();
        // At rest on 1: only space 1, in place.
        assert_eq!(slide.views(4), vec![(1, 0.0)]);
        let mut slide = *slide;
        slide.pos.x = 1.25;
        let v = slide.views(4);
        assert_eq!(v.len(), 2);
        assert!((v[0].1 + 0.25).abs() < 1e-9 && (v[1].1 - 0.75).abs() < 1e-9);
        // Adjacent: the right edge of the left one is the left edge of the right.
        assert!((v[1].1 - v[0].1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_slide_runs_to_its_target_and_ends_once() {
        let mut s = Slides::default();
        s.begin(0, 0, 3);
        let mut n = 0;
        while s.step(1.0 / 60.0) {
            n += 1;
            assert!(n < 300);
            assert!(s.get(0).unwrap().pos.x <= 3.0);
        }
        assert!(!s.active(0));
        assert!(s.take_ended() && !s.take_ended());
    }

    #[test]
    fn a_press_mid_slide_bends_it_without_a_jump() {
        let mut s = Slides::default();
        s.begin(0, 0, 1);
        for _ in 0..8 {
            s.step(1.0 / 60.0);
        }
        let (x, v) = { let p = s.get(0).unwrap().pos; (p.x, p.v) };
        s.begin(0, 0, 2);
        let p = s.get(0).unwrap();
        assert_eq!((p.pos.x, p.pos.v, p.to, p.from), (x, v, 2, 0));
        // Back to where it started reverses smoothly.
        s.begin(0, 0, 0);
        assert!(s.get(0).unwrap().pos.v > 0.0);
        while s.step(1.0 / 60.0) {}
        assert!(!s.active(0));
    }

    #[test]
    fn the_first_frame_already_needs_both_captures() {
        let mut s = Slides::default();
        s.begin(0, 2, 3);
        let mut need = s.get(0).unwrap().needed(5);
        need.sort();
        assert_eq!(need, vec![2, 3]);
    }

    #[test]
    fn a_shifted_capture_is_cropped_to_the_display() {
        let screen = LRect::new(100.0, 0.0, 1000.0, 600.0);
        assert_eq!(crop(screen, 0.0).unwrap().1, 0.0);
        let (r, u0, u1) = crop(screen, -0.25).unwrap();
        assert_eq!((r.x, r.w), (100.0, 750.0));
        assert!((u0 - 0.25).abs() < 1e-9 && u1 == 1.0);
        let (r, u0, u1) = crop(screen, 0.75).unwrap();
        assert_eq!((r.x, r.w), (850.0, 250.0));
        assert!(u0 == 0.0 && (u1 - 0.25).abs() < 1e-9);
        assert!(crop(screen, 1.0).is_none() && crop(screen, -1.0).is_none());
    }
}
