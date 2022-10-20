use std::panic;
use std::fmt::Write;

#[macro_export]  
macro_rules!log {
    ( $ ( $ t: tt) *) => {
        crate::makepad_error_log::log_impl(file!(), line!(), column!(), column!()+4, &format!( $ ( $ t) *), crate::makepad_error_log::LogType::Log)
    }
}

#[macro_export] 
macro_rules!error {
    ( $ ( $ t: tt) *) => {
        crate::makepad_error_log::log_impl(file!(), line!(), column!(), column!()+6, &format!( $ ( $ t) *), crate::makepad_error_log::LogType::Error)
    }  
}  

pub enum LogType{
    Error,
    Log,
    Panic
}

pub fn log_impl(file:&str, line:u32, column_start:u32, column_end:u32, message:&str, ty:LogType){
    for arg in std::env::args(){
        if arg == "--message-format=json"{
            let mut out = String::new();
            let _ = write!(out, "{{\"reason\":\"makepad-error-log\",");
            let _ = write!(out, "\"message\":{{\"message\":\"");
            for c in message.chars() {
                match c{
                    '\n'=>{out.push('\\');out.push('n');},
                    '\r'=>{out.push('\\');out.push('r');},
                    '\t'=>{out.push('\\');out.push('t');},
                    '\0'=>{out.push('\\');out.push('0');},
                    '\\'=>{out.push('\\');out.push('\\');},
                    '"'=>{out.push('\\');out.push('"');},
                    _=>out.push(c)
                }
            }
            let _ = write!(out, "\",");
            let _ = match ty{
                LogType::Error=> write!(out, "\"level\":\"error\","),
                LogType::Log=> write!(out, "\"level\":\"log\","),
                LogType::Panic=> write!(out, "\"level\":\"panic\","),
            };
            let _ = write!(out, "\"spans\":[{{");
            let _ = write!(out, "\"file_name\":\"{}\",", file);
            let _ = write!(out, "\"byte_start\":0,");
            let _ = write!(out, "\"byte_end\":0,");
            let _ = write!(out, "\"line_start\":{},", line);
            let _ = write!(out, "\"line_end\":{},", line);
            let _ = write!(out, "\"column_start\":{},", column_start);
            let _ = write!(out, "\"column_end\":{},", column_end);
            let _ = write!(out, "\"is_primary\":true,");
            let _ = write!(out, "\"text\":[]");
            let _ = write!(out, "}}],");
            let _ = write!(out, "\"children\":[]");
            let _ = write!(out, "}}");
            let _ = write!(out, "}}");
            println!("{}", out);
            return
        }
    }
    println!("{}:{}:{} - {}", file, line, column_start, message);
}

pub fn set_panic_hook(){
    pub fn panic_hook(info: &panic::PanicInfo) {
        if let Some(location) = info.location(){
            if let Some(s) = info.payload().downcast_ref::<&str>() {
                return log_impl(location.file(), location.line(), location.column(), location.column()+5, s, LogType::Panic);
            }
            else if let Some(s) = info.payload().downcast_ref::<String>() {
                return log_impl(location.file(), location.line(), location.column(), location.column()+5, s, LogType::Panic);
            }
        }
        eprintln!("{:?}", info);
    }
    panic::set_hook(Box::new(panic_hook));
}
