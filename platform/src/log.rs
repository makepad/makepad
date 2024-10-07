use crate::makepad_micro_serde::*;

#[macro_export]
macro_rules!log {
    ( $ ( $ t: tt) *) => {
        $crate::log::log_with_level(
            file!(), 
            line!()-1, 
            column!()-1, 
            line!()-1, 
            column!() + 3, 
            format!( $ ( $ t) *), 
            $ crate::log::LogLevel::Log
        )
    }
}

#[macro_export]
macro_rules!error {
    ( $ ( $ t: tt) *) => {
        $crate::log::log_with_level(
            file!(), 
            line!()-1, 
            column!()-1, 
            line!()-1, 
            column!() + 3, 
            format!( $ ( $ t) *), 
            $crate::log::LogLevel::Error
        )
    }
}

#[macro_export]
macro_rules! fmt_over {
    ($dst:expr, $($arg:tt)*) => {
        {
            $dst.clear();
            use std::fmt::Write;
            $dst.write_fmt(std::format_args!($($arg)*)).unwrap();
        }
    };
}

#[macro_export]
macro_rules! fmt_over_ref {
    ($dst:expr, $($arg:tt)*) => {
        {
            $dst.clear();
            use std::fmt::Write;
            $dst.write_fmt(std::format_args!($($arg)*)).unwrap();
            &$dst
        }
    };
}

#[macro_export]
macro_rules!warning {
    ( $ ( $ t: tt) *) => {
        $crate::log::log_with_level(
            file!(), 
            line!()-1, 
            column!()-1, 
            line!()-1, 
            column!() + 3, 
            format!( $ ( $ t) *), 
            $ crate::log::LogLevel::Warning
        )
    }
}


#[derive(Clone, PartialEq, Eq, Copy, Debug, SerBin, DeBin)]
pub enum LogLevel{
    Warning,
    Error,
    Log,
    Wait,
    Panic,
}

use crate::cx::Cx;
use crate::studio::{AppToStudio,StudioLogItem};

pub fn log_with_level(file_name:&str, line_start:u32, column_start:u32, line_end:u32, column_end:u32, message:String, level:LogLevel){
    // lets send out our log message on the studio websocket 
    #[cfg(target_arch = "wasm32")]{
        extern "C" {
            pub fn js_console_log(u8_ptr: u32, len: u32);
            pub fn js_console_error(u8_ptr: u32, len: u32);
        }
        let msg = format!("{}:{}:{} - {}", file_name, line_start, column_start, message);
        let buf = msg.as_bytes();
        if let LogLevel::Error = level{
            unsafe{js_console_error(buf.as_ptr() as u32, buf.len() as u32)};        
        }
        else{
            unsafe{js_console_log(buf.as_ptr() as u32, buf.len() as u32)};        
        }    
    }

    if !Cx::has_studio_web_socket() {
        #[cfg(not (target_os = "android"))]
        println!("{}:{}:{} - {}", file_name, line_start + 1, column_start + 1, message);
       // if android, also log to ADB
       #[cfg(target_os = "android")]
       {
           use std::ffi::c_int;
           extern "C" { 
               pub fn __android_log_write(prio: c_int, tag: *const u8, text: *const u8) -> c_int;
           }
           let msg = format!("{}:{}:{} - {}\0", file_name, line_start, column_start, message);
           unsafe{__android_log_write(3, "Makepad\0".as_ptr(), msg.as_ptr())};
       }
       #[cfg(target_env="ohos")]
       {
            let msg = format!("{}:{}:{} - {}\0", file_name, line_start, column_start, message);
            let hilevel:hilog_sys::LogLevel = match level {
                LogLevel::Warning => {hilog_sys::LogLevel::LOG_WARN}
                LogLevel::Error => {hilog_sys::LogLevel::LOG_ERROR}
                LogLevel::Log => {hilog_sys::LogLevel::LOG_INFO}
                _=> {hilog_sys::LogLevel::LOG_INFO}
            };
            unsafe {hilog_sys::OH_LOG_Print(hilog_sys::LogType::LOG_APP,hilevel, 0x03D00,c"makepad-ohos".as_ptr(), c"%{public}s".as_ptr(),msg.as_ptr())};
       }
    }
    else{
        Cx::send_studio_message(AppToStudio::LogItem(StudioLogItem{
            file_name: file_name.to_string(),
            line_start,
            column_start,
            line_end,
            column_end,
            message,
            explanation: None,
            level
        }));
    }
}


use std::time::Instant;

pub fn profile_start() -> Instant {
    Instant::now()
}

#[macro_export]
macro_rules!profile_end {
    ( $ inst: expr) => {
        $crate::log::log_with_level(
            file!(),
            line!(),
            column!(),
            line!(),
            column!() + 4,
            format!("Profile time {} ms", ( $ inst.elapsed().as_nanos() as f64) / 1000000f64),
            $crate::log::LogLevel::Log
        )
    }
}

#[macro_export]
macro_rules!profile_end_log {
    ( $inst:expr, $ ( $ t: tt) *) => {
        $crate::log::log_with_level(
            file!(),
            line!(),
            column!(),
            line!(),
            column!() + 4,
            format!("Profile time {} {}",( $ inst.elapsed().as_nanos() as f64) / 1000000f64, format!( $ ( $ t) *)), 
            $ crate::log::LogLevel::Log
        )
    }
}
