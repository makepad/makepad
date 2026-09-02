use makepad_micro_serde::*;
use std::fmt::Write;
use std::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
struct TraceTopic {
    name: String,
    enabled: bool,
}

#[derive(Debug)]
struct TopicSet {
    topics: Vec<TraceTopic>,
    spec: String,
}

impl TopicSet {
    const fn empty() -> Self {
        Self {
            topics: Vec::new(),
            spec: String::new(),
        }
    }

    fn parse(spec: &str) -> Self {
        let mut topics: Vec<TraceTopic> = Vec::new();
        for raw in spec.split(',') {
            let raw = raw.trim();
            let (enabled, name) = match raw.strip_prefix('-') {
                Some(name) => (false, name),
                None => (true, raw),
            };
            if !valid_topic(name) {
                continue;
            }
            if let Some(existing) = topics.iter_mut().find(|topic| topic.name == name) {
                existing.enabled = enabled;
            } else {
                topics.push(TraceTopic {
                    name: name.to_string(),
                    enabled,
                });
            }
        }
        let spec = topics
            .iter()
            .map(|topic| {
                if topic.enabled {
                    topic.name.clone()
                } else {
                    format!("-{}", topic.name)
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        Self { topics, spec }
    }

    fn enabled(&self, topic: &str) -> bool {
        if !valid_topic(topic) {
            return false;
        }
        if self
            .topics
            .iter()
            .any(|entry| !entry.enabled && topic_matches(&entry.name, topic))
        {
            return false;
        }
        self.topics
            .iter()
            .any(|entry| entry.enabled && topic_matches(&entry.name, topic))
    }
}

fn valid_topic(topic: &str) -> bool {
    !topic.is_empty()
        && topic
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'.')
}

fn topic_matches(configured: &str, topic: &str) -> bool {
    configured == "all"
        || configured == topic
        || topic
            .strip_prefix(configured)
            .is_some_and(|suffix| suffix.starts_with('.'))
}

static TRACE_TOPICS: RwLock<TopicSet> = RwLock::new(TopicSet::empty());
/// True while at least one topic is enabled: the per-frame fast path never
/// takes the lock in the common case of no tracing at all.
static TRACE_ANY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Returns whether `topic` is enabled by the process-wide trace topic set.
#[inline]
pub fn trace_enabled(topic: &str) -> bool {
    if !TRACE_ANY.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    TRACE_TOPICS
        .read()
        .expect("Trace topic lock poisoned")
        .enabled(topic)
}

/// Replaces the process-wide trace topic set with a comma-separated spec.
pub fn set_trace_topics(spec: &str) {
    let set = TopicSet::parse(spec);
    let any = set.topics.iter().any(|topic| topic.enabled);
    *TRACE_TOPICS.write().expect("Trace topic lock poisoned") = set;
    TRACE_ANY.store(any, std::sync::atomic::Ordering::Relaxed);
}

/// Returns the normalized process-wide trace topic spec.
pub fn trace_topics() -> String {
    TRACE_TOPICS
        .read()
        .expect("Trace topic lock poisoned")
        .spec
        .clone()
}

/// An enabled trace span. Dropping it logs elapsed wall-clock time.
pub struct TraceSpan {
    topic: String,
    #[cfg(not(target_arch = "wasm32"))]
    started: Instant,
}

impl Drop for TraceSpan {
    fn drop(&mut self) {
        log!(
            "[{}] elapsed {:.3} ms",
            self.topic,
            self.elapsed_ms()
        );
    }
}

/// Starts an elapsed-time span when `topic` is enabled. Never on the web:
/// `Instant` is unimplemented on wasm32 and panics, so a span there is
/// simply absent rather than fatal.
pub fn trace_span(topic: &str) -> Option<TraceSpan> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    trace_enabled(topic).then(|| TraceSpan {
        topic: topic.to_string(),
        #[cfg(not(target_arch = "wasm32"))]
        started: Instant::now(),
    })
}

impl TraceSpan {
    #[cfg(not(target_arch = "wasm32"))]
    fn elapsed_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    #[cfg(target_arch = "wasm32")]
    fn elapsed_ms(&self) -> f64 {
        0.0
    }
}

#[macro_export]
macro_rules!log {
    ( $ ( $ t: tt) *) => {
        $crate::log_with_level(
            file!(),
            line!()-1,
            column!()-1,
            line!()-1,
            column!() + 3,
            format!( $ ( $ t) *),
            $ crate::LogLevel::Log
        )
    }
}

#[macro_export]
macro_rules!error {
    ( $ ( $ t: tt) *) => {
        $crate::log_with_level(
            file!(),
            line!()-1,
            column!()-1,
            line!()-1,
            column!() + 3,
            format!( $ ( $ t) *),
            $crate::LogLevel::Error
        )
    }
}

#[macro_export]
macro_rules!warning {
    ( $ ( $ t: tt) *) => {
        $crate::log_with_level(
            file!(),
            line!()-1,
            column!()-1,
            line!()-1,
            column!() + 3,
            format!( $ ( $ t) *),
            $ crate::LogLevel::Warning
        )
    }
}

#[macro_export]
macro_rules!warn {
    ( $ ( $ t: tt) *) => {
        $crate::log_with_level(
            file!(),
            line!()-1,
            column!()-1,
            line!()-1,
            column!() + 3,
            format!( $ ( $ t) *),
            $ crate::LogLevel::Warning
        )
    }
}

#[macro_export]
macro_rules!info {
    ( $ ( $ t: tt) *) => {
        $crate::log_with_level(
            file!(),
            line!()-1,
            column!()-1,
            line!()-1,
            column!() + 3,
            format!( $ ( $ t) *),
            $ crate::LogLevel::Log
        )
    }
}

#[macro_export]
macro_rules!debug {
    ( $ ( $ t: tt) *) => {
        $crate::log_with_level(
            file!(),
            line!()-1,
            column!()-1,
            line!()-1,
            column!() + 3,
            format!( $ ( $ t) *),
            $ crate::LogLevel::Log
        )
    }
}

#[macro_export]
macro_rules!trace {
    ($topic:expr, $fmt:literal $($arg:tt)*) => {{
        let topic_value = $topic;
        let topic: &str = ::std::convert::AsRef::<str>::as_ref(&topic_value);
        if $crate::trace_enabled(topic) {
            $crate::log!("[{}] {}", topic, format_args!($fmt $($arg)*));
        }
    }};
    // Compatibility for crates using the former unconditional trace logger.
    ($($t:tt)*) => {
        $crate::log_with_level(
            file!(),
            line!()-1,
            column!()-1,
            line!()-1,
            column!() + 3,
            format!($($t)*),
            $crate::LogLevel::Log
        )
    };
}

fn log_with_level_rustc(
    file_name: &str,
    line_start: u32,
    column_start: u32,
    line_end: u32,
    column_end: u32,
    message: String,
    level: LogLevel,
) {
    println!(
        "{}",
        level.make_rustc_json(
            file_name,
            line_start,
            column_start,
            line_end,
            column_end,
            &message
        )
    );
}

pub static LOG_WITH_LEVEL: RwLock<fn(&str, u32, u32, u32, u32, String, LogLevel)> =
    RwLock::new(log_with_level_rustc);

/// An OBSERVER of everything logged, installed alongside (never instead of)
/// the logger.
///
/// It exists so a process can COLLECT what it already reports rather than
/// growing a second reporting path: the draw-shader compiler, the script
/// evaluator and every other subsystem already say what went wrong through
/// `error!`, and a tap is how a livecoding loop turns that into an answer
/// for whoever just saved the file. Off by default and free when off.
///
/// A tap must never log: it runs inside the logging call.
static LOG_TAP: RwLock<Option<fn(&str, LogLevel)>> = RwLock::new(None);
static LOG_TAP_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Install (or with `None`, remove) the log tap. One per process.
pub fn set_log_tap(tap: Option<fn(&str, LogLevel)>) {
    let mut slot = LOG_TAP.write().expect("Log tap lock poisoned");
    *slot = tap;
    LOG_TAP_ON.store(tap.is_some(), std::sync::atomic::Ordering::Release);
}

pub fn log_with_level(
    file_name: &str,
    line_start: u32,
    column_start: u32,
    line_end: u32,
    column_end: u32,
    message: String,
    level: LogLevel,
) {
    if LOG_TAP_ON.load(std::sync::atomic::Ordering::Acquire) {
        if let Ok(tap) = LOG_TAP.read() {
            if let Some(tap) = *tap {
                tap(&message, level);
            }
        }
    }
    let logger = LOG_WITH_LEVEL.read().expect("Logger lock poisoned");
    logger(
        file_name,
        line_start,
        column_start,
        line_end,
        column_end,
        message,
        level,
    );
}

#[derive(Clone, PartialEq, Eq, Copy, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum LogLevel {
    Warning,
    Error,
    Log,
    Wait,
    Panic,
}

impl LogLevel {
    pub fn make_rustc_json(
        &self,
        file: &str,
        line_start: u32,
        column_start: u32,
        line_end: u32,
        column_end: u32,
        message: &str,
    ) -> String {
        let mut out = String::new();
        let _ = write!(out, "{{\"reason\":\"makepad-error-log\",");
        let _ = write!(out, "\"message\":{{\"message\":\"");
        for c in message.chars() {
            match c {
                '\n' => {
                    out.push('\\');
                    out.push('n');
                }
                '\r' => {
                    out.push('\\');
                    out.push('r');
                }
                '\t' => {
                    out.push('\\');
                    out.push('t');
                }
                '\0' => {
                    out.push('\\');
                    out.push('0');
                }
                '\\' => {
                    out.push('\\');
                    out.push('\\');
                }
                '"' => {
                    out.push('\\');
                    out.push('"');
                }
                _ => out.push(c),
            }
        }
        let _ = write!(out, "\",");
        let _ = match self {
            LogLevel::Error => write!(out, "\"level\":\"error\","),
            LogLevel::Log => write!(out, "\"level\":\"log\","),
            LogLevel::Panic => write!(out, "\"level\":\"panic\","),
            LogLevel::Warning => write!(out, "\"level\":\"warning\","),
            LogLevel::Wait => write!(out, "\"level\":\"wait\","),
        };
        let _ = write!(out, "\"spans\":[{{");
        let _ = write!(out, "\"file_name\":\"{}\",", file);
        let _ = write!(out, "\"byte_start\":0,");
        let _ = write!(out, "\"byte_end\":0,");
        let _ = write!(out, "\"line_start\":{},", line_start + 1);
        let _ = write!(out, "\"line_end\":{},", line_end + 1);
        let _ = write!(out, "\"column_start\":{},", column_start);
        let _ = write!(out, "\"column_end\":{},", column_end);
        let _ = write!(out, "\"is_primary\":true,");
        let _ = write!(out, "\"text\":[]");
        let _ = write!(out, "}}],");
        let _ = write!(out, "\"children\":[]");
        let _ = write!(out, "}}");
        let _ = write!(out, "}}");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_matcher_hierarchy_all_and_negation() {
        let topics = TopicSet::parse("gpu,all,-gpu.profile");
        assert!(topics.enabled("gpu"));
        assert!(topics.enabled("gpu.pass"));
        assert!(topics.enabled("frame"));
        assert!(!topics.enabled("gpu.profile"));
        assert!(!topics.enabled("gpu.profile.detail"));
        assert!(!TopicSet::parse("gpu").enabled("gpuish"));
    }

    #[test]
    fn set_clear_and_get_topics() {
        set_trace_topics(" gpu.pass, invalid!, -gpu.profile ");
        assert_eq!(trace_topics(), "gpu.pass,-gpu.profile");
        assert!(trace_enabled("gpu.pass.detail"));
        assert!(!trace_enabled("gpu.profile"));

        set_trace_topics("");
        assert_eq!(trace_topics(), "");
        assert!(!trace_enabled("gpu.pass"));
    }
}
