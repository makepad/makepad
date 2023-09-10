
pub use crate::LogType;


pub fn log_with_type(file:&str, line_start:u32, column_start:u32, line_end:u32, column_end:u32, message:&str, ty:LogType){
    if std::env::args().find(|v| v == "--message-format=json").is_some(){
        let out = ty.make_json(file, line_start, column_start, line_end, column_end, message);
        println!("{}", out);
        return
    }
    println!("{}:{}:{} - {}", file, line_start, column_start, message);
}
/*
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
}*/
