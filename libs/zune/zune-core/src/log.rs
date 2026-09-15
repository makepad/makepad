/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

// #[macro_export] is required to make macros works across crates
// but it always put the macro in the crate root.
// #[doc(hidden)] + "pub use" is a workaround to namespace a macro.
pub use crate::{
    __debug as debug, __error as error, __info as info, __log_enabled as log_enabled,
    __trace as trace, __warn as warn,
};

#[repr(usize)]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum Level {
    Error = 1,
    Warn,
    Info,
    Debug,
    Trace,
}

#[doc(hidden)]
pub const ENABLED: bool = cfg!(feature = "log");

// Format here, where alloc is available, rather than requiring every no_std
// decoder module using these macros to import alloc::format. The sink is the
// existing Makepad logger; this module only keeps the decoder macro interface.
#[cfg(feature = "log")]
#[doc(hidden)]
pub fn write(
    level: Level,
    file: &str,
    line: u32,
    column: u32,
    args: core::fmt::Arguments<'_>,
) {
    use makepad_error_log::LogLevel;
    let level = match level {
        Level::Error => LogLevel::Error,
        Level::Warn => LogLevel::Warning,
        Level::Info | Level::Debug | Level::Trace => LogLevel::Log,
    };
    makepad_error_log::log_with_level(
        file, line - 1, column - 1, line - 1, column + 3,
        alloc::fmt::format(args), level,
    );
}

#[cfg(feature = "log")]
#[doc(hidden)]
#[macro_export]
macro_rules! __zune_log {
    ($level:ident, $($arg:tt)+) => {{
        $crate::log::write(
            $crate::log::Level::$level,
            file!(), line!(), column!(), format_args!($($arg)+),
        );
    }};
}

#[cfg(not(feature = "log"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __zune_log {
    ($level:ident, $($arg:tt)+) => {{}};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log_enabled {
    ($lvl:expr) => {{
        let _ = $lvl;
        $crate::log::ENABLED
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __error {
    ($($arg:tt)+) => { $crate::__zune_log!(Error, $($arg)+) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __warn {
    ($($arg:tt)+) => { $crate::__zune_log!(Warn, $($arg)+) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __info {
    ($($arg:tt)+) => { $crate::__zune_log!(Info, $($arg)+) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __debug {
    ($($arg:tt)+) => { $crate::__zune_log!(Debug, $($arg)+) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __trace {
    ($($arg:tt)+) => { $crate::__zune_log!(Trace, $($arg)+) };
}
