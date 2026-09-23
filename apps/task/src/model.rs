//! The one piece of state every view reads: the retained history, the
//! recorded detail, and where the user is looking. `App` owns it and hands it
//! to the widget tree through `Scope` on every event and draw, so the band,
//! the tiles, the table and the inspector always render the SAME sample.
//!
//! Widgets change it only through the small set of setters below, which bump
//! `version`; `App` watches `requests` for the things only it can do (talk to
//! the worker, persist pins).

use crate::backend::{is_protected, ProcKey, ProcMeta};
use crate::history::{DetailRing, ProcRecord, Sample, Store, Tier, MAX_AGE_MS, MINUTE_MS, SECOND_MS};
use crate::persist::PinRecord;
use crate::supp::SuppStore;
use crate::Theme;
use makepad_widgets::{GridColumns, GridSort, SortCycle};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The series the history band can show, in legend order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeriesId {
    Cpu,
    Gpu,
    Mem,
    DiskRead,
    DiskWrite,
    NetIn,
    NetOut,
}

pub const SERIES: [SeriesId; 7] = [
    SeriesId::Cpu,
    SeriesId::Gpu,
    SeriesId::Mem,
    SeriesId::DiskRead,
    SeriesId::DiskWrite,
    SeriesId::NetIn,
    SeriesId::NetOut,
];

impl SeriesId {
    pub fn label(self) -> &'static str {
        match self {
            SeriesId::Cpu => "CPU",
            SeriesId::Gpu => "GPU",
            SeriesId::Mem => "MEM",
            SeriesId::DiskRead => "Disk R",
            SeriesId::DiskWrite => "Disk W",
            SeriesId::NetIn => "Net in",
            SeriesId::NetOut => "Net out",
        }
    }

    /// Percent series share the fixed 0..100 axis; rates are scaled to the
    /// peak of their pair within the window.
    pub fn is_rate(self) -> bool {
        matches!(self, SeriesId::DiskRead | SeriesId::DiskWrite | SeriesId::NetIn | SeriesId::NetOut)
    }

    /// The raw value in a sample: percent, or bytes per second.
    pub fn value(self, sample: &Sample) -> Option<f32> {
        let s = &sample.system;
        match self {
            SeriesId::Cpu => Some(s.cpu_total),
            SeriesId::Gpu => s.gpu.value(),
            SeriesId::Mem => Some(sample.mem_pct()),
            SeriesId::DiskRead => s.disk.value().map(|(read, _)| read),
            SeriesId::DiskWrite => s.disk.value().map(|(_, write)| write),
            SeriesId::NetIn => Some(s.net_rx),
            SeriesId::NetOut => Some(s.net_tx),
        }
    }

    pub fn color(self, theme: &Theme) -> makepad_widgets::Vec4f {
        match self {
            SeriesId::Cpu => theme.blue,
            SeriesId::Gpu => theme.magenta,
            SeriesId::Mem => theme.green,
            SeriesId::DiskRead => theme.yellow,
            SeriesId::DiskWrite => theme.red,
            SeriesId::NetIn => theme.cyan,
            SeriesId::NetOut => crate::mix(theme.cyan, theme.magenta, 0.55),
        }
    }

    pub fn format(self, value: f32) -> String {
        if self.is_rate() {
            format!("{}/s", crate::format_bytes(value.max(0.0) as u64))
        } else {
            format!("{value:.0}%")
        }
    }
}

/// The history windows the band offers.
pub const RANGES_MS: [u64; 4] = [MINUTE_MS, 15 * MINUTE_MS, 60 * MINUTE_MS, 24 * 60 * MINUTE_MS];
pub const DEFAULT_RANGE: usize = 1;

pub use crate::columns::Column;

/// A pinned process at the view time.
pub enum PinState {
    Running(ProcRecord),
    /// Not in the view sample, and seen alive before it.
    Exited,
    /// Not in the view sample, which is earlier than its start.
    NotStarted,
}

/// What a widget asked `App` to do on its behalf.
#[derive(Default)]
pub struct Requests {
    /// The selection changed: tell the worker which process to inspect.
    pub inspect: bool,
    /// The pin list changed: persist it.
    pub pins: bool,
    /// The table's columns changed (chosen, moved or resized): persist them.
    pub columns: bool,
    /// End the selected process (`Some(force)`).
    pub terminate: Option<bool>,
    /// The view moved (scrub, live, range): redraw everything.
    pub redraw: bool,
}

pub struct Model {
    pub store: Store,
    pub details: DetailRing,
    /// What was recorded beyond the basic sample for inspected and pinned
    /// processes (see `supp.rs`).
    pub supp: SuppStore,
    pub theme: Theme,
    /// Following the newest sample.
    pub live: bool,
    /// The scrubbed time (valid when `!live`).
    pub cursor_ms: u64,
    /// The scrubbed sample itself, held so thinning or the byte budget can
    /// never swap the shown snapshot for a neighbour behind the user's back.
    pub cursor_sample: Option<Arc<Sample>>,
    /// The band's right edge while scrubbing; `None` follows the newest sample.
    pub window_end_ms: Option<u64>,
    /// Where the pointer is over the band (for the hairline and readout).
    pub hover_ms: Option<u64>,
    /// The live graphs' right edge in wall-clock ms, advanced every frame by
    /// `App` from a monotonic presentation clock (not in steps of samples),
    /// so traces slide continuously. 0 until the first sample.
    pub live_end_ms: f64,
    pub range: usize,
    /// A span set by zooming a graph, overriding the `range` preset until a
    /// preset is picked again.
    pub custom_span_ms: Option<u64>,
    pub series_on: [bool; SERIES.len()],
    pub selected: Option<ProcKey>,
    /// The metadata of the selection as last seen, so a vanished process
    /// can still be named.
    pub selected_meta: Option<Arc<ProcMeta>>,
    pub pins: Vec<PinRecord>,
    /// The newest metadata seen for each pinned process, so a pin that has
    /// exited keeps its name, start time and access to its history.
    pub pin_meta: HashMap<ProcKey, Arc<ProcMeta>>,
    pub filter: String,
    pub apps_only: bool,
    pub tree: bool,
    /// Keep the row order while values keep updating.
    pub freeze: bool,
    pub collapsed: HashSet<ProcKey>,
    /// The table's columns as the user arranged them: chosen, ordered,
    /// sized and sorted.
    pub columns: GridColumns<Column>,
    pub inspector_open: bool,
    /// Until the user shows or hides the inspector themselves, the first
    /// selection opens it; an empty inspector is never shown on its own.
    pub inspector_auto: bool,
    /// A phone-sized tile, where the open inspector replaces the table:
    /// selection never opens it there by itself.
    pub compact: bool,
    pub interval_ms: u64,
    /// Bumped on every change a view depends on.
    pub version: u64,
    pub requests: Requests,
    /// A message for the status line (kill results, journal state).
    pub notice: Option<String>,
    pub journal_status: String,
}

impl Model {
    pub fn new(theme: Theme, interval_ms: u64) -> Self {
        Self {
            store: Store::new(),
            details: DetailRing::new(),
            supp: SuppStore::new(),
            theme,
            live: true,
            cursor_ms: 0,
            cursor_sample: None,
            window_end_ms: None,
            hover_ms: None,
            live_end_ms: 0.0,
            range: DEFAULT_RANGE,
            custom_span_ms: None,
            series_on: [true, true, true, false, false, false, false],
            selected: None,
            selected_meta: None,
            pins: Vec::new(),
            pin_meta: HashMap::new(),
            filter: String::new(),
            apps_only: false,
            tree: false,
            freeze: false,
            collapsed: HashSet::new(),
            columns: GridColumns::new(
                &crate::columns::CHOOSABLE,
                &crate::columns::LEADING,
                crate::columns::default_columns(),
                Some(GridSort { column: Column::Cpu, descending: true }),
                SortCycle::Flip,
            ),
            inspector_open: false,
            inspector_auto: true,
            compact: false,
            interval_ms,
            version: 0,
            requests: Requests::default(),
            notice: None,
            journal_status: String::new(),
        }
    }

    pub fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
        self.requests.redraw = true;
    }

    pub fn range_ms(&self) -> u64 {
        self.custom_span_ms.unwrap_or(RANGES_MS[self.range.min(RANGES_MS.len() - 1)])
    }

    /// How far a graph may zoom: in to four sampling intervals (at least
    /// 2 s), out to a day.
    pub fn span_bounds(&self) -> (u64, u64) {
        ((4 * self.interval_ms).max(2 * SECOND_MS), MAX_AGE_MS)
    }

    /// Leave live keeping everything where it is drawn: the cursor on the
    /// newest sample, the window frozen at the live edge.
    pub fn freeze_view(&mut self) {
        if self.live {
            if let Some(newest) = self.store.newest_ms() {
                self.scrub_to(newest);
            }
        }
    }

    /// Zoom the shared window by `factor` about `anchor_ms`, from the
    /// window `drawn` (what the pointer saw), leaving live. The cursor stays
    /// on its sample.
    pub fn zoom(&mut self, anchor_ms: f64, factor: f64, drawn: (f64, f64)) {
        self.freeze_view();
        let (low, high) = self.span_bounds();
        let span = drawn.1 - drawn.0;
        if span <= 0.0 {
            return;
        }
        let next = (span * factor).clamp(low as f64, high as f64);
        let t = ((anchor_ms - drawn.0) / span).clamp(0.0, 1.0);
        let from = anchor_ms - next * t;
        self.custom_span_ms = Some(next.round() as u64);
        self.pan_to(from + next);
    }

    /// Move the shared window's right edge to `end_ms`, leaving live; kept
    /// within the recorded history (a little either side). The cursor stays.
    pub fn pan_to(&mut self, end_ms: f64) {
        self.freeze_view();
        let span = self.range_ms() as f64;
        let (Some(oldest), Some(newest)) = (self.store.oldest_ms(), self.store.newest_ms()) else { return };
        let high = newest as f64 + span * 0.05;
        let low = (oldest as f64 + span * 0.05).min(high);
        let end = end_ms.clamp(low, high).max(0.0).round() as u64;
        if self.window_end_ms != Some(end) {
            self.window_end_ms = Some(end);
            self.requests.redraw = true;
        }
    }

    /// The time every view shows.
    pub fn view_ms(&self) -> u64 {
        if self.live {
            self.store.newest_ms().unwrap_or(0)
        } else {
            self.cursor_ms
        }
    }

    /// The one sample every view shows.
    pub fn view_sample(&self) -> Option<&Arc<Sample>> {
        if self.live {
            self.store.latest()
        } else {
            self.cursor_sample.as_ref().or_else(|| self.store.at(self.cursor_ms))
        }
    }

    /// The band window `(from, to)`.
    pub fn window(&self) -> (u64, u64) {
        let (from, to) = self.window_f();
        (from.max(0.0) as u64, to.max(0.0) as u64)
    }

    /// The band window `(from, to)` with the fractional live edge: what the
    /// graphs draw, so positions move smoothly between samples. Frozen
    /// exactly while scrubbed.
    pub fn window_f(&self) -> (f64, f64) {
        let to = if self.live {
            self.live_end()
        } else {
            self.window_end_ms.or_else(|| self.store.newest_ms()).unwrap_or(0) as f64
        };
        (to - self.range_ms() as f64, to)
    }

    fn live_end(&self) -> f64 {
        if self.live_end_ms > 0.0 {
            self.live_end_ms
        } else {
            self.store.newest_ms().unwrap_or(0) as f64
        }
    }

    /// Where the short (60 s) traces end: the moving live edge, or the
    /// scrubbed sample's time.
    pub fn graph_end(&self) -> f64 {
        if self.live {
            self.live_end()
        } else {
            self.cursor_ms as f64
        }
    }

    /// Scrub to the recorded sample nearest `time_ms`. The band's window
    /// freezes where it was so the axis does not slide under the pointer.
    pub fn scrub_to(&mut self, time_ms: u64) {
        let Some(sample) = self.store.at(time_ms).cloned() else { return };
        let time = sample.time_ms;
        self.cursor_sample = Some(sample);
        if self.live {
            // Freeze the axis exactly where it was drawn, so nothing shifts
            // under the pointer as live turns into scrubbing.
            self.window_end_ms = Some(self.live_end().max(0.0) as u64);
        }
        self.live = false;
        if self.cursor_ms != time {
            self.cursor_ms = time;
            self.changed();
        }
        self.keep_cursor_in_window();
    }

    /// Step `delta` recorded samples from the cursor (or from now).
    pub fn step(&mut self, delta: i64) {
        let from = self.view_ms();
        let Some(index) = self.store.index_at(from) else { return };
        let last = self.store.len().saturating_sub(1) as i64;
        let target = (index as i64 + delta).clamp(0, last) as usize;
        if delta > 0 && target as i64 == last && !self.live {
            // Stepping past the newest sample is going live.
            if index as i64 == last {
                self.go_live();
                return;
            }
        }
        if let Some(entry) = self.store.entry(target) {
            let time = entry.sample.time_ms;
            self.scrub_to(time);
        }
    }

    pub fn go_live(&mut self) {
        if !self.live {
            self.live = true;
            self.cursor_sample = None;
            self.window_end_ms = None;
            // The live edge was not advanced while scrubbed: start it at the
            // newest sample, so the first live frame does not show a stale edge.
            if let Some(newest) = self.store.newest_ms() {
                self.live_end_ms = self.live_end_ms.max(newest as f64);
            }
            self.changed();
        }
    }

    pub fn go_oldest(&mut self) {
        if let Some(oldest) = self.store.oldest_ms() {
            self.window_end_ms = Some(oldest + self.range_ms());
            self.scrub_to(oldest);
        }
    }

    fn keep_cursor_in_window(&mut self) {
        let range = self.range_ms();
        let end = self.window_end_ms.unwrap_or(self.cursor_ms);
        if self.cursor_ms > end {
            self.window_end_ms = Some(self.cursor_ms);
        } else if self.cursor_ms + range < end {
            self.window_end_ms = Some(self.cursor_ms + range / 10);
        }
    }

    pub fn select(&mut self, key: Option<ProcKey>) {
        if self.selected != key {
            self.selected = key;
            self.selected_meta = key.and_then(|key| self.find_meta(key));
            if key.is_some() && self.inspector_auto && !self.compact && !self.inspector_open {
                self.inspector_open = true;
            }
            self.requests.inspect = true;
            self.changed();
        }
    }

    /// The newest metadata recorded for `key`: in the view sample, then in
    /// the latest one.
    pub fn find_meta(&self, key: ProcKey) -> Option<Arc<ProcMeta>> {
        self.view_sample()
            .and_then(|sample| sample.process(key))
            .or_else(|| self.store.latest().and_then(|sample| sample.process(key)))
            .map(|record| record.meta.clone())
            .or_else(|| self.selected_meta.clone().filter(|meta| meta.key == key))
            .or_else(|| self.pin_meta.get(&key).cloned())
    }

    /// Note the metadata of every pinned process present in `sample`.
    pub fn remember_pins(&mut self, sample: &Sample) {
        for pin in &self.pins {
            if let Some(record) = sample.process(pin.key) {
                let newer = self.pin_meta.get(&pin.key).is_none_or(|known| !Arc::ptr_eq(known, &record.meta));
                if newer {
                    self.pin_meta.insert(pin.key, record.meta.clone());
                }
            }
        }
    }

    /// Where a pinned process stands at the view time.
    pub fn pin_state(&self, key: ProcKey) -> PinState {
        if let Some(record) = self.view_sample().and_then(|sample| sample.process(key)) {
            return PinState::Running(record.clone());
        }
        let started_ms = self.find_meta(key).map(|meta| meta.started_secs * 1000).unwrap_or(0);
        if started_ms > 0 && self.view_ms() < started_ms {
            PinState::NotStarted
        } else {
            PinState::Exited
        }
    }

    pub fn is_pinned(&self, key: ProcKey) -> bool {
        self.pins.iter().any(|pin| pin.key == key)
    }

    pub fn toggle_pin(&mut self, key: ProcKey, name: &str) {
        if let Some(at) = self.pins.iter().position(|pin| pin.key == key) {
            self.pins.remove(at);
            self.pin_meta.remove(&key);
        } else if self.pins.len() < 32 {
            self.pins.push(PinRecord { key, name: name.to_string() });
            if let Some(meta) = self.find_meta(key) {
                self.pin_meta.insert(key, meta);
            }
        }
        self.requests.pins = true;
        self.changed();
    }

    /// Whether the selection may be signalled right now: live view, a
    /// verified identity, still in the newest sample, not protected.
    pub fn can_terminate(&self) -> bool {
        let Some(key) = self.selected else { return false };
        self.live
            && key.verified()
            && !is_protected(key.pid)
            && self.store.latest().is_some_and(|sample| sample.process(key).is_some())
    }

    /// Why the selection may not be signalled, for the status line.
    pub fn terminate_refusal(&self) -> Option<String> {
        let Some(key) = self.selected else { return Some("select a process first".to_string()) };
        if !self.live {
            return Some("a historical sample is shown; go Live to end a process".to_string());
        }
        if is_protected(key.pid) {
            return Some(format!("PID {} is protected and will not be signalled", key.pid));
        }
        if !key.verified() {
            return Some(format!("PID {} has no verified start time; it will not be signalled", key.pid));
        }
        if !self.store.latest().is_some_and(|sample| sample.process(key).is_some()) {
            return Some(format!("PID {} is no longer running", key.pid));
        }
        None
    }

    /// Whether the scrubbed sample has left the retained history (it is
    /// still shown, from the held `Arc`, until the cursor moves).
    pub fn cursor_expired(&self) -> bool {
        !self.live && self.cursor_sample.as_ref().is_some_and(|sample| self.store.at(sample.time_ms).is_none_or(|kept| kept.time_ms != sample.time_ms))
    }

    /// The retention tier of the view sample, for the cursor label.
    pub fn view_tier(&self) -> Tier {
        let now = self.store.newest_ms().unwrap_or(0);
        Tier::for_age(now.saturating_sub(self.view_ms()))
    }

    /// How far a recorded detail may lie before the view time and still
    /// count as "recorded then".
    pub fn detail_tolerance_ms(&self) -> u64 {
        (self.interval_ms * 3).max(3 * SECOND_MS)
    }
}
