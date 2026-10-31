//! Compact forms: the window manager's half of the contract with an app that
//! can show itself small (the app's half, with the wire types, is the module
//! docs block in `platform/src/ime.rs`: `CompactForm`, `HostedCompactForms`,
//! `HostedParked`, `HostedCompact`, `Cx::declare_compact_forms`).
//!
//! An app declares its compact forms (name and client-area size in points)
//! once, at connect. One that declares nothing is never sent anything here: it
//! stays a scaled live view, input mapped through the scale. For one that
//! does, parking runs this sequence (`t` = time since the step before; every
//! wait is bounded by [`ANSWER_SECS`], 300 ms):
//!
//! ```text
//!  park   wm  --HostedParked{scale, max}-->  app    (scale <= PARK_HINT_BELOW, window
//!                                                    not being dragged; `max` = largest
//!                                                    client area that fits the slot)
//!         the scaled live view keeps shrinking into its slot (the desk spring)
//!  answer app --HostedCompact{form: i}-->    wm     (layout switched; form i fits `max`,
//!                                                    else/None/silent for 300 ms: the
//!                                                    window stays a scaled live view and
//!                                                    the app is sent HostedParked{None})
//!         wm configures the client at the form's size and FREEZES the scaled view
//!         (its last capture keeps drawing and keeps moving with the spring)
//!  frame  app presents its first frame at the form's size (waited for 300 ms)
//!  swap   when that frame is in AND the spring has settled, the scaled capture
//!         cross-fades (FADE_SECS) to the 1:1 compact surface, centred on the slot
//!  unpark wm  --HostedParked{None}-->        app    (scale > PARK_HINT_ABOVE, or the
//!                                                    window leaves its zone)
//!         wm configures the client at the full size, freezes the compact surface
//!  answer app --HostedCompact{None}-->       wm     (full layout is back; the wm does not
//!                                                    wait for it, the frame is the signal)
//!  frame  app presents its first full-size frame (300 ms)
//!  swap   the compact capture cross-fades to the scaled live view, which grows
//! ```
//!
//! A form is a preferred size and the narrowest width it lays out at
//! (`CompactForm::flexible`): the client area granted is `granted(max)`, the
//! preferred width narrowed to the slot (a side zone on a small display is
//! narrower than any fixed strip), never under the form's minimum.
//!
//! The app must not: resize its window itself while parked (the host owns the
//! size; a form is a size the host grants and the host picks nothing else),
//! answer with a form that does not fit `max` (it is refused and undone), or
//! rely on getting a second `HostedParked` (one per park, one per unpark).
//! Hysteresis: a window is told it is parked at or below `PARK_HINT_BELOW`
//! and told it is back above `PARK_HINT_ABOVE`.
//!
//! This file is the pure part: the per-client state machine (`Compact`) and
//! the compact rect. `desk.rs` draws it (freeze, cross-fade) and sends the
//! messages, `input.rs` routes the pointer to it.
use super::Pose;
use crate::layout::LRect;
use makepad_widgets::makepad_platform::ime::{CompactForm, HostedParked, COMPACT_ANSWER_SECS};
use std::collections::HashMap;

/// How long the host waits for the app's answer, and then for the first
/// frame at the answered size.
pub const ANSWER_SECS: f64 = COMPACT_ANSWER_SECS;
/// The cross-fade between the scaled view and the compact surface.
pub const FADE_SECS: f64 = 0.14;
/// A window parked at or below this scale is told it is parked.
pub const PARK_HINT_BELOW: f64 = 0.6;
/// ...and told it is back at full size above this one (hysteresis: a window
/// dragged across the threshold does not flip its layout every frame).
pub const PARK_HINT_ABOVE: f64 = 0.7;

/// The largest client area a compact form may have in a shelf `zone_w` wide on
/// a display `screen_h` high: the zone less a gap each side,
/// at most half the display's height, less the window's frame.
pub fn slot_max(zone_w: f64, screen_h: f64, frame: (f64, f64)) -> [f64; 2] {
    [
        (zone_w - 2.0 * super::GAP - frame.0).max(0.0),
        ((screen_h - 3.0 * super::GAP) * 0.5 - frame.1).max(0.0),
    ]
}

/// The size of a compact window's slot, frame included.
pub fn slot_size(size: (f64, f64), frame: (f64, f64)) -> (f64, f64) {
    (size.0 + frame.0, size.1 + frame.1)
}

/// The rect a compact window is drawn at: its slot (`slot_size`) centred on
/// its parked slot, kept inside its zone (`span`, the zone's x range) and on
/// the display.
pub fn compact_rect(shown: LRect, slot: (f64, f64), span: (f64, f64), screen: LRect) -> LRect {
    let (w, h) = slot;
    let (cx, cy) = (shown.x + shown.w * 0.5, shown.y + shown.h * 0.5);
    LRect::new(
        (cx - w * 0.5).clamp(span.0, (span.1 - w).max(span.0)),
        (cy - h * 0.5).clamp(screen.y, (screen.y + screen.h - h).max(screen.y)),
        w,
        h,
    )
}

/// Where one client is in the sequence above.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Phase {
    /// Not told it is parked (or parked without forms).
    #[default]
    Idle,
    /// Told it is parked; no answer yet.
    Offered { at: f64 },
    /// Answered with `form`; the client is at its size, the scaled view frozen.
    Preparing { form: usize, at: f64 },
    /// First compact frame in: the scaled capture fades out over the compact surface.
    Fading { form: usize, t: f64 },
    Compact { form: usize },
    /// Told it is unparked; the client is at the full size again, the last
    /// view (`was`: the compact form, if it was compact) frozen.
    Restoring { at: f64, was: Option<usize> },
    /// First full frame in: the compact capture fades out over the live view.
    Returning { t: f64 },
    /// The app declined, did not answer, or sent a form that does not fit:
    /// a scaled live view until it is unparked.
    Declined,
}

#[derive(Clone, Debug, Default)]
struct Entry {
    forms: Vec<CompactForm>,
    phase: Phase,
    /// The host believes the app was told it is parked.
    told: bool,
    /// The `max` it was told.
    max: [f64; 2],
}

/// What a frame of a client's draw did to its state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tick {
    pub send: Option<HostedParked>,
    /// A cross-fade began: the frozen capture becomes the outgoing layer.
    pub swapped: bool,
}

fn unparked() -> HostedParked {
    HostedParked { scale: None, max: [0.0, 0.0] }
}

fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Per client: the declared forms and the phase.
#[derive(Clone, Debug, Default)]
pub struct Compact {
    clients: HashMap<u64, Entry>,
}

impl Compact {
    /// The app declared its forms (at connect).
    pub fn declare(&mut self, client: u64, forms: Vec<CompactForm>) {
        self.clients.insert(client, Entry { forms, ..Entry::default() });
    }
    pub fn phase(&self, client: u64) -> Phase {
        self.clients.get(&client).map_or(Phase::Idle, |e| e.phase)
    }
    fn form_size(&self, client: u64, form: usize) -> Option<(f64, f64)> {
        let e = self.clients.get(&client)?;
        e.forms.get(form).map(|f| f.granted(e.max))
    }

    /// Each frame of a parked-or-not client: `scale` is its shown scale when
    /// it is parked, `dragged` that the pointer holds it, `max` the slot
    /// (`slot_max`). Returns what to send, if anything: the park message on
    /// entering (not while dragged: a drag across a zone is not a park), the
    /// unpark message on leaving.
    pub fn poll(&mut self, client: u64, scale: Option<f64>, dragged: bool, max: [f64; 2], now: f64) -> Option<HostedParked> {
        let e = self.clients.get_mut(&client)?;
        let want = scale.is_some_and(|s| s <= PARK_HINT_BELOW || (e.told && s <= PARK_HINT_ABOVE));
        match (want && (e.told || !dragged), e.told) {
            (true, false) => {
                e.told = true;
                e.max = max;
                e.phase = Phase::Offered { at: now };
                Some(HostedParked { scale: scale.map(|s| s as f32), max })
            }
            (false, true) => {
                e.told = false;
                let was = e.phase;
                e.phase = match was {
                    Phase::Compact { form } | Phase::Fading { form, .. } => Phase::Restoring { at: now, was: Some(form) },
                    Phase::Preparing { .. } => Phase::Restoring { at: now, was: None },
                    _ => Phase::Idle,
                };
                // A declined app was already told (or never switched).
                (was != Phase::Declined).then(unparked)
            }
            _ => None,
        }
    }

    /// The app's answer. A form that does not fit the slot it was told is
    /// refused: the answer comes back as the message that undoes it.
    pub fn answer(&mut self, client: u64, form: Option<u32>, now: f64) -> Option<HostedParked> {
        let e = self.clients.get_mut(&client)?;
        let Phase::Offered { .. } = e.phase else { return None };
        let fits = form.map(|f| f as usize).filter(|f| e.forms.get(*f).is_some_and(|c| c.fits(e.max)));
        match fits {
            Some(form) => {
                e.phase = Phase::Preparing { form, at: now };
                None
            }
            None => {
                e.phase = Phase::Declined;
                form.map(|_| unparked())
            }
        }
    }

    /// Each drawn frame of a client: `frame_ready` is that it presented a
    /// frame at the size it is configured at, `settled` that the scaled view
    /// reached its slot. Moves the waits along (and times them out).
    pub fn tick(&mut self, client: u64, now: f64, frame_ready: bool, settled: bool) -> Tick {
        let Some(e) = self.clients.get_mut(&client) else { return Tick::default() };
        let mut out = Tick::default();
        match e.phase {
            Phase::Offered { at } if now - at > ANSWER_SECS => {
                e.phase = Phase::Declined;
                out.send = Some(unparked());
            }
            Phase::Preparing { form, .. } if frame_ready && settled => {
                e.phase = Phase::Fading { form, t: 0.0 };
                out.swapped = true;
            }
            Phase::Preparing { at, .. } if !frame_ready && now - at > ANSWER_SECS => {
                e.phase = Phase::Declined;
                out.send = Some(unparked());
            }
            Phase::Restoring { was, .. } if frame_ready => match was {
                Some(_) => {
                    e.phase = Phase::Returning { t: 0.0 };
                    out.swapped = true;
                }
                None => e.phase = Phase::Idle,
            },
            Phase::Restoring { at, .. } if now - at > ANSWER_SECS => e.phase = Phase::Idle,
            _ => {}
        }
        out
    }

    /// Advance the cross-fades; true while anything is fading or waiting
    /// (the desk keeps drawing).
    pub fn step(&mut self, dt: f64) -> bool {
        let mut busy = false;
        for e in self.clients.values_mut() {
            e.phase = match e.phase {
                Phase::Fading { form, t } if t + dt / FADE_SECS >= 1.0 => Phase::Compact { form },
                Phase::Fading { form, t } => Phase::Fading { form, t: t + dt / FADE_SECS },
                Phase::Returning { t } if t + dt / FADE_SECS >= 1.0 => Phase::Idle,
                Phase::Returning { t } => Phase::Returning { t: t + dt / FADE_SECS },
                p => p,
            };
            busy |= matches!(
                e.phase,
                Phase::Offered { .. } | Phase::Preparing { .. } | Phase::Fading { .. } | Phase::Restoring { .. } | Phase::Returning { .. }
            );
        }
        busy
    }

    /// The size the client is configured at while compact (also the size its
    /// slot is packed at): from the answer until the unpark.
    pub fn size(&self, client: u64) -> Option<(f64, f64)> {
        match self.phase(client) {
            Phase::Preparing { form, .. } | Phase::Fading { form, .. } | Phase::Compact { form } => self.form_size(client, form),
            _ => None,
        }
    }
    /// The size of the compact surface the window DRAWS as: from the swap on,
    /// until the unpark's swap.
    pub fn drawn_size(&self, client: u64) -> Option<(f64, f64)> {
        match self.phase(client) {
            Phase::Fading { form, .. } | Phase::Compact { form } | Phase::Restoring { was: Some(form), .. } => self.form_size(client, form),
            _ => None,
        }
    }
    /// The last capture stays on screen, the live view waits for its frame.
    pub fn frozen(&self, client: u64) -> bool {
        matches!(self.phase(client), Phase::Preparing { .. } | Phase::Restoring { .. })
    }
    /// The opacity of the outgoing layer of a cross-fade (None: no fade).
    pub fn fade(&self, client: u64) -> Option<f64> {
        match self.phase(client) {
            Phase::Fading { t, .. } | Phase::Returning { t } => Some(1.0 - smooth(t)),
            _ => None,
        }
    }
    /// Test helper: run a client through the whole parking sequence.
    #[cfg(test)]
    pub fn park_compact(&mut self, client: u64, forms: Vec<CompactForm>, form: u32) {
        self.declare(client, forms);
        self.poll(client, Some(0.2), false, [10_000.0, 10_000.0], 0.0).expect("told");
        self.answer(client, Some(form), 0.0);
        self.tick(client, 0.1, true, true);
        while self.step(1.0 / 60.0) {}
    }
    /// Forget clients that are gone.
    pub fn retain(&mut self, mut keep: impl FnMut(u64) -> bool) {
        self.clients.retain(|c, _| keep(*c));
    }
    /// Does any client wait on a timer or a fade?
    pub fn busy(&self) -> bool {
        self.clients.values().any(|e| {
            matches!(
                e.phase,
                Phase::Offered { .. } | Phase::Preparing { .. } | Phase::Fading { .. } | Phase::Restoring { .. } | Phase::Returning { .. }
            )
        })
    }
}

impl Pose {
    /// The pose of a compact window parked on `side`: 1:1 at `rect`.
    pub fn compact(rect: LRect, side: i8) -> Pose {
        Pose { compact: true, side, ..Pose::identity(rect) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::ime::pick_compact_form;

    fn forms() -> Vec<CompactForm> {
        vec![CompactForm::new("strip", 320.0, 100.0), CompactForm::new("tile", 220.0, 220.0)]
    }
    const MAX: [f64; 2] = [400.0, 300.0];

    #[test]
    fn the_desk_is_asked_for_frames_for_the_whole_cross_fade_and_not_after() {
        // Regression: nothing asked for frames while a fade ran, so with the
        // pointer still the swap's first frame (the old capture at full
        // opacity) stayed on screen. `busy` is what keeps the desk animating.
        let mut c = Compact::default();
        c.declare(1, forms());
        c.poll(1, Some(0.2), false, MAX, 0.0).expect("told");
        assert!(c.busy(), "offered");
        c.answer(1, Some(1), 0.1);
        assert!(c.busy(), "preparing");
        assert!(c.tick(1, 0.2, true, true).swapped);
        assert!(c.busy(), "fading");
        while c.step(1.0 / 60.0) {}
        assert!(!c.busy() && c.phase(1) == Phase::Compact { form: 1 });
    }

    #[test]
    fn a_slot_narrower_than_the_preferred_width_still_takes_a_flexible_form() {
        // A side zone on a small display leaves 134 pt: the regression was a
        // 320 pt form refused outright (the app declined, the window stayed a
        // scaled live view). The form is granted the slot's width instead.
        let flex = vec![CompactForm::flexible("row", 320.0, 130.0, 100.0)];
        let narrow = [134.4, 360.5];
        let pick = pick_compact_form(&flex, narrow);
        assert_eq!(pick, Some(0));
        let mut c = Compact::default();
        c.declare(1, flex);
        assert!(c.poll(1, Some(0.1), false, narrow, 0.0).is_some());
        assert!(c.answer(1, pick.map(|i| i as u32), 0.1).is_none());
        assert_eq!(c.size(1), Some((134.4, 100.0)));
        // a fixed-width form that does not fit is still refused
        assert_eq!(pick_compact_form(&[CompactForm::new("wide", 320.0, 100.0)], narrow), None);
        // and an unparked slot grants a wide one the preferred width
        assert_eq!(CompactForm::flexible("row", 320.0, 130.0, 100.0).granted([500.0, 300.0]), (320.0, 100.0));
    }

    #[test]
    fn the_slot_is_the_zone_less_its_gaps_and_frame_and_half_the_display_high() {
        let m = slot_max(500.0, 1440.0, (6.0, 6.0));
        assert_eq!(m[0], 500.0 - 2.0 * super::super::GAP - 6.0);
        assert_eq!(m[1], (1440.0 - 30.0) * 0.5 - 6.0);
        assert_eq!(slot_max(10.0, 100.0, (6.0, 6.0))[0], 0.0);
    }

    #[test]
    fn the_compact_rect_is_one_to_one_centred_on_the_slot_and_inside_its_zone() {
        let screen = LRect::new(0.0, 0.0, 3440.0, 1440.0);
        let zone = (0.0, 640.0);
        let slot = LRect::new(100.0, 600.0, 200.0, 120.0);
        let size = slot_size((360.0, 64.0), (6.0, 6.0));
        assert_eq!(size, (366.0, 70.0));
        let r = compact_rect(slot, size, zone, screen);
        assert_eq!((r.w, r.h), (366.0, 70.0));
        assert_eq!(r.center(), slot.center());
        let edge = LRect::new(0.0, 0.0, 40.0, 24.0);
        let r = compact_rect(edge, size, zone, screen);
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 366.0, 70.0));
        let inner = LRect::new(600.0, 600.0, 30.0, 30.0);
        let r = compact_rect(inner, size, zone, screen);
        assert_eq!(r.x + r.w, 640.0);
    }

    #[test]
    fn a_client_without_forms_is_never_sent_anything() {
        let mut c = Compact::default();
        assert_eq!(c.poll(1, Some(0.3), false, MAX, 0.0), None);
        assert_eq!(c.poll(1, None, false, MAX, 0.1), None);
        assert_eq!(c.phase(1), Phase::Idle);
        assert!(!c.tick(1, 5.0, true, true).swapped);
    }

    #[test]
    fn parking_runs_offer_answer_frame_swap_and_the_hint_has_hysteresis() {
        let mut c = Compact::default();
        c.declare(1, forms());
        // Between the thresholds, not yet told: nothing.
        assert_eq!(c.poll(1, Some(0.65), false, MAX, 0.0), None);
        // A drag across the zone is not a park.
        assert_eq!(c.poll(1, Some(0.3), true, MAX, 0.0), None);
        let sent = c.poll(1, Some(0.3), false, MAX, 0.0).expect("told parked");
        assert_eq!((sent.scale, sent.max), (Some(0.3), MAX));
        assert_eq!(c.poll(1, Some(0.65), false, MAX, 0.05), None, "hysteresis: still parked");
        assert_eq!(c.size(1), None, "nothing is configured before the answer");
        // The tile (220x220) is the larger form that fits.
        assert_eq!(c.answer(1, Some(1), 0.06), None);
        assert!(matches!(c.phase(1), Phase::Preparing { form: 1, .. }));
        assert_eq!(c.size(1), Some((220.0, 220.0)));
        assert_eq!(c.drawn_size(1), None, "still the scaled view");
        assert!(c.frozen(1));
        // The frame is in but the spring has not settled: wait.
        assert!(!c.tick(1, 0.1, true, false).swapped);
        let t = c.tick(1, 0.2, true, true);
        assert!(t.swapped && t.send.is_none());
        assert_eq!(c.drawn_size(1), Some((220.0, 220.0)));
        assert!(!c.frozen(1));
        assert_eq!(c.fade(1), Some(1.0));
        let mut steps = 0;
        while c.step(1.0 / 60.0) {
            steps += 1;
            assert!(steps < 30);
        }
        assert_eq!(c.phase(1), Phase::Compact { form: 1 });
        assert_eq!(c.fade(1), None);
    }

    #[test]
    fn an_app_that_is_silent_or_declines_or_overreaches_stays_a_scaled_live_view() {
        for how in 0..3 {
            let mut c = Compact::default();
            c.declare(1, forms());
            c.poll(1, Some(0.3), false, [250.0, 150.0], 0.0).unwrap();
            let undo = match how {
                0 => c.tick(1, ANSWER_SECS + 0.01, false, true).send,
                1 => c.answer(1, None, 0.05),
                // The tile (220x220) does not fit 250x150.
                _ => c.answer(1, Some(1), 0.05),
            };
            assert_eq!(c.phase(1), Phase::Declined, "case {how}");
            assert_eq!(undo.is_some(), how != 1, "the app is undone unless it declined itself");
            assert_eq!((c.size(1), c.drawn_size(1), c.frozen(1)), (None, None, false));
            // No second offer while it stays parked, and leaving sends nothing more.
            assert_eq!(c.poll(1, Some(0.3), false, MAX, 1.0), None);
            assert_eq!(c.poll(1, None, false, MAX, 1.1), None);
            assert_eq!(c.phase(1), Phase::Idle);
        }
    }

    #[test]
    fn a_first_frame_that_never_comes_gives_up_after_the_timeout() {
        let mut c = Compact::default();
        c.declare(1, forms());
        c.poll(1, Some(0.3), false, MAX, 0.0).unwrap();
        c.answer(1, Some(0), 0.05);
        assert_eq!(c.tick(1, 0.2, false, true).send, None);
        let t = c.tick(1, 0.05 + ANSWER_SECS + 0.01, false, true);
        assert_eq!(t.send.map(|m| m.scale), Some(None));
        assert_eq!(c.phase(1), Phase::Declined);
    }

    #[test]
    fn unparking_freezes_the_compact_view_until_the_full_frame_and_fades_back() {
        let mut c = Compact::default();
        c.declare(1, forms());
        c.poll(1, Some(0.3), false, MAX, 0.0).unwrap();
        c.answer(1, Some(0), 0.0);
        c.tick(1, 0.1, true, true);
        while c.step(1.0 / 60.0) {}
        let msg = c.poll(1, Some(0.8), false, MAX, 1.0).expect("told unparked");
        assert_eq!(msg.scale, None);
        assert!(matches!(c.phase(1), Phase::Restoring { was: Some(0), .. }));
        assert_eq!(c.size(1), None, "configured at the full size again");
        assert_eq!(c.drawn_size(1), Some((320.0, 100.0)), "the compact view stays on screen");
        assert!(c.frozen(1));
        let t = c.tick(1, 1.1, true, true);
        assert!(t.swapped);
        assert!(matches!(c.phase(1), Phase::Returning { .. }) && c.fade(1) == Some(1.0));
        while c.step(1.0 / 60.0) {}
        assert_eq!(c.phase(1), Phase::Idle);
        // A window that never became compact just unfreezes.
        c.poll(1, Some(0.3), false, MAX, 2.0).unwrap();
        c.answer(1, Some(0), 2.0);
        c.poll(1, None, false, MAX, 2.1).unwrap();
        assert!(matches!(c.phase(1), Phase::Restoring { was: None, .. }));
        assert!(!c.tick(1, 2.2, true, true).swapped);
        assert_eq!(c.phase(1), Phase::Idle);
    }
}
