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

    if !Cx::has_studio_web_socket() {
        println!("{}:{}:{} - {}", file_name, line_start + 1, column_start + 1, message);
    }
    else{
       Cx::send_studio_message(AppToStudio::LogItem(StudioLogItem{
            file_name: file_name.to_string(),
            line_start,
            column_start,
            line_end,
            column_end,
            message,
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
