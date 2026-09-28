//! The inspector's recorded views: Activity, History, Threads, Memory and
//! Files. Everything drawn here is a recorded reading at its own timestamp —
//! the basic sample for every process, the supplemental record for the
//! inspected and pinned ones (see `supp.rs`) — never a live lookup standing
//! in for a past time.
//!
//! Per frame only drawing happens: graph points are prepared once per store
//! generation, range and plot resolution, table rows once per generation and
//! view time, and live motion is the fractional window sliding over them.

use super::*;
use crate::metrics::{self, graph_points, Measure, Reading, Unit};
use crate::supp::{kind_name, thread_cpu_at, thread_points, CloseReason, Opened, ThreadKey};

/// Activity rows, in order.
pub(super) const ACTIVITY: [Measure; 17] = [
    Measure::Cpu,
    Measure::CpuTime,
    Measure::Threads,
    Measure::RunningThreads,
    Measure::ContextSwitches,
    Measure::Syscalls,
    Measure::Faults,
    Measure::Pageins,
    Measure::CowFaults,
    Measure::DiskRead,
    Measure::DiskWritten,
    Measure::IoRead,
    Measure::IoWritten,
    Measure::OpenFds,
    Measure::FdTable,
    Measure::Priority,
    Measure::Nice,
];

/// Memory figures, most telling first.
const MEMORY: [Measure; 11] = [
    Measure::Footprint,
    Measure::Resident,
    Measure::Commit,
    Measure::Virtual,
    Measure::PeakFootprint,
    Measure::PeakResident,
    Measure::PeakCommit,
    Measure::Wired,
    Measure::AnonResident,
    Measure::FileResident,
    Measure::Swapped,
];

const MEMORY_COUNTERS: [Measure; 3] = [Measure::Faults, Measure::Pageins, Measure::CowFaults];

/// What History shows first: CPU, then the first memory figure and the
/// first activity figure recorded for the process, so the recorded
/// expansion is visible without picking.
const HISTORY_DEFAULTS: [&[Measure]; 3] = [
    &[Measure::Cpu],
    &[Measure::Footprint, Measure::Resident, Measure::Commit],
    &[Measure::DiskWritten, Measure::DiskRead, Measure::IoWritten, Measure::ContextSwitches, Measure::Faults],
];

/// Timeline rows drawn at most.
const TIMELINE_ROWS: usize = 400;

pub(super) fn measure_color(measure: Measure, theme: &Theme) -> Vec4f {
    match measure {
        Measure::Cpu | Measure::CpuTime => theme.blue,
        Measure::Footprint | Measure::Resident | Measure::Commit | Measure::AnonResident => theme.green,
        Measure::Virtual | Measure::PeakFootprint | Measure::PeakResident | Measure::PeakCommit | Measure::FileResident | Measure::Wired | Measure::Swapped => {
            crate::mix(theme.green, theme.cyan, 0.5)
        }
        Measure::DiskRead | Measure::IoRead => theme.yellow,
        Measure::DiskWritten | Measure::IoWritten => theme.red,
        Measure::Faults | Measure::Pageins | Measure::CowFaults => theme.magenta,
        _ => theme.cyan,
    }
}

/// Graph points prepared for one window span and plot resolution, per
/// measure; rebuilt only when the data, the span or the resolution change.
#[derive(Default)]
pub(super) struct GraphCache {
    key: Option<(u64, u64, ProcKey, u64, u64, u64)>,
    series: HashMap<Measure, (Vec<Point>, f32)>,
}

impl GraphCache {
    fn series(&mut self, model: &Model, key: ProcKey, measure: Measure, window: (f64, f64), px: f64) -> &(Vec<Point>, f32) {
        let span = (window.1 - window.0).max(1.0);
        let bucket = fold_bucket_ms(span, px);
        let frozen = if model.live { 0 } else { window.1 as u64 };
        let cache_key = (model.store.generation, model.supp.generation, key, span as u64, bucket, frozen);
        if self.key != Some(cache_key) {
            self.key = Some(cache_key);
            self.series.clear();
        }
        self.series.entry(measure).or_insert_with(|| {
            // A little past the left edge, so the live window slides over
            // prepared points until the next sample rebuilds them.
            let from = (window.0 - span * 0.1 - bucket as f64).max(0.0) as u64;
            let readings = metrics::readings(&model.store, &model.supp, key, measure, from, u64::MAX);
            let points = fold_by_time(&graph_points(measure, &readings), bucket);
            let max = peak(&points);
            (points, max)
        })
    }
}

/// Thread CPU traces for the rows drawn, same keying as [`GraphCache`].
#[derive(Default)]
pub(super) struct ThreadTraceCache {
    key: Option<(u64, ProcKey, u64, u64, u64)>,
    traces: HashMap<ThreadKey, Vec<Point>>,
}

/// Every thread of one process as its own coloured CPU line in one graph
/// (History and Threads draw the same one). Each thread (id and
/// incarnation) gets a palette index once, in first-seen order, kept per
/// process, so it keeps its colour across samples, sorting, tab changes,
/// other processes and ends; the index is never shown.
#[derive(Default)]
pub(super) struct ThreadGraph {
    palettes: HashMap<ProcKey, (HashMap<ThreadKey, u32>, u32)>,
    key: Option<(u64, ProcKey, u64, u64, u64)>,
    lines: Vec<ThreadLine>,
    max: f32,
    /// The plot's top, from a ladder of round values; lowered only when the
    /// busiest line is well under the next step down, so it does not jump.
    top: f32,
    /// Each thread's recorded CPU at the cursor's thread observation (the
    /// same readings the Threads table shows), per record and view time.
    at_key: Option<(u64, ProcKey, u64)>,
    at: HashMap<ThreadKey, Option<f32>>,
}

#[derive(Clone)]
pub(super) struct ThreadLine {
    thread: ThreadKey,
    index: u32,
    name: String,
    /// Recorded interval CPU, folded for the plot; holes are gaps.
    points: Vec<Point>,
    peak: f32,
}

/// Round CPU tops for the thread graph, percent of one core.
const CPU_TOPS: [f32; 10] = [5.0, 10.0, 25.0, 50.0, 100.0, 200.0, 400.0, 800.0, 1600.0, 3200.0];

impl ThreadGraph {
    /// Lines for every thread with a record in the window (plus a margin
    /// for the live slide) that had begun by its right edge; rebuilt only
    /// when the record, span or resolution change, never per frame.
    fn prepare(&mut self, model: &Model, key: ProcKey, window: (f64, f64), px: f64) {
        let span = (window.1 - window.0).max(1.0);
        let bucket = fold_bucket_ms(span, px);
        let frozen = if model.live { 0 } else { window.1 as u64 };
        let cache_key = (model.supp.generation, key, span as u64, bucket, frozen);
        if self.key == Some(cache_key) {
            return;
        }
        if self.key.is_some_and(|k| k.1 != key) {
            self.top = 0.0;
        }
        self.key = Some(cache_key);
        self.lines.clear();
        self.max = 0.0;
        let Some(proc) = model.supp.get(key) else { return };
        if self.palettes.len() > 64 && !self.palettes.contains_key(&key) {
            self.palettes.clear();
        }
        let (numbers, next) = self.palettes.entry(key).or_default();
        let mut fresh: Vec<(u64, ThreadKey)> = proc
            .threads
            .iter()
            .filter(|(thread, _)| !numbers.contains_key(*thread))
            .filter_map(|(thread, track)| track.samples.first().map(|s| (s.time_ms, *thread)))
            .collect();
        fresh.sort_unstable();
        for (_, thread) in fresh {
            *next += 1;
            numbers.insert(thread, *next);
        }
        let from = (window.0 - span * 0.1 - bucket as f64).max(0.0) as u64;
        let (lo, hi) = (window.0.max(0.0) as u64, window.1.max(0.0) as u64);
        for (thread, track) in &proc.threads {
            let (Some(first), Some(last)) = (track.samples.first(), track.samples.last()) else { continue };
            // Not yet begun at the right edge, or over before the window.
            if last.time_ms < from || (!model.live && first.time_ms > hi) {
                continue;
            }
            let points = fold_by_time(&thread_points(track, from, u64::MAX), bucket);
            let peak = points.iter().filter(|p| p.time_ms >= lo && p.time_ms <= hi).fold(0.0f32, |m, p| m.max(p.value));
            self.max = self.max.max(peak);
            let index = numbers.get(thread).copied().unwrap_or(0);
            self.lines.push(ThreadLine { thread: *thread, index, name: track.name.to_string(), points, peak });
        }
        self.lines.sort_by_key(|line| line.index);
        // The top: the first round value above the busiest line; lowered
        // only once the busiest is under 60 % of the step below.
        let wanted = CPU_TOPS.iter().copied().find(|top| *top >= self.max * 1.05).unwrap_or(3200.0);
        let lower = CPU_TOPS.iter().copied().rev().find(|top| *top < self.top).unwrap_or(0.0);
        if wanted > self.top || self.max < lower * 0.6 {
            self.top = wanted;
        }
    }

    /// Each thread's recorded CPU at the thread observation for `view`
    /// (as the Threads table), refreshed when the record or view moves.
    fn values_at(&mut self, model: &Model, key: ProcKey, view: u64) {
        let at_key = (model.supp.generation, key, view);
        if self.at_key == Some(at_key) {
            return;
        }
        self.at_key = Some(at_key);
        self.at.clear();
        if let Some((_, found)) = model.supp.threads_at(key, view, tolerance(model)) {
            for (track, index) in found {
                self.at.insert((track.id, track.incarnation), thread_cpu_at(track, index));
            }
        }
    }
}

/// A distinct colour per thread number: hues a golden angle apart, so
/// neighbouring numbers never share a colour; lighter on dark themes.
pub(super) fn thread_color(number: u32, theme: &Theme) -> Vec4f {
    let hue = (number as f64 * 0.618_033_988_75).fract();
    let bg = theme.background;
    let dark = (bg.x + bg.y + bg.z) / 3.0 < 0.5;
    let (s, v) = if dark { (0.60, 0.97) } else { (0.78, 0.72) };
    let h = hue * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    vec4(r as f32, g as f32, b as f32, 1.0)
}

/// Legend entries shown before "Show all".
const LEGEND_SHORT: usize = 16;

/// One Activity or Memory row at the view time.
#[derive(Clone)]
pub(super) struct FigureRow {
    measure: Measure,
    now: Reading,
    rate: Option<f32>,
}

#[derive(Default)]
pub(super) struct FigureCache {
    key: Option<(u64, u64, ProcKey, u64)>,
    rows: Vec<FigureRow>,
}

/// One row of the Threads table at the view time.
#[derive(Clone)]
pub(super) struct ThreadRow {
    thread: ThreadKey,
    info: crate::backend::ThreadInfo,
}

#[derive(Default)]
pub(super) struct ThreadRowCache {
    key: Option<(u64, ProcKey, u64, usize, bool)>,
    /// When the thread list shown was observed, and whether it was whole.
    observed: Option<(u64, bool)>,
    /// The interval that list's rates cover (monotonic ms) and the process'
    /// CPU over the same interval, as recorded with it.
    interval: (Option<u32>, Option<f32>),
    rows: Vec<ThreadRow>,
    /// Threads seen ending inside the window: (thread, name, ended, last CPU time).
    ended: Vec<(ThreadKey, String, u64, Option<u64>)>,
}

/// One timeline entry of the Files tab.
#[derive(Clone)]
pub(super) struct FileEvent {
    time_ms: u64,
    what: String,
    fd: Option<i32>,
    kind: &'static str,
    path: String,
}

#[derive(Default)]
pub(super) struct FileCache {
    key: Option<(u64, ProcKey, u64, u64, u64)>,
    /// (fd, kind, path, first seen, last seen, state, open-and-observed)
    open: Vec<(i32, &'static str, String, u64, u64, String, bool)>,
    timeline: Vec<FileEvent>,
    /// The descriptor observation at or before the view: time, whole list,
    /// targets that could not be read.
    observed: Option<(u64, bool, u32)>,
    records: usize,
}

/// A drag on one of the large graphs: the cursor (left) or a pan (right),
/// with the graph and the window as drawn when it began, so the mapping
/// from pointer to time never drifts during the drag.
#[derive(Clone, Copy)]
pub(super) struct Gesture {
    pub pan: bool,
    pub rect: Rect,
    pub window: (f64, f64),
    pub start_x: f64,
    pub key: ProcKey,
    pub tab: InspectorTab,
}

/// The time under `x` in a graph `rect` drawn with `window`, clamped to it.
pub(super) fn time_at_f(rect: Rect, window: (f64, f64), x: f64) -> f64 {
    let t = ((x - rect.pos.x) / rect.size.x.max(1.0)).clamp(0.0, 1.0);
    window.0 + (window.1 - window.0) * t
}

/// The view time for recorded figures: the cursor, or while live the newest
/// thing recorded for this process.
fn view_time(model: &Model, key: ProcKey) -> u64 {
    if model.live {
        model.store.newest_ms().unwrap_or(0).max(model.supp.newest_ms(key).unwrap_or(0))
    } else {
        model.cursor_ms
    }
}

fn tolerance(model: &Model) -> u64 {
    if model.live {
        model.detail_tolerance_ms().max(5 * SECOND_MS)
    } else {
        model.detail_tolerance_ms()
    }
}

fn format_rate(measure: Measure, rate: Option<f32>) -> String {
    match rate {
        Some(value) if measure.is_counter() => measure.format_point(value),
        _ => String::new(),
    }
}

impl InspectorBody {
    /// The measures recorded for `key`, from the view sample or, when the
    /// process is not in it (exited, not yet started), the newest retained
    /// sample that has it; cached per generation.
    fn available(&mut self, model: &Model, key: ProcKey) -> Vec<Measure> {
        let cache_key = (model.store.generation, model.supp.generation, key);
        if let Some((k, measures)) = &self.available_cache {
            if *k == cache_key {
                return measures.clone();
            }
        }
        let view = model.view_sample().filter(|sample| sample.process(key).is_some()).or_else(|| model.store.latest_with(key, model.view_ms()).or_else(|| model.store.latest_with(key, u64::MAX)));
        let measures = metrics::available(view.map(|s| &**s), &model.supp, key);
        self.available_cache = Some((cache_key, measures.clone()));
        measures
    }

    fn figure_rows(&mut self, model: &Model, key: ProcKey, measures: &[Measure]) -> Vec<FigureRow> {
        let view = view_time(model, key);
        let cache_key = (model.store.generation, model.supp.generation, key, view);
        // One cache per figure set: each set has its own rows.
        let cache = if measures == &ACTIVITY[..] {
            &mut self.activity_rows
        } else if measures == &MEMORY[..] {
            &mut self.memory_rows
        } else {
            &mut self.counter_rows
        };
        if cache.key != Some(cache_key) {
            cache.key = Some(cache_key);
            let tol = tolerance(model);
            cache.rows = measures
                .iter()
                .filter_map(|measure| {
                    let (now, before) = metrics::reading_at(&model.store, &model.supp, key, *measure, view, tol)?;
                    let rate = before.and_then(|then| metrics::rate_between(*measure, then, now));
                    Some(FigureRow { measure: *measure, now, rate })
                })
                .collect();
        }
        cache.rows.clone()
    }

    /// Chips that pick what a tab graphs; filled when on.
    fn chips(&mut self, cx: &mut Cx2d, theme: &Theme, pen: &mut Pen, items: &[(Measure, bool)]) {
        let mut x = pen.x;
        let mut y = pen.y;
        for (measure, on) in items {
            let label = measure.short();
            let w = text_width(&self.draw_small, cx, label) + 24.0;
            if x + w > pen.x + pen.width && x > pen.x {
                x = pen.x;
                y += CHIP_ROW;
            }
            let chip = Rect { pos: dvec2(x, y), size: dvec2(w, CHIP_H) };
            if self.visible(y) {
                let color = measure_color(*measure, theme);
                if *on {
                    round(&mut self.draw_round, cx, chip, with_alpha(color, 0.18), 10.0);
                } else {
                    round_outline(&mut self.draw_round, cx, chip, with_alpha(theme.foreground, 0.0), theme.rule(), 10.0);
                }
                round(&mut self.draw_round, cx, Rect { pos: dvec2(x + 8.0, y + CHIP_H * 0.5 - 3.0), size: dvec2(6.0, 6.0) }, if *on { color } else { with_alpha(color, 0.45) }, 3.0);
                self.draw_small.color = if *on { theme.foreground } else { theme.secondary() };
                text_mid(&mut self.draw_small, cx, x + 18.0, y + CHIP_H * 0.5, label);
                self.chip_hits.push((*measure, chip));
            }
            x += w + CHIP_GAP;
        }
        pen.y = y + CHIP_ROW + 6.0;
    }

    /// One graph well: label and unit on the left, the value at the right
    /// edge (or cursor) on the right, the trace against the window.
    #[allow(clippy::too_many_arguments)]
    fn graph_well(&mut self, cx: &mut Cx2d, model: &Model, theme: &Theme, well: Rect, points: &[Point], max: f32, color: Vec4f, label: &str, unit: &str, format: impl Fn(f32) -> String) {
        if well.pos.y >= self.body.pos.y + self.body.size.y || well.pos.y + well.size.y <= self.body.pos.y {
            return;
        }
        let window = model.window_f();
        round(&mut self.draw_round, cx, well, theme.well(), 6.0);
        let inner = Rect { pos: well.pos + dvec2(2.0, 18.0), size: well.size - dvec2(4.0, 20.0) };
        // The part of the plot on screen takes the pointer gestures.
        let hit = intersect(inner, self.body);
        if hit.size.x > 0.0 && hit.size.y > 0.0 {
            self.graph_hits.push(Rect { pos: dvec2(inner.pos.x, hit.pos.y), size: dvec2(inner.size.x, hit.size.y) });
        }
        // Signed figures (nice, priority) get a range through zero.
        let low = points.iter().fold(0.0f32, |low, p| low.min(p.value));
        let min = if low < 0.0 { low * 1.15 } else { 0.0 };
        let max = max.max(0.0).max(f32::EPSILON) * 1.15;
        cx.push_clip_rect(well);
        if min < 0.0 {
            let zero_y = inner.pos.y + inner.size.y * (max as f64 / (max - min) as f64);
            hline(&mut self.draw_rect, cx, inner.pos.x, zero_y, inner.size.x, theme.rule());
        }
        draw_trace_range(&mut self.draw_seg, &mut self.draw_fill, cx, inner, points, window, min, max, Trace { color, width: 1.5, fill: 0.18 });
        cx.pop_clip_rect();
        self.draw_small.color = theme.secondary();
        self.draw_small.draw_abs(cx, well.pos + dvec2(8.0, 4.0), label);
        let lw = text_width(&self.draw_small, cx, label);
        if !unit.is_empty() {
            self.draw_small.color = theme.tertiary();
            self.draw_small.draw_abs(cx, well.pos + dvec2(16.0 + lw, 4.0), unit);
        }
        // The value at the view time: the last point at or before it.
        let at = if model.live { window.1 } else { model.cursor_ms as f64 };
        let end = points.partition_point(|p| (p.time_ms as f64) <= at);
        let value = points[..end].last().filter(|p| at - p.time_ms as f64 <= (window.1 - window.0) * 0.05 + 5000.0).map(|p| format(p.value));
        self.draw_mono.color = theme.foreground;
        text_right(&mut self.draw_mono, cx, well.pos.x + well.size.x - 8.0, well.pos.y + 3.0, &value.unwrap_or_else(|| "—".to_string()));
        if points.is_empty() {
            self.draw_small.color = theme.tertiary();
            text_mid(&mut self.draw_small, cx, inner.pos.x + 8.0, inner.pos.y + inner.size.y * 0.5, "not recorded in this window");
        }
        if !model.live && (model.cursor_ms as f64) >= window.0 && (model.cursor_ms as f64) <= window.1 {
            let x = inner.pos.x + (model.cursor_ms as f64 - window.0) / (window.1 - window.0).max(1.0) * inner.size.x;
            fill(&mut self.draw_rect, cx, Rect { pos: dvec2(x - 0.75, well.pos.y), size: dvec2(1.5, well.size.y) }, theme.accent);
        }
    }

    /// One line on what is recorded for this process beyond the basic
    /// figures, and since when.
    fn coverage_note(&mut self, cx: &mut Cx2d, theme: &Theme, pen: &mut Pen, model: &Model, key: ProcKey) {
        let since = model.supp.get(key).and_then(|p| p.points.first().map(|q| q.time_ms).or(p.thread_obs.first().map(|m| m.time_ms)));
        let how = if model.is_pinned(key) { "pinned" } else { "inspected" };
        let mut text = match since {
            Some(since) => format!("Detail recorded while {how}, since {}", LocalTime::from_epoch_ms(since).date_hm()),
            None => "Pin to record detail in the background".to_string(),
        };
        if model.supp.evicted_before_ms > 0 {
            text.push_str(&format!(" · older detail dropped by the memory budget before {}", local_hms(model.supp.evicted_before_ms)));
        }
        self.note(cx, theme, pen, &text);
    }

    /// A note that wraps to the pen's width.
    fn wrapped_note(&mut self, cx: &mut Cx2d, theme: &Theme, pen: &mut Pen, text: &str) {
        let mut line = String::new();
        for word in text.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && text_width(&self.draw_small, cx, &candidate) > pen.width {
                self.note(cx, theme, pen, &line);
                pen.y -= 4.0;
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        self.note(cx, theme, pen, &line);
    }

    // ---- Activity ----

    pub(super) fn draw_activity(&mut self, cx: &mut Cx2d, model: &Model, theme: &Theme, pen: &mut Pen, key: ProcKey) {
        let rows = self.figure_rows(model, key, &ACTIVITY);
        if rows.is_empty() {
            self.note(cx, theme, pen, "Nothing recorded for this process at this time.");
            return;
        }
        let view = view_time(model, key);
        let stale = rows.iter().any(|r| view.saturating_sub(r.now.time_ms) > 2 * SECOND_MS.max(model.interval_ms));
        if !model.live || stale {
            self.note(cx, theme, pen, &format!("Readings at or before {}; rates between each reading and the one before it.", local_hms(view)));
        }
        let text_rows: Vec<Vec<(String, Vec4f)>> = rows
            .iter()
            .map(|row| {
                vec![
                    (row.measure.label().to_string(), theme.foreground),
                    (row.measure.format_value(row.now.value), theme.foreground),
                    (format_rate(row.measure, row.rate), theme.secondary()),
                    (String::new(), theme.secondary()),
                    (local_hms(row.now.time_ms), theme.tertiary()),
                ]
            })
            .collect();
        let columns: [(&str, f64, bool, bool); 5] = [("Figure", 0.0, false, false), ("Value / total", 130.0, true, true), ("Rate", 110.0, true, true), ("Last minute", 120.0, false, false), ("Read", 80.0, true, true)];
        let (widths, first) = self.table(cx, theme, pen, &columns, &text_rows);
        // Sparklines in the "Last minute" column, and each row a link to its
        // graph in History.
        let spark_x = pen.x + widths[..3].iter().sum::<f64>();
        let end = model.graph_end();
        let window = (end - MINUTE_MS as f64, end);
        for (index, row) in rows.iter().enumerate() {
            let y = first + index as f64 * pen.line;
            let hit = Rect { pos: dvec2(pen.x, y - 4.0), size: dvec2(pen.width, pen.line) };
            self.row_links.push((row.measure, hit));
            if !self.visible(y) {
                continue;
            }
            let (points, max) = self.spark_cache.series(model, key, row.measure, window, widths[3] - 12.0).clone();
            let rect = Rect { pos: dvec2(spark_x, y - 1.0), size: dvec2(widths[3] - 14.0, pen.line - 8.0) };
            cx.push_clip_rect(rect);
            draw_trace(&mut self.draw_seg, &mut self.draw_fill, cx, rect, &points, window, max.max(f32::EPSILON) * 1.15, Trace { color: measure_color(row.measure, theme), width: 1.2, fill: 0.15 });
            cx.pop_clip_rect();
        }
        pen.y += 6.0;
        self.note(cx, theme, pen, "Click a figure to graph it.");
        if rows.iter().any(|r| r.measure == Measure::FdTable) {
            self.note(cx, theme, pen, "Descriptor table: slots the kernel allocated for descriptors, not the number open (see Files).");
        }
    }

    // ---- History ----

    /// The per-thread CPU graph: one line per thread in its own colour, on
    /// one time axis and one CPU scale with round guides, and the shared
    /// cursor; a legend below gives each thread's name and its recorded CPU
    /// at the cursor (busiest in the window first); a click on an entry
    /// picks that thread out. Returns false when nothing was recorded.
    pub(super) fn draw_thread_graph(&mut self, cx: &mut Cx2d, model: &Model, theme: &Theme, pen: &mut Pen, key: ProcKey, height: f64) -> bool {
        let window = model.window_f();
        let mut graph = std::mem::take(&mut self.thread_graph);
        graph.prepare(model, key, window, pen.width - 44.0);
        if graph.lines.is_empty() {
            self.thread_graph = graph;
            return false;
        }
        graph.values_at(model, key, view_time(model, key));
        let selected = self.selected_thread.filter(|(k, _)| *k == key).map(|(_, thread)| thread);
        let well = Rect { pos: dvec2(pen.x, pen.y), size: dvec2(pen.width, height) };
        let top = graph.top.max(5.0);
        if well.pos.y < self.body.pos.y + self.body.size.y && well.pos.y + well.size.y > self.body.pos.y {
            round(&mut self.draw_round, cx, well, theme.well(), 6.0);
            // Room on the left for the guide labels.
            let inner = Rect { pos: well.pos + dvec2(40.0, 20.0), size: well.size - dvec2(44.0, 24.0) };
            let hit = intersect(inner, self.body);
            if hit.size.x > 0.0 && hit.size.y > 0.0 {
                self.graph_hits.push(Rect { pos: dvec2(inner.pos.x, hit.pos.y), size: dvec2(inner.size.x, hit.size.y) });
            }
            // Guides at 0, half and the top.
            for fraction in [0.0, 0.5, 1.0] {
                let y = inner.pos.y + inner.size.y * (1.0 - fraction);
                hline(&mut self.draw_rect, cx, inner.pos.x, y, inner.size.x, with_alpha(theme.foreground, if fraction == 0.0 { 0.16 } else { 0.07 }));
                let value = top as f64 * fraction;
                let label = if value.fract() == 0.0 { format!("{value:.0}%") } else { format!("{value:.1}%") };
                self.draw_small.color = theme.tertiary();
                text_right(&mut self.draw_small, cx, inner.pos.x - 6.0, y - 7.0, &label);
            }
            cx.push_clip_rect(Rect { pos: dvec2(inner.pos.x, well.pos.y), size: dvec2(inner.size.x, well.size.y) });
            // Lines only, no fill, so no thread hides another. Every thread
            // with a measured rate is drawn (an idle one on the baseline);
            // a picked thread is drawn last and bold, the others dimmed.
            for line in graph.lines.iter().filter(|l| !l.points.is_empty() && Some(l.thread) != selected) {
                let mut color = thread_color(line.index, theme);
                if selected.is_some() {
                    color.w = 0.28;
                }
                draw_trace_range(&mut self.draw_seg, &mut self.draw_fill, cx, inner, &line.points, window, 0.0, top, Trace { color, width: 1.3, fill: 0.0 });
            }
            if let Some(line) = graph.lines.iter().find(|l| Some(l.thread) == selected) {
                draw_trace_range(&mut self.draw_seg, &mut self.draw_fill, cx, inner, &line.points, window, 0.0, top, Trace { color: thread_color(line.index, theme), width: 2.4, fill: 0.0 });
            }
            if !model.live && (model.cursor_ms as f64) >= window.0 && (model.cursor_ms as f64) <= window.1 {
                let x = inner.pos.x + (model.cursor_ms as f64 - window.0) / (window.1 - window.0).max(1.0) * inner.size.x;
                fill(&mut self.draw_rect, cx, Rect { pos: dvec2(x - 0.75, well.pos.y), size: dvec2(1.5, well.size.y) }, theme.accent);
            }
            cx.pop_clip_rect();
            self.draw_small.color = theme.secondary();
            self.draw_small.draw_abs(cx, well.pos + dvec2(8.0, 4.0), "CPU by thread");
            let w = text_width(&self.draw_small, cx, "CPU by thread");
            self.draw_small.color = theme.tertiary();
            self.draw_small.draw_abs(cx, well.pos + dvec2(16.0 + w, 4.0), "% of a core, one line per thread");
        }
        pen.y += height + 6.0;
        // The legend: the thread's name, then its id only where the name is
        // empty or shared (and the incarnation where an id was reused), and
        // its recorded CPU at the cursor. Busiest in the window first, in
        // whole-percent steps, so it does not reshuffle on every sample.
        let mut names: HashMap<&str, usize> = HashMap::new();
        for line in &graph.lines {
            *names.entry(line.name.as_str()).or_default() += 1;
        }
        let reused: HashSet<u64> = graph.lines.iter().filter(|l| l.thread.1 > 0).map(|l| l.thread.0).collect();
        let mut order: Vec<&ThreadLine> = graph.lines.iter().collect();
        order.sort_by(|a, b| (b.peak.round() as i64).cmp(&(a.peak.round() as i64)).then(a.index.cmp(&b.index)));
        let shown = if self.legend_all { order.len() } else { order.len().min(LEGEND_SHORT) };
        let columns = ((pen.width / 220.0).floor() as usize).max(1);
        let cell = pen.width / columns as f64;
        for (index, line) in order.iter().take(shown).enumerate() {
            let x = pen.x + (index % columns) as f64 * cell;
            let y = pen.y + (index / columns) as f64 * 20.0;
            let entry = Rect { pos: dvec2(x, y - 2.0), size: dvec2(cell - 6.0, 20.0) };
            self.legend_hits.push((line.thread, entry));
            if !self.visible(y) {
                continue;
            }
            let picked = selected == Some(line.thread);
            if picked {
                round(&mut self.draw_round, cx, entry, theme.well(), 4.0);
            }
            round(&mut self.draw_round, cx, Rect { pos: dvec2(x + 4.0, y + 5.0), size: dvec2(10.0, 3.0) }, thread_color(line.index, theme), 1.5);
            let value = graph.at.get(&line.thread).copied().flatten().map(|v| format!("{v:.1}%")).unwrap_or_else(|| "—".to_string());
            self.draw_mono.color = if picked { theme.foreground } else { theme.secondary() };
            let vw = text_width(&self.draw_mono, cx, &value);
            self.draw_mono.draw_abs(cx, dvec2(x + cell - 12.0 - vw, y), &value);
            let mut label = if line.name.is_empty() { "unnamed".to_string() } else { line.name.clone() };
            if line.name.is_empty() || names.get(line.name.as_str()).copied().unwrap_or(0) > 1 {
                label.push_str(&format!(" · {}", line.thread.0));
            }
            if reused.contains(&line.thread.0) {
                label.push_str(&format!(" ({})", line.thread.1 + 1));
            }
            self.draw_small.color = if picked { theme.foreground } else { theme.secondary() };
            text_fit(&mut self.draw_small, cx, dvec2(x + 18.0, y), cell - 40.0 - vw, &label);
        }
        pen.y += shown.div_ceil(columns) as f64 * 20.0;
        if order.len() > LEGEND_SHORT {
            let label = if self.legend_all { "Show fewer".to_string() } else { format!("Show all {} threads", order.len()) };
            if self.visible(pen.y) {
                self.draw_small.color = theme.accent;
                self.draw_small.draw_abs(cx, dvec2(pen.x + 4.0, pen.y), &label);
                self.legend_more = Rect { pos: dvec2(pen.x, pen.y - 2.0), size: dvec2(text_width(&self.draw_small, cx, &label) + 10.0, 20.0) };
            }
            pen.y += 22.0;
        }
        pen.y += 6.0;
        self.thread_graph = graph;
        true
    }

    /// The number and colour a thread has in the thread graph, if any.
    fn thread_number(&self, key: ProcKey, thread: ThreadKey) -> Option<u32> {
        self.thread_graph.palettes.get(&key).and_then(|(numbers, _)| numbers.get(&thread).copied())
    }

    pub(super) fn draw_history(&mut self, cx: &mut Cx2d, model: &Model, theme: &Theme, pen: &mut Pen, key: ProcKey) {
        let available = self.available(model, key);
        if self.history_measures.is_empty() {
            self.history_measures = HISTORY_DEFAULTS.iter().filter_map(|group| group.iter().copied().find(|m| available.contains(m))).collect();
        }
        let chips: Vec<(Measure, bool)> = available.iter().map(|m| (*m, self.history_measures.contains(m))).collect();
        self.chips(cx, theme, pen, &chips);
        let shown: Vec<Measure> = self.history_measures.iter().copied().filter(|m| available.contains(m)).collect();
        if shown.is_empty() {
            self.note(cx, theme, pen, "Pick a figure above to graph it.");
        }
        let space = (self.body.size.y - 150.0).max(120.0);
        let height = ((space - shown.len() as f64 * 8.0) / shown.len().max(1) as f64).clamp(64.0, 130.0);
        let window = model.window_f();
        // Every thread's CPU as its own line, right after the process' CPU
        // (first when CPU is not picked), whenever threads were recorded.
        let mut threads_drawn = !shown.contains(&Measure::Cpu) && self.draw_thread_graph(cx, model, theme, pen, key, 210.0);
        for measure in shown {
            let well = Rect { pos: dvec2(pen.x, pen.y), size: dvec2(pen.width, height) };
            let (points, max) = self.history_cache.series(model, key, measure, window, pen.width).clone();
            let max = if measure.unit() == Unit::Percent { max.max(5.0) } else { max };
            self.graph_well(cx, model, theme, well, &points, max, measure_color(measure, theme), measure.label(), measure.graph_unit(), |v| measure.format_point(v));
            pen.y += height + 8.0;
            if measure == Measure::Cpu && !threads_drawn {
                threads_drawn = self.draw_thread_graph(cx, model, theme, pen, key, 210.0);
            }
        }
        self.coverage_note(cx, theme, pen, model, key);
    }

    // ---- Threads ----

    fn thread_rows(&mut self, model: &Model, key: ProcKey) {
        let view = view_time(model, key);
        let window = model.window();
        let cache_key = (model.supp.generation, key, view, self.thread_sort.column, self.thread_sort.descending);
        if self.thread_rows.key == Some(cache_key) {
            return;
        }
        self.thread_rows.key = Some(cache_key);
        self.thread_rows.rows.clear();
        self.thread_rows.ended.clear();
        self.thread_rows.observed = None;
        let Some((mark, found)) = model.supp.threads_at(key, view, tolerance(model)) else { return };
        self.thread_rows.observed = Some((mark.time_ms, mark.complete));
        self.thread_rows.interval = (mark.span_ms, mark.process_pct);
        // One incarnation per id is current at one observation, so the id
        // orders and the (id, incarnation) identifies.
        let incarnation: HashMap<u64, u32> = found.iter().map(|(track, _)| (track.id, track.incarnation)).collect();
        let infos: Vec<crate::backend::ThreadInfo> = found
            .iter()
            .map(|(track, index)| {
                let sample = track.samples[*index];
                crate::backend::ThreadInfo {
                    id: track.id,
                    name: track.name.to_string(),
                    state: sample.state,
                    cpu_pct: thread_cpu_at(track, *index).map(|v| v as f64),
                    cpu_time_ns: sample.cpu_time_ns,
                }
            })
            .collect();
        let mut sorted: Vec<&crate::backend::ThreadInfo> = infos.iter().collect();
        sort_threads(&mut sorted, self.thread_sort);
        self.thread_rows.rows = sorted.into_iter().map(|info| ThreadRow { thread: (info.id, incarnation.get(&info.id).copied().unwrap_or(0)), info: info.clone() }).collect();
        if let Some(proc) = model.supp.get(key) {
            let mut ended: Vec<(ThreadKey, String, u64, Option<u64>)> = proc
                .threads
                .iter()
                .filter_map(|(thread, track)| {
                    let ended = track.ended_ms?;
                    (ended >= window.0 && ended <= view).then(|| (*thread, track.name.to_string(), ended, track.samples.last().and_then(|s| s.cpu_time_ns)))
                })
                .collect();
            ended.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
            ended.truncate(200);
            self.thread_rows.ended = ended;
        }
    }

    fn thread_trace(&mut self, model: &Model, key: ProcKey, id: ThreadKey, window: (f64, f64), px: f64) -> Vec<Point> {
        let span = (window.1 - window.0).max(1.0);
        let bucket = fold_bucket_ms(span, px);
        let frozen = if model.live { 0 } else { window.1 as u64 };
        let cache_key = (model.supp.generation, key, span as u64, bucket, frozen);
        if self.thread_traces.key != Some(cache_key) {
            self.thread_traces.key = Some(cache_key);
            self.thread_traces.traces.clear();
        }
        if let Some(points) = self.thread_traces.traces.get(&id) {
            return points.clone();
        }
        let from = (window.0 - span * 0.1 - bucket as f64).max(0.0) as u64;
        let points = model
            .supp
            .get(key)
            .and_then(|proc| proc.threads.get(&id))
            .map(|track| fold_by_time(&thread_points(track, from, u64::MAX), bucket))
            .unwrap_or_default();
        self.thread_traces.traces.insert(id, points.clone());
        points
    }

    /// Threads from the record: returns false when nothing was recorded, so
    /// the caller can fall back to the latest detail read.
    pub(super) fn draw_threads(&mut self, cx: &mut Cx2d, model: &Model, theme: &Theme, pen: &mut Pen, key: ProcKey) -> bool {
        self.thread_rows(model, key);
        // Every thread's CPU over the window as its own line, on top,
        // whenever the window has thread records, even where the cursor
        // falls in a gap or after the process ended; the interval
        // attribution and the sortable table follow.
        let graph = self.draw_thread_graph(cx, model, theme, pen, key, 250.0);
        let view = view_time(model, key);
        let Some((observed, complete)) = self.thread_rows.observed else {
            if graph {
                self.note(cx, theme, pen, &format!("No thread observation at {}.", local_hms(view)));
            }
            return graph;
        };
        let rows = self.thread_rows.rows.clone();
        if !model.live || view.saturating_sub(observed) > 2 * SECOND_MS.max(model.interval_ms) {
            self.note(cx, theme, pen, &format!("Threads as observed at {}{}", local_hms(observed), if complete { "" } else { " (a partial list)" }));
        }
        // Attribution over one interval: the process' CPU and each thread's,
        // all from CPU-time deltas between the same two reads. A thread
        // without a rate (new, reset, after a gap) is unknown, never 0; what
        // the listed threads do not account for (threads that ended inside
        // the interval, unknown ones) is shown, not hidden.
        let (span_ms, process_pct) = self.thread_rows.interval;
        let known: f32 = rows.iter().filter_map(|r| r.info.cpu_pct).map(|v| v as f32).sum();
        let unknown = rows.iter().filter(|r| r.info.cpu_pct.is_none()).count();
        let evidence = match (span_ms, process_pct) {
            (Some(span), Some(process)) => {
                let mut text = format!(
                    "Over {:.2} s to {}: process {process:.1}% · threads listed {known:.1}%",
                    span as f64 / 1000.0,
                    local_hms(observed)
                );
                if process - known > 0.5 {
                    text.push_str(&format!(" · not attributed {:.1}%", process - known));
                }
                if unknown > 0 {
                    text.push_str(&format!(" · {unknown} not yet measured"));
                }
                text
            }
            _ => format!("No rates at {}: the first read after a start or a gap only sets the baseline.", local_hms(observed)),
        };
        self.wrapped_note(cx, theme, pen, &evidence);
        let window = model.window_f();
        let selected = self.selected_thread.filter(|(k, _)| *k == key).map(|(_, thread)| thread);
        let text_rows: Vec<Vec<(String, Vec4f)>> = rows
            .iter()
            .map(|row| {
                let t = &row.info;
                let chosen = selected == Some(row.thread);
                let state_color = if t.state == ThreadState::Running { theme.green } else { theme.secondary() };
                vec![
                    (if t.name.is_empty() { "unnamed".to_string() } else { t.name.clone() }, if chosen { theme.accent } else if t.name.is_empty() { theme.secondary() } else { theme.foreground }),
                    (t.state.as_str().to_string(), state_color),
                    (t.cpu_pct.map(|p| format!("{p:.1}")).unwrap_or_else(|| "—".to_string()), theme.foreground),
                    (t.cpu_time_ns.map(format_duration_ns).unwrap_or_else(|| "—".to_string()), theme.foreground),
                    (String::new(), theme.secondary()),
                    (t.id.to_string(), theme.tertiary()),
                ]
            })
            .collect();
        let id_title = if cfg!(target_os = "macos") { "Handle" } else { "TID" };
        self.table_sort = Some((self.thread_sort.column, self.thread_sort.descending));
        let columns: [(&str, f64, bool, bool); 6] = [("Thread", 0.0, false, false), ("State", 110.0, false, false), ("% CPU", 76.0, true, true), ("CPU time", 100.0, true, true), ("Trend", 110.0, false, false), (id_title, 130.0, true, true)];
        let (widths, first) = self.table(cx, theme, pen, &columns, &text_rows);
        let trend_x = pen.x + widths[..4].iter().sum::<f64>();
        for (index, row) in rows.iter().enumerate() {
            let y = first + index as f64 * pen.line;
            self.thread_hits.push((row.thread, Rect { pos: dvec2(pen.x, y - 4.0), size: dvec2(pen.width, pen.line) }));
            if !self.visible(y) {
                continue;
            }
            // Each row carries its line's colour; the picked row is marked.
            let color = self.thread_number(key, row.thread).map(|n| thread_color(n, theme)).unwrap_or(theme.blue);
            let marker = if selected == Some(row.thread) { 4.0 } else { 2.0 };
            fill(&mut self.draw_rect, cx, Rect { pos: dvec2(pen.x - 8.0, y - 3.0), size: dvec2(marker, pen.line - 4.0) }, color);
            let points = self.thread_trace(model, key, row.thread, window, widths[4] - 12.0);
            let rect = Rect { pos: dvec2(trend_x, y - 1.0), size: dvec2(widths[4] - 14.0, pen.line - 8.0) };
            cx.push_clip_rect(rect);
            draw_trace(&mut self.draw_seg, &mut self.draw_fill, cx, rect, &points, window, peak(&points).max(5.0) * 1.15, Trace { color, width: 1.2, fill: 0.15 });
            cx.pop_clip_rect();
        }
        let ended = self.thread_rows.ended.clone();
        if !ended.is_empty() {
            pen.y += 10.0;
            if self.visible(pen.y) {
                self.draw_heading.color = theme.secondary();
                self.draw_heading.draw_abs(cx, dvec2(pen.x, pen.y), "Ended in this window");
            }
            pen.y += 22.0;
            let ended_rows: Vec<Vec<(String, Vec4f)>> = ended
                .iter()
                .map(|((id, incarnation), name, at, time)| {
                    vec![
                        (if name.is_empty() { "unnamed".to_string() } else { name.clone() }, theme.secondary()),
                        (format!("ended by {}", local_hms(*at)), theme.secondary()),
                        (time.map(format_duration_ns).unwrap_or_else(|| "—".to_string()), theme.secondary()),
                        (if *incarnation > 0 { format!("{id} #{}", incarnation + 1) } else { id.to_string() }, theme.tertiary()),
                    ]
                })
                .collect();
            let (_, first) = self.table(cx, theme, pen, &[("Thread", 0.0, false, false), ("Seen ending", 170.0, false, false), ("CPU time", 100.0, true, true), (id_title, 130.0, true, true)], &ended_rows);
            for (index, (id, _, _, _)) in ended.iter().enumerate() {
                let y = first + index as f64 * pen.line;
                self.thread_hits.push((*id, Rect { pos: dvec2(pen.x, y - 4.0), size: dvec2(pen.width, pen.line) }));
            }
        }
        pen.y += 6.0;
        self.wrapped_note(cx, theme, pen, "% CPU is each thread's CPU time over the time between two reads, the same measure as the process'. A thread id seen again after ending, or with less CPU time, starts a new line.");
        true
    }

    // ---- Memory ----

    pub(super) fn draw_memory(&mut self, cx: &mut Cx2d, model: &Model, theme: &Theme, pen: &mut Pen, key: ProcKey) {
        let rows = self.figure_rows(model, key, &MEMORY);
        // Summary: up to five figures, the ledger first.
        let summary: Vec<(String, &str)> = rows.iter().take(5).map(|r| (r.measure.format_value(r.now.value), r.measure.label())).collect();
        if summary.is_empty() {
            self.note(cx, theme, pen, "No memory figure recorded for this process at this time.");
        } else {
            let slot = (pen.width / summary.len() as f64).clamp(110.0, 190.0);
            for (index, (value, label)) in summary.iter().enumerate() {
                let x = pen.x + index as f64 * slot;
                if self.visible(pen.y) {
                    self.draw_value.color = theme.foreground;
                    text_fit(&mut self.draw_value, cx, dvec2(x, pen.y), slot - 10.0, value);
                    self.draw_small.color = theme.secondary();
                    text_fit(&mut self.draw_small, cx, dvec2(x, pen.y + 22.0), slot - 10.0, label);
                }
            }
            pen.y += 48.0;
        }
        // Trends on the shared window for the first three figures present.
        let window = model.window_f();
        let trends: Vec<Measure> = rows.iter().map(|r| r.measure).filter(|m| !matches!(m, Measure::PeakFootprint | Measure::PeakResident | Measure::PeakCommit)).take(3).collect();
        for measure in trends {
            let well = Rect { pos: dvec2(pen.x, pen.y), size: dvec2(pen.width, 70.0) };
            let (points, max) = self.memory_cache.series(model, key, measure, window, pen.width).clone();
            self.graph_well(cx, model, theme, well, &points, max, measure_color(measure, theme), measure.label(), "", |v| measure.format_point(v));
            pen.y += 78.0;
        }
        // Every figure with the OS's meaning.
        if !rows.is_empty() {
            let text_rows: Vec<Vec<(String, Vec4f)>> = rows
                .iter()
                .map(|r| vec![(r.measure.label().to_string(), theme.foreground), (r.measure.format_value(r.now.value), theme.foreground), (r.measure.meaning().to_string(), theme.secondary())])
                .collect();
            pen.y += 4.0;
            let (_, first) = self.table(cx, theme, pen, &[("Measure", 170.0, false, false), ("Bytes", 110.0, true, true), ("Meaning", 0.0, false, false)], &text_rows);
            for (index, row) in rows.iter().enumerate() {
                self.row_links.push((row.measure, Rect { pos: dvec2(pen.x, first + index as f64 * pen.line - 4.0), size: dvec2(pen.width, pen.line) }));
            }
        }
        // Fault counters with their rates.
        let counters = self.figure_rows(model, key, &MEMORY_COUNTERS);
        if !counters.is_empty() {
            pen.y += 10.0;
            let text_rows: Vec<Vec<(String, Vec4f)>> = counters
                .iter()
                .map(|r| vec![(r.measure.label().to_string(), theme.foreground), (r.measure.format_value(r.now.value), theme.foreground), (format_rate(r.measure, r.rate), theme.secondary())])
                .collect();
            let (_, first) = self.table(cx, theme, pen, &[("Counter", 170.0, false, false), ("Total", 130.0, true, true), ("Rate", 0.0, false, true)], &text_rows);
            for (index, row) in counters.iter().enumerate() {
                self.row_links.push((row.measure, Rect { pos: dvec2(pen.x, first + index as f64 * pen.line - 4.0), size: dvec2(pen.width, pen.line) }));
            }
        }
        // The address-space walk runs at the Libraries cadence only; it is a
        // different measurement, behind a disclosure.
        let (detail, _) = self.detail_for(model, key, true);
        if let Some(Detail::Ready(memory)) = detail.map(|d| &d.memory) {
            if let Some(regions) = memory.regions {
                pen.y += 8.0;
                if self.visible(pen.y) {
                    self.draw_icon.color = theme.secondary();
                    text_mid(&mut self.draw_icon, cx, pen.x + 2.0, pen.y + 9.0, if self.regions_open { ICON_CHEVRON_DOWN } else { ICON_CHEVRON_RIGHT });
                    self.draw_label.color = theme.secondary();
                    text_mid(&mut self.draw_label, cx, pen.x + 18.0, pen.y + 9.0, "Address space walk");
                    self.regions_toggle = Rect { pos: dvec2(pen.x, pen.y - 2.0), size: dvec2(220.0, 22.0) };
                }
                pen.y += 28.0;
                if self.regions_open {
                    self.section(
                        cx,
                        theme,
                        pen,
                        &format!("Regions, read {}", local_hms(detail.map(|d| d.time_ms).unwrap_or(0))),
                        &[("Regions", regions.regions.to_string()), ("Resident", format_bytes(regions.resident))],
                        &[("Private", format_bytes(regions.private_resident)), ("Shared", format_bytes(regions.shared_resident))],
                    );
                    self.wrapped_note(cx, theme, pen, "Summed resident pages per mapping: shared pages count in every process mapping them, so this is not the footprint above. Walked every 10 s while inspected.");
                }
            }
        }
        pen.y += 4.0;
        self.note(cx, theme, pen, "Click a figure to graph it in History.");
    }

    // ---- Files ----

    fn file_rows(&mut self, model: &Model, key: ProcKey) {
        let view = view_time(model, key);
        let window = model.window();
        let cache_key = (model.supp.generation, key, view, window.0 / SECOND_MS, window.1 / SECOND_MS);
        if self.file_rows.key == Some(cache_key) {
            return;
        }
        let cache = &mut self.file_rows;
        cache.key = Some(cache_key);
        cache.open.clear();
        cache.timeline.clear();
        cache.observed = None;
        cache.records = 0;
        let Some(proc) = model.supp.get(key) else { return };
        cache.records = proc.files.len();
        let tol = tolerance(model);
        let end = proc.file_obs.partition_point(|m| m.time_ms <= view);
        cache.observed = proc.file_obs[..end].last().map(|m| (m.time_ms, m.complete, m.unreadable));
        // Everything here is as of the view time: the last observation at or
        // before it that affirmed each record, and no close, sighting or
        // other fact recorded later.
        for record in &proc.files {
            if record.first_seen_ms > view || record.closed.is_some_and(|(gone, _)| gone <= view) {
                continue;
            }
            let seen = proc.last_seen_as_of(record, view).unwrap_or(record.first_seen_ms);
            let observed = seen + tol >= view;
            let state = if observed { "open".to_string() } else { format!("not observed since {}", local_hms(seen)) };
            cache.open.push((record.fd, kind_name(record.kind), record.path.to_string(), record.first_seen_ms, seen, state, observed));
        }
        cache.open.sort_by(|a, b| b.6.cmp(&a.6).then(a.0.cmp(&b.0)).then(a.3.cmp(&b.3)));
        // The timeline over the window: opens, closes and unobserved spans.
        let mut events: Vec<FileEvent> = Vec::new();
        for record in &proc.files {
            if record.first_seen_ms >= window.0 && record.first_seen_ms <= window.1 {
                let what = match record.opened {
                    Opened::AlreadyOpen => "already open when first observed".to_string(),
                    Opened::After(then) => format!("opened after {}", local_hms(then)),
                };
                events.push(FileEvent { time_ms: record.first_seen_ms, what, fd: Some(record.fd), kind: kind_name(record.kind), path: record.path.to_string() });
            }
            if let Some((gone, reason)) = record.closed {
                if gone >= window.0 && gone <= window.1 {
                    let what = match reason {
                        CloseReason::Replaced => format!("replaced; last seen {}", local_hms(record.last_seen_ms)),
                        CloseReason::Exited => format!("process exited; last seen {}", local_hms(record.last_seen_ms)),
                        CloseReason::Absent => format!("closed; last seen {}", local_hms(record.last_seen_ms)),
                    };
                    events.push(FileEvent { time_ms: gone, what, fd: Some(record.fd), kind: kind_name(record.kind), path: record.path.to_string() });
                }
            }
        }
        for (index, mark) in proc.file_obs.iter().enumerate() {
            if mark.gap_before && mark.time_ms >= window.0 && mark.time_ms <= window.1 {
                let what = match index.checked_sub(1).map(|i| proc.file_obs[i].time_ms) {
                    Some(then) => format!("not observed from {}", local_hms(then)),
                    None => "first observation".to_string(),
                };
                events.push(FileEvent { time_ms: mark.time_ms, what, fd: None, kind: "", path: String::new() });
            }
        }
        if let Some((stopped, exited)) = proc.stopped {
            if stopped >= window.0 && stopped <= window.1 {
                let what = if exited { "process exited; recording ended" } else { "recording ended (unpinned)" };
                events.push(FileEvent { time_ms: stopped, what: what.to_string(), fd: None, kind: "", path: String::new() });
            }
        }
        events.sort_by(|a, b| b.time_ms.cmp(&a.time_ms).then(a.fd.cmp(&b.fd)));
        events.truncate(TIMELINE_ROWS);
        cache.timeline = events;
    }

    /// Descriptors from the record: returns false when nothing was recorded.
    pub(super) fn draw_files(&mut self, cx: &mut Cx2d, model: &Model, theme: &Theme, pen: &mut Pen, key: ProcKey) -> bool {
        self.file_rows(model, key);
        if self.file_rows.records == 0 && self.file_rows.observed.is_none() {
            return false;
        }
        let open = self.file_rows.open.clone();
        let observed = self.file_rows.observed;
        let mut kinds: Vec<(&str, usize)> = Vec::new();
        for (_, kind, ..) in open.iter().filter(|o| o.6) {
            match kinds.iter_mut().find(|(k, _)| k == kind) {
                Some(entry) => entry.1 += 1,
                None => kinds.push((kind, 1)),
            }
        }
        let summary = kinds.iter().map(|(kind, n)| format!("{n} {kind}")).collect::<Vec<_>>().join(" · ");
        let at = observed.map(|(time, _, _)| local_hms(time)).unwrap_or_else(|| "—".to_string());
        self.text(cx, pen.x, pen.y, pen.width, theme.foreground, &format!("{} open at the observation of {at} · {summary}", open.iter().filter(|o| o.6).count()));
        pen.y += 24.0;
        if let Some((_, complete, unreadable)) = observed {
            let mut caveats = Vec::new();
            if !complete {
                caveats.push("that list was partial, so nothing missing from it counts as closed".to_string());
            }
            if unreadable > 0 {
                caveats.push(format!("{unreadable} descriptors had targets that could not be read and are not affirmed"));
            }
            if !caveats.is_empty() {
                let text = format!("Note: {}.", caveats.join("; "));
                self.wrapped_note(cx, theme, pen, &text);
            }
        }
        self.note(cx, theme, pen, "Observation times, not kernel event times.");
        let rows: Vec<Vec<(String, Vec4f)>> = open
            .iter()
            .map(|(fd, kind, path, first, last, state, observed)| {
                let dim = if *observed { theme.foreground } else { theme.secondary() };
                vec![
                    (fd.to_string(), theme.secondary()),
                    (kind.to_string(), theme.secondary()),
                    (path.clone(), dim),
                    (local_hms(*first), theme.secondary()),
                    (local_hms(*last), theme.secondary()),
                    (state.clone(), if *observed { theme.green } else { theme.yellow }),
                ]
            })
            .collect();
        self.table(cx, theme, pen, &[("FD", 50.0, true, true), ("Kind", 64.0, false, false), ("Path", 0.0, false, true), ("First seen", 84.0, true, true), ("Last seen", 84.0, true, true), ("State", 150.0, false, false)], &rows);
        let timeline = self.file_rows.timeline.clone();
        pen.y += 12.0;
        if self.visible(pen.y) {
            self.draw_heading.color = theme.secondary();
            self.draw_heading.draw_abs(cx, dvec2(pen.x, pen.y), "Timeline in the history window");
        }
        pen.y += 22.0;
        if timeline.is_empty() {
            self.note(cx, theme, pen, "No descriptor opened or closed in this window.");
            return true;
        }
        let rows: Vec<Vec<(String, Vec4f)>> = timeline
            .iter()
            .map(|e| {
                // Later than the cursor: shown for the window, set apart.
                let later = !model.live && e.time_ms > model.cursor_ms;
                vec![
                    (local_hms(e.time_ms), theme.secondary()),
                    (e.what.clone(), if later { theme.tertiary() } else if e.fd.is_none() { theme.yellow } else { theme.foreground }),
                    (e.fd.map(|fd| fd.to_string()).unwrap_or_default(), theme.secondary()),
                    (e.kind.to_string(), theme.secondary()),
                    (e.path.clone(), theme.foreground),
                ]
            })
            .collect();
        let (_, first) = self.table(cx, theme, pen, &[("Observed", 84.0, true, true), ("Event", 250.0, false, false), ("FD", 50.0, true, true), ("Kind", 64.0, false, false), ("Path", 0.0, false, true)], &rows);
        for (index, event) in timeline.iter().enumerate() {
            self.time_hits.push((event.time_ms, Rect { pos: dvec2(pen.x, first + index as f64 * pen.line - 4.0), size: dvec2(pen.width, pen.line) }));
        }
        pen.y += 6.0;
        self.note(cx, theme, pen, "Click an event to move the history cursor there.");
        true
    }
}
