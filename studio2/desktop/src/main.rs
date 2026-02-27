#[cfg(not(any(target_os = "android", target_env = "ohos")))]
fn main() {
    makepad_studio_desktop::app::app_main();
}

#[cfg(any(target_os = "android", target_env = "ohos"))]
fn main() {}
