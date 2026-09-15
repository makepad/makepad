//! Recurrence expansion, overlap lanes, timeline geometry, drafts, search.

use crate::model::*;
use makepad_civil_time::{self as civil, Day};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OccurrenceList {
    pub items: Vec<Occurrence>,
    pub truncated: bool,
}

impl OccurrenceList {
    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            truncated: false,
        }
    }
}

fn duration_days_span(timing: Timing) -> i32 {
    match timing {
        Timing::AllDay {
            start,
            end_exclusive,
        } => end_exclusive - start,
        Timing::Timed { start, end } => {
            let last = if end.minute == 0 {
                end.day
            } else {
                end.day + 1
            };
            (last - start.day).max(1)
        }
    }
}

fn occurrence_at(event: &CalendarEvent, start_day: Day) -> Option<Occurrence> {
    let timing = match event.timing {
        Timing::Timed { start, end } => {
            let dur = end.abs_minutes() - start.abs_minutes();
            let s = LocalMinute {
                day: start_day,
                minute: start.minute,
            };
            let e = s.add_minutes(dur as i32)?;
            Timing::Timed { start: s, end: e }
        }
        Timing::AllDay {
            start,
            end_exclusive,
        } => {
            let dur = end_exclusive - start;
            Timing::AllDay {
                start: start_day,
                end_exclusive: start_day + dur,
            }
        }
    };
    Some(Occurrence {
        key: OccurrenceKey {
            event_id: event.id,
            start_day,
        },
        timing,
    })
}

fn monthly_start(anchor: Day, n: i32) -> Day {
    civil::add_months(anchor, n)
}

fn yearly_start(anchor: Day, n: i32) -> Day {
    civil::add_months(anchor, n * 12)
}

fn first_n_for_step(anchor: Day, step_days: i32, range_start: Day, span_days: i32) -> i32 {
    // First n ≥ 0 whose occurrence `[start, start+span)` intersects `[range_start, …)`.
    // Intersects when start < range_end (handled by caller) and start + span > range_start
    // i.e. start > range_start - span.
    let earliest = range_start - (span_days - 1).max(0);
    if earliest <= anchor {
        0
    } else {
        let delta = earliest - anchor;
        (delta + step_days - 1) / step_days
    }
}

fn first_n_monthly(anchor: Day, range_start: Day, span_days: i32) -> i32 {
    let earliest = range_start - (span_days - 1).max(0);
    if earliest <= anchor {
        return 0;
    }
    let (ay, am, _) = civil::to_ymd(anchor);
    let (ey, em, _) = civil::to_ymd(earliest);
    let n = (ey * 12 + em as i32) - (ay * 12 + am as i32);
    n.max(0).saturating_sub(1)
}

/// Expand masters that intersect `[range_start, range_end)` (half-open days).
/// Range length must be ≤ 366 days. `limit` caps the result; `truncated` is
/// set when more would have been returned. Seeking a decades-old series is
/// O(range), not O(age).
pub fn expand_range(
    events: &[CalendarEvent],
    range_start: Day,
    range_end: Day,
    limit: usize,
) -> Result<OccurrenceList, String> {
    expand_filtered(events, range_start, range_end, limit, |_| true, |_| true)
}

/// Keep only the earliest qualifying occurrences, regardless of master order.
/// Each series is bounded by the query span; the retained collection by `limit`.
fn expand_filtered(
    events: &[CalendarEvent],
    range_start: Day,
    range_end: Day,
    limit: usize,
    include_event: impl Fn(&CalendarEvent) -> bool,
    include_occurrence: impl Fn(&Occurrence) -> bool,
) -> Result<OccurrenceList, String> {
    if range_end < range_start {
        return Err("range is inverted".into());
    }
    if range_end - range_start > MAX_QUERY_DAYS {
        return Err("range exceeds 366 days".into());
    }
    if range_start == range_end {
        return Ok(OccurrenceList::empty());
    }
    let cap = limit.min(UI_EXPAND_CAP);
    let mut retained = std::collections::BTreeMap::new();
    let mut truncated = false;
    let mut series = Vec::new();
    for event in events.iter().filter(|e| include_event(e)) {
        series.clear();
        truncated |= expand_event(event, range_start, range_end, UI_EXPAND_CAP, &mut series);
        for occurrence in series.iter().filter(|o| include_occurrence(o)) {
            retained.insert(occurrence_sort_key(occurrence, &event.title, event.id), *occurrence);
            if retained.len() > cap {
                retained.pop_last();
                truncated = true;
            }
        }
    }
    Ok(OccurrenceList { items: retained.into_values().collect(), truncated })
}

fn expand_event(
    event: &CalendarEvent,
    range_start: Day,
    range_end: Day,
    cap: usize,
    out: &mut Vec<Occurrence>,
) -> bool {
    let span = duration_days_span(event.timing).max(1);
    let anchor = event.timing.start_day();
    match event.repeat {
        Repeat::None => {
            if let Some(occ) = occurrence_at(event, anchor) {
                if occ.timing.intersects_range(range_start, range_end) {
                    if out.len() >= cap {
                        return true;
                    }
                    out.push(occ);
                }
            }
            false
        }
        Repeat::Daily => expand_step(event, range_start, range_end, cap, out, 1, span),
        Repeat::Weekly => expand_step(event, range_start, range_end, cap, out, 7, span),
        Repeat::Monthly => expand_monthly(event, range_start, range_end, cap, out, span, false),
        Repeat::Yearly => expand_monthly(event, range_start, range_end, cap, out, span, true),
    }
}

fn expand_step(
    event: &CalendarEvent,
    range_start: Day,
    range_end: Day,
    cap: usize,
    out: &mut Vec<Occurrence>,
    step: i32,
    span: i32,
) -> bool {
    let anchor = event.timing.start_day();
    let mut n = first_n_for_step(anchor, step, range_start, span);
    for _ in 0..(MAX_QUERY_DAYS + span + 2) {
        let start = civil::add_days(anchor, n * step);
        if start >= range_end {
            break;
        }
        if let Some(occ) = occurrence_at(event, start) {
            if occ.timing.intersects_range(range_start, range_end) {
                if out.len() >= cap {
                    return true;
                }
                out.push(occ);
            }
        }
        n += 1;
    }
    false
}

fn expand_monthly(
    event: &CalendarEvent,
    range_start: Day,
    range_end: Day,
    cap: usize,
    out: &mut Vec<Occurrence>,
    span: i32,
    yearly: bool,
) -> bool {
    let anchor = event.timing.start_day();
    let mut n = first_n_monthly(anchor, range_start, span);
    if yearly {
        n = (n / 12) * 12;
        if n < 0 {
            n = 0;
        }
    }
    let step = if yearly { 12 } else { 1 };
    for _ in 0..(MAX_QUERY_DAYS / 28 + 4) {
        let start = monthly_start(anchor, n);
        n += step;
        if start >= range_end && start > range_start + span {
            if start >= range_end {
                break;
            }
        }
        if let Some(occ) = occurrence_at(event, start) {
            if occ.timing.intersects_range(range_start, range_end) {
                if out.len() >= cap {
                    return true;
                }
                out.push(occ);
            }
        }
        if start >= range_end {
            break;
        }
    }
    let _ = yearly_start;
    false
}

pub fn sort_occurrences(doc: &CalendarDocument, list: &mut OccurrenceList) {
    list.items.sort_by(|a, b| {
        let ta = event_by_id(doc, a.key.event_id)
            .map(|e| e.title.as_str())
            .unwrap_or("");
        let tb = event_by_id(doc, b.key.event_id)
            .map(|e| e.title.as_str())
            .unwrap_or("");
        occurrence_sort_key(a, ta, a.key.event_id).cmp(&occurrence_sort_key(b, tb, b.key.event_id))
    });
}

pub fn occurrences_for_day(
    doc: &CalendarDocument,
    day: Day,
    visible_only: bool,
    limit: usize,
) -> OccurrenceList {
    occurrences_in_range(doc, day, day + 1, visible_only, limit)
}

pub fn occurrences_in_range(
    doc: &CalendarDocument,
    start: Day,
    end: Day,
    visible_only: bool,
    limit: usize,
) -> OccurrenceList {
    qualifying_occurrences(doc, start, end, visible_only, limit, |_| true)
}

pub fn qualifying_occurrences(
    doc: &CalendarDocument,
    start: Day,
    end: Day,
    visible_only: bool,
    limit: usize,
    include: impl Fn(&Occurrence) -> bool,
) -> OccurrenceList {
    expand_filtered(&doc.events, start, end, limit, |e| {
        !visible_only || calendar_by_id(doc, e.calendar_id).is_some_and(|c| c.visible)
    }, include).unwrap_or_else(|_| OccurrenceList::empty())
}

/// Search title, notes, and calendar name case-insensitively over
/// `[today−90, today+276)`. Hidden calendars are omitted from UI results.
pub fn search(
    doc: &CalendarDocument,
    query: &str,
    today: Day,
    visible_only: bool,
) -> OccurrenceList {
    let q = query.trim();
    if q.is_empty() {
        return OccurrenceList::empty();
    }
    let needle = q.to_lowercase();
    let start = today - SEARCH_BACK_DAYS;
    let end = today + SEARCH_FORWARD_DAYS;
    expand_filtered(&doc.events, start, end, SEARCH_LIMIT, |event| {
        let cal = calendar_by_id(doc, event.calendar_id);
        if visible_only && !cal.is_some_and(|c| c.visible) {
            return false;
        }
        event.title.to_lowercase().contains(&needle)
            || event.notes.to_lowercase().contains(&needle)
            || cal.is_some_and(|c| c.name.to_lowercase().contains(&needle))
    }, |_| true).unwrap_or_else(|_| OccurrenceList::empty())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneAssignment {
    pub index: usize,
    pub lane: usize,
    pub lanes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneInterval {
    pub index: usize,
    pub start: i64,
    pub end: i64,
    pub event_id: EventId,
}

/// Partition connected overlap groups, then greedy-assign the lowest free
/// lane. Intervals that only touch at an endpoint may share a lane.
pub fn assign_lanes(mut intervals: Vec<LaneInterval>) -> Vec<LaneAssignment> {
    if intervals.is_empty() {
        return Vec::new();
    }
    intervals.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then(b.end.cmp(&a.end))
            .then(a.event_id.cmp(&b.event_id))
            .then(a.index.cmp(&b.index))
    });
    let n = intervals.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while p[i] != i {
            let n = p[p[i]];
            p[i] = n;
            i = p[i];
        }
        i
    }
    fn union(p: &mut [usize], a: usize, b: usize) {
        let pa = find(p, a);
        let pb = find(p, b);
        if pa != pb {
            p[pb] = pa;
        }
    }
    for i in 0..n {
        for j in i + 1..n {
            if intervals[j].start >= intervals[i].end {
                break;
            }
            if intervals[i].start < intervals[j].end && intervals[j].start < intervals[i].end {
                union(&mut parent, i, j);
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut map = std::collections::BTreeMap::<usize, usize>::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        let g = *map.entry(r).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(i);
    }
    let mut out = vec![
        LaneAssignment {
            index: 0,
            lane: 0,
            lanes: 1,
        };
        n
    ];
    for group in groups {
        let mut members: Vec<usize> = group;
        members.sort_by(|&a, &b| {
            intervals[a]
                .start
                .cmp(&intervals[b].start)
                .then(intervals[b].end.cmp(&intervals[a].end))
                .then(intervals[a].event_id.cmp(&intervals[b].event_id))
                .then(intervals[a].index.cmp(&intervals[b].index))
        });
        let mut lane_end: Vec<i64> = Vec::new();
        let mut assigned: Vec<(usize, usize)> = Vec::new();
        for &i in &members {
            let mut lane = None;
            for (l, end) in lane_end.iter().enumerate() {
                if *end <= intervals[i].start {
                    lane = Some(l);
                    break;
                }
            }
            let lane = match lane {
                Some(l) => {
                    lane_end[l] = intervals[i].end;
                    l
                }
                None => {
                    lane_end.push(intervals[i].end);
                    lane_end.len() - 1
                }
            };
            assigned.push((i, lane));
        }
        let mut max_sim = 1usize;
        let mut points: Vec<(i64, i32)> = Vec::new();
        for &i in &members {
            points.push((intervals[i].start, 1));
            points.push((intervals[i].end, -1));
        }
        points.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut cur = 0i32;
        for (_, d) in points {
            cur += d;
            max_sim = max_sim.max(cur as usize);
        }
        let lanes = lane_end.len().max(max_sim).max(1);
        for (i, lane) in assigned {
            out[i] = LaneAssignment {
                index: intervals[i].index,
                lane,
                lanes,
            };
        }
    }
    out.sort_by_key(|a| a.index);
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedRect {
    pub key: OccurrenceKey,
    pub day_index: usize,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub lane: usize,
    pub lanes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AllDayRect {
    pub key: OccurrenceKey,
    pub day_index: usize,
    pub day_span: usize,
    pub lane: usize,
    pub lanes: usize,
}

pub const HOUR_HEIGHT: f64 = 64.0;
pub const MIN_BLOCK_HEIGHT: f64 = 18.0;
pub const LANE_GAP: f64 = 3.0;
pub const EVENT_INSET: f64 = 4.0;
pub const TIMELINE_HOURS: f64 = 24.0;
pub const TIMELINE_CONTENT_HEIGHT: f64 = HOUR_HEIGHT * TIMELINE_HOURS;

fn clip_timed_to_day(timing: Timing, day: Day) -> Option<(u16, u16)> {
    match timing {
        Timing::AllDay { .. } => None,
        Timing::Timed { start, end } => {
            let ds = day as i64 * MINUTES_PER_DAY as i64;
            let de = ds + MINUTES_PER_DAY as i64;
            let a = start.abs_minutes().max(ds);
            let b = end.abs_minutes().min(de);
            if b <= a {
                return None;
            }
            Some(((a - ds) as u16, (b - ds) as u16))
        }
    }
}

pub fn timed_rects_for_days(
    occs: &[Occurrence],
    days: &[Day],
    content_width: f64,
    gutter: f64,
) -> Vec<TimedRect> {
    let n = days.len().max(1);
    let col_w = ((content_width - gutter) / n as f64).max(1.0);
    let mut rects = Vec::new();
    for (day_index, day) in days.iter().copied().enumerate() {
        let mut intervals = Vec::new();
        let mut keys = Vec::new();
        for occ in occs {
            if let Some((s, e)) = clip_timed_to_day(occ.timing, day) {
                intervals.push(LaneInterval {
                    index: keys.len(),
                    start: s as i64,
                    end: e as i64,
                    event_id: occ.key.event_id,
                });
                keys.push((occ.key, s, e));
            }
        }
        let lanes = assign_lanes(intervals);
        for asg in lanes {
            let (key, s, e) = keys[asg.index];
            let lane_w =
                (col_w - EVENT_INSET * 2.0 - LANE_GAP * asg.lanes.saturating_sub(1) as f64)
                    / asg.lanes.max(1) as f64;
            let x = gutter
                + col_w * day_index as f64
                + EVENT_INSET
                + (lane_w + LANE_GAP) * asg.lane as f64;
            let y = s as f64 / 60.0 * HOUR_HEIGHT;
            let h = ((e as f64 - s as f64) / 60.0 * HOUR_HEIGHT).max(MIN_BLOCK_HEIGHT);
            rects.push(TimedRect {
                key,
                day_index,
                x,
                y,
                w: lane_w.max(1.0),
                h,
                lane: asg.lane,
                lanes: asg.lanes,
            });
        }
    }
    rects
}

pub fn all_day_lanes_for_days(occs: &[Occurrence], days: &[Day]) -> Vec<AllDayRect> {
    if days.is_empty() {
        return Vec::new();
    }
    let first = days[0];
    let last = *days.last().unwrap();
    let mut intervals = Vec::new();
    let mut meta = Vec::new();
    for occ in occs {
        let Timing::AllDay {
            start,
            end_exclusive,
        } = occ.timing
        else {
            continue;
        };
        let s = start.max(first);
        let e = end_exclusive.min(last + 1);
        if e <= s {
            continue;
        }
        let day_index = (s - first) as usize;
        let span = (e - s) as usize;
        intervals.push(LaneInterval {
            index: meta.len(),
            start: s as i64,
            end: e as i64,
            event_id: occ.key.event_id,
        });
        meta.push((occ.key, day_index, span));
    }
    let lanes = assign_lanes(intervals);
    lanes
        .into_iter()
        .map(|asg| {
            let (key, day_index, day_span) = meta[asg.index];
            AllDayRect {
                key,
                day_index,
                day_span,
                lane: asg.lane,
                lanes: asg.lanes,
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HitResult {
    Event(OccurrenceKey),
    Ambiguous(Day),
    Empty,
}

/// Same geometry as drawing. Crowded short blocks whose 44-point hit
/// regions overlap resolve as an agenda of the intersecting events.
pub fn hit_timed(rects: &[TimedRect], days: &[Day], x: f64, y: f64, min_hit: f64) -> HitResult {
    let mut hits: Vec<(OccurrenceKey, Day, f64)> = Vec::new();
    for r in rects {
        let h = r.h.max(min_hit);
        let w = r.w.max(min_hit);
        let left = r.x - (w - r.w) * 0.5;
        let top = r.y - (h - r.h) * 0.5;
        if x >= left && x < left + w && y >= top && y < top + h {
            let day = days.get(r.day_index).copied().unwrap_or(0);
            let area = w * h;
            hits.push((r.key, day, area));
        }
    }
    match hits.len() {
        0 => HitResult::Empty,
        1 => HitResult::Event(hits[0].0),
        _ => HitResult::Ambiguous(hits[0].1),
    }
}

pub fn initial_scroll_for_day(clock: ClockSnapshot, day: Day, viewport_h: f64) -> f64 {
    let minute = if day == clock.today {
        clock.minute
    } else {
        8 * 60
    };
    let y = minute as f64 / 60.0 * HOUR_HEIGHT;
    let target = if day == clock.today {
        (y - viewport_h / 3.0).max(0.0)
    } else {
        y
    };
    target.min((TIMELINE_CONTENT_HEIGHT - viewport_h).max(0.0))
}

#[derive(Clone, Debug)]
pub struct DraftFields {
    pub title: String,
    pub calendar_index: usize,
    pub all_day: bool,
    pub start_date: String,
    pub start_time: String,
    pub end_date: String,
    pub end_time: String,
    pub repeat_index: usize,
    pub notes: String,
}

impl DraftFields {
    pub fn set_all_day(&mut self, all_day: bool) {
        if all_day && !self.all_day {
            if let (Some(start), Some(end)) = (
                LocalMinute::parse_iso(&format!("{}T{}", self.start_date, self.start_time)),
                LocalMinute::parse_iso(&format!("{}T{}", self.end_date, self.end_time)),
            ) {
                let timing = Timing::Timed { start, end };
                if validate_timing(timing).is_ok() {
                    if let Some(Timing::AllDay { end_exclusive, .. }) = timing.to_all_day() {
                        self.end_date = civil::format_iso(end_exclusive - 1);
                    }
                }
            }
        }
        self.all_day = all_day;
    }
    pub fn from_event(event: &CalendarEvent) -> Self {
        let (all_day, start_date, start_time, end_date, end_time) = match event.timing {
            Timing::Timed { start, end } => (
                false,
                civil::format_iso(start.day),
                start.format_hm(),
                civil::format_iso(end.day),
                end.format_hm(),
            ),
            Timing::AllDay {
                start,
                end_exclusive,
            } => (
                true,
                civil::format_iso(start),
                "09:00".into(),
                civil::format_iso(end_exclusive - 1),
                "10:00".into(),
            ),
        };
        Self {
            title: event.title.clone(),
            calendar_index: (event.calendar_id.0.saturating_sub(1) as usize).min(3),
            all_day,
            start_date,
            start_time,
            end_date,
            end_time,
            repeat_index: match event.repeat {
                Repeat::None => 0,
                Repeat::Daily => 1,
                Repeat::Weekly => 2,
                Repeat::Monthly => 3,
                Repeat::Yearly => 4,
            },
            notes: event.notes.clone(),
        }
    }
}

pub fn default_new_event(
    clock: ClockSnapshot,
    selected: Day,
    default_cal: CalendarId,
    timeline_minute: Option<u16>,
) -> CalendarEvent {
    let start = if let Some(m) = timeline_minute {
        LocalMinute {
            day: selected,
            minute: snap_minute_15(m),
        }
    } else if selected == clock.today {
        clock.next_half_hour()
    } else {
        LocalMinute {
            day: selected,
            minute: 9 * 60,
        }
    };
    let end = start.add_minutes(60).unwrap_or(start);
    CalendarEvent {
        id: EventId(0),
        calendar_id: default_cal,
        title: String::new(),
        timing: Timing::Timed { start, end },
        repeat: Repeat::None,
        notes: String::new(),
    }
}

pub fn apply_draft_fields(
    draft: &mut EventDraft,
    fields: &DraftFields,
    calendars: &[Calendar],
) -> DraftErrors {
    let mut errors = DraftErrors::default();
    match validate_title(&fields.title) {
        Ok(t) => draft.value.title = t,
        Err(e) => {
            errors.title = Some(e);
            draft.value.title = fields.title.clone();
        }
    }
    match validate_notes(&fields.notes) {
        Ok(n) => draft.value.notes = n,
        Err(e) => {
            errors.title = errors.title.or(Some(e));
            draft.value.notes = fields.notes.clone();
        }
    }
    let cal = calendars
        .get(fields.calendar_index)
        .map(|c| c.id)
        .or_else(|| calendars.first().map(|c| c.id));
    match cal {
        Some(id) => draft.value.calendar_id = id,
        None => errors.calendar = Some("Unknown calendar".into()),
    }
    draft.value.repeat = match fields.repeat_index {
        1 => Repeat::Daily,
        2 => Repeat::Weekly,
        3 => Repeat::Monthly,
        4 => Repeat::Yearly,
        _ => Repeat::None,
    };

    let start_day = match civil::parse_iso(&fields.start_date) {
        Some(d) => d,
        None => {
            errors.start = Some("Start date is invalid".into());
            draft.value.timing.start_day()
        }
    };
    let end_day_raw = match civil::parse_iso(&fields.end_date) {
        Some(d) => d,
        None => {
            errors.end = Some("End date is invalid".into());
            start_day
        }
    };

    if fields.all_day {
        let end_exclusive = end_day_raw + 1;
        let timing = Timing::AllDay {
            start: start_day,
            end_exclusive,
        };
        if let Err(e) = validate_timing(timing) {
            errors.end = Some(e);
        }
        draft.value.timing = timing;
    } else {
        let start_hm = match parse_hm(&fields.start_time) {
            Some(hm) => hm,
            None => {
                errors.start = Some(
                    errors
                        .start
                        .unwrap_or_else(|| "Start time is invalid".into()),
                );
                (9, 0)
            }
        };
        let end_hm = match parse_hm(&fields.end_time) {
            Some(hm) => hm,
            None => {
                errors.end = Some(errors.end.unwrap_or_else(|| "End time is invalid".into()));
                (10, 0)
            }
        };
        let start =
            LocalMinute::from_hm(start_day, start_hm.0, start_hm.1).unwrap_or(LocalMinute {
                day: start_day,
                minute: 9 * 60,
            });
        let mut end =
            LocalMinute::from_hm(end_day_raw, end_hm.0, end_hm.1).unwrap_or(LocalMinute {
                day: end_day_raw,
                minute: 10 * 60,
            });
        if !draft.end_was_edited {
            if let Some(orig) = draft.original.as_ref() {
                if let Some(dur) = orig.timing.duration_minutes() {
                    if let Some(kept) = start.add_minutes(dur) {
                        end = kept;
                    }
                }
            } else if let Some(kept) = start.add_minutes(60) {
                end = kept;
            }
        }
        let timing = Timing::Timed { start, end };
        if let Err(e) = validate_timing(timing) {
            errors.end = Some(e);
        }
        draft.value.timing = timing;
    }
    errors
}

pub fn draft_is_dirty(draft: &EventDraft) -> bool {
    match &draft.original {
        Some(orig) => {
            draft.value.title != orig.title
                || draft.value.notes != orig.notes
                || draft.value.calendar_id != orig.calendar_id
                || draft.value.timing != orig.timing
                || draft.value.repeat != orig.repeat
        }
        None => {
            !draft.value.title.trim().is_empty()
                || !draft.value.notes.is_empty()
                || draft.value.repeat != Repeat::None
        }
    }
}

pub fn commit_draft(doc: &mut CalendarDocument, draft: &EventDraft) -> Result<EventId, String> {
    if doc.events.len() >= MAX_MASTERS && draft.editing.is_none() {
        return Err("too many events".into());
    }
    validate_title(&draft.value.title)?;
    validate_notes(&draft.value.notes)?;
    validate_timing(draft.value.timing)?;
    if calendar_by_id(doc, draft.value.calendar_id).is_none() {
        return Err("unknown calendar".into());
    }
    if let Some(id) = draft.editing {
        let Some(slot) = doc.events.iter_mut().find(|e| e.id == id) else {
            return Err("event is gone".into());
        };
        slot.title = draft.value.title.clone();
        slot.notes = draft.value.notes.clone();
        slot.calendar_id = draft.value.calendar_id;
        slot.timing = draft.value.timing;
        slot.repeat = draft.value.repeat;
        doc.revision = doc.revision.saturating_add(1);
        Ok(id)
    } else {
        let id = EventId(doc.next_event_id);
        doc.next_event_id = doc.next_event_id.saturating_add(1);
        let mut value = draft.value.clone();
        value.id = id;
        doc.events.push(value);
        doc.revision = doc.revision.saturating_add(1);
        Ok(id)
    }
}

pub fn delete_event(doc: &mut CalendarDocument, id: EventId) -> bool {
    let before = doc.events.len();
    doc.events.retain(|e| e.id != id);
    if doc.events.len() != before {
        doc.revision = doc.revision.saturating_add(1);
        true
    } else {
        false
    }
}

pub fn set_calendar_visible(doc: &mut CalendarDocument, id: CalendarId, visible: bool) -> bool {
    if let Some(cal) = doc.calendars.iter_mut().find(|c| c.id == id) {
        if cal.visible != visible {
            cal.visible = visible;
            doc.revision = doc.revision.saturating_add(1);
            return true;
        }
    }
    false
}

pub fn visible_calendar_count(doc: &CalendarDocument) -> usize {
    doc.calendars.iter().filter(|c| c.visible).count()
}

/// Month-grid chips: up to `slots` events, last slot reserved for `+N more`.
pub fn day_chips<'a>(occs: &'a [Occurrence], slots: usize) -> (Vec<&'a Occurrence>, usize) {
    if occs.len() <= slots {
        return (occs.iter().collect(), 0);
    }
    let shown = slots.saturating_sub(1);
    (occs.iter().take(shown).collect(), occs.len() - shown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::seed;

    fn timed(day: Day, sh: u32, sm: u32, eh: u32, em: u32) -> Timing {
        Timing::Timed {
            start: LocalMinute::from_hm(day, sh, sm).unwrap(),
            end: LocalMinute::from_hm(day, eh, em).unwrap(),
        }
    }

    fn ev(id: u32, timing: Timing, repeat: Repeat) -> CalendarEvent {
        CalendarEvent {
            id: EventId(id),
            calendar_id: CAL_WORK,
            title: format!("E{id}"),
            timing,
            repeat,
            notes: String::new(),
        }
    }

    #[test]
    fn daily_weekly_monthly_yearly_and_month_end_recovery() {
        let jan31 = civil::from_ymd(2024, 1, 31);
        let monthly = ev(1, timed(jan31, 15, 0, 16, 0), Repeat::Monthly);
        let list = expand_range(
            &[monthly.clone()],
            civil::from_ymd(2024, 1, 1),
            civil::from_ymd(2024, 4, 1),
            20,
        )
        .unwrap();
        let starts: Vec<_> = list
            .items
            .iter()
            .map(|o| civil::to_ymd(o.timing.start_day()))
            .collect();
        assert!(starts.contains(&(2024, 1, 31)));
        assert!(starts.contains(&(2024, 2, 29)));
        assert!(starts.contains(&(2024, 3, 31)));

        let feb29 = civil::from_ymd(2024, 2, 29);
        let yearly = ev(
            2,
            Timing::AllDay {
                start: feb29,
                end_exclusive: feb29 + 1,
            },
            Repeat::Yearly,
        );
        let y2024 = expand_range(
            &[yearly.clone()],
            civil::from_ymd(2024, 2, 1),
            civil::from_ymd(2024, 3, 1),
            8,
        )
        .unwrap();
        assert_eq!(
            civil::to_ymd(y2024.items[0].timing.start_day()),
            (2024, 2, 29)
        );
        let y2025 = expand_range(
            &[yearly.clone()],
            civil::from_ymd(2025, 2, 1),
            civil::from_ymd(2025, 3, 1),
            8,
        )
        .unwrap();
        assert_eq!(
            civil::to_ymd(y2025.items[0].timing.start_day()),
            (2025, 2, 28)
        );
        let y2028 = expand_range(
            &[yearly],
            civil::from_ymd(2028, 2, 1),
            civil::from_ymd(2028, 3, 1),
            8,
        )
        .unwrap();
        assert_eq!(
            civil::to_ymd(y2028.items[0].timing.start_day()),
            (2028, 2, 29)
        );

        let monday = civil::from_ymd(2026, 6, 1);
        let weekly = ev(3, timed(monday, 9, 30, 9, 45), Repeat::Weekly);
        let list = expand_range(&[weekly], monday, monday + 21, 10).unwrap();
        assert_eq!(list.items.len(), 3);
        assert_eq!(list.items[1].timing.start_day(), monday + 7);

        let daily = ev(4, timed(monday, 8, 0, 9, 0), Repeat::Daily);
        let list = expand_range(&[daily], monday, monday + 5, 10).unwrap();
        assert_eq!(list.items.len(), 5);
    }

    #[test]
    fn seeking_a_decades_old_series_stays_bounded() {
        let origin = civil::from_ymd(1970, 1, 5); // a Monday
        let weekly = ev(1, timed(origin, 9, 30, 9, 45), Repeat::Weekly);
        let start = civil::from_ymd(2026, 9, 7);
        let list = expand_range(&[weekly], start, start + 7, 20).unwrap();
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].timing.start_day(), start);
        assert!(!list.truncated);
    }

    #[test]
    fn midnight_overnight_and_multiday_intersections() {
        let d = civil::from_ymd(2026, 9, 9);
        let midnight = ev(
            1,
            Timing::Timed {
                start: LocalMinute::from_hm(d, 22, 0).unwrap(),
                end: LocalMinute::from_hm(d + 1, 0, 0).unwrap(),
            },
            Repeat::None,
        );
        assert!(midnight.timing.intersects_day(d));
        assert!(!midnight.timing.intersects_day(d + 1));

        let overnight = ev(
            2,
            Timing::Timed {
                start: LocalMinute::from_hm(d, 22, 0).unwrap(),
                end: LocalMinute::from_hm(d + 1, 1, 0).unwrap(),
            },
            Repeat::None,
        );
        assert!(overnight.timing.intersects_day(d));
        assert!(overnight.timing.intersects_day(d + 1));
        assert!(!overnight.timing.intersects_day(d + 2));

        let multi = ev(
            3,
            Timing::AllDay {
                start: d,
                end_exclusive: d + 3,
            },
            Repeat::None,
        );
        assert!(multi.timing.intersects_day(d));
        assert!(multi.timing.intersects_day(d + 2));
        assert!(!multi.timing.intersects_day(d + 3));
        let list = expand_range(&[multi], d + 1, d + 2, 10).unwrap();
        assert_eq!(list.items.len(), 1);
        let _ = midnight;
    }

    #[test]
    fn overlap_lanes_cover_the_fixture_shapes() {
        let mk = |i, s, e| LaneInterval {
            index: i,
            start: s,
            end: e,
            event_id: EventId(i as u32 + 1),
        };
        // Equal starts.
        let a = assign_lanes(vec![mk(0, 0, 60), mk(1, 0, 30)]);
        assert_eq!(a[0].lanes, 2);
        assert_ne!(a[0].lane, a[1].lane);
        // Nesting.
        let a = assign_lanes(vec![mk(0, 0, 120), mk(1, 30, 60)]);
        assert_eq!(a[0].lanes, 2);
        // Touching endpoints share a lane.
        let a = assign_lanes(vec![mk(0, 0, 60), mk(1, 60, 120)]);
        assert_eq!(a[0].lane, a[1].lane);
        assert_eq!(a[0].lanes, 1);
        // Transitive group: A overlaps B, B overlaps C.
        let a = assign_lanes(vec![mk(0, 0, 50), mk(1, 40, 90), mk(2, 80, 130)]);
        assert!(a.iter().all(|x| x.lanes >= 2));
        // Permutation-stable: reverse input order, same lanes by index.
        let fwd = assign_lanes(vec![mk(0, 0, 60), mk(1, 0, 30), mk(2, 90, 120)]);
        let rev = assign_lanes(vec![mk(2, 90, 120), mk(1, 0, 30), mk(0, 0, 60)]);
        assert_eq!(fwd, rev);
    }

    #[test]
    fn timeline_rects_and_hit_resolution_at_target_sizes() {
        let d = civil::from_ymd(2026, 9, 9);
        let occs = vec![
            Occurrence {
                key: OccurrenceKey {
                    event_id: EventId(1),
                    start_day: d,
                },
                timing: timed(d, 9, 0, 10, 0),
            },
            Occurrence {
                key: OccurrenceKey {
                    event_id: EventId(2),
                    start_day: d,
                },
                timing: timed(d, 9, 30, 10, 30),
            },
        ];
        // Wide week: 1019 content, 56 gutter, 7 days.
        let days: Vec<Day> = (0..7).map(|i| d - civil::weekday(d) as i32 + i).collect();
        let week = timed_rects_for_days(&occs, &days, 1019.0, 56.0);
        assert!(week.iter().any(|r| r.h >= MIN_BLOCK_HEIGHT));
        // Compact day 402: gutter 52, right inset already out of width.
        let day_rects = timed_rects_for_days(&occs, &[d], 402.0 - 12.0, 52.0);
        assert_eq!(day_rects.len(), 2);
        assert!(day_rects[0].lanes >= 2 || day_rects[1].lanes >= 2);
        // Short landscape week.
        let short = timed_rects_for_days(&occs, &days, 874.0, 56.0);
        assert!(!short.is_empty());
        let hit = hit_timed(
            &day_rects,
            &[d],
            day_rects[0].x + 1.0,
            day_rects[0].y + 1.0,
            44.0,
        );
        assert!(matches!(hit, HitResult::Event(_) | HitResult::Ambiguous(_)));
        assert_eq!(
            hit_timed(&day_rects, &[d], 0.0, 0.0, 44.0),
            HitResult::Empty
        );
    }

    #[test]
    fn draft_validation_duration_all_day_cancel_and_series() {
        let d = civil::from_ymd(2026, 9, 9);
        let original = ev(7, timed(d, 9, 0, 10, 0), Repeat::Weekly);
        let mut draft = EventDraft {
            editing: Some(EventId(7)),
            value: original.clone(),
            original: Some(original.clone()),
            end_was_edited: false,
        };
        let cals = vec![Calendar {
            id: CAL_WORK,
            name: "Work".into(),
            colour: CalendarColour::Work,
            visible: true,
        }];
        let mut fields = DraftFields::from_event(&original);
        fields.start_date = "2026-09-10".into();
        fields.start_time = "11:00".into();
        let errors = apply_draft_fields(&mut draft, &fields, &cals);
        assert!(errors.is_empty());
        match draft.value.timing {
            Timing::Timed { start, end } => {
                assert_eq!(start.format_hm(), "11:00");
                assert_eq!(end.format_hm(), "12:00");
            }
            _ => panic!("expected timed"),
        }
        fields.all_day = true;
        fields.end_date = "2026-09-10".into();
        apply_draft_fields(&mut draft, &fields, &cals);
        match draft.value.timing {
            Timing::AllDay {
                start,
                end_exclusive,
            } => {
                assert_eq!(start, civil::from_ymd(2026, 9, 10));
                assert_eq!(end_exclusive, civil::from_ymd(2026, 9, 11));
            }
            _ => panic!("expected all-day"),
        }
        fields.title = String::new();
        let errors = apply_draft_fields(&mut draft, &fields, &cals);
        assert!(errors.title.is_some());
        // Cancel restores original.
        draft.value = draft.original.clone().unwrap();
        assert_eq!(draft.value, original);
        assert_eq!(original.repeat, Repeat::Weekly);
        let mut doc = CalendarDocument {
            schema_version: 1,
            revision: 1,
            seed_version: 1,
            seed_anchor: d,
            next_event_id: 8,
            calendars: cals.clone(),
            events: vec![original.clone()],
            preferences: Preferences {
                wide_mode: CalendarMode::Month,
                default_calendar: CAL_WORK,
            },
        };
        draft.value.title = "Series title".into();
        commit_draft(&mut doc, &draft).unwrap();
        assert_eq!(doc.events[0].title, "Series title");
        assert_eq!(doc.events[0].repeat, Repeat::Weekly);
        assert!(delete_event(&mut doc, EventId(7)));
        assert!(doc.events.is_empty());
    }

    #[test]
    fn search_bounds_and_hidden_calendars() {
        let doc = seed(civil::from_ymd(2026, 9, 9));
        let empty = search(&doc, "   ", doc.seed_anchor, true);
        assert!(empty.items.is_empty());
        let hits = search(&doc, "standup", doc.seed_anchor, true);
        assert!(!hits.items.is_empty());
        let mut hidden = doc.clone();
        set_calendar_visible(&mut hidden, CAL_WORK, false);
        let ui = search(&hidden, "standup", hidden.seed_anchor, true);
        assert!(ui.items.is_empty());
        let all = search(&hidden, "standup", hidden.seed_anchor, false);
        assert!(!all.items.is_empty());
    }

    #[test]
    fn range_cap_and_ui_expansion_cap() {
        let d = 0;
        let daily = ev(1, timed(d, 9, 0, 10, 0), Repeat::Daily);
        assert!(expand_range(&[daily.clone()], 0, 400, 10).is_err());
        let list = expand_range(&[daily], 0, 10, 3).unwrap();
        assert_eq!(list.items.len(), 3);
        assert!(list.truncated);
    }
    #[test]
    fn old_daily_and_weekly_series_fill_the_entire_requested_range() {
        let start = civil::from_ymd(2026, 9, 9);
        for (repeat, step) in [(Repeat::Daily, 1), (Repeat::Weekly, 7)] {
            let anchor = civil::from_ymd(2024, 1, 1);
            let event = ev(1, timed(anchor, 9, 0, 10, 0), repeat);
            let list = expand_range(&[event], start, start + 31, 100).unwrap();
            let expected: Vec<_> = (start..start+31).filter(|d| (d-anchor)%step == 0).collect();
            assert_eq!(list.items.iter().map(|o| o.timing.start_day()).collect::<Vec<_>>(), expected);
            assert!(!list.truncated);
        }
    }

    #[test]
    fn filtering_precedes_limits_and_keeps_earliest_matches() {
        let today = civil::from_ymd(2026, 9, 9);
        let mut doc = seed(today);
        doc.events = (1..=12).map(|id| ev(id, timed(today-90, 9, 0, 10, 0), Repeat::Daily)).collect();
        let mut found = ev(13, timed(today-90, 11, 0, 12, 0), Repeat::Daily);
        found.title = "Réunion 日本語".into();
        doc.events.push(found);
        let results = search(&doc, "réunion", today, true);
        assert_eq!(results.items.len(), SEARCH_LIMIT);
        assert!(results.items.iter().all(|o| o.key.event_id == EventId(13)));
        assert_eq!(results.items[0].key.start_day, today-90);
        assert_eq!(results.items[99].key.start_day, today+9);
        assert!(results.truncated);
        doc.events[12].repeat = Repeat::None;
        let results = search(&doc, "日本語", today, true);
        assert_eq!(results.items.len(), 1);
        assert!(!results.truncated);
        let mut reversed = vec![ev(1, timed(today+3, 9, 0, 10, 0), Repeat::None), ev(2, timed(today, 9, 0, 10, 0), Repeat::None)];
        let forward = expand_range(&reversed, today, today+5, 1).unwrap();
        reversed.reverse();
        assert_eq!(forward, expand_range(&reversed, today, today+5, 1).unwrap());
        assert_eq!(forward.items[0].key.start_day, today);
        assert!(forward.truncated);
    }

    #[test]
    fn upcoming_groups_dates_before_all_day_priority() {
        let today = civil::from_ymd(2026, 9, 9);
        let doc = seed(today);
        let list = occurrences_in_range(&doc, today, today+31, true, UI_EXPAND_CAP);
        assert!(list.items.windows(2).all(|w| w[0].timing.start_day() <= w[1].timing.start_day()));
        let timed = ev(1, timed(today, 9, 0, 10, 0), Repeat::None);
        let all_day = ev(2, Timing::AllDay {start:today, end_exclusive:today+1}, Repeat::None);
        let list = expand_range(&[timed, all_day], today, today+1, 10).unwrap();
        assert_eq!(list.items[0].key.event_id, EventId(2));
    }

    #[test]
    fn midnight_all_day_toggle_preserves_the_half_open_date_span() {
        let today = civil::from_ymd(2026, 9, 9);
        let doc = seed(today);
        for minute in [0, 1] {
            let event = ev(1, Timing::Timed { start: LocalMinute::from_hm(today,22,0).unwrap(), end:LocalMinute::new(today+1,minute).unwrap() }, Repeat::None);
            let mut fields = DraftFields::from_event(&event);
            fields.set_all_day(true);
            let mut draft = EventDraft { editing:Some(event.id), value:event.clone(), original:Some(event.clone()), end_was_edited:true };
            assert!(apply_draft_fields(&mut draft, &fields, &doc.calendars).is_empty());
            assert_eq!(draft.value.timing, event.timing.to_all_day().unwrap());
            assert_eq!(fields.end_date, civil::format_iso(today + i32::from(minute > 0)));
        }
    }

    #[test]
    fn draft_date_and_time_fields_reject_multibyte_input_without_panicking() {
        let doc = seed(civil::from_ymd(2026, 9, 9));
        for text in ["202é-09-9", "2026-😀-9", "日本語", "é9:00", "09:é"] {
            let event = doc.events[0].clone();
            let mut fields = DraftFields::from_event(&event);
            fields.all_day = false;
            fields.start_date = text.into();
            fields.end_date = text.into();
            fields.start_time = text.into();
            fields.end_time = text.into();
            let mut draft = EventDraft {editing:Some(event.id),value:event,original:None,end_was_edited:true};
            let errors = apply_draft_fields(&mut draft, &fields, &doc.calendars);
            assert!(errors.start.is_some() && errors.end.is_some());
        }
    }

}
