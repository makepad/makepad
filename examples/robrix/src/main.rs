use makepad_widgets; // To ensure linkage if app! macro doesn't pull it in directly
use robrix::app::App; // Use the App from lib.rs

fn main() {
    // This is a common pattern for Makepad apps
    // The actual app_main! or similar macro might be defined in makepad_widgets or makepad_platform
    makepad_widgets::app_main!(App);
}
