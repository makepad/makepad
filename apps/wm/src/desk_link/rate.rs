//! Desk Link's adaptive stream rate: one total rate for the whole stream (the encoder splits it
//! over the tiles by area), decided once a second from what held the stream back.
//!
//! - Link-bound (a viewer's socket backlog held pictures back: `congested`): the rate goes down
//!   to below what was actually sent, the picture rate stays.
//! - Encoder-bound (pictures waited for the encoder, not for a viewer, and its slowest tile took
//!   most of a picture's time): that size takes the next step of [`effort`] (more streams, then
//!   the encoder's fastest tuning, then 4:2:0 last, only after [`HALF_AFTER`] seconds still
//!   bound at the fastest tuning), the picture rate stays. It steps back once
//!   it has run well clear of the budget for a while (twice as long after every step back that
//!   failed).
//! - Clean: the rate creeps back up, to the ceiling (the settings'
//!   `max_mbps` when set, the viewers' caps; none by default).
//!
//! The rate starts at [`START_KBPS`] (or the ceiling, if lower). A fixed `kbps` setting turns
//! all of this off.

use makepad_desk_link::limit;

/// Where a stream starts before anything was measured (all tiles).
pub const START_KBPS: u32 = 250_000;
/// The lowest the link can push the rate.
const FLOOR_KBPS: u32 = 20_000;
/// Congested this long in a second (ms): link-bound. Shorter holds are a burst draining (a
/// picture of all tiles at once is several ms of wire on 1 GbE), not a link that is too slow.
const CONGESTED_MS: f64 = 60.0;
/// Link-bound: the share of what got through the rate goes to.
const LINK_SHARE: f64 = 0.85;
/// Clean seconds before the rate creeps up.
const CREEP_AFTER: u32 = 3;
/// The first wait (clean seconds) before a size steps back to less encoder work; doubles per
/// failure.
const HALF_WAIT: u32 = 30;
const HALF_WAIT_MAX: u32 = 960;
/// Encoder-bound seconds in a row at the fastest 4:4:4 step before chroma goes: a reopened
/// encoder's first second (a key frame, its stream starting) is no measure of it.
const HALF_AFTER: u32 = 3;

/// How much encoder throughput a size asks for, least first: what an encoder-bound stream gives
/// up, one step at a time, until the encoder keeps the picture rate. 4:2:0 comes last: it is the
/// only step that loses picture detail.
pub mod effort {
    /// The encoder's best pictures per bit, as few streams as the size allows (one H.265 stream
    /// up to 8192 wide: no seams, one decoder).
    pub const SHARP: u8 = 0;
    /// The same, a stream per encode engine (stripes encoded at once).
    pub const SPREAD: u8 = 1;
    /// The encoder's fastest tuning (about 60% of the time), a stream per engine.
    pub const FAST: u8 = 2;
    /// And 4:2:0 (about half the encoder's work again).
    pub const HALF: u8 = 3;
}

/// What one second of streaming saw.
#[derive(Clone, Copy, Debug, Default)]
pub struct Second {
    /// Pictures a second the stream runs at.
    pub fps: u32,
    /// Pictures encoded.
    pub pictures: u64,
    /// Time a viewer was behind (its backlog held pictures back).
    pub congested_ms: f64,
    /// Frames whose picture was held back because the encoder still read the texture.
    pub held_encoder: u64,
    /// The slowest tile's encode of a picture, averaged over the second (ms).
    pub tile_ms: f64,
    /// What went out (kbit/s, all viewers' largest).
    pub sent_kbps: u64,
    pub size: [u32; 2],
    /// The most effort this stream may take ([`effort`]: the viewer's choices bound it, e.g. a
    /// forced 4:4:4 stops at `FAST`).
    pub max_effort: u8,
}

/// A size's step of [`effort`] (sizes never encoder-bound are at `SHARP` and not listed).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Step {
    size: [u32; 2],
    level: u8,
    /// Clean seconds before the next step back, and those so far.
    wait: u32,
    clean: u32,
    /// Encoder-bound seconds in a row at this level.
    bound: u32,
}

#[derive(Clone, Debug, Default)]
pub struct RateControl {
    /// 0: not started.
    kbps: u32,
    limit: u8,
    clean: u32,
    steps: Vec<Step>,
}

impl RateControl {
    /// The rate now under `ceiling` (kbit/s, all tiles).
    pub fn kbps(&self, ceiling: u32) -> u32 {
        let k = if self.kbps == 0 { START_KBPS } else { self.kbps };
        k.min(ceiling).max(1)
    }

    /// What holds the rate now (`limit::*`), under `ceiling`.
    pub fn limit(&self, ceiling: u32) -> u8 {
        let k = self.kbps(ceiling);
        if k >= ceiling && (self.kbps == 0 || self.limit == limit::CLEAN || k < self.kbps) {
            limit::CAP
        } else if self.kbps == 0 {
            limit::START
        } else {
            self.limit
        }
    }

    /// `size` starts from the top again (the viewer changed its choices).
    pub fn forget(&mut self, size: [u32; 2]) {
        self.steps.retain(|h| h.size != size);
    }

    /// The [`effort`] step `size` streams at (`max`: the most it may take).
    pub fn effort(&self, size: [u32; 2], max: u8) -> u8 {
        self.steps.iter().find(|h| h.size == size).map_or(effort::SHARP, |h| h.level).min(max)
    }

    /// One second; true when a size's effort changed (the encoder, maybe the capture, is made
    /// again).
    pub fn second(&mut self, s: &Second, ceiling: u32) -> bool {
        let budget_ms = 1000.0 / s.fps.max(1) as f64;
        let mut kbps = self.kbps(ceiling);
        let moving = s.pictures * 4 >= s.fps as u64;
        let encoder_bound = moving && s.held_encoder >= (s.fps as u64 / 20).max(2) && s.tile_ms >= budget_ms * 0.8;
        let mut changed = false;
        if s.congested_ms >= CONGESTED_MS {
            // what got through is what the link carries: the rate goes to most of that. Under
            // half the rate got through: a stall (a viewer that stopped reading), not a measure
            // of the link: down by a quarter. More than the rate got through: bursts, a little.
            let carried = (s.sent_kbps as f64 * LINK_SHARE).min(u32::MAX as f64) as u32;
            kbps = if carried < kbps / 2 {
                kbps * 3 / 4
            } else if carried < kbps {
                carried
            } else {
                kbps * 9 / 10
            }
            .max(FLOOR_KBPS);
            self.clean = 0;
            self.limit = limit::LINK;
        } else if encoder_bound {
            self.clean = 0;
            self.limit = limit::ENCODER;
            if self.effort(s.size, s.max_effort) < s.max_effort {
                match self.steps.iter_mut().find(|h| h.size == s.size) {
                    Some(h) => {
                        h.bound += 1;
                        let level = h.level.min(s.max_effort);
                        // 4:2:0 only when the fastest 4:4:4 stays behind (or a step back to it
                        // failed at once)
                        if level + 1 < effort::HALF || h.bound >= HALF_AFTER || h.clean == u32::MAX {
                            h.level = level + 1;
                            // a step back that failed: wait twice as long next time
                            if h.clean == u32::MAX {
                                h.wait = (h.wait * 2).min(HALF_WAIT_MAX);
                            }
                            h.clean = 0;
                            h.bound = 0;
                            changed = true;
                        }
                    }
                    None => {
                        self.steps.push(Step { size: s.size, level: effort::SPREAD.min(s.max_effort), wait: HALF_WAIT, clean: 0, bound: 0 });
                        changed = true;
                    }
                }
            }
        } else {
            self.clean += 1;
            if self.limit == limit::LINK || self.limit == limit::ENCODER || self.limit == limit::START {
                self.limit = limit::CLEAN;
            }
            // the rate is the encoder's average target: a desktop that needs less sends less
            // anyway, so it creeps whether used or not (a stream that went down for the link
            // and then sent little stayed down for good)
            if self.clean >= CREEP_AFTER && kbps < ceiling {
                kbps = (kbps + (kbps / 16).max(2_000)).min(ceiling);
            }
            // a step taken for the encoder's sake, now well clear of the budget: back in time
            // (a failure right after doubles the next wait: `clean` marks the step back)
            if let Some(h) = self.steps.iter_mut().find(|h| h.size == s.size && h.level > effort::SHARP) {
                h.bound = 0;
                if moving && s.held_encoder == 0 && s.tile_ms < budget_ms * 0.4 {
                    h.clean = if h.clean == u32::MAX { 1 } else { h.clean + 1 };
                    if h.clean >= h.wait {
                        h.level -= 1;
                        h.clean = u32::MAX;
                        changed = true;
                    }
                } else if h.clean == u32::MAX && !moving {
                    // still: nothing measured
                } else if h.clean == u32::MAX {
                    h.clean = 0;
                }
            }
        }
        self.kbps = kbps;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn second(congested_ms: f64, held_encoder: u64, tile_ms: f64, sent_kbps: u64, _full_chroma: bool) -> Second {
        Second { fps: 120, pictures: 110, congested_ms, held_encoder, tile_ms, sent_kbps, size: [7680, 2160], max_effort: effort::HALF }
    }

    #[test]
    fn starts_at_250_under_the_ceiling() {
        let r = RateControl::default();
        assert_eq!(r.kbps(400_000), 250_000);
        assert_eq!(r.limit(400_000), limit::START);
        assert_eq!(r.kbps(100_000), 100_000, "a small desktop's own rate is lower");
        assert_eq!(r.limit(100_000), limit::CAP);
    }

    #[test]
    fn the_link_lowers_the_rate_and_it_creeps_back() {
        let mut r = RateControl::default();
        // a busy link: congested, 230 Mbit/s got out: 85% of that
        assert!(!r.second(&second(120.0, 0, 5.0, 230_000, true), 400_000));
        assert_eq!(r.kbps(400_000), 195_500);
        assert_eq!(r.limit(400_000), limit::LINK);
        // a short hold (a burst draining) is not link-bound
        r.second(&second(40.0, 0, 5.0, 190_000, true), 400_000);
        assert_eq!((r.kbps(400_000), r.limit(400_000)), (195_500, limit::CLEAN));
        // a stall: little got out, not a measure of the link: a quarter down
        r.second(&second(900.0, 0, 5.0, 10_000, true), 400_000);
        assert_eq!(r.kbps(400_000), 146_625);
        // congested though more than the rate got out: bursts, a tenth down
        r.second(&second(80.0, 0, 5.0, 300_000, true), 400_000);
        assert_eq!(r.kbps(400_000), 131_962);
        r.second(&second(80.0, 0, 5.0, 120_000, true), 400_000);
        assert_eq!(r.kbps(400_000), 102_000);
        let base = 102_000;
        // clean: nothing for CREEP_AFTER - 1 seconds, then up by a sixteenth while it is used
        r.second(&second(0.0, 0, 5.0, 100_000, true), 400_000);
        r.second(&second(0.0, 0, 5.0, 100_000, true), 400_000);
        assert_eq!((r.kbps(400_000), r.limit(400_000)), (base, limit::CLEAN));
        r.second(&second(0.0, 0, 5.0, 100_000, true), 400_000);
        assert_eq!(r.kbps(400_000), base + base / 16);
        // little sent: it creeps all the same (the rate is only a target)
        r.second(&second(0.0, 0, 5.0, 10_000, true), 400_000);
        let up = base + base / 16;
        assert_eq!(r.kbps(400_000), up + up / 16);
        // never above the ceiling, said as the cap
        for _ in 0..60 {
            r.second(&second(0.0, 0, 5.0, 390_000, true), 400_000);
        }
        assert_eq!((r.kbps(400_000), r.limit(400_000)), (400_000, limit::CAP));
        // a lower cap later (a viewer's) holds it at once
        assert_eq!((r.kbps(150_000), r.limit(150_000)), (150_000, limit::CAP));
        // never below the floor
        for _ in 0..40 {
            r.second(&second(500.0, 0, 5.0, 1_000, true), 400_000);
        }
        assert_eq!(r.kbps(400_000), FLOOR_KBPS);
    }

    #[test]
    fn the_encoder_steps_up_one_at_a_time_and_back_in_time() {
        let mut r = RateControl::default();
        let size = [7680, 2160];
        let max = effort::HALF;
        // 7680x2160 H.265 4:4:4 sharp as one stream: ~17 ms of an 8.3 ms picture, the encoder
        // holds frames back, no viewer is behind: a stream per engine
        assert!(r.second(&second(0.0, 40, 17.0, 300_000, true), 400_000));
        assert_eq!(r.effort(size, max), effort::SPREAD);
        assert_eq!(r.limit(400_000), limit::ENCODER);
        assert_eq!(r.kbps(400_000), 250_000, "the rate stays: it was not the link");
        // still behind: fast; 4:2:0 only after HALF_AFTER seconds still behind there, and no
        // further
        assert!(r.second(&second(0.0, 40, 9.0, 300_000, true), 400_000));
        assert_eq!(r.effort(size, max), effort::FAST);
        for _ in 1..HALF_AFTER {
            assert!(!r.second(&second(0.0, 40, 9.0, 300_000, true), 400_000));
            assert_eq!(r.effort(size, max), effort::FAST, "4:4:4 holds while it may still catch up");
        }
        // a clean second in between starts the count again
        assert!(!r.second(&second(0.0, 0, 5.0, 300_000, true), 400_000));
        for _ in 1..HALF_AFTER {
            assert!(!r.second(&second(0.0, 40, 9.0, 300_000, true), 400_000));
        }
        assert!(r.second(&second(0.0, 40, 9.0, 300_000, true), 400_000));
        assert_eq!(r.effort(size, max), effort::HALF);
        assert!(!r.second(&second(0.0, 40, 9.0, 300_000, true), 400_000));
        // a held frame or two at a fast encode is not encoder-bound
        assert!(!r.second(&second(0.0, 1, 9.0, 300_000, false), 400_000));
        // ~6 ms is not clear of the budget: no step back
        for _ in 0..100 {
            assert!(!r.second(&second(0.0, 0, 6.0, 300_000, false), 400_000));
        }
        // well clear (~3 ms) for HALF_WAIT seconds: one step back
        for i in 0..HALF_WAIT {
            assert_eq!(r.second(&second(0.0, 0, 3.0, 300_000, false), 400_000), i + 1 == HALF_WAIT);
        }
        assert_eq!(r.effort(size, max), effort::FAST);
        // it fails at once: up again, and the next step back waits twice as long
        assert!(r.second(&second(0.0, 40, 9.0, 300_000, true), 400_000));
        assert_eq!(r.effort(size, max), effort::HALF);
        for _ in 0..HALF_WAIT {
            r.second(&second(0.0, 0, 3.0, 300_000, false), 400_000);
        }
        assert_eq!(r.effort(size, max), effort::HALF, "twice HALF_WAIT now");
        assert_eq!(r.effort([3840, 2160], max), effort::SHARP, "per size");
        // a viewer that wants 4:4:4: never 4:2:0
        assert_eq!(r.effort(size, effort::FAST), effort::FAST);
    }

    #[test]
    fn a_forced_444_stops_at_fast() {
        let mut r = RateControl::default();
        let s = |held| Second { fps: 120, pictures: 110, congested_ms: 0.0, held_encoder: held, tile_ms: 9.0, sent_kbps: 1, size: [7680, 4320], max_effort: effort::FAST };
        assert!(r.second(&s(40), 400_000));
        assert!(r.second(&s(40), 400_000));
        assert!(!r.second(&s(40), 400_000), "nothing more to give up");
        assert_eq!(r.effort([7680, 4320], effort::FAST), effort::FAST);
        assert_eq!(r.limit(400_000), limit::ENCODER);
    }

    #[test]
    fn a_still_desktop_decides_nothing() {
        let mut r = RateControl::default();
        let still = Second { fps: 120, pictures: 3, congested_ms: 0.0, held_encoder: 9, tile_ms: 20.0, sent_kbps: 2_000, size: [7680, 2160], max_effort: effort::HALF };
        assert!(!r.second(&still, 400_000));
        assert_eq!(r.effort([7680, 2160], effort::HALF), effort::SHARP);
    }
}
