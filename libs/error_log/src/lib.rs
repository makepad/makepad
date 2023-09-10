use std::fmt::Write;

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub mod error_log_desktop;

#[macro_export]
macro_rules!log {
    ( $ ( $ t: tt) *) => {
        $crate::makepad_error_log::log_with_type(file!(), line!(), column!(), line!(), column!() + 4, &format!( $ ( $ t) *), $ crate::makepad_error_log::LogType::Log)
    }
}

#[macro_export]
macro_rules!error {
    ( $ ( $ t: tt) *) => {
        $crate::makepad_error_log::log_with_type(file!(), line!(), column!(), line!(), column!() + 4, &format!( $ ( $ t) *), $ crate::makepad_error_log::LogType::Log)
    }
}

pub enum LogType {
    Error,
    Log,
    Panic
}

impl LogType {
    pub fn make_json(&self, file: &str, line_start: u32, column_start: u32, line_end: u32, column_end: u32, message: &str) -> String {
        let mut out = String::new();
        let _ = write!(out, "{{\"reason\":\"makepad-error-log\",");
        let _ = write!(out, "\"message\":{{\"message\":\"");
        for c in message.chars() {
            match c {
                '\n' => {out.push('\\'); out.push('n');},
                '\r' => {out.push('\\'); out.push('r');},
                '\t' => {out.push('\\'); out.push('t');},
                '\0' => {out.push('\\'); out.push('0');},
                '\\' => {out.push('\\'); out.push('\\');},
                '"' => {out.push('\\'); out.push('"');},
                _ => out.push(c)
            }
        }
        let _ = write!(out, "\",");
        let _ = match self {
            LogType::Error => write!(out, "\"level\":\"error\","),
            LogType::Log => write!(out, "\"level\":\"log\","),
            LogType::Panic => write!(out, "\"level\":\"panic\","),
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

#[cfg(target_os = "android")]
#[macro_use]
pub mod error_log_android;

#[cfg(target_arch = "wasm32")]
#[macro_use]
pub mod error_log_wasm;

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub use error_log_desktop::*;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub use error_log_desktop as makepad_error_log;

#[cfg(target_os = "android")]
pub use error_log_android::*;
#[cfg(target_os = "android")]
pub use error_log_android as makepad_error_log;

#[cfg(target_arch = "wasm32")]
pub use error_log_wasm::*;
#[cfg(target_arch = "wasm32")]
pub use error_log_wasm as makepad_error_log;

use std::time::Instant;

pub fn profile_start() -> Instant {
    Instant::now()
}

#[macro_export]
macro_rules!profile_end {
    ( $ inst: expr) => {
        $crate::makepad_error_log::log_with_type(
            file!(),
            line!(),
            column!(),
            line!(),
            column!() + 4,
            &format!("Profile time {} ms", ( $ inst.elapsed().as_nanos() as f64) / 1000000f64),
            $crate::makepad_error_log::LogType::Log
        )
    }
}

