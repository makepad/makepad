#[cfg(not(target_os = "android"))]
fn main() {
    makepad_example_video_formats::app::app_main()
}

#[cfg(target_os = "android")]
fn main() {}
