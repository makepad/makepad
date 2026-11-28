//! An app exposes its AI tools by opening a port in its process's service registry.
//!
//! A window-manager-hosted process also registers with its host over the HAL
//! or studio transport. Both the process's own `app` endpoint and the WM's
//! chat pane can then call its tools. An embedded HAL domain shares its
//! parent's process and registers only with its host.
//!
//! Each peer answers the port's registration nonce with its own endpoint.
//! The port accepts calls addressed to that endpoint and returns answers to
//! their origin. When both peers are attached, the app receives port-unique
//! call and subscription ids and can use them across frames.
//!
//! A port never executes anything. What a call does is the app's closed
//! match over its own tool names; the port only carries it.

use crate::wire::*;
use makepad_platform::studio::AppToStudio;
use makepad_platform::thread::SignalToUI;
use makepad_platform::{Cx, Event};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

/// What the engine wants from the service, as the app reads it each frame.
#[derive(Clone, Debug, PartialEq)]
pub enum PortEvent {
    /// The host accepted the registration; the port now has an address.
    Registered(EndpointId),
    Call(ServiceCall),
    /// Stop this call if you can; no reply is expected.
    Cancel { call_id: String },
    /// Start publishing matching messages under this host-issued id.
    Subscribe { sub_id: String, topic: String, filter: Option<String> },
    /// Stop publishing under this id.
    Unsubscribe { sub_id: String },
    /// The host's chat pane came up or went away.
    ChatOpen { open: bool },
}

/// The engine's end of an in-process service: what the registry holds.
/// The registry issues the endpoint and answers `Registered` itself.
pub struct ServiceLink {
    pub manifest: ServiceManifest,
    /// Engine → service.
    pub down: Sender<HostedDown>,
    /// Service → engine.
    pub up: Receiver<HostedUp>,
}

/// The transport's end of a link the HOST bridges itself — the window
/// manager feeds `up` from the child's frames and forwards `down` to it.
pub struct ServiceLinkHost {
    pub up: Sender<HostedUp>,
    pub down: Receiver<HostedDown>,
}

impl ServiceLink {
    /// A link and its bridge end. The registry takes the link; whoever
    /// moves frames takes the host end.
    pub fn pair(manifest: ServiceManifest) -> (ServiceLink, ServiceLinkHost) {
        let (down_tx, down_rx) = channel();
        let (up_tx, up_rx) = channel();
        (
            ServiceLink { manifest, down: down_tx, up: up_rx },
            ServiceLinkHost { up: up_tx, down: down_rx },
        )
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
enum Peer {
    Local,
    Host,
}

enum HostTransport {
    Hosted,
    #[cfg(not(target_arch = "wasm32"))]
    Domain(makepad_platform::os::hal::HalParentSender),
    #[cfg(test)]
    TestChannel { up: Sender<HostedUp>, down: Receiver<HostedDown> },
}

impl HostTransport {
    fn send(&self, msg: ServiceUp) {
        let frame = HostedUp { from: None, msg };
        match self {
            Self::Hosted => Cx::send_studio_message(AppToStudio::Custom(frame.to_json())),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Domain(sender) => sender.send(&frame.to_json()),
            #[cfg(test)]
            Self::TestChannel { up, .. } => { let _ = up.send(frame); }
        }
    }
}

struct LocalPeer {
    up: Sender<HostedUp>,
    down: Receiver<HostedDown>,
    endpoint: Option<EndpointId>,
    confirmations: ServiceConfirmations,
}

impl LocalPeer {
    fn send(up: &Sender<HostedUp>, msg: ServiceUp) {
        if up.send(HostedUp { from: None, msg }).is_ok() {
            SignalToUI::set_ui_signal();
        }
    }
}

struct HostPeer {
    transport: HostTransport,
    endpoint: Option<EndpointId>,
    logged_rejection: bool,
    confirmations: ServiceConfirmations,
}

impl HostPeer {
    fn new(transport: HostTransport) -> Self {
        Self { transport, endpoint: None, logged_rejection: false, confirmations: Default::default() }
    }
}

/// Both peers mint their own ids. The app uses a unique id until the call
/// completes or the subscription ends. Reverse maps handle peer cancellations.
#[derive(Default)]
struct RouteTable {
    next_seq: u64,
    calls: HashMap<String, (Peer, String)>,
    call_rev: HashMap<(Peer, String), String>,
    subs: HashMap<String, (Peer, String)>,
    sub_rev: HashMap<(Peer, String), String>,
}

impl RouteTable {
    fn insert(
        next_seq: &mut u64,
        forward: &mut HashMap<String, (Peer, String)>,
        reverse: &mut HashMap<(Peer, String), String>,
        peer: Peer,
        orig: String,
    ) -> String {
        let key = (peer, orig);
        if let Some(id) = reverse.get(&key) {
            return id.clone();
        }
        *next_seq += 1;
        let prefix = match peer { Peer::Local => 'l', Peer::Host => 'h' };
        let id = format!("p{prefix}_{next_seq}");
        forward.insert(id.clone(), key.clone());
        reverse.insert(key, id.clone());
        id
    }

    fn insert_call(&mut self, peer: Peer, orig: String) -> String {
        Self::insert(&mut self.next_seq, &mut self.calls, &mut self.call_rev, peer, orig)
    }

    fn insert_sub(&mut self, peer: Peer, orig: String) -> String {
        Self::insert(&mut self.next_seq, &mut self.subs, &mut self.sub_rev, peer, orig)
    }

    fn peek_call(&self, app_id: &str) -> Option<(Peer, String)> {
        self.calls.get(app_id).cloned()
    }

    fn peek_sub(&self, app_id: &str) -> Option<(Peer, String)> {
        self.subs.get(app_id).cloned()
    }

    fn take_call(&mut self, app_id: &str) -> Option<(Peer, String)> {
        let key = self.calls.remove(app_id)?;
        self.call_rev.remove(&key);
        Some(key)
    }

    fn take_sub(&mut self, app_id: &str) -> Option<(Peer, String)> {
        let key = self.subs.remove(app_id)?;
        self.sub_rev.remove(&key);
        Some(key)
    }

    fn remove_peer_call(&mut self, peer: Peer, orig: &str) -> Option<String> {
        let id = self.call_rev.remove(&(peer, orig.to_owned()))?;
        self.calls.remove(&id);
        Some(id)
    }

    fn remove_peer_sub(&mut self, peer: Peer, orig: &str) -> Option<String> {
        let id = self.sub_rev.remove(&(peer, orig.to_owned()))?;
        self.subs.remove(&id);
        Some(id)
    }

    fn drain_peer(&mut self, peer: Peer) -> (Vec<String>, Vec<String>) {
        fn drain(
            peer: Peer,
            forward: &mut HashMap<String, (Peer, String)>,
            reverse: &mut HashMap<(Peer, String), String>,
        ) -> Vec<String> {
            let mut ids = Vec::new();
            reverse.retain(|(owner, _), id| {
                if *owner == peer {
                    forward.remove(id);
                    ids.push(std::mem::take(id));
                    false
                } else {
                    true
                }
            });
            ids.sort_unstable();
            ids
        }
        (drain(peer, &mut self.calls, &mut self.call_rev), drain(peer, &mut self.subs, &mut self.sub_rev))
    }
}

static NEXT_PORT_TAG: AtomicU32 = AtomicU32::new(1);

/// The prompts owned by one registered endpoint. Both a port and a host that
/// executes a service directly use this path. The original arguments stay at the caller.
#[derive(Default)]
pub struct ServiceConfirmations {
    pending: Vec<(String, std::sync::Arc<u64>)>,
}

impl ServiceConfirmations {
    pub fn is_empty(&self) -> bool { self.pending.is_empty() }

    pub fn contains(&self, call_id: &str) -> bool {
        self.pending.iter().any(|(id, _)| id == call_id)
    }

    pub fn ask(&mut self, cx: &mut Cx, app: &str, call_id: String, tool: String, args: String) {
        use makepad_strict_json::{parse_depth, Value, TOOL_JSON_DEPTH};
        if self.contains(&call_id) { return }
        fn text(s: &str, json: bool) -> String {
            let mut out = String::with_capacity(s.len());
            for c in s.chars() {
                match c {
                    // Keep visible punctuation readable; escape controls and invisible direction marks.
                    '\'' | '"' => out.push(c),
                    '\\' if json => out.push(c), // the JSON writer already escaped it
                    _ => out.extend(c.escape_debug()),
                }
            }
            out
        }
        let lines = match parse_depth(args.as_bytes(), TOOL_JSON_DEPTH) {
            Ok(Value::Obj(fields)) => fields.into_iter().map(|(key, value)| {
                let value = match value {
                    Value::Str(s) => text(&s, false),
                    other => text(&other.to_json(), true),
                };
                format!("{}: {value}", text(&key, false))
            }).collect(),
            _ => vec![text(&args, false)],
        };
        let title = format!("A program outside {} asks to run {}", text(app, false), text(&tool, false));
        self.pending.push((call_id, cx.ask_confirmation(title, lines)));
    }

    pub fn withdraw(&mut self, cx: &mut Cx, call_id: &str) -> bool {
        let Some(index) = self.pending.iter().position(|(id, _)| id == call_id) else { return false };
        cx.withdraw_confirmation(*self.pending.swap_remove(index).1);
        true
    }

    pub fn answers(&mut self, cx: &mut Cx, mut send: impl FnMut(ServiceUp)) {
        self.pending.retain(|(id, ticket)| {
            if let Some(allow) = cx.take_confirmation_answer(**ticket) {
                send(ServiceUp::ConfirmAnswer { call_id: id.clone(), allow });
                false
            } else { true }
        });
    }

    pub fn withdraw_all(&mut self, cx: &mut Cx) {
        for (_, ticket) in self.pending.drain(..) {
            cx.withdraw_confirmation(*ticket);
        }
    }
}

impl Drop for ServiceConfirmations {
    fn drop(&mut self) {
        if !self.pending.is_empty() {
            self.pending.clear();
            SignalToUI::set_ui_signal();
        }
    }
}

/// One app's service and its attachments to the local registry and window manager.
pub struct AiServicePort {
    manifest: ServiceManifest,
    /// Each peer answers this nonce on its own channel.
    port_tag: u32,
    local: Option<LocalPeer>,
    host: Option<HostPeer>,
    last_context: RefCell<Option<ServiceContext>>,
    routes: RefCell<RouteTable>,
    chat_open: bool,
}

impl AiServicePort {
    fn new(mut manifest: ServiceManifest) -> Result<Self, String> {
        for problem in manifest.admit() {
            makepad_platform::error!("{problem}");
        }
        manifest.validate()?;
        Ok(Self {
            manifest,
            port_tag: NEXT_PORT_TAG.fetch_add(1, Ordering::Relaxed),
            local: None,
            host: None,
            last_context: Default::default(),
            routes: Default::default(),
            chat_open: false,
        })
    }

    /// Open only toward the host. A standalone process has no host transport.
    pub fn hosted(cx: &Cx, manifest: ServiceManifest) -> Option<AiServicePort> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(sender) = cx.hal_parent_sender() {
            return Self::with_host_transport(manifest, HostTransport::Domain(sender));
        }
        if !cx.in_makepad_studio() {
            return None;
        }
        Self::with_host_transport(manifest, HostTransport::Hosted)
    }

    fn with_host_transport(manifest: ServiceManifest, transport: HostTransport) -> Option<Self> {
        let mut port = Self::new(manifest).map_err(|e| {
            makepad_platform::error!("ai service manifest refused: {e}");
        }).ok()?;
        port.attach_host(transport);
        Some(port)
    }

    fn attach_host(&mut self, transport: HostTransport) {
        transport.send(self.registration());
        self.host = Some(HostPeer::new(transport));
    }

    /// Open the port in-process. The returned link goes to the embedding
    /// host's own `ServiceRegistry`, which issues the endpoint.
    pub fn in_process(manifest: ServiceManifest) -> Result<(AiServicePort, ServiceLink), String> {
        let mut port = Self::new(manifest)?;
        let (link, host) = ServiceLink::pair(port.manifest.clone());
        port.local = Some(LocalPeer {
            up: host.up,
            down: host.down,
            endpoint: None,
            confirmations: Default::default(),
        });
        port.register();
        Ok((port, link))
    }

    pub fn manifest(&self) -> &ServiceManifest {
        &self.manifest
    }

    /// The host's address when known, falling back to the local registry's address.
    pub fn endpoint(&self) -> Option<&EndpointId> {
        self.host.as_ref().and_then(|h| h.endpoint.as_ref())
            .or_else(|| self.local.as_ref().and_then(|l| l.endpoint.as_ref()))
    }

    /// Whether the host's chat pane is showing, as last told.
    pub fn chat_open(&self) -> bool {
        self.chat_open
    }

    fn is_dual(&self) -> bool {
        self.local.is_some() && self.host.is_some()
    }

    fn registration(&self) -> ServiceUp {
        ServiceUp::Register { manifest: self.manifest.clone(), port_tag: self.port_tag }
    }

    /// Announce the current manifest to each attached peer.
    pub fn register(&self) {
        self.send_all(self.registration());
    }

    /// Process the host event and drain the local registry's channel on every event.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<PortEvent> {
        let mut out = Vec::new();
        if let Some(host) = &mut self.host {
            match event {
                Event::HostAttached => {
                    host.endpoint = None;
                    host.logged_rejection = false;
                    host.confirmations.withdraw_all(cx);
                    self.drain_routes(Peer::Host, &mut out);
                    self.send_to(Peer::Host, self.registration());
                    if let Some(context) = self.last_context.get_mut().clone() {
                        self.send_to(Peer::Host, ServiceUp::Context(context));
                    }
                }
                Event::Custom(json) => match HostedDown::parse_checked(json) {
                    Some(Ok(frame)) => out.extend(self.accept_from(Peer::Host, frame, Some(cx))),
                    Some(Err(rejected)) => {
                        if !host.logged_rejection {
                            host.logged_rejection = true;
                            makepad_platform::error!("ai service rejected a frame from the window manager: {}", rejected.error);
                        }
                        if (rejected.mismatch || rejected.kind == RejectedKind::Call)
                            && host.endpoint.as_ref().is_some_and(|ep| {
                                Some(ep.as_str()) == rejected.endpoint.as_deref()
                            })
                        {
                            if let Some(id) = rejected.call_id.filter(|id| is_opaque_id(id)) {
                                let mut result = if rejected.mismatch {
                                    ToolResult::unavailable(id, HOSTED_BUILD_MISMATCH)
                                } else {
                                    ToolResult::failed(id, rejected.error)
                                };
                                result.bound();
                                host.transport.send(ServiceUp::Result(result));
                            }
                        }
                    }
                    None => {}
                },
                _ => {}
            }
            #[cfg(test)]
            while let Some(HostTransport::TestChannel { down, .. }) = self.host.as_ref().map(|h| &h.transport) {
                let Ok(frame) = down.try_recv() else { break };
                out.extend(self.accept_from(Peer::Host, frame, Some(cx)));
            }
        }
        let disconnected = self.drain_local(Some(cx), &mut out);
        if let Some(local) = &mut self.local {
            if !disconnected && !local.confirmations.is_empty() {
                local.confirmations.answers(cx, |msg| LocalPeer::send(&local.up, msg));
            }
        }
        if let Some(host) = &mut self.host {
            if !host.confirmations.is_empty() {
                host.confirmations.answers(cx, |msg| host.transport.send(msg));
            }
        }
        out
    }

    fn drain_local(&mut self, mut cx: Option<&mut Cx>, out: &mut Vec<PortEvent>) -> bool {
        while let Some(local) = &mut self.local {
            match local.down.try_recv() {
                Ok(frame) => out.extend(self.accept_from(Peer::Local, frame, cx.as_deref_mut())),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if let Some(cx) = cx {
                        local.confirmations.withdraw_all(cx);
                    }
                    // A disconnected channel stays disconnected. Retire its routes once.
                    if self.host.is_some() && local.endpoint.take().is_some() {
                        self.drain_routes(Peer::Local, out);
                    }
                    return true;
                }
            }
        }
        false
    }

    fn drain_routes(&mut self, peer: Peer, out: &mut Vec<PortEvent>) {
        if self.is_dual() {
            let (calls, subs) = self.routes.get_mut().drain_peer(peer);
            out.extend(calls.into_iter().map(|call_id| PortEvent::Cancel { call_id }));
            out.extend(subs.into_iter().map(|sub_id| PortEvent::Unsubscribe { sub_id }));
        }
    }

    fn accept_from(&mut self, peer: Peer, frame: HostedDown, cx: Option<&mut Cx>) -> Option<PortEvent> {
        frame.msg.validate().ok()?;
        let dual = self.is_dual();
        let (endpoint, confirmations) = match peer {
            Peer::Host => {
                let host = self.host.as_mut()?;
                (&mut host.endpoint, &mut host.confirmations)
            }
            Peer::Local => {
                let local = self.local.as_mut()?;
                (&mut local.endpoint, &mut local.confirmations)
            }
        };
        if let ServiceDown::Registered { port_tag, endpoint: assigned } = frame.msg {
            if port_tag != self.port_tag {
                return None;
            }
            *endpoint = Some(assigned.clone());
            return Some(PortEvent::Registered(assigned));
        }
        if frame.to.as_ref()? != endpoint.as_ref()? {
            return None;
        }
        Some(match frame.msg {
            ServiceDown::Registered { .. } => unreachable!(),
            ServiceDown::Call(mut call) => {
                if dual {
                    call.call_id = self.routes.get_mut().insert_call(peer, call.call_id);
                }
                PortEvent::Call(call)
            }
            ServiceDown::Confirm { call_id, tool, args } => {
                confirmations.ask(cx?, &self.manifest.label, call_id, tool, args);
                return None;
            }
            ServiceDown::Cancel { mut call_id } => {
                if cx.is_some_and(|cx| confirmations.withdraw(cx, &call_id)) {
                    return None;
                }
                if dual {
                    call_id = self.routes.get_mut().remove_peer_call(peer, &call_id)?;
                }
                PortEvent::Cancel { call_id }
            }
            ServiceDown::Subscribe { mut sub_id, topic, filter } => {
                if dual {
                    sub_id = self.routes.get_mut().insert_sub(peer, sub_id);
                }
                PortEvent::Subscribe { sub_id, topic, filter }
            }
            ServiceDown::Unsubscribe { mut sub_id } => {
                if dual {
                    sub_id = self.routes.get_mut().remove_peer_sub(peer, &sub_id)?;
                }
                PortEvent::Unsubscribe { sub_id }
            }
            ServiceDown::ChatOpen { open } => {
                self.chat_open = open;
                PortEvent::ChatOpen { open }
            }
        })
    }

    /// Answer a call. Bounded here so an app cannot flood the model.
    pub fn reply(&self, mut result: ToolResult) {
        if self.is_dual() {
            let Some((peer, orig)) = self.routes.borrow_mut().take_call(&result.call_id) else { return };
            result.call_id = orig;
            result.bound();
            self.send_to(peer, ServiceUp::Result(result));
        } else {
            result.bound();
            self.send_all(ServiceUp::Result(result));
        }
    }

    /// Report real work (for example, a completed batch) and show where it is.
    /// Silence marks the call "not responding"; it never cancels it. Long
    /// computations should report here at their existing worker boundaries.
    pub fn progress(&self, call_id: &str, note: &str, permille: u16) {
        self.progress_task(call_id, note, permille, None);
    }

    /// [`Self::progress`] with the call's task card now: the chat draws the card (input
    /// picture, bar, stage, where it runs, Stop) in place of the note.
    pub fn progress_task(&self, call_id: &str, note: &str, permille: u16, mut task: Option<TaskCard>) {
        let mut note = note.to_string();
        if note.len() > MAX_NOTE_BYTES {
            truncate_to_char_boundary(&mut note, MAX_NOTE_BYTES - 3);
            note.push('…');
        }
        if let Some(task) = &mut task {
            task.bound();
        }
        let frame = |call_id| ServiceUp::Progress { call_id, note, permille: permille.min(1000), task };
        if self.is_dual() {
            let Some((peer, orig)) = self.routes.borrow().peek_call(call_id) else { return };
            self.send_to(peer, frame(orig));
        } else {
            self.send_all(frame(call_id.to_owned()));
        }
    }

    /// The app's volatile state for the model's next turn. Replaces the
    /// previous text; empty clears it.
    pub fn set_context(&self, text: &str) {
        let mut text = text.to_string();
        if text.len() > MAX_CONTEXT_BYTES {
            truncate_to_char_boundary(&mut text, MAX_CONTEXT_BYTES - 3);
            text.push('…');
        }
        let context = ServiceContext { text };
        if self.host.is_some() {
            *self.last_context.borrow_mut() = Some(context.clone());
        }
        self.send_all(ServiceUp::Context(context));
    }

    /// Publish one message to a subscription the host asked this service
    /// to maintain. Text is bounded and oversized structured data is
    /// dropped whole before it leaves the app.
    pub fn publish(&self, sub_id: impl Into<String>, mut message: Message) {
        message.bound();
        let sub_id = sub_id.into();
        let frame = |sub_id| ServiceUp::Message {
            sub_id,
            topic: message.topic,
            text: message.text,
            data: message.data,
            final_: message.final_,
        };
        if self.is_dual() {
            let route = if message.final_ {
                self.routes.borrow_mut().take_sub(&sub_id)
            } else {
                self.routes.borrow().peek_sub(&sub_id)
            };
            let Some((peer, orig)) = route else { return };
            self.send_to(peer, frame(orig));
        } else {
            self.send_all(frame(sub_id));
        }
    }

    /// Leave on purpose. The host also forgets a service whose process
    /// dies, so most apps never call this.
    pub fn unregister(&self) {
        self.send_all(ServiceUp::Unregister);
    }

    #[cfg(test)]
    fn drain_host_test(&mut self, mut cx: Option<&mut Cx>, out: &mut Vec<PortEvent>) {
        while let Some(HostTransport::TestChannel { down, .. }) = self.host.as_ref().map(|h| &h.transport) {
            let Ok(frame) = down.try_recv() else { break };
            out.extend(self.accept_from(Peer::Host, frame, cx.as_deref_mut()));
        }
    }

    #[cfg(test)]
    pub(crate) fn test_drain(&mut self) -> Vec<PortEvent> {
        let mut out = Vec::new();
        self.drain_host_test(None, &mut out);
        self.drain_local(None, &mut out);
        out
    }

    fn send_to(&self, peer: Peer, msg: ServiceUp) {
        match peer {
            Peer::Local => {
                if let Some(local) = &self.local {
                    LocalPeer::send(&local.up, msg);
                }
            }
            Peer::Host => {
                if let Some(host) = &self.host {
                    host.transport.send(msg);
                }
            }
        }
    }

    fn send_all(&self, msg: ServiceUp) {
        if let Some(host) = &self.host {
            if let Some(local) = &self.local {
                LocalPeer::send(&local.up, msg.clone());
            }
            host.transport.send(msg);
        } else if let Some(local) = &self.local {
            LocalPeer::send(&local.up, msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> ServiceManifest {
        ServiceManifest::new("files", "Files", "The file browser.").with_tool(ToolDef::new(
            "stat",
            "One path's kind and size.",
            r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
            Risk::Read,
        ))
        .with_topic(TopicDef::new("watch", "Changes to a watched path."))
    }

    fn ep(s: &str) -> EndpointId {
        EndpointId(s.to_string())
    }

    /// Drain the in-process channel the way `handle_event` does, without a Cx.
    fn pump(port: &mut AiServicePort) -> Vec<PortEvent> {
        port.test_drain()
    }

    #[test]
    fn a_port_registers_learns_its_endpoint_and_then_takes_only_its_own_frames() {
        let (mut port, link) = AiServicePort::in_process(files()).unwrap();
        // The registration went up, unstamped, with the port's nonce.
        let reg = link.up.try_recv().unwrap();
        assert_eq!(reg.from, None);
        let tag = match reg.msg {
            ServiceUp::Register { port_tag, ref manifest } => {
                assert_eq!(manifest.id, "files");
                port_tag
            }
            other => panic!("{other:?}"),
        };
        assert!(port.endpoint().is_none());
        // A call before registration, and one for someone else, are ignored.
        link.down.send(HostedDown { to: Some(ep("other")), msg: ServiceDown::Call(ServiceCall { call_id: "c0".into(), tool: "stat".into(), args: "{}".into(), caller: None, origin: CallOrigin::Conversation }) }).unwrap();
        link.down.send(HostedDown { to: None, msg: ServiceDown::Registered { port_tag: tag + 100, endpoint: ep("not-me") } }).unwrap();
        link.down.send(HostedDown { to: None, msg: ServiceDown::Registered { port_tag: tag, endpoint: ep("e1") } }).unwrap();
        link.down.send(HostedDown { to: Some(ep("e1")), msg: ServiceDown::Call(ServiceCall { call_id: "c1".into(), tool: "stat".into(), args: "{}".into(), caller: None, origin: CallOrigin::Conversation }) }).unwrap();
        link.down.send(HostedDown { to: Some(ep("e2")), msg: ServiceDown::Call(ServiceCall { call_id: "c2".into(), tool: "stat".into(), args: "{}".into(), caller: None, origin: CallOrigin::Conversation }) }).unwrap();
        link.down.send(HostedDown { to: Some(ep("e1")), msg: ServiceDown::ChatOpen { open: true } }).unwrap();
        let events = pump(&mut port);
        assert_eq!(events.len(), 3, "{events:?}");
        assert_eq!(events[0], PortEvent::Registered(ep("e1")));
        assert!(matches!(&events[1], PortEvent::Call(c) if c.call_id == "c1"));
        assert_eq!(events[2], PortEvent::ChatOpen { open: true });
        assert_eq!(port.endpoint(), Some(&ep("e1")));
        assert!(port.chat_open());
        port.reply(ToolResult::ok("c1", "a file, 12 bytes", "stat ~/x"));
        port.set_context("[files] cwd=~");
        let up: Vec<HostedUp> = link.up.try_iter().collect();
        assert!(matches!(&up[0].msg, ServiceUp::Result(r) if r.call_id == "c1" && r.outcome.is_ok()));
        assert!(matches!(&up[1].msg, ServiceUp::Context(c) if c.text == "[files] cwd=~"));
        assert!(up.iter().all(|f| f.from.is_none()), "a port never stamps itself");
    }

    #[test]
    fn two_ports_in_one_process_get_distinct_tags() {
        let (a, la) = AiServicePort::in_process(files()).unwrap();
        let (b, lb) = AiServicePort::in_process(files()).unwrap();
        let ta = match la.up.try_recv().unwrap().msg { ServiceUp::Register { port_tag, .. } => port_tag, _ => unreachable!() };
        let tb = match lb.up.try_recv().unwrap().msg { ServiceUp::Register { port_tag, .. } => port_tag, _ => unreachable!() };
        assert_ne!(ta, tb);
        assert_ne!(a.port_tag, b.port_tag);
    }

    #[test]
    fn a_bad_tool_is_left_out_and_a_bad_service_is_refused() {
        let with_bad_tool = files().with_tool(ToolDef::new("Stat", "Not an identifier.", r#"{"type":"object"}"#, Risk::Read));
        let (_port, link) = AiServicePort::in_process(with_bad_tool).unwrap();
        match link.up.try_recv().unwrap().msg {
            ServiceUp::Register { manifest, .. } => assert_eq!(manifest, files(), "the good tools are served, the bad one is left out"),
            other => panic!("{other:?}"),
        }
        let mut bad_service = files();
        bad_service.id = "Files".into();
        assert!(AiServicePort::in_process(bad_service).is_err());
    }

    #[test]
    fn a_reply_is_bounded_on_the_way_out() {
        let (port, link) = AiServicePort::in_process(files()).unwrap();
        let _reg = link.up.try_recv().unwrap();
        let reply = |r| {
            port.reply(r);
            let ServiceUp::Result(r) = link.up.try_recv().expect("the reply reaches the link").msg else {
                panic!("expected a tool result on the link");
            };
            r
        };
        let text = "x".repeat(32 * 1024);
        let r = reply(ToolResult::ok("c1", &text, "n"));
        assert_eq!(r.outcome, ToolOutcome::Ok, "a 32 KiB reply succeeds");
        assert!(r.text == text, "the port delivers the 32 KiB reply intact");

        let oversized = MAX_RESULT_WIRE_BYTES + 1;
        let r = reply(ToolResult::ok("c2", "x".repeat(oversized), ""));
        assert_eq!(r.outcome, ToolOutcome::Failed, "the port fails a reply over the raw wire cap");
        assert_eq!(r.text, result_too_large(oversized), "the port reports the original byte length");
        assert!(ServiceUp::Result(r).validate().is_ok(), "the receiver accepts the raw-size failure");

        let heavy = ToolResult::ok("c3", "\x00".repeat(700 * 1024), "");
        let bytes = HostedUp { from: None, msg: ServiceUp::Result(heavy.clone()) }.to_json().len();
        let r = reply(heavy);
        assert_eq!(r.outcome, ToolOutcome::Failed, "the port fails a reply over the encoded cap");
        assert_eq!(r.text, result_frame_too_large(bytes), "the failure reports the encoded byte length");
        let frame = HostedUp { from: Some(EndpointId("\"".repeat(MAX_ID_BYTES))), msg: ServiceUp::Result(r) };
        assert!(HostedUp::parse(&frame.to_json()) == Some(frame), "the worst-case stamped failure round-trips");
    }

    #[test]
    fn subscriptions_reach_the_port_and_publications_go_back_up() {
        let (mut port, link) = AiServicePort::in_process(files()).unwrap();
        let register = link.up.try_recv().unwrap();
        let tag = match register.msg {
            ServiceUp::Register { port_tag, .. } => port_tag,
            other => panic!("{other:?}"),
        };
        link.down
            .send(HostedDown {
                to: None,
                msg: ServiceDown::Registered { port_tag: tag, endpoint: ep("e1") },
            })
            .unwrap();
        link.down
            .send(HostedDown {
                to: Some(ep("e1")),
                msg: ServiceDown::Subscribe {
                    sub_id: "s1".into(),
                    topic: "watch".into(),
                    filter: Some(r#"{"path":"/tmp"}"#.into()),
                },
            })
            .unwrap();
        link.down
            .send(HostedDown {
                to: Some(ep("e1")),
                msg: ServiceDown::Unsubscribe { sub_id: "s1".into() },
            })
            .unwrap();
        let events = pump(&mut port);
        assert!(matches!(
            events.as_slice(),
            [
                PortEvent::Registered(_),
                PortEvent::Subscribe { sub_id, topic, filter: Some(filter) },
                PortEvent::Unsubscribe { sub_id: unsubscribed }
            ] if sub_id == "s1" && topic == "watch" && filter.contains("path") && unsubscribed == "s1"
        ));
        port.publish("s1", Message::new("watch", "changed").with_data(r#"{"path":"/tmp/a"}"#));
        assert!(matches!(
            link.up.try_recv().unwrap().msg,
            ServiceUp::Message { sub_id, topic, text, .. }
                if sub_id == "s1" && topic == "watch" && text == "changed"
        ));
    }
}

// ------------------------------------------------------------- open

impl AiServicePort {
    /// Register in the process's registry and also with its window manager when hosted.
    /// Embedded HAL domains share the parent's process and attach only to their host.
    pub fn open(cx: &mut Cx, manifest: ServiceManifest) -> Option<AiServicePort> {
        #[cfg(not(target_arch = "wasm32"))]
        if cx.hal_is_embedded() {
            return Self::hosted(cx, manifest);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(sender) = cx.hal_parent_sender() {
            return Self::open_with_host(Some(HostTransport::Domain(sender)), &crate::engine::ServiceRegistry::process(), manifest);
        }
        Self::open_in(cx.in_makepad_studio(), &crate::engine::ServiceRegistry::process(), manifest)
    }

    /// Register in `registry`, also attaching the studio transport when hosted.
    pub fn open_in(hosted: bool, registry: &crate::engine::ServiceRegistry, manifest: ServiceManifest) -> Option<AiServicePort> {
        Self::open_with_host(hosted.then_some(HostTransport::Hosted), registry, manifest)
    }

    fn open_with_host(
        host_transport: Option<HostTransport>,
        registry: &crate::engine::ServiceRegistry,
        manifest: ServiceManifest,
    ) -> Option<Self> {
        // in_process validates and queues Register before the registry receives the link.
        let (mut port, link) = Self::in_process(manifest).map_err(|e| {
            makepad_platform::error!("ai service refused: {e}");
        }).ok()?;
        if let Err(e) = registry.register(link, "in this window", None) {
            makepad_platform::error!("ai service refused: {e}");
            return None;
        }
        if let Some(transport) = host_transport {
            port.attach_host(transport);
        }
        Some(port)
    }
}

#[cfg(test)]
mod open_tests {
    use super::*;
    use crate::engine::{RegistryUp, ServiceRegistry};

    fn sheets() -> ServiceManifest {
        ServiceManifest::new("sheets", "Sheets", "The spreadsheet.").with_tool(ToolDef::new(
            "summary",
            "The sheet's name, size and header row.",
            r#"{"type":"object","properties":{}}"#,
            Risk::Read,
        ))
    }

    #[test]
    fn a_standalone_open_is_registered_and_addressed_at_once() {
        let registry = ServiceRegistry::new();
        let mut port = AiServicePort::open_in(false, &registry, sheets()).expect("a valid manifest opens");
        assert_eq!(registry.services().len(), 1);
        assert_eq!(registry.services()[0].id, "sheets");
        let events = port.test_drain();
        assert!(matches!(events.as_slice(), [PortEvent::Registered(_)]), "{events:?}");
    }

    #[test]
    fn a_bad_manifest_opens_nothing_either_way() {
        let mut bad = sheets();
        bad.id = "Sheets".into();
        let registry = ServiceRegistry::new();
        assert!(AiServicePort::open_in(false, &registry, bad.clone()).is_none());
        assert!(registry.services().is_empty());
        assert!(AiServicePort::open_in(true, &registry, bad).is_none());
    }

    #[test]
    fn a_call_dispatched_before_the_first_pump_still_reaches_the_port() {
        // The port announced itself at open; the registry answers that Register at
        // registration, so a call sent before any pump lands AFTER the port learned its
        // address — Registered, then Call.
        let registry = ServiceRegistry::new();
        let mut port = AiServicePort::open_in(false, &registry, sheets()).expect("opens");
        let endpoint = registry.pick_instance("sheets", None).unwrap();
        let call = ServiceCall { call_id: "c1".into(), tool: "summary".into(), args: "{}".into(), caller: None, origin: CallOrigin::Conversation };
        assert!(registry.send(&endpoint, ServiceDown::Call(call)));
        let events = port.test_drain();
        assert!(
            matches!(events.as_slice(), [PortEvent::Registered(_), PortEvent::Call(c)] if c.call_id == "c1"),
            "{events:?}"
        );
    }

    struct DualPort {
        cx: Cx,
        port: AiServicePort,
        registry: ServiceRegistry,
        local_id: EndpointId,
        host: ServiceLink,
        host_id: EndpointId,
    }

    impl DualPort {
        fn new() -> Self {
            let manifest = sheets().with_topic(TopicDef::new("watch", "Sheet changes."));
            let registry = ServiceRegistry::new();
            let (mut port, local) = AiServicePort::in_process(manifest.clone()).unwrap();
            let local_id = EndpointId("l0".into());
            registry.register_as(local, local_id.clone(), "in this window", None).unwrap();
            registry.pump();

            let (host, transport) = ServiceLink::pair(manifest);
            port.attach_host(HostTransport::TestChannel { up: transport.up, down: transport.down });
            let ServiceUp::Register { port_tag, .. } = host.up.try_recv().unwrap().msg else {
                panic!("host did not receive registration");
            };
            let mut cx = Cx::new(Box::new(|_, _| {}));
            assert_eq!(port.handle_event(&mut cx, &Event::Signal), [PortEvent::Registered(local_id.clone())]);
            assert_eq!(port.endpoint(), Some(&local_id));
            let host_id = EndpointId("w7".into());
            host.down.send(HostedDown {
                to: None,
                msg: ServiceDown::Registered { port_tag, endpoint: host_id.clone() },
            }).unwrap();
            assert_eq!(port.handle_event(&mut cx, &Event::Signal), [PortEvent::Registered(host_id.clone())]);
            assert_eq!(port.endpoint(), Some(&host_id));
            Self { cx, port, registry, local_id, host, host_id }
        }

        fn deliver(&mut self, peer: Peer, msg: ServiceDown) -> Vec<PortEvent> {
            match peer {
                Peer::Local => assert!(self.registry.send(&self.local_id, msg)),
                Peer::Host => self.host.down.send(HostedDown { to: Some(self.host_id.clone()), msg }).unwrap(),
            }
            self.port.handle_event(&mut self.cx, &Event::Signal)
        }

        fn call(&mut self, peer: Peer) -> String {
            let events = self.deliver(peer, ServiceDown::Call(ServiceCall {
                call_id: "app1".into(), tool: "summary".into(), args: "{}".into(),
                caller: None, origin: CallOrigin::Remote,
            }));
            let [PortEvent::Call(call)] = events.as_slice() else { panic!("{events:?}") };
            call.call_id.clone()
        }

        fn host_messages(&self) -> Vec<ServiceUp> {
            self.host.up.try_iter().map(|f| f.msg).collect()
        }

        fn subscribe(&mut self, peer: Peer) -> String {
            let events = self.deliver(peer, ServiceDown::Subscribe {
                sub_id: "s1".into(), topic: "watch".into(), filter: None,
            });
            let [PortEvent::Subscribe { sub_id, .. }] = events.as_slice() else { panic!("{events:?}") };
            sub_id.clone()
        }

        // Observe even frames the registry consumes internally (Register, Context, ConfirmAnswer).
        fn observe_local_during(&mut self, event: &Event) -> (Vec<PortEvent>, Vec<ServiceUp>) {
            let (record, frames) = channel();
            let up = std::mem::replace(&mut self.port.local.as_mut().unwrap().up, record);
            let events = self.port.handle_event(&mut self.cx, event);
            let mut messages = Vec::new();
            for frame in frames.try_iter() {
                messages.push(frame.msg.clone());
                up.send(frame).unwrap();
            }
            self.port.local.as_mut().unwrap().up = up;
            (events, messages)
        }
    }

    struct HeadlessDualPort {
        port: AiServicePort,
        registry: ServiceRegistry,
        local_id: EndpointId,
        host: ServiceLink,
        host_id: EndpointId,
    }

    impl HeadlessDualPort {
        fn new() -> Self {
            let manifest = sheets().with_topic(TopicDef::new("watch", "Sheet changes."));
            let registry = ServiceRegistry::new();
            let (mut port, local) = AiServicePort::in_process(manifest.clone()).unwrap();
            let local_id = EndpointId("l0".into());
            registry.register_as(local, local_id.clone(), "in this window", None).unwrap();
            registry.pump();

            let (host, transport) = ServiceLink::pair(manifest);
            port.attach_host(HostTransport::TestChannel { up: transport.up, down: transport.down });
            let ServiceUp::Register { port_tag, .. } = host.up.try_recv().unwrap().msg else {
                panic!("host did not receive registration");
            };
            assert_eq!(port.test_drain(), [PortEvent::Registered(local_id.clone())]);
            assert_eq!(port.endpoint(), Some(&local_id));
            let host_id = EndpointId("w7".into());
            host.down.send(HostedDown {
                to: None,
                msg: ServiceDown::Registered { port_tag, endpoint: host_id.clone() },
            }).unwrap();
            assert_eq!(port.test_drain(), [PortEvent::Registered(host_id.clone())]);
            assert_eq!(port.endpoint(), Some(&host_id));
            Self { port, registry, local_id, host, host_id }
        }

        fn deliver(&mut self, peer: Peer, msg: ServiceDown) -> Vec<PortEvent> {
            match peer {
                Peer::Local => assert!(self.registry.send(&self.local_id, msg)),
                Peer::Host => self.host.down.send(HostedDown { to: Some(self.host_id.clone()), msg }).unwrap(),
            }
            self.port.test_drain()
        }

        fn host_messages(&self) -> Vec<ServiceUp> {
            self.host.up.try_iter().map(|f| f.msg).collect()
        }
    }

    #[test]
    fn dual_peer_routes_colliding_call_ids_to_originating_peer_only() {
        let mut p = DualPort::new();
        let local = p.call(Peer::Local);
        let host = p.call(Peer::Host);
        assert_eq!((local.as_str(), host.as_str()), ("pl_1", "ph_2"));
        p.port.progress(&local, "loading", 250);
        p.port.reply(ToolResult::ok(&local, "local", "done"));
        assert!(matches!(p.registry.pump().as_slice(), [
            RegistryUp::Progress { call_id, note, permille: 250, .. }, RegistryUp::Result(_, r)
        ] if call_id == "app1" && note == "loading" && r == &ToolResult::ok("app1", "local", "done")));
        assert!(p.host_messages().is_empty());

        p.port.reply(ToolResult::ok(&host, "host", "done"));
        assert_eq!(p.host_messages(), [ServiceUp::Result(ToolResult::ok("app1", "host", "done"))]);
        p.port.reply(ToolResult::ok(&local, "duplicate", "done"));
        assert!(p.registry.pump().is_empty());
        assert!(p.host_messages().is_empty());
    }

    #[test]
    fn dual_peer_reply_bounds_against_restored_call_id_and_worst_case_stamp() {
        let mut p = HeadlessDualPort::new();
        let orig_id = "\"".repeat(MAX_ID_BYTES);
        let worst_ep = EndpointId(orig_id.clone());
        let deliver_host_call = |p: &mut HeadlessDualPort| {
            let events = p.deliver(Peer::Host, ServiceDown::Call(ServiceCall {
                call_id: orig_id.clone(), tool: "summary".into(), args: "{}".into(),
                caller: None, origin: CallOrigin::Remote,
            }));
            let [PortEvent::Call(call)] = events.as_slice() else { panic!("expected one host call: {events:?}") };
            call.call_id.clone()
        };
        let host_reply = |p: &HeadlessDualPort, app_id: String, text: String| {
            p.port.reply(ToolResult::ok(app_id, text, ""));
            let mut messages = p.host_messages().into_iter();
            let Some(ServiceUp::Result(r)) = messages.next() else { panic!("expected a host result") };
            assert!(messages.next().is_none(), "the host receives exactly one reply");
            assert_eq!(r.call_id, orig_id, "the reply carries the peer's original call ID");
            r
        };
        let stamped_bytes = |result| {
            let frame = HostedUp { from: Some(worst_ep.clone()), msg: ServiceUp::Result(result) };
            let json = frame.to_json();
            assert!(json.len() <= MAX_FRAME_BYTES, "the reply with the worst-case stamp fits the frame cap");
            assert!(HostedUp::parse(&json) == Some(frame), "the worst-case stamped reply round-trips");
            json.len()
        };

        let app_id = deliver_host_call(&mut p);
        assert_eq!(app_id, "ph_1", "the first host call has the short synthetic ID");
        let r = host_reply(&p, app_id, format!("{}xxx", "\x00".repeat(698_984)));
        assert_eq!(r.outcome, ToolOutcome::Failed, "the port checks the size after restoring the ID");
        assert_eq!(
            r.text,
            result_frame_too_large(MAX_UNSTAMPED_FRAME_BYTES + 39),
            "the failure reports the actual unstamped size after restoring the ID",
        );
        assert!(ServiceUp::Result(r.clone()).validate().is_ok(), "the receiver accepts the restored-ID failure");
        stamped_bytes(r);

        let app_id = deliver_host_call(&mut p);
        let text = "\x00".repeat(698_978);
        let r = host_reply(&p, app_id, text.clone());
        assert_eq!(r.outcome, ToolOutcome::Ok, "the reply exactly at the ceiling succeeds");
        assert!(r.text == text, "the exact-ceiling reply retains every text byte");
        assert_eq!(stamped_bytes(r), MAX_FRAME_BYTES, "the worst-case stamp fills the frame exactly");

        let app_id = deliver_host_call(&mut p);
        let r = host_reply(&p, app_id, text + "x");
        assert_eq!(r.outcome, ToolOutcome::Failed, "one extra printable byte fails cleanly");
        assert_eq!(r.text, result_frame_too_large(MAX_UNSTAMPED_FRAME_BYTES + 1), "one byte over reports the exact size");
        stamped_bytes(r);
    }

    #[test]
    fn dual_peer_cancel_only_cancels_the_sending_peers_call() {
        let mut p = DualPort::new();
        let local = p.call(Peer::Local);
        let host = p.call(Peer::Host);
        let cancel = ServiceDown::Cancel { call_id: "app1".into() };
        assert_eq!(p.deliver(Peer::Host, cancel.clone()), [PortEvent::Cancel { call_id: host.clone() }]);
        assert!(p.deliver(Peer::Host, cancel).is_empty());
        p.port.reply(ToolResult::ok(host, "stale", "done"));
        p.port.reply(ToolResult::ok(local, "local", "done"));
        assert_eq!(p.registry.pump(), [RegistryUp::Result(p.local_id.clone(), ToolResult::ok("app1", "local", "done"))]);
        assert!(p.host_messages().is_empty());
    }

    #[test]
    fn dual_peer_subscriptions_isolate_colliding_sub_ids() {
        let mut p = DualPort::new();
        let local = p.subscribe(Peer::Local);
        let host = p.subscribe(Peer::Host);
        assert_ne!(local, host);
        p.port.publish(&local, Message::new("watch", "local"));
        p.port.publish(&host, Message::new("watch", "host"));
        assert_eq!(p.registry.pump(), [RegistryUp::Message {
            endpoint: p.local_id.clone(), sub_id: "s1".into(), message: Message::new("watch", "local"),
        }]);
        assert!(matches!(p.host_messages().as_slice(),
            [ServiceUp::Message { sub_id, text, .. }] if sub_id == "s1" && text == "host"));
        let unsubscribe = ServiceDown::Unsubscribe { sub_id: "s1".into() };
        assert_eq!(p.deliver(Peer::Local, unsubscribe), [PortEvent::Unsubscribe { sub_id: local.clone() }]);
        p.port.publish(local, Message::new("watch", "late"));
        p.port.publish(host, Message::new("watch", "again"));
        assert!(p.registry.pump().is_empty());
        assert!(matches!(p.host_messages().as_slice(),
            [ServiceUp::Message { sub_id, text, .. }] if sub_id == "s1" && text == "again"));
    }

    #[test]
    fn dual_peer_final_publication_retires_subscription_route() {
        let mut p = DualPort::new();
        let host = p.subscribe(Peer::Host);
        let local = p.subscribe(Peer::Local);
        p.port.publish(&host, Message::new("watch", "done").final_message());
        assert!(matches!(p.host_messages().as_slice(),
            [ServiceUp::Message { sub_id, text, final_: true, .. }] if sub_id == "s1" && text == "done"));
        p.port.publish(&host, Message::new("watch", "late"));
        assert!(p.host_messages().is_empty());
        assert!(p.registry.pump().is_empty());

        let events = p.port.handle_event(&mut p.cx, &Event::HostAttached);
        assert!(events.is_empty(), "reattaching Host should not unsubscribe a finished route: {events:?}");
        p.port.publish(&local, Message::new("watch", "still subscribed"));
        assert_eq!(p.registry.pump(), [RegistryUp::Message {
            endpoint: p.local_id.clone(), sub_id: "s1".into(), message: Message::new("watch", "still subscribed"),
        }]);
        assert_eq!(p.host_messages(), [p.port.registration()]);
    }

    #[test]
    fn dual_peer_host_attached_resets_only_host_and_keeps_local_alive() {
        let mut p = DualPort::new();
        p.port.set_context("v1");
        p.registry.pump();
        assert!(p.registry.dynamic_context().contains("v1"));
        let context = ServiceUp::Context(ServiceContext { text: "v1".into() });
        assert_eq!(p.host_messages(), [context.clone()]);
        let local = p.call(Peer::Local);
        let host = p.call(Peer::Host);
        let local_sub = p.subscribe(Peer::Local);
        let host_sub = p.subscribe(Peer::Host);

        let (events, local_frames) = p.observe_local_during(&Event::HostAttached);
        assert_eq!(events, [PortEvent::Cancel { call_id: host }, PortEvent::Unsubscribe { sub_id: host_sub }]);
        assert!(local_frames.is_empty(), "a host reattachment must send nothing to local");
        assert_eq!(p.host_messages(), [p.port.registration(), context]);
        assert_eq!(p.port.endpoint(), Some(&p.local_id));
        p.port.reply(ToolResult::ok(local, "local", "done"));
        p.port.publish(&local_sub, Message::new("watch", "changed"));
        assert_eq!(p.registry.pump(), [
            RegistryUp::Result(p.local_id.clone(), ToolResult::ok("app1", "local", "done")),
            RegistryUp::Message { endpoint: p.local_id.clone(), sub_id: "s1".into(), message: Message::new("watch", "changed") },
        ]);
        assert!(p.host_messages().is_empty());

        // After the host comes back, losing local must retire only the local routes.
        let registered = ServiceDown::Registered { port_tag: p.port.port_tag, endpoint: p.host_id.clone() };
        assert_eq!(p.deliver(Peer::Host, registered), [PortEvent::Registered(p.host_id.clone())]);
        let host = p.call(Peer::Host);
        let local = p.call(Peer::Local);
        p.registry.unregister(&p.local_id);
        assert_eq!(p.port.handle_event(&mut p.cx, &Event::Signal),
            [PortEvent::Cancel { call_id: local }, PortEvent::Unsubscribe { sub_id: local_sub }]);
        assert!(p.port.handle_event(&mut p.cx, &Event::Signal).is_empty());
        p.port.reply(ToolResult::ok(host, "host", "done"));
        assert_eq!(p.host_messages(), [ServiceUp::Result(ToolResult::ok("app1", "host", "done"))]);
    }

    #[test]
    fn dual_peer_confirmations_are_isolated_per_peer() {
        let mut p = DualPort::new();
        let confirm = ServiceDown::Confirm { call_id: "app1".into(), tool: "summary".into(), args: "{}".into() };
        assert!(p.deliver(Peer::Local, confirm.clone()).is_empty());
        let head = p.cx.confirmation_head();
        let local = head.get().expect("local prompt");
        assert!(p.deliver(Peer::Host, confirm.clone()).is_empty());
        assert!(p.cx.answer_confirmation(local, true));
        let host = head.get().expect("host prompt with the same call id is still queued");
        assert_ne!(local, host);
        let (events, local_frames) = p.observe_local_during(&Event::Signal);
        assert!(events.is_empty());
        assert_eq!(local_frames, [ServiceUp::ConfirmAnswer { call_id: "app1".into(), allow: true }]);
        assert!(p.host_messages().is_empty());
        assert_eq!(head.get(), Some(host));

        assert!(p.deliver(Peer::Local, confirm).is_empty());
        assert!(p.deliver(Peer::Host, ServiceDown::Cancel { call_id: "app1".into() }).is_empty());
        let remaining = head.get().expect("withdrawing host keeps the next local prompt");
        assert_ne!(remaining, host);
        assert!(p.cx.answer_confirmation(remaining, false));
        let (_, local_frames) = p.observe_local_during(&Event::Signal);
        assert_eq!(local_frames, [ServiceUp::ConfirmAnswer { call_id: "app1".into(), allow: false }]);
        assert_eq!(head.get(), None);
        assert!(p.host_messages().is_empty());
    }

    #[test]
    fn dual_peer_confirmations_survive_the_other_peers_disconnect() {
        let mut p = DualPort::new();
        let head = p.cx.confirmation_head();
        let confirm = ServiceDown::Confirm { call_id: "app1".into(), tool: "summary".into(), args: "{}".into() };
        p.deliver(Peer::Host, confirm.clone());
        let host = head.get().expect("Host prompt");
        p.deliver(Peer::Local, confirm.clone());
        p.port.handle_event(&mut p.cx, &Event::HostAttached);
        let local = head.get().expect("Local prompt survives Host reattachment");
        assert_ne!(local, host, "reattachment must withdraw the Host prompt at the front");

        assert_eq!(p.host_messages(), [p.port.registration()]);
        let registered = ServiceDown::Registered { port_tag: p.port.port_tag, endpoint: p.host_id.clone() };
        assert_eq!(p.deliver(Peer::Host, registered), [PortEvent::Registered(p.host_id.clone())]);
        p.deliver(Peer::Host, confirm);
        p.registry.unregister(&p.local_id);
        p.port.handle_event(&mut p.cx, &Event::Signal);
        let host = head.get().expect("Host prompt survives Local disconnect");
        assert_ne!(host, local, "disconnect must withdraw the Local prompt at the front");

        assert!(p.cx.answer_confirmation(host, true));
        let (_, local_frames) = p.observe_local_during(&Event::Signal);
        assert!(local_frames.is_empty(), "a Host confirmation answer must go only to Host");
        assert_eq!(p.host_messages(), [ServiceUp::ConfirmAnswer { call_id: "app1".into(), allow: true }]);
        assert_eq!(head.get(), None);
    }

    #[test]
    fn dual_peer_hosted_build_mismatch_replies_on_host_only() {
        let mut p = DualPort::new();
        // An older Call predates the required origin field.
        let json = r#"{"wm_ai":{"to":["w7"],"msg":{"Call":[{"call_id":"c9","tool":"summary","args":"{}"}]}}}"#;
        let (events, local_frames) = p.observe_local_during(&Event::Custom(json.into()));
        assert!(events.is_empty());
        assert!(local_frames.is_empty());
        assert_eq!(p.host_messages(), [ServiceUp::Result(ToolResult::unavailable("c9", HOSTED_BUILD_MISMATCH))]);

        p.port.handle_event(&mut p.cx, &Event::Custom(json.replace("c9", "bad id")));
        assert!(p.host_messages().is_empty(), "a mismatch with an invalid call id must get no reply");

        let oversized = HostedDown {
            to: Some(p.host_id.clone()),
            msg: ServiceDown::Call(ServiceCall {
                call_id: "c10".into(), tool: "summary".into(), args: "x".repeat(MAX_ARGS_BYTES + 1),
                caller: None, origin: CallOrigin::Remote,
            }),
        };
        let (events, local_frames) = p.observe_local_during(&Event::Custom(oversized.to_json()));
        assert!(events.is_empty(), "an oversized call never reaches the tool");
        assert!(local_frames.is_empty(), "a rejected host call sends nothing to the local peer");
        assert_eq!(p.host_messages(), [ServiceUp::Result(ToolResult::failed("c10", arguments_too_large(MAX_ARGS_BYTES + 1)))],
            "the host receives the argument limit for its rejected call");
    }

    #[test]
    fn open_in_hosted_still_registers_in_local_registry() {
        let registry = ServiceRegistry::new();
        let mut bad = sheets();
        bad.id = "Sheets".into();
        assert!(AiServicePort::open_in(true, &registry, bad).is_none());
        assert!(registry.services().is_empty());
        let mut port = AiServicePort::open_in(true, &registry, sheets()).unwrap();
        let services = registry.services();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].id, "sheets");
        assert!(port.is_dual());
        assert!(matches!(port.test_drain().as_slice(), [PortEvent::Registered(_)]));
    }
}
