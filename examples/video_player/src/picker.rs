//! Native file picker via `robius-file-picker` (desktop + Android).

use makepad_widgets::*;
use robius_file_picker::FileDialog;

/// Posted from the picker callback onto the UI action queue.
#[derive(Clone, Debug, Default)]
pub struct PickedMediaAction {
    /// `None` means cancelled or picker error (see `error`).
    pub path_or_uri: Option<String>,
    pub error: Option<String>,
}

fn video_file_filters() -> Vec<(String, Vec<String>)> {
    vec![
        (
            "Video".into(),
            vec![
                "mp4".into(),
                "mkv".into(),
                "webm".into(),
                "avi".into(),
                "mov".into(),
                "flv".into(),
                "ts".into(),
                "m4v".into(),
                "wmv".into(),
                "mpg".into(),
                "mpeg".into(),
                "m3u8".into(),
            ],
        ),
        ("All Files".into(), vec!["*".into()]),
    ]
}

/// Open the native video/file picker. Result arrives as [`PickedMediaAction`].
pub fn pick_local_video() {
    let mut dialog = FileDialog::new().set_title("Open Video");
    dialog = dialog.set_filters(video_file_filters());
    let result = dialog.pick_video(|result| {
        let action = match result {
            Ok(Some(file)) => {
                let path_or_uri = file
                    .path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .or_else(|| file.uri().map(|u| u.to_string()));
                if let Some(ref s) = path_or_uri {
                    log!("video_player: picked {}", s);
                }
                PickedMediaAction {
                    path_or_uri,
                    error: None,
                }
            }
            Ok(None) => PickedMediaAction {
                path_or_uri: None,
                error: None,
            },
            Err(e) => {
                error!("video_player: pick error: {e}");
                PickedMediaAction {
                    path_or_uri: None,
                    error: Some(e.to_string()),
                }
            }
        };
        Cx::post_action(action);
    });
    if let Err(e) = result {
        error!("video_player: failed to show file dialog: {e}");
        Cx::post_action(PickedMediaAction {
            path_or_uri: None,
            error: Some(e.to_string()),
        });
    }
}
