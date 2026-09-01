//! Opening a video file, through the platform's own native file dialog.
//!
//! `Cx::open_select_file_dialog` runs the OS panel (NSOpenPanel,
//! IFileOpenDialog, or the desktop's dialog helper on Linux) off the UI
//! thread and answers later with a [`FileDialogAction`] — which is why the
//! result is handled in `handle_actions` rather than returned from the call
//! that opened the dialog.

use makepad_widgets::makepad_platform::file_dialogs::{FileDialog, FileDialogAction};
use makepad_widgets::*;

/// Tags our dialog, so its answer is not confused with another's.
pub const PICK_VIDEO: LiveId = live_id!(pick_video);

const VIDEO_EXTENSIONS: [&str; 12] = [
    "mp4", "mkv", "webm", "avi", "mov", "flv", "ts", "m4v", "wmv", "mpg", "mpeg", "m3u8",
];

/// Open the native video picker. The answer arrives as a
/// [`FileDialogAction`]; read it with [`picked_video`].
pub fn pick_local_video(cx: &mut Cx) {
    let dialog = FileDialog::new()
        .set_id(PICK_VIDEO)
        .set_title("Open Video".to_string())
        .add_filter(
            "Video".to_string(),
            VIDEO_EXTENSIONS.iter().map(|e| e.to_string()).collect(),
        )
        .add_filter("All Files".to_string(), vec!["*".to_string()]);
    cx.open_select_file_dialog(dialog);
}

/// The path the user chose, if this action is our dialog answering with
/// one. Cancelling yields `None` and needs no handling — nothing was armed.
pub fn picked_video(action: &Action) -> Option<String> {
    let picked = action.downcast_ref::<FileDialogAction>()?;
    if picked.id() != PICK_VIDEO {
        return None;
    }
    Some(picked.path()?.to_string_lossy().into_owned())
}
