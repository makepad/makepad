use makepad_micro_serde::*;
use std::fmt::Write;
use std::sync::RwLock;

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

pub fn log_with_level(
    file_name: &str,
    line_start: u32,
    column_start: u32,
    line_end: u32,
    column_end: u32,
    message: String,
    level: LogLevel,
) {
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

#[derive(Clone, PartialEq, Eq, Copy, Debug, SerBin, DeBin)]
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
