#[cfg(not(target_os = "android"))]
fn main() {
    makepad_example_window_record_mp4::app::app_main()
}

#[cfg(target_os = "android")]
fn main() {}
