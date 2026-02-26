use crate::protocol::{
    EventSample, GCSample, GPUSample, LogEntry, LogLevel, LogSource, QueryId,
};
use makepad_live_id::LiveId;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default)]
pub struct LogQuery {
    pub build_id: Option<QueryId>,
    pub level: Option<String>,
    pub source: Option<LogSource>,
    pub file: Option<String>,
    pub pattern: Option<String>,
    pub since_index: Option<usize>,
}

impl LogQuery {
    pub fn matches(&self, entry: &LogEntry) -> bool {
        if let Some(build_id) = self.build_id {
            if entry.build_id != Some(build_id) {
                return false;
            }
        }
        if let Some(source) = self.source {
            if entry.source != source {
                return false;
            }
        }
        if let Some(since) = self.since_index {
            if entry.index < since {
                return false;
            }
        }
        if let Some(file) = &self.file {
            if entry.file_name.as_deref() != Some(file.as_str()) {
                return false;
            }
        }
        if let Some(pattern) = &self.pattern {
            if !entry.message.contains(pattern) {
                return false;
            }
        }
        if let Some(level) = &self.level {
            if !matches_level(&entry.level, level) {
                return false;
            }
        }
        true
    }
}

fn matches_level(level: &LogLevel, query: &str) -> bool {
    match query {
        "error" | "Error" => matches!(level, LogLevel::Error),
        "warning" | "Warning" | "warn" | "Warn" => matches!(level, LogLevel::Warning),
        "log" | "Log" | "info" | "Info" => matches!(level, LogLevel::Log),
        _ => false,
    }
}

pub struct AppendLogEntry {
    pub build_id: Option<QueryId>,
    pub level: LogLevel,
    pub source: LogSource,
    pub message: String,
    pub file_name: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub timestamp: Option<f64>,
}

#[derive(Default)]
pub struct LogStore {
    entries: Vec<LogEntry>,
}

impl LogStore {
    pub fn append(&mut self, entry: AppendLogEntry) -> (usize, LogEntry) {
        let index = self.entries.len();
        let entry = LogEntry {
            index,
            timestamp: entry.timestamp.unwrap_or_else(now_seconds),
            build_id: entry.build_id,
            level: entry.level,
            source: entry.source,
            message: entry.message,
            file_name: entry.file_name,
            line: entry.line,
            column: entry.column,
        };
        self.entries.push(entry.clone());
        (index, entry)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn query(&self, query: &LogQuery) -> Vec<(usize, LogEntry)> {
        self.entries
            .iter()
            .filter(|entry| query.matches(entry))
            .map(|entry| (entry.index, entry.clone()))
            .collect()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProfilerQuery {
    pub build_id: Option<QueryId>,
    pub sample_type: Option<LiveId>,
    pub time_start: Option<f64>,
    pub time_end: Option<f64>,
    pub max_samples: Option<usize>,
}

#[derive(Default)]
pub struct ProfilerStore {
    event_samples: Vec<(Option<QueryId>, EventSample)>,
    gpu_samples: Vec<(Option<QueryId>, GPUSample)>,
    gc_samples: Vec<(Option<QueryId>, GCSample)>,
}

impl ProfilerStore {
    pub fn append_event(&mut self, build_id: Option<QueryId>, sample: EventSample) {
        self.event_samples.push((build_id, sample));
    }

    pub fn append_gpu(&mut self, build_id: Option<QueryId>, sample: GPUSample) {
        self.gpu_samples.push((build_id, sample));
    }

    pub fn append_gc(&mut self, build_id: Option<QueryId>, sample: GCSample) {
        self.gc_samples.push((build_id, sample));
    }

    pub fn query(&self, query: &ProfilerQuery) -> (Vec<EventSample>, Vec<GPUSample>, Vec<GCSample>, usize) {
        let event_selected: Vec<EventSample> = self
            .event_samples
            .iter()
            .filter(|(build_id, sample)| query_profiler_sample(query, *build_id, sample.at, SAMPLE_TYPE_EVENT))
            .map(|(_, sample)| sample.clone())
            .collect();
        let gpu_selected: Vec<GPUSample> = self
            .gpu_samples
            .iter()
            .filter(|(build_id, sample)| query_profiler_sample(query, *build_id, sample.at, SAMPLE_TYPE_GPU))
            .map(|(_, sample)| sample.clone())
            .collect();
        let gc_selected: Vec<GCSample> = self
            .gc_samples
            .iter()
            .filter(|(build_id, sample)| query_profiler_sample(query, *build_id, sample.at, SAMPLE_TYPE_GC))
            .map(|(_, sample)| sample.clone())
            .collect();

        let total = event_selected.len() + gpu_selected.len() + gc_selected.len();
        let max = query.max_samples.unwrap_or(1024);
        (
            downsample(&event_selected, max),
            downsample(&gpu_selected, max),
            downsample(&gc_selected, max),
            total,
        )
    }
}

fn query_profiler_sample(
    query: &ProfilerQuery,
    build_id: Option<QueryId>,
    at: f64,
    sample_type: LiveId,
) -> bool {
    if let Some(query_build_id) = query.build_id {
        if build_id != Some(query_build_id) {
            return false;
        }
    }
    if let Some(query_sample_type) = query.sample_type {
        if query_sample_type != sample_type {
            return false;
        }
    }
    if let Some(start) = query.time_start {
        if at < start {
            return false;
        }
    }
    if let Some(end) = query.time_end {
        if at > end {
            return false;
        }
    }
    true
}

fn downsample<T: Clone>(items: &[T], max: usize) -> Vec<T> {
    if max == 0 || items.is_empty() {
        return Vec::new();
    }
    if items.len() <= max {
        return items.to_vec();
    }
    if max == 1 {
        return vec![items[0].clone()];
    }

    let mut out = Vec::with_capacity(max);
    let n = items.len() - 1;
    let m = max - 1;
    for i in 0..max {
        let idx = i * n / m;
        out.push(items[idx].clone());
    }
    out
}

pub const SAMPLE_TYPE_EVENT: LiveId = LiveId::from_str("event");
pub const SAMPLE_TYPE_GPU: LiveId = LiveId::from_str("gpu");
pub const SAMPLE_TYPE_GC: LiveId = LiveId::from_str("gc");

pub fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs_f64())
        .unwrap_or(0.0)
}
