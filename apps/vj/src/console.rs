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

/// The master clipped, and nobody has cleared it.
///
/// A peak meter that shows only the current buffer never shows a clip: it is
/// one buffer long and the eye is elsewhere. So it latches.
#[derive(Default)]
pub struct ClipLatch {
    lit: bool,
}

impl ClipLatch {
    pub fn saw(&mut self, peak: f32) {
        if peak >= 1.0 {
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
    if health.phones_starved > 0 {
        line.push_str(&format!("   phones {}", health.phones_starved));
    }
    line.push_str(&format!("   master {:.0}%", master.clamp(0.0, 1.0) * 100.0));
    if clipped {
        line.push_str("  CLIP");
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
    // The poison count only appears once there is one: it means a panic
    // happened somewhere else in the app, and a permanent "poisoned 0"
    // would teach the eye to skip the line that matters.
    text.push_str(&format!(
        "silenced {}, phones {}",
        health.contended, health.phones_starved
    ));
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
    fn the_one_line_shows_the_master_level_and_says_when_it_clipped() {
        let clipped = summary_line(&health(), 0.25, true);
        assert!(clipped.contains("CLIP"), "the thing you must not miss: {clipped}");
        let clean = summary_line(&health(), 0.25, false);
        assert!(!clean.contains("CLIP"), "{clean}");
        assert!(clean.contains("25%"), "the level itself: {clean}");
    }

    #[test]
    fn a_clip_stays_lit_once_it_has_happened_until_it_is_cleared() {
        let mut latch = ClipLatch::default();
        assert!(!latch.lit());
        latch.saw(0.5);
        assert!(!latch.lit(), "an ordinary level is not a clip");
        latch.saw(1.0);
        assert!(latch.lit(), "full scale is");
        latch.saw(0.1);
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

    #[test]
    fn the_view_toggle_names_itself_and_survives_a_round_trip() {
        for view in [ConsoleView::Numbers, ConsoleView::Log, ConsoleView::Both] {
            assert_eq!(ConsoleView::from_index(view.index()), view);
            assert!(!view.label().is_empty());
        }
        assert_eq!(ConsoleView::from_index(99), ConsoleView::Both, "the default");
    }
}
