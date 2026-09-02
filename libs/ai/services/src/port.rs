//! The app side: a port that exposes one service to whichever host is there.
//!
//! An app builds its manifest and opens a port. Hosted by the window manager
//! (`--stdin-loop` under wm) the port registers over the studio protocol
//! and calls arrive as `Event::Custom` frames. Embedding the chat panel
//! itself, the app opens an in-process port and hands the matching
//! [`ServiceLink`] to its own engine's registry. Either way the app sees the
//! same thing: [`PortEvent::Call`]s out of `handle_event`, answered with
//! [`AiServicePort::reply`] whenever the work is done — the same frame or
//! many frames later, from the main thread or a worker's channel.
//!
//! Identity. A port has no address until the host answers its `Register`
//! with [`ServiceDown::Registered`]; the port matches that answer on the
//! nonce it sent (`port_tag`), keeps the endpoint, and from then on takes
//! only frames addressed to that endpoint — so several ports in one
//! process, or several instances of one app, never see each other's
//! calls. A port never claims an address on the way up: the host stamps
//! senders itself.
//!
//! A port never executes anything. What a call does is the app's closed
//! match over its own tool names; the port only carries it.

use crate::wire::*;
use makepad_platform::studio::AppToStudio;
use makepad_platform::thread::SignalToUI;
use makepad_platform::{Cx, Event};
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

enum Transport {
    InProcess { up: Sender<HostedUp>, down: Receiver<HostedDown> },
    Hosted,
}

static NEXT_PORT_TAG: AtomicU32 = AtomicU32::new(1);

/// One app's service, open to its host.
pub struct AiServicePort {
    manifest: ServiceManifest,
    transport: Transport,
    /// The nonce this port registers with; how `Registered` finds it.
    port_tag: u32,
    endpoint: Option<EndpointId>,
    chat_open: bool,
}

impl AiServicePort {
    /// Open the port toward the window manager hosting this process.
    /// `None` when the process is standalone (nothing was sent) — the app
    /// then decides whether to embed its own chat panel.
    ///
    /// The manifest must validate; a bad one is a programming error in the
    /// app and is refused loudly rather than registered half-way.
    pub fn hosted(cx: &Cx, manifest: ServiceManifest) -> Option<AiServicePort> {
        if !cx.in_makepad_studio() {
            return None;
        }
        if let Err(e) = manifest.validate() {
            makepad_platform::error!("ai service manifest refused: {e}");
            return None;
        }
        let port = AiServicePort {
            manifest,
            transport: Transport::Hosted,
            port_tag: NEXT_PORT_TAG.fetch_add(1, Ordering::Relaxed),
            endpoint: None,
            chat_open: false,
        };
        port.register();
        Some(port)
    }

    /// Open the port in-process. The returned link goes to the embedding
    /// host's own `ServiceRegistry`, which issues the endpoint.
    pub fn in_process(manifest: ServiceManifest) -> Result<(AiServicePort, ServiceLink), String> {
        manifest.validate()?;
        let (link, host) = ServiceLink::pair(manifest.clone());
        let port = AiServicePort {
            manifest,
            transport: Transport::InProcess { up: host.up, down: host.down },
            port_tag: NEXT_PORT_TAG.fetch_add(1, Ordering::Relaxed),
            endpoint: None,
            chat_open: false,
        };
        port.register();
        Ok((port, link))
    }

    pub fn manifest(&self) -> &ServiceManifest {
        &self.manifest
    }

    /// The address the host gave this port; `None` until `Registered` came.
    pub fn endpoint(&self) -> Option<&EndpointId> {
        self.endpoint.as_ref()
    }

    /// Whether the host's chat pane is showing, as last told.
    pub fn chat_open(&self) -> bool {
        self.chat_open
    }

    /// Announce (or re-announce) the manifest. Done when opened; a
    /// warm-pool instance does it again on `Adopted`, and any app may
    /// after a reload changed its tools. The host answers `Registered`.
    pub fn register(&self) {
        self.send(ServiceUp::Register { manifest: self.manifest.clone(), port_tag: self.port_tag });
    }

    /// Drain what the host wants. Hosted: the studio `Custom` frames under
    /// our envelope; in-process: the channel, checked on every event.
    /// Frames for another endpoint are ignored.
    pub fn handle_event(&mut self, _cx: &mut Cx, event: &Event) -> Vec<PortEvent> {
        let mut frames = Vec::new();
        match &mut self.transport {
            Transport::Hosted => {
                if let Event::Custom(json) = event {
                    if let Some(down) = HostedDown::parse(json) {
                        frames.push(down);
                    }
                }
            }
            Transport::InProcess { down, .. } => loop {
                match down.try_recv() {
                    Ok(msg) => frames.push(msg),
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                }
            },
        }
        let mut out = Vec::new();
        for frame in frames {
            if let Some(ev) = self.accept(frame) {
                out.push(ev);
            }
        }
        out
    }

    /// One frame through the address filter.
    fn accept(&mut self, frame: HostedDown) -> Option<PortEvent> {
        match frame.msg {
            ServiceDown::Registered { port_tag, endpoint } => {
                if port_tag != self.port_tag {
                    return None;
                }
                self.endpoint = Some(endpoint.clone());
                Some(PortEvent::Registered(endpoint))
            }
            msg => {
                let mine = match (&frame.to, &self.endpoint) {
                    (Some(to), Some(me)) => to == me,
                    _ => false,
                };
                if !mine {
                    return None;
                }
                Some(match msg {
                    ServiceDown::Registered { .. } => unreachable!(),
                    ServiceDown::Call(call) => PortEvent::Call(call),
                    ServiceDown::Cancel { call_id } => PortEvent::Cancel { call_id },
                    ServiceDown::ChatOpen { open } => {
                        self.chat_open = open;
                        PortEvent::ChatOpen { open }
                    }
                })
            }
        }
    }

    /// Answer a call. Bounded here so an app cannot flood the model.
    pub fn reply(&self, mut result: ToolResult) {
        result.bound();
        self.send(ServiceUp::Result(result));
    }

    /// Keep a long call alive and show where it is.
    pub fn progress(&self, call_id: &str, note: &str, permille: u16) {
        let mut note = note.to_string();
        if note.len() > MAX_NOTE_BYTES {
            truncate_to_char_boundary(&mut note, MAX_NOTE_BYTES - 3);
            note.push('…');
        }
        self.send(ServiceUp::Progress { call_id: call_id.to_string(), note, permille: permille.min(1000) });
    }

    /// The app's volatile state for the model's next turn. Replaces the
    /// previous text; empty clears it.
    pub fn set_context(&self, text: &str) {
        let mut text = text.to_string();
        if text.len() > MAX_CONTEXT_BYTES {
            truncate_to_char_boundary(&mut text, MAX_CONTEXT_BYTES - 3);
            text.push('…');
        }
        self.send(ServiceUp::Context(ServiceContext { text }));
    }

    /// Leave on purpose. The host also forgets a service whose process
    /// dies, so most apps never call this.
    pub fn unregister(&self) {
        self.send(ServiceUp::Unregister);
    }

    /// Test seam: drain the in-process channel the way `handle_event`
    /// does, without a `Cx`.
    #[cfg(test)]
    pub(crate) fn test_drain(&mut self) -> Vec<PortEvent> {
        let frames: Vec<HostedDown> = match &mut self.transport {
            Transport::InProcess { down, .. } => down.try_iter().collect(),
            Transport::Hosted => Vec::new(),
        };
        frames.into_iter().filter_map(|f| self.accept(f)).collect()
    }

    fn send(&self, msg: ServiceUp) {
        // Never a claim of identity on the way up: `from` is the host's.
        let frame = HostedUp { from: None, msg };
        match &self.transport {
            Transport::Hosted => {
                Cx::send_studio_message(AppToStudio::Custom(frame.to_json()));
            }
            Transport::InProcess { up: tx, .. } => {
                if tx.send(frame).is_ok() {
                    // The engine polls on events; make sure one comes.
                    SignalToUI::set_ui_signal();
                }
            }
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
    }

    fn ep(s: &str) -> EndpointId {
        EndpointId(s.to_string())
    }

    /// Drain the in-process channel the way `handle_event` does, without a Cx.
    fn pump(port: &mut AiServicePort) -> Vec<PortEvent> {
        let frames: Vec<HostedDown> = match &mut port.transport {
            Transport::InProcess { down, .. } => down.try_iter().collect(),
            _ => unreachable!(),
        };
        frames.into_iter().filter_map(|f| port.accept(f)).collect()
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
        link.down.send(HostedDown { to: Some(ep("other")), msg: ServiceDown::Call(ServiceCall { call_id: "c0".into(), tool: "stat".into(), args: "{}".into() }) }).unwrap();
        link.down.send(HostedDown { to: None, msg: ServiceDown::Registered { port_tag: tag + 100, endpoint: ep("not-me") } }).unwrap();
        link.down.send(HostedDown { to: None, msg: ServiceDown::Registered { port_tag: tag, endpoint: ep("e1") } }).unwrap();
        link.down.send(HostedDown { to: Some(ep("e1")), msg: ServiceDown::Call(ServiceCall { call_id: "c1".into(), tool: "stat".into(), args: "{}".into() }) }).unwrap();
        link.down.send(HostedDown { to: Some(ep("e2")), msg: ServiceDown::Call(ServiceCall { call_id: "c2".into(), tool: "stat".into(), args: "{}".into() }) }).unwrap();
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
    fn a_bad_manifest_is_refused_before_anything_is_sent() {
        let mut bad = files();
        bad.tools[0].name = "Stat".into();
        assert!(AiServicePort::in_process(bad).is_err());
    }

    #[test]
    fn a_reply_is_bounded_on_the_way_out() {
        let (port, link) = AiServicePort::in_process(files()).unwrap();
        let _reg = link.up.try_recv().unwrap();
        port.reply(ToolResult::ok("c1", "x".repeat(MAX_RESULT_BYTES * 2), "n"));
        match link.up.try_recv().unwrap().msg {
            ServiceUp::Result(r) => assert!(r.text.len() <= MAX_RESULT_BYTES),
            other => panic!("{other:?}"),
        }
    }
}
