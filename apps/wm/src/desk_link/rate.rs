//! Desk Link's adaptive stream rate: one total rate for the whole stream (the encoder splits it
//! over the tiles by area), decided once a second from what held the stream back.
//!
//! - Link-bound (a viewer's socket backlog held pictures back: `congested`): the rate goes down
//!   to below what was actually sent, the picture rate stays.
//! - Encoder-bound (pictures waited for the encoder, not for a viewer, and its slowest tile took
//!   most of a picture's time): a 4:4:4 stream at that size goes 4:2:0 (about half the encode
//!   work), the picture rate stays. It tries 4:4:4 again once 4:2:0 has run well clear of the
//!   budget for a while (twice as long after every try that failed).
//! - Clean: the rate creeps back up, to the ceiling (the size's own rate, the settings'
//!   `max_mbps`, the viewers' caps).
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
/// The first wait (clean seconds at 4:2:0) before a size tries 4:4:4 again; doubles per failure.
const HALF_WAIT: u32 = 30;
const HALF_WAIT_MAX: u32 = 960;

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
    /// The stream was 4:4:4.
    pub full_chroma: bool,
}

/// A size that streams 4:2:0 because the encoder could not keep up at 4:4:4.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Half {
    size: [u32; 2],
    /// Streams 4:2:0 now (false: trying 4:4:4 again).
    active: bool,
    /// Clean seconds at 4:2:0 before the next try, and those so far.
    wait: u32,
    clean: u32,
}

#[derive(Clone, Debug, Default)]
pub struct RateControl {
    /// 0: not started.
    kbps: u32,
    limit: u8,
    clean: u32,
    half: Vec<Half>,
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

    /// `size` streams 4:2:0 for the encoder's sake.
    pub fn half_chroma(&self, size: [u32; 2]) -> bool {
        self.half.iter().any(|h| h.size == size && h.active)
    }

    /// One second; true when a size's chroma changed (the capture is made again).
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
            if s.full_chroma {
                match self.half.iter_mut().find(|h| h.size == s.size) {
                    // a try at 4:4:4 that failed: wait twice as long next time
                    Some(h) => {
                        h.active = true;
                        h.wait = (h.wait * 2).min(HALF_WAIT_MAX);
                        h.clean = 0;
                    }
                    None => self.half.push(Half { size: s.size, active: true, wait: HALF_WAIT, clean: 0 }),
                }
                changed = true;
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
            // 4:2:0 for the encoder's sake, now well clear of the budget: 4:4:4 again in time
            if let Some(h) = self.half.iter_mut().find(|h| h.size == s.size && h.active) {
                if moving && s.held_encoder == 0 && s.tile_ms < budget_ms * 0.4 {
                    h.clean += 1;
                    if h.clean >= h.wait {
                        h.active = false;
                        changed = true;
                    }
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

    fn second(congested_ms: f64, held_encoder: u64, tile_ms: f64, sent_kbps: u64, full_chroma: bool) -> Second {
        Second { fps: 120, pictures: 110, congested_ms, held_encoder, tile_ms, sent_kbps, size: [7680, 2160], full_chroma }
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
    fn the_encoder_takes_444_away_and_gives_it_back_in_time() {
        let mut r = RateControl::default();
        let size = [7680, 2160];
        // a 4090 at 4:4:4 7680x2160: tiles take ~14 ms of an 8.3 ms picture, the encoder holds
        // frames back, no viewer is behind
        assert!(r.second(&second(0.0, 40, 14.0, 300_000, true), 400_000));
        assert!(r.half_chroma(size));
        assert_eq!(r.limit(400_000), limit::ENCODER);
        assert_eq!(r.kbps(400_000), 250_000, "the rate stays: it was not the link");
        // a held frame or two at a fast encode is not encoder-bound
        assert!(!r.second(&second(0.0, 1, 14.0, 300_000, false), 400_000));
        // 4:2:0 at ~6 ms is not clear of the budget: no try
        for _ in 0..100 {
            assert!(!r.second(&second(0.0, 0, 6.0, 300_000, false), 400_000));
        }
        // well clear (~3 ms) for HALF_WAIT seconds: 4:4:4 again
        for i in 0..HALF_WAIT {
            assert_eq!(r.second(&second(0.0, 0, 3.0, 300_000, false), 400_000), i + 1 == HALF_WAIT);
        }
        assert!(!r.half_chroma(size));
        // it fails again: back to 4:2:0, and the next try waits twice as long
        assert!(r.second(&second(0.0, 40, 14.0, 300_000, true), 400_000));
        assert!(r.half_chroma(size));
        for _ in 0..HALF_WAIT {
            r.second(&second(0.0, 0, 3.0, 300_000, false), 400_000);
        }
        assert!(r.half_chroma(size), "twice HALF_WAIT now");
        assert!(!r.half_chroma([3840, 2160]), "per size");
    }

    #[test]
    fn a_still_desktop_decides_nothing() {
        let mut r = RateControl::default();
        let still = Second { fps: 120, pictures: 3, congested_ms: 0.0, held_encoder: 9, tile_ms: 20.0, sent_kbps: 2_000, size: [7680, 2160], full_chroma: true };
        assert!(!r.second(&still, 400_000));
        assert!(!r.half_chroma([7680, 2160]));
    }
}
