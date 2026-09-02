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
//! A port never executes anything. What a call does is the app's closed
//! match over its own tool names; the port only carries it.

use crate::wire::*;
use makepad_platform::studio::AppToStudio;
use makepad_platform::thread::SignalToUI;
use makepad_platform::{Cx, Event};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

/// What the engine wants from the service, as the app reads it each frame.
#[derive(Clone, Debug, PartialEq)]
pub enum PortEvent {
    Call(ServiceCall),
    /// Stop this call if you can; no reply is expected.
    Cancel { call_id: String },
    /// The host's chat pane came up or went away.
    ChatOpen { open: bool },
}

/// The engine's end of an in-process service: what the registry holds.
pub struct ServiceLink {
    pub manifest: ServiceManifest,
    /// Engine → service.
    pub down: Sender<ServiceDown>,
    /// Service → engine.
    pub up: Receiver<ServiceUp>,
}

/// The transport's end of a link the HOST bridges itself — the window
/// manager feeds `up` from the child's frames and forwards `down` to it.
pub struct ServiceLinkHost {
    pub up: Sender<ServiceUp>,
    pub down: Receiver<ServiceDown>,
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
    InProcess { up: Sender<ServiceUp>, down: Receiver<ServiceDown> },
    Hosted,
}

/// One app's service, open to its host.
pub struct AiServicePort {
    manifest: ServiceManifest,
    transport: Transport,
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
        let port = AiServicePort { manifest, transport: Transport::Hosted, chat_open: false };
        port.register();
        Some(port)
    }

    /// Open the port in-process. The returned link goes to the embedding
    /// app's own `ServiceRegistry`.
    pub fn in_process(manifest: ServiceManifest) -> Result<(AiServicePort, ServiceLink), String> {
        manifest.validate()?;
        let (link, host) = ServiceLink::pair(manifest.clone());
        let port = AiServicePort {
            manifest,
            transport: Transport::InProcess { up: host.up, down: host.down },
            chat_open: false,
        };
        Ok((port, link))
    }

    pub fn manifest(&self) -> &ServiceManifest {
        &self.manifest
    }

    /// Whether the host's chat pane is showing, as last told.
    pub fn chat_open(&self) -> bool {
        self.chat_open
    }

    /// Announce (or re-announce) the manifest. Hosted ports do this when
    /// opened; a warm-pool instance does it again on `Adopted`, and any app
    /// may after a reload changed its tools.
    pub fn register(&self) {
        self.send(ServiceUp::Register(self.manifest.clone()));
    }

    /// Drain what the host wants. Hosted: the studio `Custom` frames under
    /// our envelope; in-process: the channel, checked on every event.
    pub fn handle_event(&mut self, _cx: &mut Cx, event: &Event) -> Vec<PortEvent> {
        let mut out = Vec::new();
        match &mut self.transport {
            Transport::Hosted => {
                if let Event::Custom(json) = event {
                    if let Some(down) = ServiceDown::parse_hosted(json) {
                        out.push(down);
                    }
                }
            }
            Transport::InProcess { down, .. } => loop {
                match down.try_recv() {
                    Ok(msg) => out.push(msg),
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                }
            },
        }
        out.into_iter()
            .map(|down| match down {
                ServiceDown::Call(call) => PortEvent::Call(call),
                ServiceDown::Cancel { call_id } => PortEvent::Cancel { call_id },
                ServiceDown::ChatOpen { open } => {
                    self.chat_open = open;
                    PortEvent::ChatOpen { open }
                }
            })
            .collect()
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

    fn send(&self, up: ServiceUp) {
        match &self.transport {
            Transport::Hosted => {
                Cx::send_studio_message(AppToStudio::Custom(up.to_hosted_json()));
            }
            Transport::InProcess { up: tx, .. } => {
                if tx.send(up).is_ok() {
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

    #[test]
    fn an_in_process_port_carries_calls_down_and_results_up() {
        let (port, link) = AiServicePort::in_process(files()).unwrap();
        assert_eq!(link.manifest.id, "files");
        link.down
            .send(ServiceDown::Call(ServiceCall { call_id: "c1".into(), tool: "stat".into(), args: "{}".into() }))
            .unwrap();
        link.down.send(ServiceDown::ChatOpen { open: true }).unwrap();
        // Drain the channel the way handle_event does, without a Cx.
        let mut port = port;
        let events: Vec<PortEvent> = match &mut port.transport {
            Transport::InProcess { down, .. } => down.try_iter().collect::<Vec<_>>(),
            _ => unreachable!(),
        }
        .into_iter()
        .map(|d| match d {
            ServiceDown::Call(c) => PortEvent::Call(c),
            ServiceDown::Cancel { call_id } => PortEvent::Cancel { call_id },
            ServiceDown::ChatOpen { open } => PortEvent::ChatOpen { open },
        })
        .collect();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], PortEvent::Call(c) if c.tool == "stat"));
        port.reply(ToolResult::ok("c1", "a file, 12 bytes", "stat ~/x"));
        port.set_context("[files] cwd=~");
        let up: Vec<ServiceUp> = link.up.try_iter().collect();
        assert!(matches!(&up[0], ServiceUp::Result(r) if r.call_id == "c1" && r.outcome.is_ok()));
        assert!(matches!(&up[1], ServiceUp::Context(c) if c.text == "[files] cwd=~"));
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
        port.reply(ToolResult::ok("c1", "x".repeat(MAX_RESULT_BYTES * 2), "n"));
        match link.up.try_recv().unwrap() {
            ServiceUp::Result(r) => assert!(r.text.len() <= MAX_RESULT_BYTES),
            other => panic!("{other:?}"),
        }
    }
}
