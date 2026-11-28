//! The bus's in-process transport: how the WM's own `os` service and its
//! module instances reach an assistant that runs IN THIS PROCESS.
//!
//! When the pane's chat is the aichat child (a process), every service
//! frame travels as a studio `Custom` frame through `ai_bus.rs`. When the
//! chat is the aichat MODULE seated in the pane in-process — the web
//! superbuild, or a desktop that switched `aichat` to module hosting —
//! there is no socket to frame anything over: each service is an ordinary
//! in-process [`ServiceLink`] registered in the process's registry
//! ([`ServiceRegistry::process`], which the chat root shows), and the WM
//! keeps the host half of every link:
//! calls arrive on its `down` channel, answers go back on `up`. Dropping
//! a host half is how an instance leaves — the registry sees the link
//! close and forgets the endpoint.
//!
//! This is the leg the web build runs everything on.

use makepad_app_host::ClientId;
use makepad_ai_services::engine::ServiceRegistry;
use makepad_ai_services::port::{ServiceConfirmations, ServiceLink, ServiceLinkHost};
use makepad_ai_services::wire::*;
use makepad_widgets::*;
use std::collections::HashMap;

/// What the assistant asked of the WM through an in-process link.
pub enum PaneCall {
    /// A call on the WM's own `os` service.
    Os(ServiceCall),
    /// A call on a module instance's service.
    Instance(ClientId, ServiceCall),
    /// The assistant gave up on an instance's call.
    Cancel(ClientId, String),
    /// The assistant subscribed to a module topic.
    Subscribe {
        client: ClientId,
        sub_id: String,
        topic: String,
        filter: Option<String>,
    },
    /// The assistant ended a module subscription.
    Unsubscribe { client: ClientId, sub_id: String },
}

struct PaneService {
    host: ServiceLinkHost,
    label: String,
    confirmations: ServiceConfirmations,
}

impl PaneService {
    fn drain(&mut self, cx: &mut Cx, mut dispatch: impl FnMut(ServiceDown)) {
        loop {
            let frame = match self.host.down.try_recv() {
                Ok(frame) => frame,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.confirmations.withdraw_all(cx);
                    return;
                }
            };
            match frame.msg {
                ServiceDown::Confirm { call_id, tool, args } =>
                    self.confirmations.ask(cx, &self.label, call_id, tool, args),
                ServiceDown::Cancel { ref call_id } if self.confirmations.withdraw(cx, call_id) => {}
                msg => dispatch(msg),
            }
        }
        self.confirmations.answers(cx, |msg| {
            let _ = self.host.up.send(HostedUp { from: None, msg });
        });
    }
}

#[derive(Default)]
pub struct PaneLinks {
    os: Option<PaneService>,
    instances: HashMap<ClientId, PaneService>,
}

impl PaneLinks {
    /// One link: the app-side half is registered in `registry`, announced
    /// with its manifest exactly as a port announces itself; the host half
    /// stays here.
    fn open(registry: &ServiceRegistry, manifest: ServiceManifest) -> PaneService {
        let label = manifest.label.clone();
        let (link, host) = ServiceLink::pair(manifest.clone());
        let _ = host.up.send(HostedUp { from: None, msg: ServiceUp::Register { manifest, port_tag: 0 } });
        if let Err(e) = registry.register(link, "in this process", None) {
            log!("wm: the registry refused a service: {e}");
        }
        PaneService { host, label, confirmations: Default::default() }
    }

    /// The WM's own service, once.
    pub fn open_os(&mut self, _cx: &mut Cx, manifest: ServiceManifest) {
        if self.os.is_none() {
            self.os = Some(Self::open(&ServiceRegistry::process(), manifest));
        }
    }

    pub fn has_os(&self) -> bool {
        self.os.is_some()
    }

    /// A module instance joins the process's registry.
    pub fn open_instance(&mut self, _cx: &mut Cx, client: ClientId, manifest: ServiceManifest) {
        let host = Self::open(&ServiceRegistry::process(), manifest);
        self.instances.insert(client, host);
    }

    /// The instance is gone: closing its link is the unregistration.
    pub fn close_instance(&mut self, client: ClientId) -> bool {
        self.instances.remove(&client).is_some()
    }

    pub fn is_instance(&self, client: ClientId) -> bool {
        self.instances.contains_key(&client)
    }

    /// Everything the assistant sent down the links since the last drain.
    /// `Registered` and `ChatOpen` are the registry's bookkeeping, not
    /// calls; a closed registry side means the chat root went away, and
    /// the link is simply kept for the next one.
    pub fn drain(&mut self, cx: &mut Cx) -> Vec<PaneCall> {
        let mut out = Vec::new();
        if let Some(os) = &mut self.os {
            os.drain(cx, |msg| {
                if let ServiceDown::Call(call) = msg { out.push(PaneCall::Os(call)); }
            });
        }
        for (&client, host) in &mut self.instances {
            host.drain(cx, |msg| match msg {
                ServiceDown::Call(call) => out.push(PaneCall::Instance(client, call)),
                ServiceDown::Cancel { call_id } => out.push(PaneCall::Cancel(client, call_id)),
                ServiceDown::Subscribe { sub_id, topic, filter } =>
                    out.push(PaneCall::Subscribe { client, sub_id, topic, filter }),
                ServiceDown::Unsubscribe { sub_id } => out.push(PaneCall::Unsubscribe { client, sub_id }),
                _ => {}
            });
        }
        out
    }

    pub fn reply_os(&self, result: ToolResult) {
        if let Some(os) = &self.os {
            let _ = os.host.up.send(HostedUp { from: None, msg: ServiceUp::Result(result) });
        }
    }

    pub fn reply(&self, client: ClientId, result: ToolResult) -> bool {
        self.send_up(client, ServiceUp::Result(result))
    }

    pub fn publish(&self, client: ClientId, sub_id: String, message: Message) -> bool {
        self.send_up(
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

    fn send_up(&self, client: ClientId, msg: ServiceUp) -> bool {
        match self.instances.get(&client) {
            Some(host) => host.host.up.send(HostedUp { from: None, msg }).is_ok(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheets() -> ServiceManifest {
        ServiceManifest::new("sheets", "Sheets", "The spreadsheet.")
            .with_tool(ToolDef::new(
                "summary",
                "The sheet on screen.",
                r#"{"type":"object","properties":{}}"#,
                Risk::Read,
            ))
            .with_topic(TopicDef::new("watch", "Sheet changes."))
    }

    #[test]
    fn links_are_adopted_by_a_registry_and_calls_come_back_as_pane_calls() {
        let registry = ServiceRegistry::new();
        let mut links = PaneLinks::default();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        links.os = Some(PaneLinks::open(&registry, crate::ai_bus::AiBus::os_manifest(&[])));
        links.instances.insert(4, PaneLinks::open(&registry, sheets()));
        // The registry answered each Register at registration.
        let endpoints: Vec<EndpointId> = vec![registry.pick_instance("os", None).unwrap(), registry.pick_instance("sheets", None).unwrap()];
        let _ = registry.pump();
        assert_eq!(registry.services().len(), 2);
        // A call the engine sends down reaches the host half as a pane call…
        let call = ServiceCall { call_id: "c1".into(), tool: "summary".into(), args: "{}".into(), caller: None, origin: CallOrigin::Conversation };
        assert!(registry.send(&endpoints[1], ServiceDown::Call(call)));
        let calls = links.drain(&mut cx);
        assert!(matches!(calls.as_slice(), [PaneCall::Instance(4, c)] if c.call_id == "c1"), "one instance call");
        // …and the answer goes back up the same link, into the registry.
        assert!(links.reply(4, ToolResult::ok("c1", "Sheet 1", "")));
        let up = registry.pump();
        assert_eq!(up.len(), 1);
        // Subscriptions use the same down leg, and module publications use
        // the same authenticated up leg back into the engine registry.
        assert!(registry.send(
            &endpoints[1],
            ServiceDown::Subscribe {
                sub_id: "lease-s1".into(),
                topic: "watch".into(),
                filter: Some(r#"{"sheet":1}"#.into()),
            },
        ));
        assert!(matches!(
            links.drain(&mut cx).as_slice(),
            [PaneCall::Subscribe { client: 4, sub_id, topic, filter: Some(filter) }]
                if sub_id == "lease-s1" && topic == "watch" && filter.contains("sheet")
        ));
        assert!(links.publish(4, "lease-s1".into(), Message::new("watch", "A1 changed")));
        assert!(matches!(
            registry.pump().as_slice(),
            [makepad_ai_services::engine::RegistryUp::Message { endpoint, sub_id, message }]
                if endpoint == &endpoints[1]
                    && sub_id == "lease-s1"
                    && message.text == "A1 changed"
        ));
        assert!(registry.send(
            &endpoints[1],
            ServiceDown::Unsubscribe { sub_id: "lease-s1".into() },
        ));
        assert!(matches!(
            links.drain(&mut cx).as_slice(),
            [PaneCall::Unsubscribe { client: 4, sub_id }] if sub_id == "lease-s1"
        ));
        for endpoint in &endpoints {
            let id = "held";
            assert!(registry.send(endpoint, ServiceDown::Confirm {
                call_id: id.into(), tool: "test".into(), args: "{}".into(),
            }));
            assert!(links.drain(&mut cx).is_empty(), "a prompt must not reach an executor");
            let ticket = cx.confirmation_head().get().unwrap();
            assert!(cx.confirmation_text(ticket).unwrap().0.ends_with(" asks to run test"));
            assert!(registry.send(endpoint, ServiceDown::Cancel { call_id: id.into() }));
            assert!(links.drain(&mut cx).is_empty(), "a held cancel belongs to the prompt");
            assert!(cx.confirmation_head().get().is_none());
        }
        // Closing the instance closes its link: the registry forgets it.
        assert!(links.close_instance(4));
        assert!(!links.close_instance(4));
        let _ = registry.pump();
        assert_eq!(registry.services().len(), 1);
        assert_eq!(registry.services()[0].id, "os");
        assert!(!links.reply(4, ToolResult::ok("c2", "", "")), "a closed link takes no reply");
    }

    #[test]
    fn external_files_operations_wait_for_a_local_press_before_reaching_the_module() {
        use std::sync::{atomic::AtomicBool, mpsc::channel};
        let registry = ServiceRegistry::new();
        let mut links = PaneLinks::default();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        // The desktop WM does not link app crates. Exercise its module executor boundary
        // with the two destructive operations published by the Files module.
        let mut manifest = ServiceManifest::new("files", "Files", "File operations.");
        for tool in ["mkdir", "rename"] {
            manifest = manifest.with_tool(ToolDef::new(tool, tool, r#"{"type":"object"}"#, Risk::Destructive));
        }
        links.instances.insert(7, PaneLinks::open(&registry, manifest));
        for (tool, args) in [("mkdir", r#"{"path":"folder"}"#), ("rename", r#"{"path":"folder","new_name":"renamed"}"#)] {
            let (send, waiting) = channel();
            let caller = registry.clone();
            let job = std::thread::spawn(move || {
                makepad_ai_services::app_service::call(&caller, "files", tool, None, args.into(), None,
                    &AtomicBool::new(false), &mut |note, _| { let _ = send.send(note.to_owned()); })
            });
            assert_eq!(waiting.recv().unwrap(), "waiting for the person to confirm in Files");
            assert!(links.drain(&mut cx).is_empty(), "{tool} reached the executor before Allow");
            let id = cx.confirmation_head().get().unwrap();
            assert!(cx.confirmation_text(id).unwrap().0.ends_with(tool));
            assert!(cx.answer_confirmation(id, true));
            let call = loop {
                let mut received = links.drain(&mut cx);
                if let Some(call) = received.pop() {
                    assert!(received.is_empty(), "Allow must dispatch once");
                    break call;
                }
                assert!(!job.is_finished(), "caller ended before the module got {tool}");
                std::thread::yield_now();
            };
            let PaneCall::Instance(7, call) = call else { panic!("call reached the wrong executor") };
            assert_eq!((call.tool.as_str(), call.args.as_str(), call.origin), (tool, args, CallOrigin::Remote));
            links.reply(7, ToolResult::ok(call.call_id, "done", ""));
            assert_eq!(job.join().unwrap().outcome, ToolOutcome::Ok);
        }
    }
}
