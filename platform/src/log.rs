use crate::makepad_micro_serde::*;

#[macro_export]
macro_rules!log {
    ( $ ( $ t: tt) *) => {
        $crate::log::log_with_type(file!(), line!(), column!()+1, line!(), column!() + 4, &format!( $ ( $ t) *), $ crate::log::LogType::Log)
    }
}

#[macro_export]
macro_rules!error {
    ( $ ( $ t: tt) *) => {
        $crate::log::log_with_type(file!(), line!(), column!()+1, line!(), column!() + 4, &format!( $ ( $ t) *), $ crate::log::LogType::Error)
    }
}

#[derive(SerBin, DeBin)]
pub enum LogType {
    Error,
    Log,
    Panic
}

use crate::cx::Cx;
use crate::studio::AppToStudio;

pub fn log_with_type(file:&str, line_start:u32, column_start:u32, line_end:u32, column_end:u32, message:&str, ty:LogType){
    // lets send out our log message on the studio websocket 
    
    /*if std::env::args().find(|v| v == "--message-format=json").is_some(){
        let out = ty.make_json(file, line_start, column_start, line_end, column_end, message);
        println!("{}", out);
        return
    }*/
    Cx::send_studio_message(AppToStudio::Log{
        file: file.to_string(),
        line_start,
        column_start,
        line_end,
        column_end,
        message:message.to_string(),
        ty
    });
        println!("{}:{}:{} - {}", file, line_start, column_start, message);
    }
// alright let log