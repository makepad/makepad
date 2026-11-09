//! Errors and warning diagnostics.
//!
//! Translated from `src/engine/engine_util_errmem.c`, `engine_core_util.c` (mj_warning)
//! and `engine_util_misc.c` (mju_warningText and byte formatting) of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust; panic and stderr replace error and warning handlers.

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

// The default configuration has no enabled topics. There is no setter yet.
// Topic zero is unfiltered, as in mju_isTopicEnabled.
pub fn is_topic_enabled(topic: i32) -> bool {
    topic == 0
}

std::thread_local! {
    static IN_LOG: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// Default console handler and dispatch. Configuration, file logging, timestamp
// formatting and custom handlers are outside this default-only logging port.
pub fn message(msg: &LogMessage<'_>) {
    IN_LOG.with(|in_log| {
        if in_log.get() {
            return;
        }
        // Error handlers do not return; do not leave the guard set after a panic.
        if msg.level != LOG_ERROR {
            in_log.set(true);
        }
        if !((msg.level == LOG_INFO || msg.level == LOG_DEBUG) && !is_topic_enabled(msg.topic)) {
            use std::io::Write;
            let type_ = if msg.level == LOG_ERROR {
                "ERROR"
            } else if msg.level == LOG_WARNING {
                "WARNING"
            } else if msg.level == LOG_INFO {
                "INFO"
            } else {
                "DEBUG"
            };
            let mut text = type_.as_bytes().to_vec();
            if let Some(func) = msg.func {
                text.extend_from_slice(format!(" {func}").as_bytes());
            }
            if let Some(file) = msg.file.filter(|_| msg.line != 0) {
                let basename = file.rsplit(['/', '\\']).next().unwrap_or(file);
                text.extend_from_slice(format!(" ({basename}:{})", msg.line).as_bytes());
            }
            text.extend_from_slice(b": ");
            let len = msg
                .subject
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(msg.subject.len());
            text.extend_from_slice(&msg.subject[..len]);
            text.push(b'\n');
            if let Some(body) = msg.body {
                text.extend_from_slice(body.as_bytes());
                text.push(b'\n');
            }
            if msg.level != LOG_DEBUG {
                text.push(b'\n');
            }
            if msg.level >= LOG_WARNING {
                let _ = std::io::stderr().write_all(&text);
            } else {
                let _ = std::io::stdout().write_all(&text);
            }
            if msg.level == LOG_ERROR {
                let subject = String::from_utf8_lossy(&msg.subject[..len]);
                panic!("{subject}");
            }
        }
        if msg.level != LOG_ERROR {
            in_log.set(false);
        }
    });
}

pub fn error(msg: &str) -> ! {
    panic!("{msg}");
}

pub fn warning_text(msg: &str) {
    eprintln!("{msg}");
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
