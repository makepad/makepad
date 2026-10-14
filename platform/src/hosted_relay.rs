//! The host half of the hosted-child relay, and the conversions both halves
//! share.
//!
//! A hosted child with no OS window and no JVM (an Android child process of
//! the WM) sends `AppToStudio::Relay(ChildRelay)` for what it cannot do
//! itself: HTTP with TLS, the clipboard actions menu, opening a URL,
//! permission prompts, file pickers. A host keeps one [`HostRelayServer`],
//! hands it every relay a child sends ([`HostRelayServer::on_child`]) and
//! every event and action it sees ([`HostRelayServer::on_event`],
//! [`HostRelayServer::on_actions`]), and sends what those return to the
//! named client as `StudioToApp::Relay`. The host's own ids never collide
//! with the child's: each relayed request gets a fresh host id, mapped back
//! when the answer arrives.

use {
    crate::{
        action::Actions,
        cx::Cx,
        cx_api::{CxOsApi, OpenUrlInPlace},
        event::Event,
        file_dialogs::{FileDialog, FileDialogAction, Filter},
        makepad_live_id::LiveId,
        makepad_math::*,
        makepad_network::{HttpError, HttpMethod, HttpProgress, HttpRequest, HttpResponse, NetworkResponse},
        permission::{Permission, PermissionResult, PermissionStatus},
        studio::{
            ChildRelay, HostRelay, RelayFileDialog, RelayFileDialogResult, RelayHttpEvent,
            RelayHttpRequest, RelayHttpResponse, RelayPermission,
        },
    },
    std::{collections::HashMap, path::PathBuf},
};

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

pub fn relay_http_request(request_id: LiveId, request: &HttpRequest) -> RelayHttpRequest {
    RelayHttpRequest {
        request_id: request_id.0,
        metadata_id: request.metadata_id.0,
        url: request.url.clone(),
        method: request.method.as_str().to_string(),
        headers: request.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        body: request.body.clone(),
        ignore_ssl_cert: request.ignore_ssl_cert,
        is_streaming: request.is_streaming,
        max_response_body_bytes: request.max_response_body_bytes,
    }
}

pub fn http_request_from_relay(relay: &RelayHttpRequest) -> HttpRequest {
    let method = match relay.method.as_str() {
        "HEAD" => HttpMethod::HEAD,
        "POST" => HttpMethod::POST,
        "PUT" => HttpMethod::PUT,
        "DELETE" => HttpMethod::DELETE,
        "CONNECT" => HttpMethod::CONNECT,
        "OPTIONS" => HttpMethod::OPTIONS,
        "TRACE" => HttpMethod::TRACE,
        "PATCH" => HttpMethod::PATCH,
        _ => HttpMethod::GET,
    };
    let mut request = HttpRequest::new(relay.url.clone(), method);
    request.metadata_id = LiveId(relay.metadata_id);
    request.headers = relay.headers.iter().cloned().collect();
    request.body = relay.body.clone();
    request.ignore_ssl_cert = relay.ignore_ssl_cert;
    request.is_streaming = relay.is_streaming;
    request.max_response_body_bytes = relay.max_response_body_bytes;
    request
}

fn relay_response(request_id: u64, response: &HttpResponse) -> RelayHttpResponse {
    RelayHttpResponse {
        request_id,
        metadata_id: response.metadata_id.0,
        status_code: response.status_code,
        headers: response.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        body: response.body.as_ref().map(|b| b.to_vec()),
    }
}

fn response_from_relay(relay: &RelayHttpResponse) -> HttpResponse {
    HttpResponse::new(
        LiveId(relay.metadata_id),
        relay.status_code,
        relay.headers.iter().cloned().collect(),
        relay.body.clone(),
    )
}

/// A host-side network response, re-addressed to the child's request id.
/// `None` for websocket traffic (never relayed).
pub fn relay_http_event(child_request_id: u64, response: &NetworkResponse) -> Option<RelayHttpEvent> {
    Some(match response {
        NetworkResponse::HttpResponse { response, .. } => {
            RelayHttpEvent::Response(relay_response(child_request_id, response))
        }
        NetworkResponse::HttpStreamChunk { response, .. } => {
            RelayHttpEvent::StreamChunk(relay_response(child_request_id, response))
        }
        NetworkResponse::HttpStreamComplete { response, .. } => {
            RelayHttpEvent::StreamComplete(relay_response(child_request_id, response))
        }
        NetworkResponse::HttpError { error, .. } => RelayHttpEvent::Error {
            request_id: child_request_id,
            metadata_id: error.metadata_id.0,
            message: error.message.clone(),
        },
        NetworkResponse::HttpProgress { progress, .. } => RelayHttpEvent::Progress {
            request_id: child_request_id,
            loaded: progress.loaded,
            total: progress.total,
        },
        _ => return None,
    })
}

/// The child side: a relayed answer as the network response its own
/// runtime would have produced.
pub fn network_response_from_relay(event: &RelayHttpEvent) -> (LiveId, NetworkResponse, bool) {
    // The bool: this answer ends the request.
    match event {
        RelayHttpEvent::Response(r) => (
            LiveId(r.request_id),
            NetworkResponse::HttpResponse { request_id: LiveId(r.request_id), response: response_from_relay(r) },
            true,
        ),
        RelayHttpEvent::StreamChunk(r) => (
            LiveId(r.request_id),
            NetworkResponse::HttpStreamChunk { request_id: LiveId(r.request_id), response: response_from_relay(r) },
            false,
        ),
        RelayHttpEvent::StreamComplete(r) => (
            LiveId(r.request_id),
            NetworkResponse::HttpStreamComplete { request_id: LiveId(r.request_id), response: response_from_relay(r) },
            true,
        ),
        RelayHttpEvent::Error { request_id, metadata_id, message } => (
            LiveId(*request_id),
            NetworkResponse::HttpError {
                request_id: LiveId(*request_id),
                error: HttpError { message: message.clone(), metadata_id: LiveId(*metadata_id) },
            },
            true,
        ),
        RelayHttpEvent::Progress { request_id, loaded, total } => (
            LiveId(*request_id),
            NetworkResponse::HttpProgress {
                request_id: LiveId(*request_id),
                progress: HttpProgress { loaded: *loaded, total: *total },
            },
            false,
        ),
    }
}

/// What a hosted child may open through its host: web, mail, phone and map
/// links. Not `file:` / `content:` / `intent:` or app schemes, which would
/// let a child reach the host's files or start arbitrary activities.
pub fn url_scheme_allowed(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else { return false };
    matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "mailto" | "tel" | "geo")
}

pub fn permission_name(permission: Permission) -> &'static str {
    match permission {
        Permission::AudioInput => "audio_input",
        Permission::Camera => "camera",
        Permission::HeadsetCamera => "headset_camera",
        Permission::SceneAccess => "scene_access",
        Permission::Location => "location",
    }
}

pub fn permission_from_name(name: &str) -> Option<Permission> {
    Some(match name {
        "audio_input" => Permission::AudioInput,
        "camera" => Permission::Camera,
        "headset_camera" => Permission::HeadsetCamera,
        "scene_access" => Permission::SceneAccess,
        "location" => Permission::Location,
        _ => return None,
    })
}

pub fn permission_status_name(status: PermissionStatus) -> &'static str {
    match status {
        PermissionStatus::Granted => "granted",
        PermissionStatus::NotDetermined => "not_determined",
        PermissionStatus::DeniedCanRetry => "denied_can_retry",
        PermissionStatus::DeniedPermanent => "denied_permanent",
    }
}

pub fn permission_status_from_name(name: &str) -> PermissionStatus {
    match name {
        "granted" => PermissionStatus::Granted,
        "denied_can_retry" => PermissionStatus::DeniedCanRetry,
        "denied_permanent" => PermissionStatus::DeniedPermanent,
        _ => PermissionStatus::NotDetermined,
    }
}

/// Set by a host that shows the file and folder pickers for its children
/// (the Linux WM draws them): a hosted child then relays its file dialogs
/// instead of opening its own. Not passed on to a child's own children
/// (`makepad_app_host::launch::configure` removes it).
pub const HOST_FILE_DIALOGS_ENV: &str = "MAKEPAD_HOST_FILE_DIALOGS";

/// Whether this process's host shows its file dialogs (`HOST_FILE_DIALOGS_ENV`).
pub fn host_serves_file_dialogs() -> bool {
    static SERVES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SERVES.get_or_init(|| std::env::var(HOST_FILE_DIALOGS_ENV).is_ok_and(|v| v == "1"))
}

pub fn relay_file_dialog(kind: &str, dialog: &FileDialog) -> RelayFileDialog {
    RelayFileDialog {
        id: dialog.id.0,
        kind: kind.to_string(),
        title: dialog.title.clone(),
        filename: dialog.filename.clone(),
        location: dialog.location.as_ref().map(|p| p.to_string_lossy().into_owned()),
        filters: dialog.filters.iter().map(|f| (f.description.clone(), f.extensions.clone())).collect(),
        multiple: dialog.multiple,
    }
}

fn file_dialog_from_relay(relay: &RelayFileDialog, id: LiveId) -> FileDialog {
    let mut dialog = FileDialog::new();
    dialog.id = id;
    dialog.title = relay.title.clone();
    dialog.filename = relay.filename.clone();
    dialog.location = relay.location.as_ref().map(PathBuf::from);
    dialog.filters = relay
        .filters
        .iter()
        .map(|(description, extensions)| Filter { description: description.clone(), extensions: extensions.clone() })
        .collect();
    dialog.multiple = relay.multiple;
    dialog
}

/// The child side: a picker's relayed answer as the action the app waits for.
pub fn file_dialog_action_from_relay(result: &RelayFileDialogResult) -> FileDialogAction {
    let id = LiveId(result.id);
    let mut paths: Vec<PathBuf> = result.paths.iter().map(PathBuf::from).collect();
    match (result.folder, result.save) {
        (true, _) => match paths.pop() {
            Some(path) => FileDialogAction::FolderSelected(path),
            None => FileDialogAction::FolderCancelled,
        },
        (false, true) => match paths.pop() {
            Some(path) => FileDialogAction::SaveFileSelected { id, path },
            None => FileDialogAction::SaveFileCancelled { id },
        },
        (false, false) if paths.is_empty() => FileDialogAction::FileCancelled { id },
        (false, false) => FileDialogAction::FileSelected { id, paths },
    }
}

// ---------------------------------------------------------------------------
// Host side
// ---------------------------------------------------------------------------

struct PendingDialog {
    client: u64,
    child_id: u64,
    save: bool,
    folder: bool,
}

/// What a host does for its hosted children (see the module doc).
#[derive(Default)]
pub struct HostRelayServer {
    http: HashMap<LiveId, (u64, u64)>,
    permissions: HashMap<i32, (u64, i32)>,
    dialogs: HashMap<LiveId, PendingDialog>,
    folder_dialogs: Vec<PendingDialog>,
    /// Host clipboard read id -> (client, the child's id).
    clipboard_reads: HashMap<u64, (u64, u64)>,
}

impl HostRelayServer {
    /// A relay from `client`, whose window's top-left sits at `origin` in
    /// the host's coordinates (the clipboard menu is placed over the
    /// child's rect). Returns answers to send right away (none yet: every
    /// answer comes back through `on_event` / `on_actions`).
    pub fn on_child(&mut self, cx: &mut Cx, client: u64, origin: Vec2d, relay: ChildRelay) -> Vec<(u64, HostRelay)> {
        match relay {
            ChildRelay::ShowClipboardActions { has_selection, x, y, width, height, keyboard_shift } => {
                let rect = Rect { pos: origin + dvec2(x, y), size: dvec2(width, height) };
                cx.show_clipboard_actions(has_selection, rect, keyboard_shift);
            }
            ChildRelay::HideClipboardActions => cx.hide_clipboard_actions(),
            ChildRelay::HttpRequest(request) => {
                let host_id = LiveId::unique();
                self.http.insert(host_id, (client, request.request_id));
                cx.http_request(host_id, http_request_from_relay(&request));
            }
            ChildRelay::CancelHttpRequest { request_id } => {
                let found = self.http.iter().find(|(_, (c, r))| *c == client && *r == request_id).map(|(k, _)| *k);
                if let Some(host_id) = found {
                    self.http.remove(&host_id);
                    cx.cancel_http_request(host_id);
                }
            }
            ChildRelay::OpenUrl { url } => {
                if url_scheme_allowed(&url) {
                    cx.open_url(&url, OpenUrlInPlace::No);
                } else {
                    crate::error!("relay: a hosted child's open_url with a refused scheme: {url}");
                }
            }
            ChildRelay::CheckPermission(p) | ChildRelay::RequestPermission(p)
                if permission_from_name(&p.permission).is_none() =>
            {
                crate::error!("relay: unknown permission {:?}", p.permission);
                return vec![(
                    client,
                    HostRelay::PermissionResult {
                        request_id: p.request_id,
                        permission: p.permission,
                        status: "denied_permanent".into(),
                    },
                )];
            }
            ChildRelay::CheckPermission(RelayPermission { request_id, permission }) => {
                let id = cx.check_permission(permission_from_name(&permission).unwrap());
                self.permissions.insert(id, (client, request_id));
            }
            ChildRelay::RequestPermission(RelayPermission { request_id, permission }) => {
                let id = cx.request_permission(permission_from_name(&permission).unwrap());
                self.permissions.insert(id, (client, request_id));
            }
            ChildRelay::FileDialog(dialog) => {
                let host_id = LiveId::unique();
                let settings = file_dialog_from_relay(&dialog, host_id);
                let pending = PendingDialog {
                    client,
                    child_id: dialog.id,
                    save: dialog.kind.starts_with("save"),
                    folder: dialog.kind.ends_with("folder"),
                };
                match dialog.kind.as_str() {
                    "save_file" => cx.open_save_file_dialog(settings),
                    "select_folder" => cx.open_select_folder_dialog(settings),
                    "save_folder" => cx.platform_ops.push_back(crate::cx_api::CxOsOp::SaveFolderDialog(settings)),
                    _ => cx.open_select_file_dialog(settings),
                }
                if pending.folder {
                    self.folder_dialogs.push(pending);
                } else {
                    self.dialogs.insert(host_id, pending);
                }
            }
            ChildRelay::ReadClipboard { request_id } => {
                let host_id = cx.read_clipboard();
                self.clipboard_reads.insert(host_id, (client, request_id));
            }
            ChildRelay::SetPrimarySelection { text } => cx.set_primary_selection(&text),
        }
        Vec::new()
    }

    /// Answers the event carries for children: relayed HTTP traffic and
    /// permission results.
    pub fn on_event(&mut self, event: &Event) -> Vec<(u64, HostRelay)> {
        let mut out = Vec::new();
        match event {
            Event::NetworkResponses(responses) if !self.http.is_empty() => {
                for response in responses {
                    let host_id = match response {
                        NetworkResponse::HttpResponse { request_id, .. }
                        | NetworkResponse::HttpStreamChunk { request_id, .. }
                        | NetworkResponse::HttpStreamComplete { request_id, .. }
                        | NetworkResponse::HttpError { request_id, .. }
                        | NetworkResponse::HttpProgress { request_id, .. } => *request_id,
                        _ => continue,
                    };
                    let Some(&(client, child_id)) = self.http.get(&host_id) else { continue };
                    let ends = !matches!(
                        response,
                        NetworkResponse::HttpStreamChunk { .. } | NetworkResponse::HttpProgress { .. }
                    );
                    if ends {
                        self.http.remove(&host_id);
                    }
                    if let Some(ev) = relay_http_event(child_id, response) {
                        out.push((client, HostRelay::Http(ev)));
                    }
                }
            }
            Event::Clipboard(e) => {
                if let Some((client, request_id)) = self.clipboard_reads.remove(&e.request) {
                    let png = match &e.content.image {
                        Some(crate::clipboard_read::ClipboardImage::Png(png)) => png.to_vec(),
                        _ => Vec::new(),
                    };
                    out.push((client, HostRelay::Clipboard { request_id, text: e.content.text.clone().unwrap_or_default(), png }));
                }
            }
            Event::PermissionResult(PermissionResult { permission, request_id, status }) => {
                if let Some((client, child_id)) = self.permissions.remove(request_id) {
                    out.push((
                        client,
                        HostRelay::PermissionResult {
                            request_id: child_id,
                            permission: permission_name(*permission).into(),
                            status: permission_status_name(*status).into(),
                        },
                    ));
                }
            }
            _ => {}
        }
        out
    }

    /// Picker answers (they arrive as actions).
    pub fn on_actions(&mut self, actions: &Actions) -> Vec<(u64, HostRelay)> {
        let mut out = Vec::new();
        for action in actions {
            if self.dialogs.is_empty() && self.folder_dialogs.is_empty() {
                continue;
            }
            let Some(action) = action.downcast_ref::<FileDialogAction>() else { continue };
            let (pending, paths) = match action {
                FileDialogAction::FolderSelected(path) if !self.folder_dialogs.is_empty() => {
                    (self.folder_dialogs.remove(0), vec![path.clone()])
                }
                FileDialogAction::FolderCancelled if !self.folder_dialogs.is_empty() => {
                    (self.folder_dialogs.remove(0), Vec::new())
                }
                FileDialogAction::FileSelected { id, paths } => match self.dialogs.remove(id) {
                    Some(p) => (p, paths.clone()),
                    None => continue,
                },
                FileDialogAction::SaveFileSelected { id, path } => match self.dialogs.remove(id) {
                    Some(p) => (p, vec![path.clone()]),
                    None => continue,
                },
                FileDialogAction::FileCancelled { id } | FileDialogAction::SaveFileCancelled { id } => {
                    match self.dialogs.remove(id) {
                        Some(p) => (p, Vec::new()),
                        None => continue,
                    }
                }
                _ => continue,
            };
            let mut result = RelayFileDialogResult {
                id: pending.child_id,
                save: pending.save,
                folder: pending.folder,
                paths: Vec::new(),
            };
            // A save target or a folder the picker names by `content://` URI
            // is writable only through a JVM, and there is no write-back
            // path for a child yet: it is answered as cancelled rather than
            // handed a path it cannot use.
            let unusable = paths.iter().any(|p| p.to_string_lossy().starts_with("content://"));
            if (pending.save || pending.folder) && unusable {
                crate::log!("relay: a {} picked for a hosted child is a content:// URI; answered as cancelled",
                    if pending.folder { "folder" } else { "save target" });
                out.push((pending.client, HostRelay::FileDialog(result)));
                continue;
            }
            result.paths = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            out.push((pending.client, HostRelay::FileDialog(result)));
        }
        out
    }

    /// A child went away: its outstanding HTTP requests are cancelled and
    /// its pending answers dropped.
    pub fn forget_client(&mut self, cx: &mut Cx, client: u64) {
        let gone: Vec<LiveId> = self.http.iter().filter(|(_, (c, _))| *c == client).map(|(k, _)| *k).collect();
        for host_id in gone {
            self.http.remove(&host_id);
            cx.cancel_http_request(host_id);
        }
        self.permissions.retain(|_, (c, _)| *c != client);
        self.dialogs.retain(|_, p| p.client != client);
        self.folder_dialogs.retain(|p| p.client != client);
        self.clipboard_reads.retain(|_, (c, _)| *c != client);
    }
}

// ---------------------------------------------------------------------------
// Child side
// ---------------------------------------------------------------------------

impl Cx {
    /// A host's answer to one of this child's relays.
    pub(crate) fn handle_host_relay(&mut self, relay: HostRelay) {
        match relay {
            HostRelay::Http(event) => {
                let _ = event;
            }
            HostRelay::PermissionResult { request_id, permission, status } => {
                let Some(permission) = permission_from_name(&permission) else { return };
                self.call_event_handler(&Event::PermissionResult(PermissionResult {
                    permission,
                    request_id,
                    status: permission_status_from_name(&status),
                }));
            }
            HostRelay::FileDialog(result) => {
                Cx::post_action(file_dialog_action_from_relay(&result));
            }
            HostRelay::Clipboard { request_id, text, png } => {
                use crate::clipboard_read::{ClipboardContent, ClipboardEvent, ClipboardImage};
                let content = ClipboardContent {
                    supported: true,
                    text: (!text.is_empty()).then_some(text),
                    image: (!png.is_empty()).then(|| ClipboardImage::Png(std::sync::Arc::new(png))),
                    files: Vec::new(),
                };
                self.call_event_handler(&Event::Clipboard(ClipboardEvent { request: request_id, content: std::sync::Arc::new(content) }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Host and child ids never meet: the host answers under the child's id.
    #[test]
    fn http_answers_are_readdressed_to_the_child_request() {
        let mut request = HttpRequest::new("https://api.example/x".into(), HttpMethod::POST);
        request.set_header("A".into(), "1".into());
        request.set_body(vec![9]);
        let relay = relay_http_request(LiveId(77), &request);
        let back = http_request_from_relay(&relay);
        assert_eq!(back, request);

        let host = NetworkResponse::HttpResponse {
            request_id: LiveId(5),
            response: HttpResponse::new(LiveId(3), 200, Default::default(), Some(b"ok".to_vec())),
        };
        let ev = relay_http_event(77, &host).unwrap();
        let (id, child, ends) = network_response_from_relay(&ev);
        assert_eq!(id, LiveId(77));
        assert!(ends);
        assert!(matches!(child, NetworkResponse::HttpResponse { request_id, ref response }
            if request_id == LiveId(77) && response.body_string().as_deref() == Some("ok")));
    }

    #[test]
    fn server_maps_network_responses_to_their_client_and_forgets_finished_ones() {
        let mut server = HostRelayServer::default();
        server.http.insert(LiveId(5), (42, 77));
        let chunk = NetworkResponse::HttpStreamChunk {
            request_id: LiveId(5),
            response: HttpResponse::new(LiveId(0), 200, Default::default(), Some(vec![1])),
        };
        let done = NetworkResponse::HttpStreamComplete {
            request_id: LiveId(5),
            response: HttpResponse::new(LiveId(0), 200, Default::default(), None),
        };
        let other = NetworkResponse::HttpResponse {
            request_id: LiveId(6),
            response: HttpResponse::new(LiveId(0), 200, Default::default(), None),
        };
        let out = server.on_event(&Event::NetworkResponses(vec![chunk, other.clone()]));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 42);
        assert!(server.http.contains_key(&LiveId(5)));
        let out = server.on_event(&Event::NetworkResponses(vec![done]));
        assert!(matches!(&out[0].1, HostRelay::Http(RelayHttpEvent::StreamComplete(r)) if r.request_id == 77));
        assert!(server.http.is_empty());
        assert!(server.on_event(&Event::NetworkResponses(vec![other])).is_empty());
    }

    #[test]
    fn only_link_schemes_are_opened_for_children() {
        for ok in ["https://makepad.nl", "HTTP://x", "mailto:a@b", "tel:+31", "geo:52,4"] {
            assert!(url_scheme_allowed(ok), "{ok}");
        }
        for bad in ["file:///data/x", "content://x", "intent://x#Intent;end", "javascript:1", "noscheme"] {
            assert!(!url_scheme_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn permission_and_picker_answers_round_trip() {
        for p in [Permission::AudioInput, Permission::Camera, Permission::HeadsetCamera, Permission::SceneAccess, Permission::Location] {
            assert_eq!(permission_from_name(permission_name(p)), Some(p));
        }
        for st in [PermissionStatus::Granted, PermissionStatus::NotDetermined, PermissionStatus::DeniedCanRetry, PermissionStatus::DeniedPermanent] {
            assert_eq!(permission_status_from_name(permission_status_name(st)), st);
        }
        let picked = RelayFileDialogResult { id: 9, save: false, folder: false, paths: vec!["/a".into(), "/b".into()] };
        assert!(matches!(file_dialog_action_from_relay(&picked), FileDialogAction::FileSelected { id, ref paths } if id == LiveId(9) && paths.len() == 2));
        let cancelled = RelayFileDialogResult { id: 9, save: true, folder: false, paths: vec![] };
        assert!(matches!(file_dialog_action_from_relay(&cancelled), FileDialogAction::SaveFileCancelled { id } if id == LiveId(9)));
        let folder = RelayFileDialogResult { id: 0, save: false, folder: true, paths: vec!["/d".into()] };
        assert!(matches!(file_dialog_action_from_relay(&folder), FileDialogAction::FolderSelected(_)));
    }
}
