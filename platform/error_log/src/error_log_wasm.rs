#[macro_export]
macro_rules!log {
    ( $ ( $ t: tt) *) => {
        crate::makepad_error_log::console_log_impl(&format!("{}:{} - {}", file!(), line!(), format!( $ ( $ t) *)))
    }
}

#[macro_export]
macro_rules!error {
    ( $ ( $ t: tt) *) => {
        crate::makepad_error_log::console_error_impl(&format!("{}:{} - {}", file!(), line!(), format!( $ ( $ t) *)))
    }
}

extern "C" {
    pub fn js_console_log(chars: u32, len: u32);
}

pub fn console_log_impl(val: &str) {
    unsafe {
        let chars = val.chars().collect::<Vec<char >> ();
        js_console_log(chars.as_ptr() as u32, chars.len() as u32);
    }
}

extern "C" { 
    pub fn js_console_error(chars: u32, len: u32);
}

pub fn console_error_impl(val: &str) {
    unsafe {
        let chars = val.chars().collect::<Vec<char >> ();
        js_console_error(chars.as_ptr() as u32, chars.len() as u32);
    }
}