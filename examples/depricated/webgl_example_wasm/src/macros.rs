#[cfg(target_arch = "wasm32")]
#[allow(unused_macros)]
macro_rules! println {
    ($($arg:tt)*) => ({
        let string = std::format_args!($($arg)*).to_string();
        #[allow(unused_unsafe)]
        unsafe {
            $crate::console_log(string.as_ptr() as i32, string.len() as i32)
        };
    })
}
