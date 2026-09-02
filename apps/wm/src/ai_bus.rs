//! The window manager's half of the AI services bus.
//!
//! The aichat child (the pane) is one client; every other client's AI
//! service reaches it through here. The WM parses only the envelope and
//! the routing fields, never the tools:
//!
//! - an up-frame from client C is stamped `from = endpoint(C)` (the WM's
//!   own id for that client — never the sender's claim) and forwarded to
//!   the pane; the last `Register` from each client is remembered so the
//!   pane gets a REPLAY of every registration when it (re)connects;
//! - a down-frame from the pane names a target endpoint: it goes to that
//!   client's socket, or, for the WM's own `os` endpoint, is answered here;
//! - a client that dies produces a synthetic `Unregister` for the pane;
//! - the pane sliding in or out is broadcast as `ChatOpen` to every
//!   registered client, so an app's own embedded chat can step aside.
//!
//! The `os` service is the WM as an app: list, launch, focus, close, and
//! open — a file in its associated app, through the same typed
//! `OpenRequest` a file browser's double-click takes.

use crate::hub::ClientId;
use makepad_ai_services::wire::*;
use makepad_strict_json as json;
use std::collections::HashMap;

/// The WM's own service endpoint.
pub const OS_ENDPOINT: &str = "os";

/// What the bus wants the WM to do with a frame.
pub enum Route {
    /// Send this JSON to that client's studio socket.
    ToClient(ClientId, String),
    /// Send this JSON to the pane client.
    ToPane(String),
    /// A call for the WM itself; answer with `os_reply`.
    Os(ServiceCall),
    Drop,
}

#[derive(Default)]
pub struct AiBus {
    pub pane_client: Option<ClientId>,
    /// The last manifest each client registered, for replay.
    manifests: HashMap<ClientId, ServiceManifest>,
}

impl AiBus {
    pub fn endpoint_of(client: ClientId) -> EndpointId {
        EndpointId(format!("w{client}"))
    }

    fn client_of(endpoint: &EndpointId) -> Option<ClientId> {
        endpoint.as_str().strip_prefix('w')?.parse::<ClientId>().ok()
    }

    pub fn is_pane(&self, client: ClientId) -> bool {
        self.pane_client == Some(client)
    }

    /// Every client with a live registration, oldest first.
    pub fn registered_clients(&self) -> Vec<ClientId> {
        let mut clients: Vec<ClientId> = self.manifests.keys().copied().collect();
        clients.sort_unstable();
        clients
    }

    /// The `os` manifest the pane learns about the WM from.
    pub fn os_manifest(apps: &[(String, String)]) -> ServiceManifest {
        let mut brief = String::from(
            "The desktop itself: which apps exist and run, starting and focusing them. \
             Apps that are not running have no tools until `os.launch` starts them \
             (their tools appear on the next turn). Known apps: ",
        );
        brief.push_str(
            &apps.iter().map(|(id, label)| format!("{label} (`{id}`)")).collect::<Vec<_>>().join(", "),
        );
        brief.push('.');
        ServiceManifest::new(OS_ENDPOINT, "Desktop", brief)
            .with_tool(ToolDef::new(
                "list",
                "The apps this desktop knows, and which are running.",
                r#"{"type":"object","properties":{}}"#,
                Risk::Read,
            ))
            .with_tool(ToolDef::new(
                "launch",
                "Start an app (or focus it if it is already running). Its tools become available on the next turn.",
                r#"{"type":"object","properties":{"app":{"type":"string","description":"the app id from os.list"}},"required":["app"]}"#,
                Risk::Act,
            ))
            .with_tool(ToolDef::new(
                "focus",
                "Bring a running app to the front.",
                r#"{"type":"object","properties":{"app":{"type":"string"}},"required":["app"]}"#,
                Risk::Act,
            ))
            .with_tool(ToolDef::new(
                "close",
                "Close a running app's window.",
                r#"{"type":"object","properties":{"app":{"type":"string"}},"required":["app"]}"#,
                Risk::Act,
            ))
            .with_tool(ToolDef::new(
                "open",
                "Open a file in its associated app (images, video, csv, pdf, html) as a new window; `app` overrides the association.",
                r#"{"type":"object","properties":{"path":{"type":"string","description":"absolute path of the file"},"app":{"type":"string","description":"an app id from os.list, optional"}},"required":["path"]}"#,
                Risk::Act,
            ))
    }

    /// The frames the pane must see when it connects: the WM's own
    /// registration, then every client's last one.
    pub fn replay(&self, os_manifest: ServiceManifest) -> Vec<String> {
        let mut out = vec![HostedUp {
            from: Some(EndpointId(OS_ENDPOINT.into())),
            msg: ServiceUp::Register { manifest: os_manifest, port_tag: 0 },
        }
        .to_json()];
        for client in self.registered_clients() {
            out.push(
                HostedUp {
                    from: Some(Self::endpoint_of(client)),
                    msg: ServiceUp::Register { manifest: self.manifests[&client].clone(), port_tag: 0 },
                }
                .to_json(),
            );
        }
        out
    }

    /// The pane-state broadcast: one `ChatOpen` frame per registered
    /// client, addressed to its endpoint.
    pub fn chat_open_frames(&self, open: bool) -> Vec<(ClientId, String)> {
        self.registered_clients()
            .into_iter()
            .map(|client| {
                let frame = HostedDown { to: Some(Self::endpoint_of(client)), msg: ServiceDown::ChatOpen { open } };
                (client, frame.to_json())
            })
            .collect()
    }

    /// A `Custom` frame from `client`. The WM's own `WmRequest` envelope is
    /// not ours and yields `Drop`.
    pub fn on_custom(&mut self, client: ClientId, json: &str) -> Route {
        if self.is_pane(client) {
            let Some(down) = HostedDown::parse(json) else { return Route::Drop };
            let Some(to) = down.to.clone() else { return Route::Drop };
            if to.as_str() == OS_ENDPOINT {
                return match down.msg {
                    ServiceDown::Call(call) => Route::Os(call),
                    _ => Route::Drop,
                };
            }
            return match Self::client_of(&to) {
                Some(target) if self.manifests.contains_key(&target) => Route::ToClient(target, down.to_json()),
                _ => Route::Drop,
            };
        }
        let Some(mut up) = HostedUp::parse(json) else { return Route::Drop };
        // The sender's claim is never used: the link IS the identity.
        up.from = Some(Self::endpoint_of(client));
        match &up.msg {
            ServiceUp::Register { manifest, .. } => {
                self.manifests.insert(client, manifest.clone());
            }
            ServiceUp::Unregister => {
                self.manifests.remove(&client);
            }
            _ => {}
        }
        Route::ToPane(up.to_json())
    }

    /// A client died: the pane hears an `Unregister` on its behalf.
    pub fn client_died(&mut self, client: ClientId) -> Option<String> {
        if self.is_pane(client) {
            self.pane_client = None;
            return None;
        }
        self.manifests.remove(&client)?;
        Some(HostedUp { from: Some(Self::endpoint_of(client)), msg: ServiceUp::Unregister }.to_json())
    }

    /// The WM's answer to one of its own calls, as a frame for the pane.
    pub fn os_reply(result: ToolResult) -> String {
        HostedUp { from: Some(EndpointId(OS_ENDPOINT.into())), msg: ServiceUp::Result(result) }.to_json()
    }

    /// A string argument of an os call, trimmed. `None` when absent or
    /// not a string — the caller refuses, it never guesses.
    pub fn str_arg(call: &ServiceCall, key: &str) -> Option<String> {
        match json::parse(call.args.as_bytes()) {
            Ok(json::Value::Obj(fields)) => fields
                .into_iter()
                .find(|(k, _)| k == key)
                .and_then(|(_, v)| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty()),
            _ => None,
        }
    }

    /// The `app` argument, lowercased like a registry id.
    pub fn app_arg(call: &ServiceCall) -> Option<String> {
        Self::str_arg(call, "app").map(|s| s.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> ServiceManifest {
        ServiceManifest::new("files", "Files", "The file browser.").with_tool(ToolDef::new(
            "stat",
            "Stat a path.",
            r#"{"type":"object","properties":{"path":{"type":"string"}}}"#,
            Risk::Read,
        ))
    }

    fn call(tool: &str, args: &str) -> ServiceCall {
        ServiceCall { call_id: "c".into(), tool: tool.into(), args: args.into() }
    }

    #[test]
    fn up_frames_are_stamped_and_replayed_and_down_frames_are_routed() {
        let mut bus = AiBus { pane_client: Some(9), ..Default::default() };
        // A client registers: stamped with the WM's endpoint, forwarded.
        let up = HostedUp { from: Some(EndpointId("lie".into())), msg: ServiceUp::Register { manifest: files(), port_tag: 3 } };
        match bus.on_custom(4, &up.to_json()) {
            Route::ToPane(json) => {
                let parsed = HostedUp::parse(&json).unwrap();
                assert_eq!(parsed.from, Some(EndpointId("w4".into())), "the sender's claim is overwritten");
            }
            _ => panic!("expected ToPane"),
        }
        assert_eq!(bus.registered_clients(), vec![4]);
        // Replay carries os first, then the client.
        let replay = bus.replay(AiBus::os_manifest(&[("files".into(), "Files".into())]));
        assert_eq!(replay.len(), 2);
        assert!(replay[0].contains("\"os\"") && replay[1].contains("\"w4\""));
        // The pane addresses the client; the WM routes by endpoint.
        let down = HostedDown { to: Some(EndpointId("w4".into())), msg: ServiceDown::Call(call("stat", "{}")) };
        assert!(matches!(bus.on_custom(9, &down.to_json()), Route::ToClient(4, _)));
        // An os call is the WM's own.
        let os = HostedDown { to: Some(EndpointId("os".into())), msg: ServiceDown::Call(call("launch", r#"{"app":"Route"}"#)) };
        match bus.on_custom(9, &os.to_json()) {
            Route::Os(call) => assert_eq!(AiBus::app_arg(&call).as_deref(), Some("route")),
            _ => panic!("expected Os"),
        }
        // A frame to an unknown endpoint, or the WM's own envelope, drops.
        let stray = HostedDown { to: Some(EndpointId("w77".into())), msg: ServiceDown::ChatOpen { open: true } };
        assert!(matches!(bus.on_custom(9, &stray.to_json()), Route::Drop));
        assert!(matches!(bus.on_custom(4, r#"{"wm":{"Close":{}}}"#), Route::Drop));
        // Death → synthetic Unregister; the pane's own death clears the pane.
        let bye = bus.client_died(4).unwrap();
        assert!(bye.contains("Unregister") && bye.contains("\"w4\""));
        assert!(bus.client_died(4).is_none());
        assert!(bus.client_died(9).is_none());
        assert_eq!(bus.pane_client, None);
    }

    #[test]
    fn chat_open_reaches_every_registered_client() {
        let mut bus = AiBus { pane_client: Some(9), ..Default::default() };
        assert!(bus.chat_open_frames(true).is_empty());
        for client in [6, 4] {
            let up = HostedUp { from: None, msg: ServiceUp::Register { manifest: files(), port_tag: 0 } };
            bus.on_custom(client, &up.to_json());
        }
        let frames = bus.chat_open_frames(true);
        assert_eq!(frames.iter().map(|(c, _)| *c).collect::<Vec<_>>(), vec![4, 6]);
        for (client, json) in frames {
            let down = HostedDown::parse(&json).unwrap();
            assert_eq!(down.to, Some(AiBus::endpoint_of(client)));
            assert_eq!(down.msg, ServiceDown::ChatOpen { open: true });
        }
    }

    #[test]
    fn os_arguments_are_strings_or_nothing() {
        let c = call("open", r#"{"path":"  /tmp/a b.png ","app":"Image"}"#);
        assert_eq!(AiBus::str_arg(&c, "path").as_deref(), Some("/tmp/a b.png"));
        assert_eq!(AiBus::app_arg(&c).as_deref(), Some("image"));
        assert_eq!(AiBus::str_arg(&c, "missing"), None);
        // Wrong type, empty string, no object: refused upstream, never guessed.
        assert_eq!(AiBus::str_arg(&call("open", r#"{"path":7}"#), "path"), None);
        assert_eq!(AiBus::str_arg(&call("open", r#"{"path":"  "}"#), "path"), None);
        assert_eq!(AiBus::str_arg(&call("open", "[]"), "path"), None);
        // The manifest lists the five tools with object schemas.
        let manifest = AiBus::os_manifest(&[]);
        assert!(manifest.validate().is_ok());
        assert!(manifest.tool("open").is_some());
        assert_eq!(manifest.tools.len(), 5);
    }
}
