//! The first-draw watch: a run generation (one `Cx`) that had its Startup and a frame beat and
//! still painted no window after a few rounds of its event loop raises one flight-recorder event
//! ([`CAT`]), so a start that shows nothing names itself: under `makepad run` the recorder writes
//! the recording at once (an `auto-...-nodraw.mkpf`; `makepad inspect file=<it> pf events
//! cat=window.nodraw`), and no capture has to be made for it.
//! The event carries the draw state of that moment (a redraw asked for, dirty passes, next-frame
//! callbacks, whether the app may draw, the windows' readiness), the events dispatched before it
//! (repeats folded) and whether a LiveEdit or a code patch came before any paint. (An 11-09
//! restart in place ran Startup, a beat, a LiveEdit and a beat, then sat idle and drew nothing.)
//!
//! A round counts only when the app has a window made and not paused and nothing its first frame
//! legitimately waits for is on its way: a frame asked of the display and not yet announced, a
//! first frame held for its glyphs and pipelines, a host that shows nothing of the app. The watch
//! ends at the first window paint or at its event; from then on each hook is one branch.

use crate::event::Event;

/// the event's category (the flight recorder writes a recording for it: `relax_flight_mark`)
pub(crate) const CAT: &str = "window.nodraw";
/// the events before it, marked just before it (in its recording)
pub(crate) const CAT_EVENTS: &str = "window.nodraw.events";
/// rounds with no window painted and nothing on its way, before the event
pub(crate) const DRY_ROUNDS: u32 = 8;
/// the idle wait of a loop whose run is watched: a run that paints nothing and that nothing
/// wakes still counts its rounds
pub(crate) const IDLE_ROUND_NS: u64 = 250_000_000;
/// runs of events kept (a run: one kind of event, repeated)
const RING: usize = 12;
/// the events text's budget (the flight recorder keeps 168 bytes of a mark's text)
const EVENTS_TEXT: usize = 150;

#[derive(Default)]
pub(crate) struct FirstDrawWatch {
    /// a window painted, or the event was raised: nothing more is watched
    done: bool,
    started: bool,
    beats: u32,
    /// `beats` at the last round
    beats_seen: u32,
    /// a beat since the last round held the first frame for its resources
    deferred: bool,
    dry: u32,
    live_edits: u32,
    patches: u32,
    events: u32,
    /// runs of events seen (the ring keeps the last [`RING`])
    runs: u32,
    /// (event kind, repeats), oldest first from `head` once full
    ring: [(u16, u16); RING],
    head: usize,
    len: usize,
}

/// What a round of the event loop ended with.
pub(crate) struct Round {
    /// a window painted (`Cx::window_paints`)
    pub painted: bool,
    /// the app has a window made (not a tray dropdown made on first use) and not paused
    /// (`Cx::set_window_paused`)
    pub window: bool,
    /// the host shows nothing of the app (`hosted_frames_held`)
    pub hidden: bool,
    /// frame work is pending (`hal_has_work`)
    pub work: bool,
    /// a frame was asked of the display again: the last ask went unanswered
    pub reasked: bool,
}

impl FirstDrawWatch {
    pub(crate) fn done(&self) -> bool {
        self.done
    }

    /// Watched and waiting for a first paint: started, beat, nothing painted, no event yet.
    pub(crate) fn armed(&self) -> bool {
        !self.done && self.started && self.beats > 0
    }

    pub(crate) fn note_event(&mut self, event: &Event) {
        if self.done {
            return;
        }
        match event {
            Event::Startup => self.started = true,
            Event::LiveEdit => self.live_edits += 1,
            _ => {}
        }
        self.events += 1;
        let kind = event.to_u32() as u16;
        if self.len > 0 {
            let last = &mut self.ring[(self.head + RING - 1) % RING];
            if last.0 == kind {
                last.1 = last.1.saturating_add(1);
                return;
            }
        }
        self.runs += 1;
        self.ring[self.head] = (kind, 1);
        self.head = (self.head + 1) % RING;
        self.len = (self.len + 1).min(RING);
    }

    #[cfg(relax)]
    pub(crate) fn note_patch(&mut self) {
        if !self.done {
            self.patches += 1;
        }
    }

    pub(crate) fn note_beat(&mut self) {
        if !self.done {
            self.beats += 1;
        }
    }

    pub(crate) fn note_deferred(&mut self) {
        if !self.done {
            self.deferred = true;
        }
    }

    /// One round of the event loop: true when it raises the event (once per run).
    pub(crate) fn round(&mut self, r: Round) -> bool {
        if self.done {
            return false;
        }
        if r.painted {
            self.done = true;
            return false;
        }
        let beat = std::mem::replace(&mut self.beats_seen, self.beats) != self.beats;
        let deferred = std::mem::take(&mut self.deferred);
        // (a frame on its way: asked of the display, or held for its resources, or the host's)
        if !self.started || self.beats == 0 || !r.window || r.hidden || deferred || (r.work && !beat && !r.reasked) {
            return false;
        }
        self.dry += 1;
        if self.dry < DRY_ROUNDS {
            return false;
        }
        self.done = true;
        true
    }

    /// (dry rounds, beats, LiveEdits, patches, events) at the event
    pub(crate) fn counts(&self) -> (u32, u32, u32, u32, u32) {
        (self.dry, self.beats, self.live_edits, self.patches, self.events)
    }

    /// The events before the first paint, oldest first, repeats as `Name*n`; the newest are kept
    /// when they do not all fit.
    pub(crate) fn events_text(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut used = 0;
        let mut cut = false;
        for i in 0..self.len {
            let (kind, n) = self.ring[(self.head + RING - 1 - i) % RING];
            let name = Event::name_from_u32(kind as u32);
            let part = if n > 1 { format!("{name}*{n}") } else { name.to_string() };
            if used + part.len() + 1 > EVENTS_TEXT {
                cut = true;
                break;
            }
            used += part.len() + 1;
            parts.push(part);
        }
        if cut || self.runs as usize > parts.len() {
            parts.push("..".to_string());
        }
        parts.reverse();
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> Round {
        Round { painted: false, window: true, hidden: false, work: false, reasked: false }
    }

    #[test]
    fn a_run_that_beats_and_paints_nothing_raises_once() {
        // the 11-09 shape: Startup, a beat, a LiveEdit, a beat, then idle rounds with no paint
        let mut w = FirstDrawWatch::default();
        w.note_event(&Event::Startup);
        w.note_event(&Event::Signal);
        w.note_beat();
        w.note_event(&Event::LiveEdit);
        w.note_beat();
        assert!(w.armed());
        let mut raised = 0;
        for _ in 0..DRY_ROUNDS * 3 {
            w.note_event(&Event::Signal);
            if w.round(idle()) {
                raised += 1;
            }
        }
        assert_eq!(raised, 1);
        assert!(!w.armed());
        let (dry, beats, live_edits, patches, _) = w.counts();
        assert_eq!((dry, beats, live_edits, patches), (DRY_ROUNDS, 2, 1, 0));
        assert_eq!(w.events_text(), format!("Startup Signal LiveEdit Signal*{}", DRY_ROUNDS));
    }

    #[test]
    fn a_first_frame_on_its_way_counts_no_round() {
        let mut w = FirstDrawWatch::default();
        w.note_event(&Event::Startup);
        w.note_beat();
        for _ in 0..DRY_ROUNDS * 4 {
            // held for its glyphs and pipelines
            w.note_beat();
            w.note_deferred();
            assert!(!w.round(Round { work: true, ..idle() }));
            // a frame asked of the display, not yet announced
            assert!(!w.round(Round { work: true, ..idle() }));
            // a host that shows nothing of the app
            assert!(!w.round(Round { hidden: true, ..idle() }));
            // no window to paint (a windowless run, a tray app's dropdown not made yet, windows
            // the app paused)
            assert!(!w.round(Round { window: false, ..idle() }));
        }
        assert_eq!(w.counts().0, 0);
        // then the first paint ends the watch
        assert!(!w.round(Round { painted: true, ..idle() }));
        assert!(w.done());
        for _ in 0..DRY_ROUNDS * 2 {
            assert!(!w.round(idle()));
        }
    }

    #[test]
    fn beats_that_paint_nothing_and_asks_unanswered_count() {
        let mut w = FirstDrawWatch::default();
        w.note_event(&Event::Startup);
        for k in 0..DRY_ROUNDS {
            if k % 2 == 0 {
                // a beat with work that painted no window and held nothing
                w.note_beat();
                assert_eq!(w.round(Round { work: true, ..idle() }), k + 1 == DRY_ROUNDS);
            } else {
                // a frame asked of the display again
                assert_eq!(w.round(Round { work: true, reasked: true, ..idle() }), k + 1 == DRY_ROUNDS);
            }
        }
        assert!(w.done());
    }

    #[test]
    fn no_round_counts_before_startup_and_a_beat() {
        let mut w = FirstDrawWatch::default();
        for _ in 0..DRY_ROUNDS * 2 {
            assert!(!w.round(idle()));
        }
        w.note_event(&Event::Startup);
        for _ in 0..DRY_ROUNDS * 2 {
            assert!(!w.round(idle()));
        }
        assert!(!w.armed());
        w.note_beat();
        assert!(w.armed());
    }

    #[test]
    fn the_events_text_keeps_the_newest_within_its_budget() {
        let mut w = FirstDrawWatch::default();
        // older runs than the ring holds
        for k in 0..40 {
            w.note_event(if k % 2 == 0 { &Event::Foreground } else { &Event::Background });
        }
        let text = w.events_text();
        assert!(text.starts_with(".. Foreground Background"), "{text}");
        assert_eq!(text.split(' ').count(), RING + 1, "{text}");
        assert!(text.ends_with("Background"), "{text}");
        // runs longer than the text's budget
        let mut w = FirstDrawWatch::default();
        for k in 0..40 {
            w.note_event(if k % 2 == 0 { &Event::ClearAtlasses } else { &Event::WindowLostFocus(crate::window::WindowId(0, 0)) });
        }
        let text = w.events_text();
        assert!(text.len() <= EVENTS_TEXT + 3, "{text}");
        assert!(text.starts_with(".. "), "{text}");
        assert!(text.split(' ').count() < RING + 1, "{text}");
        assert!(text.ends_with("WindowLostFocus"), "{text}");
    }
}
