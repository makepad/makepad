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

use crate::clients::{spawn_client, AppDef, ClientLine};
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
    /// A retarget that arrived while its viewer had no socket yet: a
    /// freshly spawned viewer is in `warm` the instant it is spawned, but
    /// only connects a second or two later, and `send_wm_event` to a
    /// senderless slot is a silent no-op. Dialing that fast (Space, then
    /// ArrowDown before the first frame) would otherwise leave the panel
    /// showing the file from the command line while the requester believes
    /// it moved on. Flushed by `App::drain_hub` on `HubEvent::Connected`.
    pub pending: HashMap<ClientId, String>,
}

/// The pure slot-state half of Quick Look, kept free of `Cx` so the type
/// map and the dead-process cleanup are unit-testable (see the tests at
/// the bottom of this file); `App` owns only the I/O around it.
impl PreviewCache {
    /// This type's warm viewer, if the process behind it is still usable.
    /// `usable` is the caller's liveness test (the client is still in the
    /// table and is not already closing); a slot that fails it is DROPPED
    /// here, so the next request respawns instead of talking to a corpse.
    pub fn warm_client(
        &mut self,
        viewer_app: &str,
        usable: impl Fn(ClientId) -> bool,
    ) -> Option<ClientId> {
        let id = self.warm.get(viewer_app).copied()?;
        if usable(id) {
            return Some(id);
        }
        self.warm.remove(viewer_app);
        self.pending.remove(&id);
        None
    }

    /// Remember a freshly spawned viewer as this TYPE's warm slot. One per
    /// type: a second spawn of the same type replaces the entry.
    pub fn remember(&mut self, viewer_app: &str, id: ClientId) {
        self.warm.insert(viewer_app.to_string(), id);
    }

    /// A client died (or was closed for real): drop every reference to it.
    /// Returns true when it was the visible panel or its requester, so the
    /// caller knows the panel is gone.
    pub fn forget_client(&mut self, id: ClientId) -> bool {
        self.warm.retain(|_, warm| *warm != id);
        self.pending.remove(&id);
        let was_active = self
            .active
            .as_ref()
            .map(|a| a.client == id || a.requester == id)
            .unwrap_or(false);
        if was_active {
            self.active = None;
        }
        was_active
    }

    /// True when a panel of a DIFFERENT viewer type is currently up: the
    /// type switch that has to hide (never kill) the old one first.
    pub fn switching_type(&self, viewer_app: &str) -> bool {
        self.active
            .as_ref()
            .map(|a| a.viewer_app != viewer_app)
            .unwrap_or(false)
    }

    /// Is this client the panel that is up right now?
    pub fn is_showing(&self, id: ClientId) -> bool {
        self.active.as_ref().map(|a| a.client == id).unwrap_or(false)
    }

    /// Is `client` the app that asked for the panel now showing?
    pub fn is_requester(&self, client: ClientId) -> bool {
        self.active
            .as_ref()
            .map(|a| a.requester == client)
            .unwrap_or(false)
    }

    /// Park a retarget until the viewer's socket exists (see `pending`).
    /// Last write wins — only the newest file is worth showing.
    pub fn queue_pending(&mut self, id: ClientId, path: String) {
        self.pending.insert(id, path);
    }

    /// Take whatever was parked for a client that has just connected.
    pub fn take_pending(&mut self, id: ClientId) -> Option<String> {
        self.pending.remove(&id)
    }

    /// Every warm viewer process, for the WM's own shutdown.
    pub fn warm_clients(&self) -> Vec<ClientId> {
        let mut out: Vec<ClientId> = self.warm.values().copied().collect();
        out.sort_unstable();
        out
    }

    /// Nothing is warm, showing or parked any more: the WM closed
    /// everything (or is going down) and every viewer process went with it.
    pub fn clear(&mut self) {
        self.warm.clear();
        self.pending.clear();
        self.active = None;
    }
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
    // find_app, not the menu list: the image/pdf viewers are launchable
    // without being menu rows.
    let app = crate::clients::find_app(&req.app)?;
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
    // Never `warm`: a Quick-Look viewer has its own warm-cache mechanism
    // (`PreviewCache`), which keeps a viewer alive between panels rather
    // than standing one by before the first.
    spawn_client(&app, id, hub_port, None, None, &extra, false, lines)
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

    fn shown(cache: &mut PreviewCache, requester: ClientId, app: &str, client: ClientId, p: &str) {
        cache.active = Some(ActivePreview {
            requester,
            viewer_app: app.to_string(),
            client,
            path: PathBuf::from(p),
        });
    }

    #[test]
    fn the_type_map_holds_one_warm_viewer_per_viewer_type() {
        let mut cache = PreviewCache::default();
        cache.remember("mpimage", 7);
        cache.remember("mppdf", 8);
        assert_eq!(cache.warm_client("mpimage", |_| true), Some(7));
        assert_eq!(cache.warm_client("mppdf", |_| true), Some(8));
        // A type nobody has previewed yet has no warm client.
        assert_eq!(cache.warm_client("mpvideo", |_| true), None);
        // Respawning a type replaces its slot rather than accumulating.
        cache.remember("mpimage", 11);
        assert_eq!(cache.warm_client("mpimage", |_| true), Some(11));
        assert_eq!(cache.warm_clients(), vec![8, 11]);
    }

    #[test]
    fn a_dead_viewer_clears_its_slot_at_the_next_request() {
        let mut cache = PreviewCache::default();
        cache.remember("mpvideo", 4);
        cache.queue_pending(4, "/clip.mp4".to_string());
        // The process died (or is closing): the entry goes, so the caller
        // spawns a fresh viewer instead of writing to a dead socket...
        let dead = |id: ClientId| id != 4;
        assert_eq!(cache.warm_client("mpvideo", dead), None);
        assert!(cache.warm.is_empty());
        assert!(cache.pending.is_empty());
        // ...and the cleared slot stays cleared for a live successor.
        cache.remember("mpvideo", 5);
        assert_eq!(cache.warm_client("mpvideo", |_| true), Some(5));
    }

    #[test]
    fn removing_a_client_forgets_it_everywhere() {
        let mut cache = PreviewCache::default();
        cache.remember("mpimage", 3);
        cache.queue_pending(3, "/b.png".to_string());
        shown(&mut cache, 1, "mpimage", 3, "/b.png");
        // The VIEWER dying takes the panel down with it.
        assert!(cache.forget_client(3));
        assert!(cache.active.is_none());
        assert!(cache.warm.is_empty());
        assert_eq!(cache.take_pending(3), None);

        // So does the REQUESTER dying — but the viewer stays warm, it did
        // nothing wrong and the next app to dial reuses it.
        let mut cache = PreviewCache::default();
        cache.remember("mpimage", 3);
        shown(&mut cache, 1, "mpimage", 3, "/b.png");
        assert!(cache.forget_client(1));
        assert!(cache.active.is_none());
        assert_eq!(cache.warm_client("mpimage", |_| true), Some(3));

        // An unrelated client is not the panel and changes nothing.
        assert!(!cache.forget_client(99));
    }

    #[test]
    fn the_type_switch_and_the_requester_are_recognised() {
        let mut cache = PreviewCache::default();
        assert!(!cache.switching_type("mpimage"), "nothing is up yet");
        shown(&mut cache, 1, "mpimage", 3, "/b.png");
        assert!(!cache.switching_type("mpimage"), "same type = a retarget");
        assert!(cache.switching_type("mppdf"), "png -> pdf hides the float");
        assert!(cache.is_showing(3));
        assert!(!cache.is_showing(4));
        assert!(cache.is_requester(1));
        assert!(!cache.is_requester(3), "the viewer is not the requester");
        cache.active = None;
        assert!(!cache.is_requester(1));
    }

    #[test]
    fn a_retarget_before_the_socket_exists_is_parked_and_flushed_once() {
        let mut cache = PreviewCache::default();
        cache.queue_pending(6, "/one.png".to_string());
        // Dialing again before the viewer connects: only the newest file
        // is worth showing.
        cache.queue_pending(6, "/two.png".to_string());
        assert_eq!(cache.take_pending(6), Some("/two.png".to_string()));
        assert_eq!(cache.take_pending(6), None, "flushed exactly once");
    }

    #[test]
    fn a_close_all_takes_the_whole_cache_with_it() {
        // Hiding keeps a viewer warm; closing everything (CTRL+ALT+DELETE,
        // or the WM going down) must not leave one running invisibly.
        let mut cache = PreviewCache::default();
        cache.remember("mpimage", 3);
        cache.remember("mppdf", 4);
        cache.queue_pending(4, "/doc.pdf".to_string());
        shown(&mut cache, 1, "mpimage", 3, "/b.png");
        assert_eq!(cache.warm_clients(), vec![3, 4]);
        cache.clear();
        assert!(cache.warm_clients().is_empty());
        assert!(cache.active.is_none());
        assert_eq!(cache.take_pending(4), None);
        assert_eq!(cache.warm_client("mpimage", |_| true), None);
    }
}
