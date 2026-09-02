//! The WARM VIEWER half of Quick Look v2. (The requester half — the app
//! that asks for a panel and dials it through a selection — is
//! apps/files/src/preview.rs.)
//!
//! A viewer started as `video --preview <path>` is not thrown away when
//! the panel closes: wm keeps the process warm and points it at the next
//! file instead of paying a process launch per arrow key. What arrives is
//! `StudioToApp::Custom` on the studio channel, which the platform hands to
//! the app as `Event::Custom(json)`; `makepad_wm_api::WmEvent::parse` turns that
//! back into the typed vocabulary. Two of those events are the viewer's:
//!
//! * `PreviewFile { path }` — show that file IN PLACE. No respawn, no new
//!   window, and the view reset to its fit default.
//! * `PreviewUnload` — drop what is loaded (here: stop playback, tear the
//!   decoder and the audio queue down, blank the picture) and idle as a
//!   blank themed panel at near-zero cost until the next `PreviewFile`.
//!   **An unload is not a quit**: only `CloseRequested` / `Kill` from the
//!   WM, or the viewer's own Escape/Q, end the process.
//!
//! The state machine lives here rather than in the widget because this is
//! the part worth testing: saying what the next state is needs no `Cx`.

use makepad_widgets::Cx;
use makepad_wm_api::{WmEvent, WmRequest};
use std::path::{Path, PathBuf};

/// What the shell must do about one `WmEvent`.
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewAction {
    /// Load this file in place, resetting the view to its fit default.
    Show(PathBuf),
    /// Drop the loaded resource and idle blank.
    Unload,
    /// The window manager asked for a graceful shutdown.
    Close,
    /// Nothing to do: an event addressed to a preview *requester*, or one
    /// this viewer has no opinion about.
    Ignore,
}

/// What a warm viewer is, and what it is showing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewState {
    /// `--preview` was on the command line: this process is a Quick Look
    /// viewer, so an unload must leave it running.
    preview: bool,
    /// The file on screen; `None` while unloaded.
    showing: Option<PathBuf>,
}

impl PreviewState {
    /// The state a `<bin> [--preview] [path]` launch starts in.
    pub fn new(preview: bool, path: Option<&Path>) -> Self {
        Self {
            preview,
            showing: path.map(|p| p.to_path_buf()),
        }
    }

    /// The file on screen, `None` while unloaded.
    pub fn showing(&self) -> Option<&Path> {
        self.showing.as_deref()
    }

    pub fn is_loaded(&self) -> bool {
        self.showing.is_some()
    }

    /// Fold one WM event into the state and say what the shell must do.
    pub fn on_wm_event(&mut self, event: &WmEvent) -> PreviewAction {
        match event {
            WmEvent::PreviewFile { path } => {
                let path = PathBuf::from(path);
                self.showing = Some(path.clone());
                PreviewAction::Show(path)
            }
            WmEvent::PreviewUnload => {
                if self.showing.take().is_none() {
                    // Already idle. Unloading nothing is not work.
                    return PreviewAction::Ignore;
                }
                PreviewAction::Unload
            }
            WmEvent::CloseRequested => PreviewAction::Close,
            _ => PreviewAction::Ignore,
        }
    }

    /// The window title for the current state: the Quick Look form while
    /// this is a preview viewer, the app's own form otherwise.
    pub fn title(&self, app: &str) -> String {
        let name = self
            .showing
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string());
        match (self.preview, name) {
            (true, Some(name)) => format!("Preview \u{2014} {name}"),
            (true, None) => "Preview".to_string(),
            (false, Some(name)) => format!("{app} \u{2014} {name}"),
            (false, None) => app.to_string(),
        }
    }
}

/// Escape / Q in a HOSTED preview hides the panel and leaves this viewer
/// warm for the next file; standalone (or in a normal tile) it is still a
/// quit. True when the window manager took the request, meaning the caller
/// must NOT quit.
pub fn hide_panel(cx: &Cx, preview: bool) -> bool {
    preview && makepad_wm_api::send(cx, &WmRequest::PreviewClose)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_warm_viewer_unloads_and_reloads() {
        let mut state = PreviewState::new(true, Some(Path::new("/a/one.mp4")));
        assert!(state.is_loaded());
        assert_eq!(state.title("video"), "Preview \u{2014} one.mp4");

        // The panel hid: drop everything, stay alive, say nothing.
        assert_eq!(
            state.on_wm_event(&WmEvent::PreviewUnload),
            PreviewAction::Unload
        );
        assert!(!state.is_loaded());
        assert_eq!(state.showing(), None);
        assert_eq!(state.title("video"), "Preview");
        // A second unload is not more work.
        assert_eq!(
            state.on_wm_event(&WmEvent::PreviewUnload),
            PreviewAction::Ignore
        );

        // Dialed at the next file: load in place.
        let show = WmEvent::PreviewFile {
            path: "/a/two.mp4".into(),
        };
        assert_eq!(
            state.on_wm_event(&show),
            PreviewAction::Show(PathBuf::from("/a/two.mp4"))
        );
        assert!(state.is_loaded());
        assert_eq!(state.title("video"), "Preview \u{2014} two.mp4");
    }

    #[test]
    fn a_retarget_replaces_without_an_unload_in_between() {
        // Dialing through a folder is one PreviewFile per selection; the
        // panel never hides, so an unload must not be invented.
        let mut state = PreviewState::new(true, Some(Path::new("/a/one.mp4")));
        for name in ["two.mp4", "three.mp4"] {
            let event = WmEvent::PreviewFile {
                path: format!("/a/{name}"),
            };
            assert_eq!(
                state.on_wm_event(&event),
                PreviewAction::Show(PathBuf::from(format!("/a/{name}")))
            );
        }
        assert_eq!(state.showing(), Some(Path::new("/a/three.mp4")));
    }

    #[test]
    fn the_wire_reaches_the_state_machine() {
        // Exactly what lands in Event::Custom.
        let json = WmEvent::PreviewFile {
            path: "/a/b.mp4".into(),
        }
        .to_json();
        let event = WmEvent::parse(&json).expect("a WM event");
        let mut state = PreviewState::new(true, None);
        assert_eq!(
            state.on_wm_event(&event),
            PreviewAction::Show(PathBuf::from("/a/b.mp4"))
        );
        // Somebody else's Custom JSON is not ours to act on.
        assert!(WmEvent::parse(r#"{"wm_fullscreen":true}"#).is_none());
        assert!(WmEvent::parse("not json").is_none());
    }

    #[test]
    fn only_a_close_ends_the_process() {
        let mut state = PreviewState::new(true, Some(Path::new("/a/one.mp4")));
        // An unload is never a close — that is the whole point of a warm
        // viewer.
        assert_eq!(
            state.on_wm_event(&WmEvent::PreviewUnload),
            PreviewAction::Unload
        );
        assert_eq!(
            state.on_wm_event(&WmEvent::CloseRequested),
            PreviewAction::Close
        );
        // Events addressed to the preview REQUESTER are not ours.
        for event in [
            WmEvent::PreviewShown {
                path: "/a/one.mp4".into(),
            },
            WmEvent::PreviewHidden,
            WmEvent::Focus { focused: true },
            WmEvent::Hosted {
                theme_splash: "/t/theme.splash".into(),
            },
        ] {
            assert_eq!(state.on_wm_event(&event), PreviewAction::Ignore);
        }
    }

    #[test]
    fn a_standalone_run_keeps_the_apps_own_title() {
        let state = PreviewState::new(false, Some(Path::new("/a/one.mp4")));
        assert_eq!(state.title("video"), "video \u{2014} one.mp4");
        assert_eq!(PreviewState::new(false, None).title("video"), "video");
    }
}
