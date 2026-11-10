//! Errors and warning diagnostics.
//!
//! Translated from `src/engine/engine_util_errmem.c`, `engine_core_util.c` (mj_warning)
//! and `engine_util_misc.c` (mju_warningText and byte formatting) of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust; a panic replaces the error exit, and one global handler
//! (`set_log_handler`) replaces the console, file and legacy user handlers.

use crate::data::Data;
use crate::types::*;

// mjLogMessage fields, including the optional text and source metadata from mjtype.h.
pub struct LogMessage<'a> {
    pub level: i32,
    pub topic: i32,
    pub subject: [u8; 1024],
    pub body: Option<&'a str>,
    pub func: Option<&'a str>,
    pub file: Option<&'a str>,
    pub line: i32,
    pub timestamp: bool,
}

impl Default for LogMessage<'_> {
    fn default() -> Self {
        Self {
            level: 0,
            topic: 0,
            subject: [0; 1024],
            body: None,
            func: None,
            file: None,
            line: 0,
            timestamp: false,
        }
    }
}

impl LogMessage<'_> {
    /// The subject up to its terminating zero.
    pub fn subject_text(&self) -> std::borrow::Cow<'_, str> {
        let len = self.subject.iter().position(|&b| b == 0).unwrap_or(self.subject.len());
        String::from_utf8_lossy(&self.subject[..len])
    }

    /// The message as the console handler prints it, without its closing newline:
    /// `WARNING func (file:line): subject`, then the body on a line of its own.
    pub fn text(&self) -> String {
        let type_ = if self.level == LOG_ERROR {
            "ERROR"
        } else if self.level == LOG_WARNING {
            "WARNING"
        } else if self.level == LOG_INFO {
            "INFO"
        } else {
            "DEBUG"
        };
        let mut text = type_.to_string();
        if let Some(func) = self.func {
            text.push_str(&format!(" {func}"));
        }
        if let Some(file) = self.file.filter(|_| self.line != 0) {
            let basename = file.rsplit(['/', '\\']).next().unwrap_or(file);
            text.push_str(&format!(" ({basename}:{})", self.line));
        }
        text.push_str(": ");
        text.push_str(&self.subject_text());
        if let Some(body) = self.body {
            text.push('\n');
            text.push_str(body);
        }
        text
    }
}

// The default configuration has no enabled topics. There is no setter yet.
// Topic zero is unfiltered, as in mju_isTopicEnabled.
pub fn is_topic_enabled(topic: i32) -> bool {
    topic == 0
}

/// mjfLogHandler: gets every message `message` dispatches. An error message panics once its
/// handler returns.
pub type LogHandler = fn(&LogMessage<'_>);

// global log handler
static LOG_HANDLER: std::sync::RwLock<LogHandler> = std::sync::RwLock::new(default_log_handler);

/// mju_setLogHandler: set the active log handler (None: the default console handler); returns
/// the previous one.
pub fn set_log_handler(handler: Option<LogHandler>) -> LogHandler {
    std::mem::replace(&mut *LOG_HANDLER.write().unwrap(), handler.unwrap_or(default_log_handler))
}

// default log handler: topic filtering and console output. File logging and its
// configuration are outside this port.
fn default_log_handler(msg: &LogMessage<'_>) {
    if (msg.level == LOG_INFO || msg.level == LOG_DEBUG) && !is_topic_enabled(msg.topic) {
        return;
    }
    use std::io::Write;
    let mut text = msg.text().into_bytes();
    text.push(b'\n');
    // add blank line after message except for DEBUG, for compactness
    if msg.level != LOG_DEBUG {
        text.push(b'\n');
    }
    if msg.level >= LOG_WARNING {
        let _ = std::io::stderr().write_all(&text);
    } else {
        let _ = std::io::stdout().write_all(&text);
    }
}

std::thread_local! {
    static IN_LOG: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// dispatch a structured log message to the active handler
pub fn message(msg: &LogMessage<'_>) {
    IN_LOG.with(|in_log| {
        // recursion guard: silently drop messages dispatched from within a handler
        if in_log.get() {
            return;
        }
        let handler = *LOG_HANDLER.read().unwrap();
        if msg.level != LOG_ERROR {
            in_log.set(true);
            handler(msg);
            in_log.set(false);
        } else {
            // Error handlers do not return: one that does returns into the panic.
            handler(msg);
            panic!("{}", msg.subject_text());
        }
    });
}

pub fn error(msg: &str) -> ! {
    panic!("{msg}");
}

// mju_warning: a warning message to the active handler
pub fn warning_text(msg: &str) {
    let mut m = LogMessage { level: LOG_WARNING, ..Default::default() };
    crate::util_misc::strncpy(&mut m.subject, msg.as_bytes(), 1024);
    message(&m);
}

pub fn warning(d: &mut Data, warning: i32, info: i32) {
    if warning < 0 || warning >= NWARNING {
        error(&format!("invalid warning type {warning}"));
    }
    d.warning[warning as usize].lastinfo = info;
    if d.warning[warning as usize].number == 0 {
        warning_text(&format!(
            "{} Time = {:.4}.",
            warning_text_of(warning, info as usize),
            d.time
        ));
    }
    d.warning[warning as usize].number += 1;
}

pub fn warning_text_of(warning: i32, info: usize) -> String {
    match warning {
        WARN_INERTIA => format!("Inertia matrix is too close to singular, or the effective-metric solve failed to converge, at DOF {info}. Check model."),
        WARN_CONTACTFULL => format!("Too many contacts. The arena memory is full, increase arena memory allocation.(ncon = {info})"),
        WARN_CNSTRFULL => format!(
            "Insufficient arena memory for the number of constraints generated. Increase arena memory allocation above {} bytes.",
            crate::util_misc::write_num_bytes(info)
        ),
        WARN_BADQPOS => format!("Nan, Inf or huge value in QPOS at DOF {info}. The simulation is unstable."),
        WARN_BADQVEL => format!("Nan, Inf or huge value in QVEL at DOF {info}. The simulation is unstable."),
        WARN_BADQACC => format!("Nan, Inf or huge value in QACC at DOF {info}. The simulation is unstable."),
        WARN_BADCTRL => format!("Nan, Inf or huge value in CTRL at ACTUATOR {info}. The simulation is unstable."),
        _ => format!("Unknown warning type {warning}."),
    }
}
