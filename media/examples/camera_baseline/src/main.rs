#[cfg(not(target_os = "android"))]
fn main() {
    makepad_example_camera_baseline::app::app_main()
}

#[cfg(target_os = "android")]
fn main() {}
