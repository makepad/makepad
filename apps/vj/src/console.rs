//! The strip under the lists: one line of live numbers at rest, the app's
//! own log when it is opened, and every decision about it that can be made
//! without drawing anything.

use crate::mixer::AudioHealth;

/// The strip closed: one line, the numbers.
pub const CLOSED_POINTS: f64 = 24.0;

/// The seam between the explorer and the strip, mirroring the `spacing` the
/// pane is declared with. The explorer's size is worked out here, so the gap
/// has to be counted here too.
pub const GAP_POINTS: f64 = 6.0;
/// Open, it is never so short that the log is a single line.
pub const OPEN_MIN_POINTS: f64 = 72.0;
pub const OPEN_DEFAULT_POINTS: f64 = 190.0;
/// What the lists keep for themselves, whatever the console asks for.
const LISTS_KEEP_POINTS: f64 = 120.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleView {
    Numbers,
    Log,
    Both,
}

impl ConsoleView {
    pub fn label(self) -> &'static str {
        match self {
            ConsoleView::Numbers => "numbers",
            ConsoleView::Log => "log",
            ConsoleView::Both => "both",
        }
    }

    pub fn index(self) -> usize {
        match self {
            ConsoleView::Numbers => 0,
            ConsoleView::Log => 1,
            ConsoleView::Both => 2,
        }
    }

    /// An index nobody wrote reads as the default rather than as a panic:
    /// this comes off a settings file a hand can edit.
    pub fn from_index(index: usize) -> ConsoleView {
        match index {
            0 => ConsoleView::Numbers,
            1 => ConsoleView::Log,
            _ => ConsoleView::Both,
        }
    }
}

pub struct Console {
    pub open: bool,
    pub view: ConsoleView,
    /// What it opens to. Kept while closed, so closing forgets nothing.
    open_points: f64,
    pub filter: String,
}

impl Console {
    pub fn new() -> Console {
        Console {
            open: false,
            view: ConsoleView::Both,
            open_points: OPEN_DEFAULT_POINTS,
            filter: String::new(),
        }
    }

    /// The height the strip asks for, given the room the lists column has.
    pub fn extent(&self, room: f64) -> f64 {
        if !self.open {
            return CLOSED_POINTS;
        }
        let ceiling = (room - LISTS_KEEP_POINTS).max(OPEN_MIN_POINTS);
        self.open_points.clamp(OPEN_MIN_POINTS, ceiling)
    }

    /// Set what it opens to, from a drag or from the settings file. A number
    /// that is not one leaves it where it was.
    pub fn set_open_height(&mut self, points: f64, room: f64) {
        if !points.is_finite() {
            return;
        }
        let ceiling = (room - LISTS_KEEP_POINTS).max(OPEN_MIN_POINTS);
        self.open_points = points.clamp(OPEN_MIN_POINTS, ceiling);
    }

    /// `(numbers pane, log pane)`: what the opened area shows. The one line
    /// is not a pane — it is always there.
    pub fn panes(&self) -> (bool, bool) {
        if !self.open {
            return (false, false);
        }
        match self.view {
            ConsoleView::Numbers => (true, false),
            ConsoleView::Log => (false, true),
            ConsoleView::Both => (true, true),
        }
    }

    pub fn to_text(&self) -> String {
        format!(
            "console_open {}\nconsole_height {}\nconsole_view {}\n",
            u8::from(self.open),
            self.open_points,
            self.view.index(),
        )
    }

    /// One `key value` line from the lists settings file. Anything else is
    /// somebody else's line and is left alone.
    pub fn apply_line(&mut self, key: &str, value: &str) {
        match key {
            "console_open" => self.open = value.trim() == "1",
            "console_height" => {
                if let Ok(points) = value.trim().parse::<f64>() {
                    if points.is_finite() {
                        self.open_points = points.max(OPEN_MIN_POINTS);
                    }
                }
            }
            "console_view" => {
                if let Ok(index) = value.trim().parse::<usize>() {
                    self.view = ConsoleView::from_index(index);
                }
            }
            _ => {}
        }
    }
}

impl Default for Console {
    fn default() -> Console {
        Console::new()
    }
}

/// The master was driven past what it can pass, and nobody has cleared it.
///
/// It latches because the event is shorter than a glance: whatever lit it
/// was over in a fraction of a second and the eye was elsewhere.
///
/// What lights it is the limiter having to pull the mix back, NOT a sample
/// reaching full scale. This master cannot reach full scale -- the ceiling
/// clamps every sample below it, and that guarantee is the limiter's whole
/// job -- so a light wired to a clipped sample would have been a light that
/// could never come on. Being held down is the thing the operator can
/// actually do something about.
#[derive(Default)]
pub struct ClipLatch {
    lit: bool,
}

/// How far the limiter has to pull the master back before it is worth
/// saying so. A look-ahead limiter ticks over by fractions of a decibel on
/// anything loud, which is it working, not a warning.
pub const OVERLOAD_DB: f32 = 1.0;

impl ClipLatch {
    pub fn saw(&mut self, reduction_db: f32) {
        if reduction_db >= OVERLOAD_DB {
            self.lit = true;
        }
    }

    pub fn lit(&self) -> bool {
        self.lit
    }

    pub fn clear(&mut self) {
        self.lit = false;
    }
}


/// How a level READS, as against what it measures.
///
/// A meter fed the raw peak of the newest buffer is unreadable: it is
/// twenty different numbers a second and the eye takes an average of the
/// flicker rather than the loudest thing that happened. So the bar goes up
/// the instant a peak arrives -- missing a transient is the one thing a
/// peak meter may not do -- and comes down slowly, at the rate a meter is
/// conventionally read by. Above it rides a mark holding the highest recent
/// peak, for the operator who looked away.
///
/// This lives on the UI thread and nowhere near the mixer. The audio thread
/// must not learn how fast a screen refreshes, and how a number LOOKS is
/// not a decision about the mix. It is also why this is a plain type with a
/// `dt` argument rather than anything that reads a clock: it can then be
/// held to its own arithmetic in a test.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeterBallistics {
    level: f32,
    hold: f32,
    /// Seconds the hold mark has stood where it is.
    held_for: f32,
    /// The bar and mark last handed to the shader, and whether any ever were.
    pushed: Option<(f32, f32)>,
}

/// Twenty decibels in about 1.7 seconds, which is the fall a peak meter is
/// read by. As a time constant that is 1.7 / ln(10).
const RELEASE_SECS: f32 = 0.74;
/// How long the mark stands before it starts to follow the bar down. Long
/// enough to look up at, short enough that it is still about now.
const HOLD_SECS: f32 = 0.5;
/// A one-pole never arrives, so under this the meter is simply out. Without
/// it a settled meter would ask to be redrawn for the rest of the session,
/// each time by an amount no screen can show.
const SILENCE: f32 = 0.001;
/// Below this much of the column, a move cannot be seen: a meter is at most
/// a couple of hundred device pixels tall, so this is a fraction of one.
const DEADBAND: f32 = 1.0 / 256.0;

impl MeterBallistics {
    /// One tick. `peak` is the highest sample since the last tick, `dt` the
    /// seconds since it.
    pub fn tick(&mut self, peak: f32, dt: f32) {
        // A tick can be enormous: this rides a 20 Hz timer that Windows
        // services at the bottom of the message queue, and returning from a
        // page that was not pumping hands over one gap of whatever length.
        // Clamped here rather than at the call site, because the guard
        // belongs to the arithmetic it protects.
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
        let peak = if peak.is_finite() { peak.max(0.0) } else { 0.0 };
        // 1 - e^(-dt/tau): the share of the remaining distance to cover in
        // this tick. Being a function of dt is the whole point -- the tick
        // is not evenly spaced, and a per-tick constant would make the fall
        // speed a function of how busy the machine is.
        let fall = 1.0 - (-dt / RELEASE_SECS).exp();

        if peak >= self.level {
            self.level = peak;
        } else {
            self.level += (peak - self.level) * fall;
            if self.level < SILENCE {
                self.level = 0.0;
            }
        }

        if self.level >= self.hold {
            self.hold = self.level;
            self.held_for = 0.0;
        } else {
            self.held_for += dt;
            if self.held_for >= HOLD_SECS {
                // It follows the bar down rather than dropping to meet it:
                // a mark that jumps reads as a new event, which is the one
                // thing it is not.
                self.hold += (self.level - self.hold) * fall;
                if self.hold < SILENCE {
                    self.hold = 0.0;
                }
            }
        }
    }

    /// The level as it was measured, ballistics and all: a share of full
    /// scale. This is the one to PRINT. The taper below is for drawing.
    pub fn amplitude(&self) -> f32 {
        self.level.clamp(0.0, 1.0)
    }

    /// The bar, on the scale it is drawn on.
    ///
    /// The square root is the display taper the meter has always used: it
    /// gives the quiet half of the range room, where a linear amplitude
    /// column spends most of its height on the top few decibels.
    pub fn level(&self) -> f32 {
        self.level.clamp(0.0, 1.0).sqrt()
    }

    /// The hold mark, on the same scale.
    pub fn hold(&self) -> f32 {
        self.hold.clamp(0.0, 1.0).sqrt()
    }

    /// The bar to draw, or `None` when nothing has moved enough to see.
    ///
    /// This is the whole reason the meter is cheap: pushing a uniform marks
    /// the pass for repaint, so a meter that reports every tick repaints the
    /// window twenty times a second forever, including with nothing
    /// playing. It reports the DISPLAY value, because that is where being
    /// visible is decided -- a threshold on the raw amplitude is blind at
    /// the bottom of a square-root scale and twitchy at the top.
    pub fn take_push(&mut self) -> Option<(f32, f32)> {
        let now = (self.level(), self.hold());
        // Both, because the mark moves on its own: while the bar sits still
        // at the end of a phrase the mark is still coming down over it, and
        // a deadband that watched only the bar would freeze it there.
        let moved = match self.pushed {
            None => true,
            Some((level, hold)) => {
                (now.0 - level).abs() >= DEADBAND || (now.1 - hold).abs() >= DEADBAND
            }
        };
        // The last step to nothing always goes. A meter left standing a
        // deadband above zero is a meter saying something is playing.
        let landed = self.pushed.is_some_and(|(level, hold)| level > 0.0 || hold > 0.0)
            && now == (0.0, 0.0);
        (moved || landed).then(|| {
            self.pushed = Some(now);
            now
        })
    }
}

/// The one line, in the order a glance wants it: what the render cost, then
/// anything that has actually gone wrong, then the master level.
pub fn summary_line(health: &AudioHealth, master: f32, clipped: bool) -> String {
    let mut line = String::with_capacity(64);
    match health.budget_used() {
        Some(share) => line.push_str(&format!(
            "{:.0}% of {} fr",
            share * 100.0,
            health.buffer_frames
        )),
        None => line.push_str("idle"),
    }
    if health.contended > 0 {
        line.push_str(&format!("   silenced {}", health.contended));
    }
    // The buffers the render could not fill in time: the one dropout this
    // engine can have, and until now the one number the line did not show.
    // Only once there has been one, so the healthy line does not move.
    if health.overruns > 0 {
        line.push_str(&format!("   overran {}", health.overruns));
    }
    if health.phones_starved > 0 {
        line.push_str(&format!("   phones {}", health.phones_starved));
    }
    line.push_str(&format!("   master {:.0}%", master.clamp(0.0, 1.0) * 100.0));
    if clipped {
        // Not CLIP: nothing clipped, and saying so would send an operator
        // looking for a fault in a signal path that is behaving.
        line.push_str("  OVER");
    }
    line
}

/// The opened numbers pane: everything the one line had no room for, one
/// reading per line. `meters` is the mixer's five peaks in its own order,
/// `decks` the two pre-fader deck levels.
pub fn detail_text(health: &AudioHealth, meters: &[f32; 5], decks: [f32; 2]) -> String {
    let mut text = String::with_capacity(160);
    if health.device_rate > 0.0 {
        text.push_str(&format!("device {:.0} Hz\n", health.device_rate));
    }
    text.push_str(&format!(
        "render {:.2} ms now, {:.2} ms worst\n",
        health.render_nanos as f64 / 1e6,
        health.render_max_nanos as f64 / 1e6,
    ));
    // Microseconds, not milliseconds: the setup and the bookkeeping are
    // small next to the mixing, and rounding them to 0.00 ms would say
    // nothing at all.
    text.push_str(&format!(
        "  setup {} us, mix {} us, after {} us\n",
        health.stages.setup / 1_000,
        health.stages.mix / 1_000,
        health.stages.publish / 1_000,
    ));
    // The poison count only appears once there is one: it means a panic
    // happened somewhere else in the app, and a permanent "poisoned 0"
    // would teach the eye to skip the line that matters.
    text.push_str(&format!(
        "silenced {}, phones {}",
        health.contended, health.phones_starved
    ));
    if health.overruns > 0 {
        text.push_str(&format!(", overran {}", health.overruns));
    }
    if health.poisoned > 0 {
        text.push_str(&format!(", POISONED {}", health.poisoned));
    }
    text.push('\n');
    text.push_str(&format!(
        "deck A {:.0}%, deck B {:.0}%\n",
        decks[0].clamp(0.0, 1.0) * 100.0,
        decks[1].clamp(0.0, 1.0) * 100.0,
    ));
    text.push_str(&format!(
        "video {:.0}%, sfx {:.0}%",
        meters[crate::mixer::METER_VIDEO].clamp(0.0, 1.0) * 100.0,
        meters[crate::mixer::METER_SFX].clamp(0.0, 1.0) * 100.0,
    ));
    text
}

/// Whether the operator's filter keeps this line. An empty filter keeps
/// everything, and case is not the operator's problem.
pub fn filter_keeps(line: &str, filter: &str) -> bool {
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }
    line.to_lowercase().contains(&filter.to_lowercase())
}

/// The log pane's text: the newest `rows` lines the filter keeps, oldest
/// first, so the newest sits at the bottom where a tail belongs.
pub fn pane_text(lines: &[String], filter: &str, rows: usize) -> String {
    let kept: Vec<&String> = lines.iter().filter(|line| filter_keeps(line, filter)).collect();
    let start = kept.len().saturating_sub(rows);
    kept[start..].iter().map(|line| line.as_str()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::AudioHealth;

    fn health() -> AudioHealth {
        AudioHealth {
            poisoned: 0,
            overruns: 0,
            stages: crate::mixer::StageNanos::default(),
            contended: 0,
            phones_starved: 0,
            render_nanos: 500_000,
            render_max_nanos: 900_000,
            buffer_frames: 512,
            device_rate: 48_000.0,
        }
    }

    #[test]
    fn the_one_line_leads_with_what_the_render_cost() {
        // 512 frames at 48k is 10.67ms of playing time; half a millisecond
        // of render is 4.7% of it, which prints as 5.
        let line = summary_line(&health(), 0.5, false);
        assert!(line.starts_with("5%"), "the budget comes first: {line}");
        assert!(line.contains("512"), "and it says what buffer that was: {line}");
    }

    #[test]
    fn the_numbers_say_where_the_callbacks_time_went() {
        let mut health = health();
        health.stages = crate::mixer::StageNanos { setup: 12_000, mix: 430_000, publish: 8_000 };
        let text = detail_text(&health, &[0.0; 5], [0.0, 0.0]);
        assert!(text.contains("setup 12 us, mix 430 us, after 8 us"), "{text}");
    }

    #[test]
    fn a_panic_elsewhere_shows_up_in_the_numbers_and_silence_does_not_pretend_to_be_it() {
        let quiet = detail_text(&health(), &[0.0; 5], [0.0, 0.0]);
        assert!(quiet.contains("silenced 0, phones 0"));
        assert!(!quiet.contains("POISONED"), "nothing to say while nothing has gone wrong");
        let mut hurt = health();
        hurt.poisoned = 3;
        let text = detail_text(&hurt, &[0.0; 5], [0.0, 0.0]);
        assert!(text.contains("POISONED 3"), "and it is unmissable when there is: {text}");
    }

    #[test]
    fn the_one_line_claims_no_budget_it_cannot_know() {
        let mut fresh = health();
        fresh.buffer_frames = 0;
        fresh.device_rate = 0.0;
        let line = summary_line(&fresh, 0.0, false);
        assert!(line.starts_with("idle"), "before the first buffer it says so: {line}");
        assert!(!line.contains(" fr"), "and it claims no buffer it has not seen: {line}");
    }

    /// The render's own dropout count was kept and never shown: the line
    /// reported a lock-contention figure that is zero by construction. It
    /// shows the buffers that overran now -- and only once there has been
    /// one, so the healthy line is the line it always was.
    #[test]
    fn the_one_line_counts_the_buffers_the_render_missed() {
        assert_eq!(summary_line(&health(), 0.5, false), "5% of 512 fr   master 50%");
        let mut late = health();
        late.overruns = 3;
        let line = summary_line(&late, 0.5, false);
        assert!(line.contains("overran 3"), "{line}");
        assert!(detail_text(&late, &[0.0; 5], [0.0, 0.0]).contains("overran 3"));
        assert!(!detail_text(&health(), &[0.0; 5], [0.0, 0.0]).contains("overran"));
    }

    #[test]
    fn the_one_line_names_a_dropout_only_once_there_has_been_one() {
        let quiet = summary_line(&health(), 0.5, false);
        assert!(!quiet.contains("silenced"), "{quiet}");
        assert!(!quiet.contains("phones"), "{quiet}");
        let mut bad = health();
        bad.contended = 3;
        bad.phones_starved = 1;
        let loud = summary_line(&bad, 0.5, false);
        assert!(loud.contains("silenced 3"), "{loud}");
        assert!(loud.contains("phones 1"), "{loud}");
    }

    #[test]
    fn the_one_line_shows_the_master_level_and_says_when_it_was_held_back() {
        let over = summary_line(&health(), 0.25, true);
        assert!(over.contains("OVER"), "the thing you must not miss: {over}");
        assert!(!over.contains("CLIP"), "and it does not claim a clip: {over}");
        let clean = summary_line(&health(), 0.25, false);
        assert!(!clean.contains("OVER"), "{clean}");
        assert!(clean.contains("25%"), "the level itself: {clean}");
    }

    #[test]
    fn an_overload_stays_lit_once_it_has_happened_until_it_is_cleared() {
        let mut latch = ClipLatch::default();
        assert!(!latch.lit());
        latch.saw(0.0);
        assert!(!latch.lit(), "a limiter that never worked is not a warning");
        latch.saw(0.3);
        assert!(!latch.lit(), "nor is a limiter merely doing its job");
        latch.saw(OVERLOAD_DB);
        assert!(latch.lit(), "a whole decibel of holding back is");
        latch.saw(0.0);
        assert!(latch.lit(), "and it stays lit, or nobody would ever see it");
        latch.clear();
        assert!(!latch.lit());
    }

    #[test]
    fn a_closed_console_is_one_line_and_an_open_one_is_what_it_was_dragged_to() {
        let mut console = Console::new();
        assert!(!console.open);
        assert_eq!(console.extent(600.0), CLOSED_POINTS);
        console.open = true;
        assert_eq!(console.extent(600.0), OPEN_DEFAULT_POINTS);
        console.set_open_height(300.0, 600.0);
        assert_eq!(console.extent(600.0), 300.0);
        console.open = false;
        assert_eq!(console.extent(600.0), CLOSED_POINTS, "closing forgets nothing");
        console.open = true;
        assert_eq!(console.extent(600.0), 300.0);
    }

    #[test]
    fn the_console_never_takes_so_much_room_that_the_lists_are_useless() {
        let mut console = Console::new();
        console.open = true;
        console.set_open_height(10_000.0, 400.0);
        let extent = console.extent(400.0);
        assert!(extent < 400.0, "the lists keep room: {extent}");
        assert!(extent >= OPEN_MIN_POINTS);
        console.set_open_height(1.0, 400.0);
        assert_eq!(console.extent(400.0), OPEN_MIN_POINTS, "and it stays usable");
    }

    #[test]
    fn dragging_the_grip_up_makes_the_console_taller() {
        // The console lies BELOW its grip, so dragging up must grow it.
        let mut console = Console::new();
        console.open = true;
        console.set_open_height(150.0, 800.0);
        let grabbed = console.extent(800.0);
        console.set_open_height(grabbed + 40.0, 800.0);
        assert_eq!(console.extent(800.0), 190.0);
        console.set_open_height(grabbed - 100.0, 800.0);
        assert_eq!(console.extent(800.0), OPEN_MIN_POINTS, "and never below usable");
    }

    #[test]
    fn what_is_shown_follows_the_view_and_whether_it_is_open() {
        let mut console = Console::new();
        assert_eq!(console.panes(), (false, false), "closed shows neither pane");
        console.open = true;
        console.view = ConsoleView::Both;
        assert_eq!(console.panes(), (true, true));
        console.view = ConsoleView::Log;
        assert_eq!(console.panes(), (false, true));
        console.view = ConsoleView::Numbers;
        assert_eq!(console.panes(), (true, false));
    }

    #[test]
    fn the_height_and_the_view_survive_a_trip_through_the_settings_file() {
        let mut console = Console::new();
        console.open = true;
        console.view = ConsoleView::Log;
        console.set_open_height(220.0, 900.0);
        let text = console.to_text();
        let mut read_back = Console::new();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            read_back.apply_line(key, value);
        }
        assert!(read_back.open);
        assert_eq!(read_back.view, ConsoleView::Log);
        assert_eq!(read_back.extent(900.0), 220.0);
    }

    #[test]
    fn a_settings_line_nobody_wrote_is_ignored_rather_than_believed() {
        let mut console = Console::new();
        console.apply_line("console_height", "not a number");
        console.apply_line("console_height", "NaN");
        console.apply_line("console_view", "9");
        console.apply_line("something_else", "7");
        assert_eq!(console.extent(900.0), CLOSED_POINTS);
        assert_eq!(console.view, ConsoleView::Both);
    }

    #[test]
    fn an_empty_filter_keeps_everything_and_a_filter_narrows_it() {
        assert!(filter_keeps("[I] apps/vj/src/media.rs:2161 - ui-hitch", ""));
        assert!(filter_keeps("[E] apps/vj/src/main.rs:12 - device lost", "device"));
        assert!(!filter_keeps("[I] apps/vj/src/media.rs:2161 - ui-hitch", "device"));
        assert!(
            filter_keeps("[E] DEVICE lost", "device"),
            "the operator types in whatever case they like"
        );
    }

    #[test]
    fn the_opened_pane_says_what_the_one_line_had_no_room_for() {
        let detail = detail_text(&health(), &[0.5, 0.1, 0.4, 0.3, 0.0], [0.4, 0.3]);
        assert!(detail.contains("48000"), "the device rate: {detail}");
        assert!(detail.contains("0.90 ms"), "the render high-water: {detail}");
        assert!(detail.contains("deck A"), "and both decks: {detail}");
        assert!(detail.contains("deck B"), "{detail}");
        let mut idle = health();
        idle.device_rate = 0.0;
        assert!(
            !detail_text(&idle, &[0.0; 5], [0.0, 0.0]).contains("48000"),
            "and it claims no rate it has not been told"
        );
    }

    #[test]
    fn the_pane_shows_the_newest_lines_that_fit_oldest_first() {
        let lines: Vec<String> = (0..10).map(|n| format!("line {n}")).collect();
        assert_eq!(pane_text(&lines, "", 3), "line 7\nline 8\nline 9");
        assert_eq!(pane_text(&lines, "line 1", 3), "line 1", "filter, then tail");
        assert_eq!(pane_text(&[], "", 3), "");
    }

    /// A meter that shows the newest buffer and nothing else is a flicker.
    /// The bar must take a transient the instant it lands and let it go at
    /// the rate the eye reads a meter by.
    #[test]
    fn the_bar_takes_a_peak_at_once_and_lets_it_go_by_the_convention() {
        let mut m = MeterBallistics::default();
        m.tick(1.0, 0.05);
        assert_eq!(m.level(), 1.0, "a peak has to be there the tick it arrives");
        // Twenty decibels is a factor of ten in amplitude, and the display
        // is the square root of that: sqrt(0.1) = 0.316.
        for _ in 0..34 {
            m.tick(0.0, 0.05);
        }
        assert!(
            (m.level() - 0.316).abs() < 0.01,
            "1.7s should be 20 dB down, which reads {:.3}, got {:.3}",
            0.316,
            m.level()
        );
        // And a peak on the way down is taken immediately too, or the meter
        // under-reads exactly when the music gets loud again.
        m.tick(0.81, 0.05);
        assert_eq!(m.level(), 0.9, "0.81 amplitude reads 0.9");
    }

    /// The reason the fall is a time constant and not a per-tick figure.
    /// This tick is a 20 Hz timer serviced at the bottom of the message
    /// queue: it arrives late, it coalesces, and it stops entirely while
    /// another page is up. A per-tick constant would make how fast the
    /// meter falls a function of how busy the machine is.
    #[test]
    fn the_fall_does_not_depend_on_how_often_the_meter_is_ticked() {
        let mut often = MeterBallistics::default();
        let mut seldom = MeterBallistics::default();
        often.tick(1.0, 0.01);
        seldom.tick(1.0, 0.01);
        for _ in 0..100 {
            often.tick(0.0, 0.01);
        }
        for _ in 0..4 {
            seldom.tick(0.0, 0.25);
        }
        assert!(
            (often.level() - seldom.level()).abs() < 1e-3,
            "one second is one second: {:.4} against {:.4}",
            often.level(),
            seldom.level()
        );
        // A gap longer than any real one cannot dump the meter to zero in a
        // single step -- returning to the page would look like a cut.
        let mut returned = MeterBallistics::default();
        returned.tick(1.0, 0.05);
        returned.tick(0.0, 30.0);
        assert!(returned.level() > 0.0, "a huge gap is clamped, not obeyed");
        assert!(returned.level() < 1.0, "but it is still a fall");
    }

    /// The mark is for the glance that was somewhere else.
    #[test]
    fn the_mark_stands_a_moment_and_then_rides_the_bar_down() {
        let mut m = MeterBallistics::default();
        m.tick(1.0, 0.05);
        assert_eq!(m.hold(), 1.0);
        for _ in 0..8 {
            m.tick(0.0, 0.05);
        }
        assert_eq!(m.hold(), 1.0, "still standing at 0.45s");
        assert!(m.level() < 0.9, "while the bar has gone: {:.3}", m.level());
        for _ in 0..20 {
            m.tick(0.0, 0.05);
        }
        assert!(m.hold() < 1.0, "and after half a second it follows");
        assert!(m.hold() >= m.level(), "never below the bar it marks");
        // A louder peak reclaims it at once and restarts the standing.
        m.tick(1.0, 0.05);
        assert_eq!(m.hold(), 1.0);
    }

    /// Pushing a uniform marks the pass for repaint, so a meter that reports
    /// every tick repaints the window twenty times a second forever.
    #[test]
    fn a_settled_meter_stops_asking_to_be_drawn() {
        let mut m = MeterBallistics::default();
        m.tick(1.0, 0.05);
        assert!(m.take_push().is_some(), "a fresh meter always reports");
        // Ten seconds of nothing: long enough for the bar AND the mark,
        // which stands half a second before it even starts down.
        let mut last = None;
        for _ in 0..200 {
            m.tick(0.0, 0.05);
            if let Some(push) = m.take_push() {
                last = Some(push);
            }
        }
        assert_eq!(last, Some((0.0, 0.0)), "the last thing it says is: out");
        for _ in 0..40 {
            m.tick(0.0, 0.05);
            assert_eq!(m.take_push(), None, "and then it says nothing at all");
        }
        // A move too small to see is not worth a repaint; one that can be
        // seen is.
        m.tick(0.000_01, 0.05);
        assert_eq!(m.take_push(), None, "below the deadband");
        m.tick(0.25, 0.05);
        assert_eq!(m.take_push(), Some((0.5, 0.5)), "and above it");
    }

    /// The mixer hands over a peak, not a level, and a peak can be anything
    /// a broken effect produced.
    #[test]
    fn nothing_a_meter_is_handed_can_wedge_it() {
        let mut m = MeterBallistics::default();
        m.tick(f32::NAN, 0.05);
        m.tick(f32::INFINITY, 0.05);
        m.tick(-1.0, 0.05);
        assert_eq!(m.level(), 0.0, "none of that is a level");
        m.tick(1.0, f32::NAN);
        assert_eq!(m.level(), 1.0, "and a broken dt still takes the peak");
        let before = m.level();
        m.tick(0.0, f32::NAN);
        assert_eq!(m.level(), before, "it simply does not advance time");
    }

    #[test]
    fn the_view_toggle_names_itself_and_survives_a_round_trip() {
        for view in [ConsoleView::Numbers, ConsoleView::Log, ConsoleView::Both] {
            assert_eq!(ConsoleView::from_index(view.index()), view);
            assert!(!view.label().is_empty());
        }
        assert_eq!(ConsoleView::from_index(99), ConsoleView::Both, "the default");
    }
}
