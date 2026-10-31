//! Pictures the person attaches (a screenshot dropped on the chat): an
//! absolute path to an image file is a reference the model SEES, not text.
//! The core only recognises the reference and asks the loader the window
//! registered for the picture (the core has no image codec: no UI, no GPU).

pub use makepad_ai_hub::providers::provider::ToolImage;
use std::path::Path;
use std::sync::OnceLock;

/// The longest side a picture keeps.
pub const MAX_SIDE: usize = 1024;
/// Pictures one message carries (what the chat session accepts).
pub const MAX_PER_MESSAGE: usize = 4;

/// Brings a picture file down to what the model gets.
pub type Loader = fn(&str) -> Result<ToolImage, String>;
static LOADER: OnceLock<Loader> = OnceLock::new();

/// Registers the picture loader (the window does, at start).
pub fn set_loader(loader: Loader) {
    let _ = LOADER.set(loader);
}

/// An absolute path naming an image file: a picture reference.
pub fn is_picture_ref(reference: &str) -> bool {
    let path = Path::new(reference);
    path.is_absolute()
        && reference.len() <= 4096
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp"))
}

/// The picture at `reference` as the model gets it.
pub fn load(reference: &str) -> Result<ToolImage, String> {
    match LOADER.get() {
        Some(loader) => loader(reference),
        None => Err("pictures are not available in this build (no image loader registered)".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_absolute_image_paths_are_pictures() {
        assert!(is_picture_ref("/home/a/Screenshots/screenshot-2026-10-12-090503.png"));
        assert!(is_picture_ref("/x/y.JPG"));
        assert!(!is_picture_ref("src/a.png"), "a workspace file is text context");
        assert!(!is_picture_ref("/x/notes.txt"));
    }
}
