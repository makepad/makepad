pub use makepad_widgets;
use makepad_widgets::*;

mod audio;
mod spectrum;
mod app;

pub use app::*;

app_main!(App);
