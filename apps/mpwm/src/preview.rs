//! Spawning the viewer a client asked for.
//!
//! The vocabulary lives in `libs/mp_wm_api` (`WmRequest::Preview` /
//! `Open`): a client names a path and, optionally, the app to open it
//! with; the WM resolves the association (`mp_wm_api::viewer_for`), finds
//! the registry entry and hosts it — as a floating Quick-Look popup for a
//! preview, as a normal tile for an open. The client never spawns
//! anything itself while hosted.

use std::collections::HashMap;
use std::path::PathBuf;

use mp_wm_api::{viewer_for, WmRequest};

use crate::clients::{registry, spawn_client, AppDef, ClientLine};
use crate::hub::ClientId;

/// The Quick Look warm-viewer cache, one live client per viewer TYPE
/// (`mp_wm_api::viewer_for`'s app id — "mpimage", "mppdf", …), kept alive
/// and idling once its float is hidden so the next preview of that type
/// is instant: no respawn, no "starting…" flash, no build/exec on every
/// arrow key. Killed only when the WM tears the client down for real (WM
/// exit, workspace close-all) — see `App::remove_client`.
#[derive(Default)]
pub struct PreviewCache {
    /// viewer app id -> its warm `ClientId`. Entries survive a hide; a
    /// dead process clears its own entry (`App::remove_client`) so the
    /// next request respawns instead of talking to a dead socket.
    pub warm: HashMap<String, ClientId>,
    /// Whichever warm client is the CURRENTLY VISIBLE float, if any — one
    /// panel, reused across retargets within the same requester/session.
    pub active: Option<ActivePreview>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivePreview {
    /// The tile that asked for this preview (mpfiles, say) — keeps key
    /// focus throughout and is the one told `PreviewShown`/`PreviewHidden`.
    pub requester: ClientId,
    /// The viewer app id (`mp_wm_api::viewer_for`), also the `warm` key.
    pub viewer_app: String,
    /// The warm client currently showing it.
    pub client: ClientId,
    pub path: PathBuf,
}

/// A resolved preview/open request.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenRequest {
    pub app: String,
    pub path: PathBuf,
    pub preview: bool,
}

impl OpenRequest {
    /// Resolve a `Preview`/`Open` request against the association table.
    pub fn from_request(req: &WmRequest) -> Option<Self> {
        let (app, path, preview) = match req {
            WmRequest::Preview { app, path } => (app.clone(), path.clone(), true),
            WmRequest::Open { app, path } => (app.clone(), path.clone(), false),
            _ => return None,
        };
        let path = PathBuf::from(path);
        let app = app.unwrap_or_else(|| viewer_for(&path).to_string());
        Some(Self { app, path, preview })
    }
}

/// The registry entry for an app id, with the file (and `--preview`)
/// appended to its args.
pub fn app_for_request(req: &OpenRequest) -> Option<(AppDef, Vec<String>)> {
    let app = registry().iter().find(|a| a.id == req.app)?.clone();
    let mut extra = Vec::new();
    if req.preview {
        extra.push("--preview".to_string());
    }
    extra.push(req.path.to_string_lossy().to_string());
    Some((app, extra))
}

/// Spawn the app for a request. Returns the new client slot.
pub fn spawn_for_request(
    req: &OpenRequest,
    id: ClientId,
    hub_port: u16,
    lines: std::sync::mpsc::Sender<ClientLine>,
) -> Result<crate::clients::ClientSlot, String> {
    let (app, extra) = app_for_request(req)
        .ok_or_else(|| format!("no app '{}' for {}", req.app, req.path.display()))?;
    spawn_client(&app, id, hub_port, None, None, &extra, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_without_an_app_takes_the_association() {
        let req = WmRequest::Preview {
            app: None,
            path: "/a/b c.png".to_string(),
        };
        let r = OpenRequest::from_request(&req).unwrap();
        assert!(r.preview);
        assert_eq!(r.app, "mpimage");
        assert_eq!(r.path, PathBuf::from("/a/b c.png"));

        let video = OpenRequest::from_request(&WmRequest::Open {
            app: None,
            path: "/clip.mp4".to_string(),
        })
        .unwrap();
        assert!(!video.preview);
        assert_eq!(video.app, "mpvideo");
        // Anything without a viewer falls back to the terminal.
        let other = OpenRequest::from_request(&WmRequest::Open {
            app: None,
            path: "/notes.rs".to_string(),
        })
        .unwrap();
        assert_eq!(other.app, "mpterm");
    }

    #[test]
    fn a_named_app_wins_over_the_association() {
        let r = OpenRequest::from_request(&WmRequest::Preview {
            app: Some("mpsheets".to_string()),
            path: "/x.png".to_string(),
        })
        .unwrap();
        assert_eq!(r.app, "mpsheets");
        // Only Preview/Open resolve to a viewer.
        assert!(OpenRequest::from_request(&WmRequest::Close).is_none());
    }

    #[test]
    fn preview_passes_the_flag_before_the_path() {
        let req = OpenRequest {
            app: "mpimage".to_string(),
            path: PathBuf::from("/a.png"),
            preview: true,
        };
        let (app, extra) = app_for_request(&req).unwrap();
        assert_eq!(app.id, "mpimage");
        assert_eq!(extra, vec!["--preview".to_string(), "/a.png".to_string()]);
        let plain = OpenRequest {
            preview: false,
            ..req
        };
        let (_, extra) = app_for_request(&plain).unwrap();
        assert_eq!(extra, vec!["/a.png".to_string()]);
    }
}
