//! What a hosted child asks its host to do for it, and the host's answers.
//!
//! A child with no OS window and no JVM (an Android child process of the WM)
//! cannot reach the system services an ordinary app would: TLS, the
//! clipboard actions menu, the URL handler, permission prompts, file
//! pickers. It sends a [`ChildRelay`] instead (`AppToStudio::Relay`), the
//! host performs it with its own platform and answers with a [`HostRelay`]
//! (`StudioToApp::Relay`).
//!
//! Both enums are encoded by ordinal like the rest of the protocol: they only
//! ever grow by APPENDING variants (pinned by `relay_tag_tests`). Types stay
//! protocol-local (plain strings, numbers and byte vectors) so this crate
//! does not depend on the platform's network or permission types; the
//! platform converts at the edges.

use makepad_micro_serde::*;

/// An HTTP request a child hands to its host.
#[derive(Clone, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RelayHttpRequest {
    /// The child's own request id (`LiveId.0`); every answer carries it.
    pub request_id: u64,
    /// The request's `metadata_id` (`LiveId.0`), echoed on the response.
    pub metadata_id: u64,
    pub url: String,
    /// `HttpMethod::as_str()`, e.g. "GET".
    pub method: String,
    pub headers: Vec<(String, Vec<String>)>,
    pub body: Option<Vec<u8>>,
    pub ignore_ssl_cert: bool,
    pub is_streaming: bool,
    pub max_response_body_bytes: u64,
}

/// A response (or part of one) the host relays back.
#[derive(Clone, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RelayHttpResponse {
    pub request_id: u64,
    pub metadata_id: u64,
    pub status_code: u16,
    pub headers: Vec<(String, Vec<String>)>,
    pub body: Option<Vec<u8>>,
}

/// What became of a relayed HTTP request.
#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub enum RelayHttpEvent {
    Response(RelayHttpResponse),
    StreamChunk(RelayHttpResponse),
    StreamComplete(RelayHttpResponse),
    Error {
        request_id: u64,
        metadata_id: u64,
        message: String,
    },
    Progress {
        request_id: u64,
        loaded: u64,
        total: u64,
    },
}

/// A permission, as its stable name ("camera", "audio_input", "location",
/// "headset_camera", "scene_access").
#[derive(Clone, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RelayPermission {
    pub request_id: i32,
    pub permission: String,
}

/// A file picker the child wants shown.
#[derive(Clone, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RelayFileDialog {
    /// The dialog's `id` (`LiveId.0`), echoed on the answer.
    pub id: u64,
    /// "select_file", "save_file", "select_folder" or "save_folder".
    pub kind: String,
    pub title: Option<String>,
    pub filename: Option<String>,
    pub location: Option<String>,
    /// (name, extensions) pairs.
    pub filters: Vec<(String, Vec<String>)>,
    pub multiple: bool,
}

/// The picker's answer: the chosen paths (empty = cancelled). The paths are
/// the host's, valid in the child because both run as the same app.
#[derive(Clone, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RelayFileDialogResult {
    pub id: u64,
    pub save: bool,
    pub folder: bool,
    pub paths: Vec<String>,
}

/// Child → host.
#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub enum ChildRelay {
    /// Show the system clipboard actions menu (Copy/Cut/Paste/Select all)
    /// over this rect of the child's window, in the child's logical pixels.
    ShowClipboardActions {
        has_selection: bool,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        keyboard_shift: f64,
    },
    HideClipboardActions,
    HttpRequest(RelayHttpRequest),
    CancelHttpRequest { request_id: u64 },
    /// Open a URL with the system's handler (browser, mail, maps…).
    OpenUrl { url: String },
    CheckPermission(RelayPermission),
    RequestPermission(RelayPermission),
    FileDialog(RelayFileDialog),
}

/// Host → child.
#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub enum HostRelay {
    Http(RelayHttpEvent),
    /// `status`: "granted", "not_determined", "denied_can_retry",
    /// "denied_permanent".
    PermissionResult {
        request_id: i32,
        permission: String,
        status: String,
    },
    FileDialog(RelayFileDialogResult),
}

#[cfg(test)]
mod relay_tag_tests {
    use super::*;

    fn tag<T: SerBin>(msg: &T) -> u16 {
        let bytes = msg.serialize_bin();
        u16::from_le_bytes([bytes[0], bytes[1]])
    }

    /// Relay variants are tagged by ordinal: new ones go LAST.
    #[test]
    fn relay_tags_are_pinned_and_round_trip() {
        assert_eq!(tag(&ChildRelay::HideClipboardActions), 1);
        assert_eq!(tag(&ChildRelay::HttpRequest(RelayHttpRequest::default())), 2);
        assert_eq!(tag(&ChildRelay::OpenUrl { url: String::new() }), 4);
        assert_eq!(tag(&ChildRelay::FileDialog(RelayFileDialog::default())), 7);
        assert_eq!(tag(&HostRelay::Http(RelayHttpEvent::Progress { request_id: 0, loaded: 0, total: 0 })), 0);
        assert_eq!(tag(&HostRelay::FileDialog(RelayFileDialogResult::default())), 2);

        let req = ChildRelay::HttpRequest(RelayHttpRequest {
            request_id: 7,
            url: "https://example.com/a?b".into(),
            method: "POST".into(),
            headers: vec![("Accept".into(), vec!["a".into(), "b".into()])],
            body: Some(vec![1, 2, 3]),
            max_response_body_bytes: u64::MAX,
            ..Default::default()
        });
        assert_eq!(ChildRelay::deserialize_bin(&req.serialize_bin()).unwrap(), req);
        let ev = HostRelay::Http(RelayHttpEvent::Response(RelayHttpResponse {
            request_id: 7,
            status_code: 200,
            body: Some(b"{}".to_vec()),
            ..Default::default()
        }));
        assert_eq!(HostRelay::deserialize_bin(&ev.serialize_bin()).unwrap(), ev);
    }
}
