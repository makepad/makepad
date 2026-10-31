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
//!
//! The WM is the one bus owner of its process tree: every hosted client's
//! service is also in the WM's process registry
//! ([`ServiceRegistry::process`]), under the client's `w<id>` endpoint, so
//! the WM's makepad-remote `app` service lists and calls it (a hosted child
//! serves no `app` of its own). The registry's calls go down to the client
//! like the pane's; the client's answers to them go back to the registry,
//! everything else to the pane. The WM also answers a client's `Register`
//! itself (the endpoint is the WM's to issue), so a port has its address
//! without a pane.

use makepad_app_host::ClientId;
use makepad_ai_services::engine::ServiceRegistry;
use makepad_ai_services::port::{ServiceLink, ServiceLinkHost};
use makepad_ai_services::wire::*;
use makepad_strict_json as json;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::TryRecvError;

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
    /// A frame for an IN-PROCESS instance (a module the WM hosts itself):
    /// the host runs its executor and answers with `local_reply`.
    Local(ClientId, ServiceDown),
    Drop,
}

#[derive(Default)]
pub struct AiBus {
    pub pane_client: Option<ClientId>,
    /// The last manifest each client registered, for replay.
    manifests: HashMap<ClientId, ServiceManifest>,
    /// Clients that are module instances in this process (`m<id>`
    /// endpoints): no socket, their frames are made and answered here.
    /// This leg is what the web superbuild runs everything on.
    locals: HashSet<ClientId>,
    /// The process registry the clients' services are bridged into; `None`
    /// keeps the bus to the pane alone (tests).
    registry: Option<ServiceRegistry>,
    /// Per process client: the host half of its link in the registry.
    links: HashMap<ClientId, ServiceLinkHost>,
    /// Calls the registry sent down: their answers go back to it.
    registry_calls: HashSet<String>,
    /// Frames for clients made by the bus itself (`Registered` answers).
    outbox: Vec<(ClientId, String)>,
    /// Clients that registered since the last look (the WM adds their pid).
    joined: Vec<ClientId>,
}

impl AiBus {
    /// A bus that also serves its clients through `registry`.
    pub fn with_registry(registry: ServiceRegistry) -> AiBus {
        AiBus { registry: Some(registry), ..Default::default() }
    }

    pub fn registry(&self) -> Option<&ServiceRegistry> {
        self.registry.as_ref()
    }

    /// Frames the bus made for clients, to put on their sockets.
    pub fn take_client_frames(&mut self) -> Vec<(ClientId, String)> {
        std::mem::take(&mut self.outbox)
    }

    /// Clients that registered since the last call.
    pub fn take_joined(&mut self) -> Vec<ClientId> {
        std::mem::take(&mut self.joined)
    }

    /// What the registry sent down since the last drain: calls and cancels
    /// for the clients, readdressed to their `w<id>` endpoints.
    pub fn drain_registry(&mut self) -> Vec<(ClientId, String)> {
        let mut out = Vec::new();
        let mut gone = Vec::new();
        for (client, host) in &self.links {
            loop {
                match host.down.try_recv() {
                    Ok(HostedDown { msg, .. }) => {
                        match &msg {
                            ServiceDown::Call(call) => {
                                self.registry_calls.insert(call.call_id.clone());
                            }
                            ServiceDown::Cancel { call_id } => {
                                self.registry_calls.remove(call_id);
                            }
                            // the registry's own bookkeeping; the WM answered Register itself
                            _ => continue,
                        }
                        out.push((*client, HostedDown { to: Some(Self::endpoint_of(*client)), msg }.to_json()));
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        gone.push(*client);
                        break;
                    }
                }
            }
        }
        for client in gone {
            self.links.remove(&client);
        }
        out
    }

    /// A client's frame into its registry link: a registration opens the
    /// link; an answer to a registry call goes there and nowhere else.
    /// True when the frame is the registry's alone.
    fn to_registry(&mut self, client: ClientId, up: &HostedUp) -> bool {
        let Some(registry) = &self.registry else { return false };
        let endpoint = Self::endpoint_of(client);
        match &up.msg {
            ServiceUp::Register { manifest, port_tag } => {
                self.outbox.push((
                    client,
                    HostedDown { to: Some(endpoint.clone()), msg: ServiceDown::Registered { port_tag: *port_tag, endpoint: endpoint.clone() } }.to_json(),
                ));
                match self.links.get(&client) {
                    Some(host) => {
                        let _ = host.up.send(HostedUp { from: None, msg: up.msg.clone() });
                    }
                    None => {
                        let (link, host) = ServiceLink::pair(manifest.clone());
                        let _ = host.up.send(HostedUp { from: None, msg: up.msg.clone() });
                        match registry.register_as(link, endpoint, "hosted by the window manager", None) {
                            Ok(()) => {
                                self.links.insert(client, host);
                                self.joined.push(client);
                            }
                            Err(e) => makepad_widgets::log!("wm: the registry refused client {client}: {e}"),
                        }
                    }
                }
                false
            }
            ServiceUp::Unregister => {
                if self.links.remove(&client).is_some() {
                    registry.unregister(&endpoint);
                }
                false
            }
            ServiceUp::Result(r) => self.forward_registry_answer(client, &r.call_id.clone(), true, up),
            ServiceUp::Progress { call_id, .. } => self.forward_registry_answer(client, &call_id.clone(), false, up),
            ServiceUp::Context(_) => {
                if let Some(host) = self.links.get(&client) {
                    let _ = host.up.send(HostedUp { from: None, msg: up.msg.clone() });
                }
                false
            }
            ServiceUp::Message { .. } => false,
        }
    }

    fn forward_registry_answer(&mut self, client: ClientId, call_id: &str, last: bool, up: &HostedUp) -> bool {
        if !self.registry_calls.contains(call_id) {
            return false;
        }
        if last {
            self.registry_calls.remove(call_id);
        }
        if let Some(host) = self.links.get(&client) {
            let _ = host.up.send(HostedUp { from: None, msg: up.msg.clone() });
        }
        true
    }

    pub fn endpoint_of(client: ClientId) -> EndpointId {
        EndpointId(format!("w{client}"))
    }

    /// `w<id>` for a process client, `m<id>` for an in-process instance.
    pub fn endpoint_for(&self, client: ClientId) -> EndpointId {
        if self.locals.contains(&client) {
            EndpointId(format!("m{client}"))
        } else {
            Self::endpoint_of(client)
        }
    }

    /// (is local, client) from an endpoint string; `None` for neither kind.
    fn client_of(endpoint: &EndpointId) -> Option<(bool, ClientId)> {
        let s = endpoint.as_str();
        let local = match s.chars().next() {
            Some('w') => false,
            Some('m') => true,
            _ => return None,
        };
        s[1..].parse::<ClientId>().ok().map(|c| (local, c))
    }

    /// An in-process instance joins the bus: remembered like any client's
    /// registration (so the replay carries it) and announced to the pane
    /// now with the frame this returns.
    pub fn register_local(&mut self, client: ClientId, manifest: ServiceManifest) -> String {
        self.locals.insert(client);
        self.manifests.insert(client, manifest.clone());
        HostedUp { from: Some(self.endpoint_for(client)), msg: ServiceUp::Register { manifest, port_tag: 0 } }.to_json()
    }

    /// An in-process instance's answer, as a frame for the pane.
    pub fn local_reply(&self, client: ClientId, result: ToolResult) -> String {
        self.local_up(client, ServiceUp::Result(result))
    }

    /// An in-process instance's asynchronous publication, stamped with the
    /// same module endpoint as its call results.
    pub fn local_message(&self, client: ClientId, sub_id: String, message: Message) -> String {
        self.local_up(
            client,
            ServiceUp::Message {
                sub_id,
                topic: message.topic,
                text: message.text,
                data: message.data,
                final_: message.final_,
            },
        )
    }

    fn local_up(&self, client: ClientId, msg: ServiceUp) -> String {
        HostedUp { from: Some(self.endpoint_for(client)), msg }.to_json()
    }

    pub fn local_clients(&self) -> Vec<ClientId> {
        let mut out: Vec<ClientId> = self.locals.iter().copied().collect();
        out.sort_unstable();
        out
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
             Apps that are not running have no tools until `os.launch` starts them; a running app's \
             tools are already in your table — call them, never launch it again. Known apps: ",
        );
        brief.push_str(
            &apps.iter().map(|(id, label)| format!("{label} (`{id}`)")).collect::<Vec<_>>().join(", "),
        );
        brief.push('.');
        ServiceManifest::new(OS_ENDPOINT, "Desktop", brief)
            .with_tool(ToolDef::new(
                "list",
                "The apps this desktop knows (its own and the Wayland apps of its launcher), and which are running.",
                r#"{"type":"object","properties":{}}"#,
                Risk::Read,
            ))
            .with_tool(ToolDef::new(
                "launch",
                "Start an app that is NOT running (see the running list), by its id from os.list (`wl.<name>` ids are Wayland apps). A running app's tools are already available — call them directly instead of launching.",
                r#"{"type":"object","properties":{"app":{"type":"string","description":"the app id from os.list"}},"required":["app"]}"#,
                Risk::Act,
            ))
            .with_tool(ToolDef::new(
                "run",
                "Run a program (a command line) as a Wayland window on this desktop. Only for programs os.list does not have.",
                r#"{"type":"object","properties":{"command":{"type":"string","description":"the program and its arguments"}},"required":["command"]}"#,
                Risk::Destructive,
            ))
            .with_tool(ToolDef::new(
                "focus",
                "Bring a running app to the front.",
                r#"{"type":"object","properties":{"app":{"type":"string"}},"required":["app"]}"#,
                Risk::Act,
            ))
            .with_tool(ToolDef::new(
                "maximize",
                "Make a running app's window fill the desktop (a new window may open small).",
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
                    from: Some(self.endpoint_for(client)),
                    msg: ServiceUp::Register { manifest: self.manifests[&client].clone(), port_tag: 0 },
                }
                .to_json(),
            );
        }
        out
    }

    /// The pane-state broadcast: one `ChatOpen` frame per registered
    /// PROCESS client, addressed to its endpoint (an in-process instance
    /// hears it from the host directly).
    pub fn chat_open_frames(&self, open: bool) -> Vec<(ClientId, String)> {
        self.registered_clients()
            .into_iter()
            .filter(|client| !self.locals.contains(client))
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
                Some((true, target)) if self.locals.contains(&target) => Route::Local(target, down.msg),
                Some((false, target)) if !self.locals.contains(&target) && self.manifests.contains_key(&target) => {
                    Route::ToClient(target, down.to_json())
                }
                _ => Route::Drop,
            };
        }
        let Some(mut up) = HostedUp::parse(json) else { return Route::Drop };
        // The sender's claim is never used: the link IS the identity.
        up.from = Some(Self::endpoint_of(client));
        if self.to_registry(client, &up) {
            return Route::Drop;
        }
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
        if self.links.remove(&client).is_some() {
            if let Some(registry) = &self.registry {
                registry.unregister(&Self::endpoint_of(client));
            }
        }
        self.manifests.remove(&client)?;
        let from = Some(self.endpoint_for(client));
        self.locals.remove(&client);
        Some(HostedUp { from, msg: ServiceUp::Unregister }.to_json())
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
    use makepad_ai_services::engine::{RegistryUp, ServiceRegistry};
    use makepad_ai_services::port::ServiceLink;

    fn files() -> ServiceManifest {
        ServiceManifest::new("files", "Files", "The file browser.").with_tool(ToolDef::new(
            "stat",
            "Stat a path.",
            r#"{"type":"object","properties":{"path":{"type":"string"}}}"#,
            Risk::Read,
        ))
    }

    fn call(tool: &str, args: &str) -> ServiceCall {
        ServiceCall { call_id: "c".into(), tool: tool.into(), args: args.into(), caller: None }
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
    fn hosted_services_are_served_through_the_process_registry() {
        let registry = ServiceRegistry::new();
        let mut bus = AiBus::with_registry(registry.clone());
        bus.pane_client = Some(9);
        // A client registers: the WM answers Registered itself and the
        // registry lists the service under the client's endpoint.
        let up = HostedUp { from: None, msg: ServiceUp::Register { manifest: files(), port_tag: 3 } };
        assert!(matches!(bus.on_custom(4, &up.to_json()), Route::ToPane(_)));
        let frames = bus.take_client_frames();
        assert_eq!(frames.len(), 1);
        let reg = HostedDown::parse(&frames[0].1).unwrap();
        assert_eq!(reg.msg, ServiceDown::Registered { port_tag: 3, endpoint: EndpointId("w4".into()) });
        assert_eq!(bus.take_joined(), vec![4]);
        let w4 = EndpointId("w4".into());
        assert_eq!(registry.manifest(&w4).map(|m| m.id), Some("files".into()));
        // A registry call goes down to the client, addressed to w4…
        let rx = registry.call_remote(&w4, call("stat", "{}")).unwrap();
        let down = bus.drain_registry();
        assert!(matches!(down.as_slice(), [(4, json)] if HostedDown::parse(json).unwrap().to == Some(w4.clone())));
        // …and its answer goes back to the registry, not to the pane.
        let answer = HostedUp { from: None, msg: ServiceUp::Result(ToolResult::ok("c", "a file", "")) };
        assert!(matches!(bus.on_custom(4, &answer.to_json()), Route::Drop));
        registry.pump_remote();
        assert!(matches!(rx.try_recv(), Ok(RegistryUp::Result(e, r)) if e == w4 && r.call_id == "c"));
        // The pane's own calls are answered to the pane as before.
        let pane_answer = HostedUp { from: None, msg: ServiceUp::Result(ToolResult::ok("p1", "x", "")) };
        assert!(matches!(bus.on_custom(4, &pane_answer.to_json()), Route::ToPane(_)));
        // The client dies: the registry forgets it.
        assert!(bus.client_died(4).is_some());
        assert!(registry.manifest(&w4).is_none());
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
    fn pubsub_frames_cross_the_hosted_bus_in_both_directions() {
        let mut bus = AiBus { pane_client: Some(9), ..Default::default() };
        let manifest = files().with_topic(TopicDef::new("changes", "File changes."));
        let register = HostedUp {
            from: None,
            msg: ServiceUp::Register { manifest: manifest.clone(), port_tag: 0 },
        };
        let register = match bus.on_custom(4, &register.to_json()) {
            Route::ToPane(json) => HostedUp::parse(&json).expect("valid registration"),
            _ => panic!("expected the registration to reach the pane"),
        };

        // This is the aichat side of the same bridge: the registry owns
        // the engine half and the WM endpoint remains its routing identity.
        let registry = ServiceRegistry::new();
        let endpoint = EndpointId("w4".into());
        let (link, host) = ServiceLink::pair(manifest);
        registry.register_as(link, endpoint.clone(), "hosted by wm", None).unwrap();
        host.up.send(register).unwrap();
        assert!(registry.pump().is_empty());
        let _registered = host.down.try_recv().expect("registry acknowledgement");

        assert!(registry.send(
            &endpoint,
            ServiceDown::Subscribe {
                sub_id: "s1".into(),
                topic: "changes".into(),
                filter: Some(r#"{"kind":"done"}"#.into()),
            },
        ));
        let subscribe = host.down.try_recv().expect("subscription from engine");
        let routed = match bus.on_custom(9, &subscribe.to_json()) {
            Route::ToClient(4, json) => HostedDown::parse(&json).expect("valid down-frame"),
            _ => panic!("expected the subscription to reach client 4"),
        };
        assert_eq!(routed, subscribe);

        assert!(registry.send(&endpoint, ServiceDown::Unsubscribe { sub_id: "s1".into() }));
        let unsubscribe = host.down.try_recv().expect("unsubscription from engine");
        let routed = match bus.on_custom(9, &unsubscribe.to_json()) {
            Route::ToClient(4, json) => HostedDown::parse(&json).expect("valid down-frame"),
            _ => panic!("expected the unsubscription to reach client 4"),
        };
        assert_eq!(routed, unsubscribe);

        let message = HostedUp {
            from: Some(EndpointId("forged".into())),
            msg: ServiceUp::Message {
                sub_id: "s1".into(),
                topic: "changes".into(),
                text: "finished".into(),
                data: Some(r#"{"rows":3}"#.into()),
                final_: true,
            },
        };
        let message = match bus.on_custom(4, &message.to_json()) {
            Route::ToPane(json) => HostedUp::parse(&json).expect("valid up-frame"),
            _ => panic!("expected the message to reach the pane"),
        };
        assert_eq!(message.from, Some(endpoint.clone()));
        host.up.send(message).unwrap();
        assert!(matches!(
            registry.pump().as_slice(),
            [RegistryUp::Message { endpoint: from, sub_id, message }]
                if from == &endpoint
                    && sub_id == "s1"
                    && message.topic == "changes"
                    && message.text == "finished"
                    && message.final_
        ));
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
        // The manifest lists the seven tools with object schemas; `run`
        // starts any program, so it is declared destructive.
        let manifest = AiBus::os_manifest(&[]);
        assert!(manifest.validate().is_ok());
        assert!(manifest.tool("open").is_some());
        assert_eq!(manifest.tool("run").map(|t| t.risk), Some(Risk::Destructive));
        assert_eq!(manifest.tools.len(), 7);
    }
}

#[cfg(test)]
mod local_tests {
    use super::*;

    fn sheets() -> ServiceManifest {
        ServiceManifest::new("sheets", "Sheets", "The spreadsheet.").with_tool(ToolDef::new(
            "summary",
            "The sheet on screen.",
            r#"{"type":"object","properties":{}}"#,
            Risk::Read,
        ))
    }

    #[test]
    fn an_in_process_instance_is_a_local_endpoint_on_the_same_bus() {
        let mut bus = AiBus { pane_client: Some(9), ..Default::default() };
        // Registering announces it with an `m` endpoint, and the replay
        // carries it like any client's registration.
        let announce = bus.register_local(4, sheets());
        let up = HostedUp::parse(&announce).unwrap();
        assert_eq!(up.from, Some(EndpointId("m4".into())));
        assert!(matches!(up.msg, ServiceUp::Register { .. }));
        assert_eq!(bus.local_clients(), vec![4]);
        let replay = bus.replay(AiBus::os_manifest(&[]));
        assert_eq!(replay.len(), 2);
        assert!(replay[1].contains("\"m4\""));
        // A call addressed to it is the host's to run; the wrong kind of
        // address for the same id drops.
        let call = ServiceCall { call_id: "c1".into(), tool: "summary".into(), args: "{}".into(), caller: None };
        let down = HostedDown { to: Some(EndpointId("m4".into())), msg: ServiceDown::Call(call.clone()) };
        match bus.on_custom(9, &down.to_json()) {
            Route::Local(4, ServiceDown::Call(c)) => assert_eq!(c.call_id, "c1"),
            _ => panic!("expected Local"),
        }
        let wrong = HostedDown { to: Some(EndpointId("w4".into())), msg: ServiceDown::Call(call) };
        assert!(matches!(bus.on_custom(9, &wrong.to_json()), Route::Drop));
        // The answer goes up from the local endpoint.
        let reply = bus.local_reply(4, ToolResult::ok("c1", "Sheet 1", ""));
        let up = HostedUp::parse(&reply).unwrap();
        assert_eq!(up.from, Some(EndpointId("m4".into())));
        let publication = bus.local_message(4, "lease-s1".into(), Message::new("watch", "changed"));
        let up = HostedUp::parse(&publication).unwrap();
        assert!(matches!(
            up,
            HostedUp {
                from: Some(EndpointId(ref from)),
                msg: ServiceUp::Message { ref sub_id, ref text, .. },
            } if from == "m4" && sub_id == "lease-s1" && text == "changed"
        ));
        // ChatOpen goes to process clients only; the host tells locals itself.
        assert!(bus.chat_open_frames(true).is_empty());
        // Death: an Unregister from the local endpoint, then nothing.
        let bye = bus.client_died(4).unwrap();
        assert!(bye.contains("Unregister") && bye.contains("\"m4\""));
        assert!(bus.local_clients().is_empty());
        assert!(bus.client_died(4).is_none());
    }
}
