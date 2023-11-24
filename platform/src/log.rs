//use crate::web_socket::WebSocket;
//use crate::studio::*;




// lets make a global websocket with a mutex
//pub(crate) fn init_websocket(){
    // lets spawn a sender thread
    
//}

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

pub enum LogType {
    Error,
    Log,
    Panic
}

pub fn log_with_type(file:&str, line_start:u32, column_start:u32, _line_end:u32, _column_end:u32, message:&str, _ty:LogType){
    // lets send out our log message on the studio websocket 
    
    /*if std::env::args().find(|v| v == "--message-format=json").is_some(){
        let out = ty.make_json(file, line_start, column_start, line_end, column_end, message);
        println!("{}", out);
        return
    }*/
    println!("{}:{}:{} - {}", file, line_start, column_start, message);
}
// alright let log