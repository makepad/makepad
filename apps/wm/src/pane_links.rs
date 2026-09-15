//! The bus's in-process transport: how the WM's own `os` service and its
//! module instances reach an assistant that runs IN THIS PROCESS.
//!
//! When the pane's chat is the aichat child (a process), every service
//! frame travels as a studio `Custom` frame through `ai_bus.rs`. When the
//! chat is the aichat MODULE seated in the pane in-process — the web
//! superbuild, or a desktop that switched `aichat` to module hosting —
//! there is no socket to frame anything over: each service is an ordinary
//! in-process [`ServiceLink`] the chat root adopts into its registry
//! (parked on `Cx` as [`PendingServiceLinks`], the same lot the Window's
//! F10 overlay drains), and the WM keeps the host half of every link:
//! calls arrive on its `down` channel, answers go back on `up`. Dropping
//! a host half is how an instance leaves — the registry sees the link
//! close and forgets the endpoint.
//!
//! This is the leg the web build runs everything on.

use crate::hub::ClientId;
use makepad_ai_services::port::{PendingServiceLinks, ServiceLink, ServiceLinkHost};
use makepad_ai_services::wire::*;
use makepad_widgets::*;
use std::collections::HashMap;
use std::sync::mpsc::TryRecvError;

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

#[derive(Default)]
pub struct PaneLinks {
    os: Option<ServiceLinkHost>,
    instances: HashMap<ClientId, ServiceLinkHost>,
}

impl PaneLinks {
    /// One link: the app-side half goes to the lot the chat root drains,
    /// announced with its manifest exactly as a port announces itself; the
    /// host half stays here.
    fn open(pending: &mut PendingServiceLinks, manifest: ServiceManifest) -> ServiceLinkHost {
        let (link, host) = ServiceLink::pair(manifest.clone());
        let _ = host.up.send(HostedUp { from: None, msg: ServiceUp::Register { manifest, port_tag: 0 } });
        pending.links.push(link);
        host
    }

    /// The WM's own service, once.
    pub fn open_os(&mut self, cx: &mut Cx, manifest: ServiceManifest) {
        if self.os.is_none() {
            self.os = Some(Self::open(cx.global::<PendingServiceLinks>(), manifest));
        }
    }

    pub fn has_os(&self) -> bool {
        self.os.is_some()
    }

    /// A module instance joins: its link waits in the lot until the chat
    /// root is up, and is adopted the moment it is.
    pub fn open_instance(&mut self, cx: &mut Cx, client: ClientId, manifest: ServiceManifest) {
        let host = Self::open(cx.global::<PendingServiceLinks>(), manifest);
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
    pub fn drain(&mut self) -> Vec<PaneCall> {
        let mut out = Vec::new();
        if let Some(os) = &self.os {
            loop {
                match os.down.try_recv() {
                    Ok(HostedDown { msg: ServiceDown::Call(call), .. }) => out.push(PaneCall::Os(call)),
                    Ok(_) => {}
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                }
            }
        }
        for (client, host) in &self.instances {
            loop {
                match host.down.try_recv() {
                    Ok(HostedDown { msg: ServiceDown::Call(call), .. }) => out.push(PaneCall::Instance(*client, call)),
                    Ok(HostedDown { msg: ServiceDown::Cancel { call_id }, .. }) => out.push(PaneCall::Cancel(*client, call_id)),
                    Ok(HostedDown { msg: ServiceDown::Subscribe { sub_id, topic, filter }, .. }) => {
                        out.push(PaneCall::Subscribe { client: *client, sub_id, topic, filter })
                    }
                    Ok(HostedDown { msg: ServiceDown::Unsubscribe { sub_id }, .. }) => {
                        out.push(PaneCall::Unsubscribe { client: *client, sub_id })
                    }
                    Ok(_) => {}
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                }
            }
        }
        out
    }

    pub fn reply_os(&self, result: ToolResult) {
        if let Some(os) = &self.os {
            let _ = os.up.send(HostedUp { from: None, msg: ServiceUp::Result(result) });
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
            Some(host) => host.up.send(HostedUp { from: None, msg }).is_ok(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::engine::ServiceRegistry;

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
        let mut pending = PendingServiceLinks::default();
        let mut links = PaneLinks::default();
        links.os = Some(PaneLinks::open(&mut pending, crate::ai_bus::AiBus::os_manifest(&[])));
        links.instances.insert(4, PaneLinks::open(&mut pending, sheets()));
        assert_eq!(pending.links.len(), 2);
        // The chat root adopts the lot; the registry answers each Register.
        let registry = ServiceRegistry::new();
        let endpoints: Vec<EndpointId> = pending
            .take()
            .into_iter()
            .map(|link| registry.register(link, "in this process", None).expect("adopted"))
            .collect();
        let _ = registry.pump();
        assert_eq!(registry.services().len(), 2);
        // A call the engine sends down reaches the host half as a pane call…
        let call = ServiceCall { call_id: "c1".into(), tool: "summary".into(), args: "{}".into() };
        assert!(registry.send(&endpoints[1], ServiceDown::Call(call)));
        let calls = links.drain();
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
            links.drain().as_slice(),
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
            links.drain().as_slice(),
            [PaneCall::Unsubscribe { client: 4, sub_id }] if sub_id == "lease-s1"
        ));
        // Closing the instance closes its link: the registry forgets it.
        assert!(links.close_instance(4));
        assert!(!links.close_instance(4));
        let _ = registry.pump();
        assert_eq!(registry.services().len(), 1);
        assert_eq!(registry.services()[0].id, "os");
        assert!(!links.reply(4, ToolResult::ok("c2", "", "")), "a closed link takes no reply");
    }
}
