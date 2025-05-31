// Re-export the app from app.rs
pub mod app;
pub use app::App;

// This is important to register the live design components from makepad_widgets
// and any custom components defined in app.rs
use makepad_widgets::*;
live_design!{
    // Import the App component from our app module
    import robrix::app::App;
}
