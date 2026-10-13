//! Apple backends: one Metal core under two shells, AppKit (macOS) and UIKit (iOS, tvOS). The
//! shell-neutral half of the window system (ops, event inbox, main-loop state) is `shell`.

#[cfg(target_os = "macos")]
pub mod appkit;
pub mod media;
pub mod metal;
pub mod shell;
#[cfg(any(target_os = "ios", target_os = "tvos"))]
pub mod uikit;
pub use metal::Metal;
