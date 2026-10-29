//! The adaptive stream rate of every Desk Link stream (a desktop's and an XR picture's): one
//! total rate for the whole stream (the encoder splits it over the tiles by area), decided once
//! a second from what held the stream back.
//!
//! - Link-bound (a viewer's socket backlog held pictures back: `congested`; or the link's round
//!   trip grew past its idle one, [`LinkTrip`]: `link_queued`): the rate goes down to below
//!   what was actually sent, the picture rate stays.
//! - Encoder-bound (pictures waited for the encoder, not for a viewer, and its slowest tile took
//!   most of a picture's time): that size takes the next step of [`effort`] (more streams, then
//!   the encoder's fastest tuning, then 4:2:0 last, only after [`HALF_AFTER`] seconds still
//!   bound at the fastest tuning), the picture rate stays. It steps back once
//!   it has run well clear of the budget for a while (twice as long after every step back that
//!   failed).
//! - Clean: the rate creeps back up while pictures flow: freely up to the start rate (a stream
//!   that went down for the link and then sent little would stay down for good), above it only
//!   to [`PROVEN`] times what got through that second, so the bound comes from measurement (no
//!   fixed ceiling by default; the settings' `max_mbps` and the viewers' caps stay opt-in). A
//!   still desktop (or no viewer: no seconds at all) holds the rate.
//!
//! - Decode-bound (a watching viewer's decode of a picture, its slowest stream's, took most of a
//!   picture's time): the size streams in more H.265 bands, enough that each band's decode fits
//!   (the viewer decodes the bands at once, a thread each); the rate stays (the viewer's backlog
//!   that second is its decoder's, not the link's). A step that did not cut the decode time
//!   (a decoder that runs the bands one after another) is the last.
//!
//! A session starts at the last rate a viewer left with (its link, measured), else at
//! [`START_KBPS`] (or the ceiling, if lower). A fixed `kbps` setting turns all of this off.

use crate::limit;

/// Where a stream starts before anything was measured (all tiles).
pub const START_KBPS: u32 = 250_000;
/// The lowest the link can push the rate.
const FLOOR_KBPS: u32 = 20_000;
/// Congested this long in a second (ms): link-bound. Shorter holds are a burst draining (a
/// picture of all tiles at once is several ms of wire on 1 GbE), not a link that is too slow.
const CONGESTED_MS: f64 = 60.0;
/// Link-bound: the share of what got through the rate goes to.
const LINK_SHARE: f64 = 0.85;
/// Clean seconds (pictures flowing) before the rate creeps up.
const CREEP_AFTER: u32 = 3;
/// Above the start rate the creep goes at most to this times what got through in the second.
const PROVEN: f64 = 1.25;
/// The viewers whose last rate is remembered.
const REMEMBER: usize = 16;
/// A rate line again when the rate rose by this since the last one said (a creep is many steps).
const NOTE_RISE: f64 = 1.25;
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

/// A viewer's decode of a picture over this share of the picture's time: decode-bound. A
/// stream's decoder takes one picture at a time, so it keeps up until its decode nears the
/// picture's whole time (Apple M4: 8K wide in two bands decodes in 6.7 ms, 81% at 120, and
/// keeps 120).
const DECODE_BOUND: f64 = 0.9;
/// More bands aim the decode of a picture at this share of its time.
const DECODE_AIM: f64 = 0.7;
/// The most bands decode asks for.
pub const MAX_BANDS: u32 = 8;

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
    /// The slowest watching viewer's decode of a picture (ms, its slowest stream's; 0: none said).
    pub decode_ms: f64,
    /// The streams a picture is encoded in now (H.265 bands; 0: decode is no matter here, e.g.
    /// H.264 tiles, which the decoder's level already sizes).
    pub streams: u32,
    /// The link's round trip grew past its idle one this second ([`LinkTrip::queued`]): a
    /// queue on the way, link-bound as a viewer's backlog is.
    pub link_queued: bool,
}

/// A link's idle round trip, learned from the lowest seen (creeping up 2% a second so a changed
/// route is learned), and whether a second's round trips say it queues: its median
/// [`QUEUED_MS`] past the idle one. For a stream whose viewer reports each picture taken (the
/// XR headset) rather than a socket backlog.
#[derive(Clone, Copy, Debug, Default)]
pub struct LinkTrip {
    /// µs; 0 until the first second.
    base_us: u32,
}

/// How much longer than the idle link a second's median round trip may take before the link
/// counts as queueing (ms).
pub const QUEUED_MS: f64 = 10.0;

impl LinkTrip {
    pub fn base_ms(&self) -> f64 {
        self.base_us as f64 / 1000.0
    }

    /// A second's median and lowest round trip (ms; 0: none measured): whether the link queued.
    pub fn queued(&mut self, trip_ms: f64, trip_min_ms: f64) -> bool {
        if trip_ms <= 0.0 {
            return false;
        }
        let min_us = (trip_min_ms * 1000.0) as u32;
        let base = self.base_us;
        self.base_us = if base == 0 { min_us } else { min_us.min(base + base / 50 + 1) };
        trip_ms > self.base_ms() + QUEUED_MS
    }
}

/// The bands a size streams in for the viewers' decoders, and the decode that asked for them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bands {
    size: [u32; 2],
    bands: u32,
    /// The decode (ms) with the streams before this step.
    decode_ms: f64,
    streams: u32,
    /// More bands did not help: no further step.
    last: bool,
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
    /// The lowest the link pushes the rate (0: [`FLOOR_KBPS`]).
    floor: u32,
    limit: u8,
    clean: u32,
    steps: Vec<Step>,
    bands: Vec<Bands>,
    /// Seconds still to skip after the bands changed (the viewer's stats describe the old ones).
    settle: u32,
    /// The rate each viewer last left with (newest last).
    known: Vec<(String, u32)>,
    /// The rate and limit the last rate line said.
    noted: Option<(u32, u8)>,
}

impl RateControl {
    /// A controller whose link-bound rate never goes below `kbps`.
    pub fn with_floor(kbps: u32) -> RateControl {
        RateControl { floor: kbps, ..RateControl::default() }
    }

    fn floor(&self) -> u32 {
        if self.floor == 0 { FLOOR_KBPS } else { self.floor }
    }

    /// From the start again: the rate the next [`Self::kbps`] says is the start rate under its
    /// ceiling (the app changed the stream's bitrate: it applies at once).
    pub fn restart(&mut self) {
        self.kbps = 0;
        self.limit = limit::START;
        self.clean = 0;
    }

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

    /// A session starts (the first watching viewer, `who`): from the rate `who` last left with,
    /// else from [`START_KBPS`]; what the last session measured is not this viewer's link.
    pub fn begin(&mut self, who: &str) {
        self.kbps = self.known.iter().rev().find(|k| k.0 == who).map_or(0, |k| k.1);
        self.limit = limit::START;
        self.clean = 0;
    }

    /// `who` leaves: the rate now is the last good one for its next session.
    pub fn remember(&mut self, who: &str) {
        if self.kbps == 0 {
            return;
        }
        self.known.retain(|k| k.0 != who);
        if self.known.len() >= REMEMBER {
            self.known.remove(0);
        }
        self.known.push((who.to_string(), self.kbps));
    }

    /// Whether a rate line is due for `kbps` held by `limit`: a new limit, a drop, or a rise of
    /// [`NOTE_RISE`] since the last line (a creep is not a line a second).
    pub fn note_due(&mut self, kbps: u32, limit: u8) -> bool {
        let due = match self.noted {
            None => true,
            Some((k, l)) => l != limit || kbps < k || kbps as f64 >= k as f64 * NOTE_RISE,
        };
        if due {
            self.noted = Some((kbps, limit));
        }
        due
    }

    /// `size` starts from the top again (the viewer changed its choices).
    pub fn forget(&mut self, size: [u32; 2]) {
        self.steps.retain(|h| h.size != size);
    }

    /// The bands the viewers' decode asked for go (a viewer left: maybe the slow one).
    pub fn forget_decode(&mut self) {
        self.bands.clear();
    }

    /// The fewest H.265 bands the viewers' measured decode asks of `size` (1: no ask), and the
    /// decode (ms) that asked.
    pub fn decode_bands(&self, size: [u32; 2]) -> (u32, f64) {
        self.bands.iter().find(|b| b.size == size).map_or((1, 0.0), |b| (b.bands, b.decode_ms))
    }

    /// A viewer's decode was behind: `size` (now in `s.streams` streams) takes enough bands for
    /// each to decode in [`DECODE_AIM`] of the picture's time. True when the bands changed.
    fn decode_step(&mut self, s: &Second, budget_ms: f64) -> bool {
        let at = self.bands.iter().position(|b| b.size == s.size);
        if let Some(b) = at.map(|i| self.bands[i]) {
            if b.last {
                return false;
            }
            // more bands that did not cut the decode by a fifth: they decode one after another;
            // back to the bands before (fewer seams), and no further step
            if s.streams > b.streams && s.decode_ms > b.decode_ms * 0.8 {
                let back = &mut self.bands[at.unwrap()];
                back.last = true;
                back.bands = b.streams;
                self.settle = 2;
                return true;
            }
        }
        // enough bands for each to decode in DECODE_AIM of the picture's time, at most twice as
        // many a step (a decoder that runs them one after another shows after one step)
        let streams = s.streams.max(1);
        let want = ((streams as f64 * s.decode_ms / (budget_ms * DECODE_AIM)).ceil() as u32).clamp(streams + 1, streams * 2).min(MAX_BANDS);
        if want <= streams {
            return false;
        }
        let b = Bands { size: s.size, bands: want, decode_ms: s.decode_ms, streams, last: false };
        match at {
            Some(i) => self.bands[i] = b,
            None => self.bands.push(b),
        }
        self.settle = 2;
        true
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
        let decode_bound = moving && s.streams > 0 && s.decode_ms >= budget_ms * DECODE_BOUND;
        let mut changed = false;
        if self.settle > 0 {
            // the bands just changed: what the viewers said was of the old ones
            self.settle -= 1;
        } else if decode_bound {
            // the viewer's backlog is its decoder's: more bands, not less rate
            self.clean = 0;
            changed = self.decode_step(s, budget_ms);
        } else if s.congested_ms >= CONGESTED_MS || s.link_queued {
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
            .max(self.floor());
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
            if self.limit == limit::LINK || self.limit == limit::ENCODER || self.limit == limit::START {
                self.limit = limit::CLEAN;
            }
            // only seconds with pictures say anything about the link: a still desktop (frames 0)
            // holds the rate (it crept all night to 1 Tbit/s once)
            if moving && s.sent_kbps > 0 {
                self.clean += 1;
            }
            // up to the start rate whatever was used (a desktop that needs less sends less
            // anyway, and a stream that went down for the link and then sent little would stay
            // down for good); above it only to what the link proved it carries
            if self.clean >= CREEP_AFTER && moving && s.sent_kbps > 0 {
                let proven = (s.sent_kbps as f64 * PROVEN).min(u32::MAX as f64) as u32;
                let bound = proven.max(START_KBPS).min(ceiling);
                if kbps < bound {
                    kbps = (kbps + (kbps / 16).max(2_000)).min(bound);
                }
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
        Second { fps: 120, pictures: 110, congested_ms, held_encoder, tile_ms, sent_kbps, size: [7680, 2160], max_effort: effort::HALF, decode_ms: 0.0, streams: 2, link_queued: false }
    }

    #[test]
    fn a_viewer_behind_on_decode_gets_more_bands_not_less_rate() {
        let mut r = RateControl::default();
        let size = [7680, 2160];
        let s = |decode_ms: f64, streams: u32| Second { decode_ms, streams, congested_ms: 400.0, ..second(400.0, 0, 5.0, 100_000, true) };
        // one stream that takes 12 ms of an 8.3 ms picture: 12 / 5.8 ms (70%) -> 3, twice at most: 2
        assert!(r.second(&s(12.0, 1), 400_000));
        assert_eq!(r.decode_bands(size), (2, 12.0));
        assert_eq!(r.kbps(400_000), START_KBPS, "the viewer's backlog was its decoder's");
        // the next two seconds describe the old bands: nothing decided
        assert!(!r.second(&s(12.0, 1), 400_000));
        assert!(!r.second(&s(12.0, 1), 400_000));
        assert_eq!(r.decode_bands(size), (2, 12.0));
        // two bands still behind (a slower decoder): more
        assert!(r.second(&s(8.0, 2), 400_000));
        assert_eq!(r.decode_bands(size).0, 3);
        r.second(&s(8.0, 2), 400_000);
        r.second(&s(8.0, 2), 400_000);
        // three that decode no faster: they run one after another: back to two, no further step
        assert!(r.second(&s(7.8, 3), 400_000));
        assert_eq!(r.decode_bands(size).0, 2);
        for _ in 0..5 {
            assert!(!r.second(&s(8.0, 2), 400_000));
        }
        assert_eq!(r.decode_bands(size).0, 2);
        // a far slower one: twice the bands a step at most
        let mut r = RateControl::default();
        assert!(r.second(&s(30.0, 2), 400_000));
        assert_eq!(r.decode_bands(size).0, 4);
        // a decode within the picture's time decides nothing about bands (the M4's two 8K bands)
        let mut r = RateControl::default();
        assert!(!r.second(&Second { decode_ms: 6.7, streams: 2, ..second(0.0, 0, 5.0, 100_000, true) }, 400_000));
        assert_eq!(r.decode_bands(size), (1, 0.0));
        // no word from the viewer, or H.264 tiles: nothing either
        assert!(!r.second(&Second { decode_ms: 20.0, streams: 0, ..second(0.0, 0, 5.0, 100_000, true) }, 400_000));
        assert_eq!(r.decode_bands(size), (1, 0.0));
        // a viewer left: from the top
        r.decode_step(&Second { decode_ms: 9.6, streams: 1, ..second(0.0, 0, 5.0, 100_000, true) }, 1000.0 / 120.0);
        assert_eq!(r.decode_bands(size).0, 2);
        r.forget(size);
        assert_eq!(r.decode_bands(size).0, 2, "the viewer's choices change no decoder");
        r.forget_decode();
        assert_eq!(r.decode_bands(size), (1, 0.0));
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
        let s = |held| Second { fps: 120, pictures: 110, congested_ms: 0.0, held_encoder: held, tile_ms: 9.0, sent_kbps: 1, size: [7680, 4320], max_effort: effort::FAST, decode_ms: 0.0, streams: 2, link_queued: false };
        assert!(r.second(&s(40), 400_000));
        assert!(r.second(&s(40), 400_000));
        assert!(!r.second(&s(40), 400_000), "nothing more to give up");
        assert_eq!(r.effort([7680, 4320], effort::FAST), effort::FAST);
        assert_eq!(r.limit(400_000), limit::ENCODER);
    }

    #[test]
    fn no_pictures_hold_the_rate() {
        // the 203 journal: frames 0 all night, the rate crept to 1 Tbit/s
        let mut r = RateControl::default();
        let none = Second { pictures: 0, sent_kbps: 0, ..second(0.0, 0, 0.0, 0, true) };
        for _ in 0..10_000 {
            r.second(&none, u32::MAX);
        }
        assert_eq!(r.kbps(u32::MAX), START_KBPS);
        // a link-lowered rate holds too while nothing moves
        r.second(&second(120.0, 0, 5.0, 100_000, true), u32::MAX);
        let low = r.kbps(u32::MAX);
        for _ in 0..100 {
            r.second(&none, u32::MAX);
        }
        assert_eq!(r.kbps(u32::MAX), low);
    }

    #[test]
    fn uncapped_the_creep_is_bounded_by_what_got_through() {
        let mut r = RateControl::default();
        // a desktop that needs 100 Mbit/s: the target stays at the start rate
        for _ in 0..1000 {
            r.second(&second(0.0, 0, 5.0, 100_000, true), u32::MAX);
        }
        assert_eq!(r.kbps(u32::MAX), START_KBPS);
        // one that uses all of it and gets 300 through: up to 1.25x that, never further
        for _ in 0..1000 {
            r.second(&second(0.0, 0, 5.0, 300_000, true), u32::MAX);
        }
        assert_eq!(r.kbps(u32::MAX), 375_000);
        // each step is a sixteenth at most (and only while it is used)
        let mut r = RateControl::default();
        for _ in 0..CREEP_AFTER {
            r.second(&second(0.0, 0, 5.0, 1_000_000, true), u32::MAX);
        }
        assert_eq!(r.kbps(u32::MAX), START_KBPS + START_KBPS / 16);
        // a set ceiling still holds
        for _ in 0..1000 {
            r.second(&second(0.0, 0, 5.0, 1_000_000, true), 400_000);
        }
        assert_eq!((r.kbps(400_000), r.limit(400_000)), (400_000, limit::CAP));
    }

    #[test]
    fn a_session_starts_from_the_viewers_last_rate_or_the_start() {
        let mut r = RateControl::default();
        r.begin("mac@10.0.0.2");
        for _ in 0..1000 {
            r.second(&second(0.0, 0, 5.0, 300_000, true), u32::MAX);
        }
        r.remember("mac@10.0.0.2");
        // another viewer: not what this one's link carried
        r.begin("phone@10.0.0.9");
        assert_eq!((r.kbps(u32::MAX), r.limit(u32::MAX)), (START_KBPS, limit::START));
        r.second(&second(120.0, 0, 5.0, 200_000, true), u32::MAX);
        r.remember("phone@10.0.0.9");
        // each comes back at its own
        r.begin("mac@10.0.0.2");
        assert_eq!(r.kbps(u32::MAX), 375_000);
        r.begin("phone@10.0.0.9");
        assert_eq!(r.kbps(u32::MAX), 170_000, "85% of what its link carried");
    }

    #[test]
    fn a_rate_line_on_change_not_every_creep() {
        let mut r = RateControl::default();
        assert!(r.note_due(250_000, limit::CLEAN));
        assert!(!r.note_due(265_625, limit::CLEAN), "one creep step");
        assert!(!r.note_due(300_000, limit::CLEAN));
        assert!(r.note_due(312_500, limit::CLEAN), "a quarter up since the last line");
        assert!(r.note_due(300_000, limit::LINK), "the link");
        assert!(r.note_due(290_000, limit::LINK), "every drop");
        assert!(r.note_due(290_000, limit::CAP), "a new limit");
    }

    #[test]
    fn a_queueing_link_lowers_the_rate_and_a_restart_takes_the_cap_at_once() {
        // an XR stream: one engine-bound shape (no effort steps), the headset's reports say the
        // link's round trip
        let mut trip = LinkTrip::default();
        assert!(!trip.queued(0.0, 0.0), "nothing measured");
        assert!(!trip.queued(6.0, 4.0));
        assert_eq!(trip.base_ms(), 4.0);
        assert!(!trip.queued(13.0, 3.0), "within QUEUED_MS of the idle trip");
        assert!(trip.queued(25.0, 9.0), "a queue: the idle trip holds (it creeps up 2% a second)");
        assert!(trip.base_ms() < 3.1);
        let mut r = RateControl::with_floor(15_000);
        let xr = |sent_kbps: u64, link_queued: bool| Second { fps: 90, pictures: 88, sent_kbps, size: [3712, 2784], max_effort: effort::SHARP, link_queued, ..Second::default() };
        assert_eq!(r.kbps(100_000), 100_000, "starts at the app's bitrate");
        r.second(&xr(90_000, true), 100_000);
        assert_eq!(r.kbps(100_000), 76_500);
        assert_eq!(r.limit(100_000), limit::LINK);
        assert!(!r.second(&xr(70_000, false), 100_000), "no effort steps for a fixed shape");
        for _ in 0..40 {
            r.second(&xr(3_000, true), 100_000);
        }
        assert_eq!(r.kbps(100_000), 15_000, "its own floor");
        r.restart();
        assert_eq!(r.kbps(120_000), 120_000, "a new bitrate applies at once");
    }

    #[test]
    fn a_still_desktop_decides_nothing() {
        let mut r = RateControl::default();
        let still = Second { fps: 120, pictures: 3, congested_ms: 0.0, held_encoder: 9, tile_ms: 20.0, sent_kbps: 2_000, size: [7680, 2160], max_effort: effort::HALF, decode_ms: 0.0, streams: 2, link_queued: false };
        assert!(!r.second(&still, 400_000));
        assert_eq!(r.effort([7680, 2160], effort::HALF), effort::SHARP);
    }
}
