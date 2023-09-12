use std::panic;
pub use crate::LogType;

extern "C" {
    pub fn js_console_log(chars: u32, len: u32);
    pub fn js_console_error(chars: u32, len: u32);
}

pub fn log_with_type(file:&str, line_start:u32, column_start:u32, _line_end:u32, _column_end:u32, message:&str, ty:LogType){
    let msg = format!("{}:{}:{} - {}", file, line_start, column_start, message);
    let chars = msg.chars().collect::<Vec<char >> ();
    if let LogType::Error = ty{
        unsafe{js_console_error(chars.as_ptr() as u32, chars.len() as u32)};        
    }
    else{
        unsafe{js_console_log(chars.as_ptr() as u32, chars.len() as u32)};        
    }
}

#[export_name = "wasm_init_panic_hook"]
pub unsafe extern "C" fn init_panic_hook() {
    pub fn panic_hook(info: &panic::PanicInfo) {
        error!("{}", info)
    }
    panic::set_hook(Box::new(panic_hook));
}

