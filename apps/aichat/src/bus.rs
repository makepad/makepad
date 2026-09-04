//! The client half of the window manager's service bus.
//!
//! Under the WM the other apps are not in this process. The WM forwards
//! their up-frames to the aichat child as studio `Custom` frames, each
//! stamped with the endpoint the WM issued to the sender, and forwards
//! the aichat child's down-frames (which name their target endpoint) back
//! to the right client. This adapter turns those frames into ordinary
//! [`ServiceLink`]s in the panel's registry, so the engine never knows
//! whether a service is a channel away or a process away.
//!
//! One link per endpoint. A `Register` from an endpoint the registry does
//! not know creates the link and registers it under the WM's endpoint id
//! (`register_as`); a later `Register` from the same endpoint is just the
//! manifest going down the existing link, where the registry answers it.
//! The WM tells us about a dead client by sending `Unregister` on its
//! behalf. Everything the registry sends down a bus link is drained here
//! and put on the wire to the WM.

use makepad_ai_services::engine::ServiceRegistry;
use makepad_ai_services::port::{ServiceLink, ServiceLinkHost};
use makepad_ai_services::wire::*;
use makepad_widgets::makepad_platform::studio::AppToStudio;
use makepad_widgets::*;
use std::collections::HashMap;

#[derive(Default)]
pub struct ServiceBus {
    hosts: HashMap<EndpointId, ServiceLinkHost>,
}

impl ServiceBus {
    /// An up-frame from the WM. `None` for frames that are not the bus's.
    pub fn on_custom(&mut self, registry: &ServiceRegistry, json: &str) -> bool {
        let Some(frame) = HostedUp::parse(json) else { return false };
        let Some(from) = frame.from.clone() else { return true };
        match (&frame.msg, self.hosts.get(&from)) {
            (ServiceUp::Register { manifest, .. }, None) => {
                let (link, host) = ServiceLink::pair(manifest.clone());
                if registry.register_as(link, from.clone(), "", None).is_ok() {
                    let _ = host.up.send(frame);
                    self.hosts.insert(from, host);
                }
            }
            (_, Some(host)) => {
                let _ = host.up.send(frame);
            }
            (_, None) => {}
        }
        true
    }

    /// Put every frame the registry sent down a bus link on the wire.
    /// Links whose registry side is gone are dropped.
    pub fn relay_down(&mut self, registry: &ServiceRegistry) {
        for frame in self.drain_down(registry) {
            Cx::send_studio_message(AppToStudio::Custom(frame.to_json()));
        }
    }

    fn drain_down(&mut self, registry: &ServiceRegistry) -> Vec<HostedDown> {
        let mut gone: Vec<EndpointId> = Vec::new();
        let mut frames = Vec::new();
        for (endpoint, host) in &self.hosts {
            loop {
                match host.down.try_recv() {
                    Ok(mut frame) => {
                        // `Registered` travels without a target; the WM
                        // routes it by the endpoint we register as.
                        if frame.to.is_none() {
                            frame.to = Some(endpoint.clone());
                        }
                        frames.push(frame);
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        gone.push(endpoint.clone());
                        break;
                    }
                }
            }
        }
        for endpoint in gone {
            self.hosts.remove(&endpoint);
            registry.unregister(&endpoint);
        }
        frames
    }

    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::engine::{EngineCore, NoModelWithReason, RegistryUp};

    #[test]
    fn hosted_subscriptions_and_messages_cross_the_adapter_unchanged() {
        let registry = ServiceRegistry::new();
        let mut bus = ServiceBus::default();
        let endpoint = EndpointId("w4".into());
        let manifest = ServiceManifest::new("flow", "Flow", "A flow.")
            .with_topic(TopicDef::new("run", "Run events."));
        let register = HostedUp {
            from: Some(endpoint.clone()),
            msg: ServiceUp::Register { manifest, port_tag: 4 },
        };
        assert!(bus.on_custom(&registry, &register.to_json()));
        registry.pump();
        let host = bus.hosts.get(&endpoint).unwrap();
        let _registered = host.down.try_recv().unwrap();
        assert!(registry.send(
            &endpoint,
            ServiceDown::Subscribe {
                sub_id: "s1".into(),
                topic: "run".into(),
                filter: None,
            },
        ));
        assert!(matches!(
            host.down.try_recv().unwrap().msg,
            ServiceDown::Subscribe { sub_id, topic, filter: None }
                if sub_id == "s1" && topic == "run"
        ));
        let message = HostedUp {
            from: Some(endpoint.clone()),
            msg: ServiceUp::Message {
                sub_id: "s1".into(),
                topic: "run".into(),
                text: "finished".into(),
                data: None,
                final_: true,
            },
        };
        assert!(bus.on_custom(&registry, &message.to_json()));
        assert!(matches!(
            registry.pump().as_slice(),
            [RegistryUp::Message { endpoint: from, sub_id, message }]
                if from == &endpoint && sub_id == "s1" && message.final_
        ));
    }

    #[test]
    fn engine_shutdown_is_flushed_through_the_hosted_service_bus() {
        let registry = ServiceRegistry::new();
        let mut bus = ServiceBus::default();
        let endpoint = EndpointId("w4".into());
        let manifest = ServiceManifest::new("flow", "Flow", "A flow.")
            .with_tool(ToolDef::new(
                "watch",
                "Watch a run.",
                r#"{"type":"object","properties":{}}"#,
                Risk::Read,
            ))
            .with_topic(TopicDef::new("run", "Run events."));
        assert!(bus.on_custom(
            &registry,
            &HostedUp {
                from: Some(endpoint.clone()),
                msg: ServiceUp::Register { manifest, port_tag: 4 },
            }
            .to_json(),
        ));
        let mut engine = EngineCore::new(
            registry.clone(),
            Box::new(NoModelWithReason::new("not used by the tool console")),
            None,
            0x44,
        );
        engine.send("/flow.watch {}", 0.0);
        let call_id = bus
            .drain_down(&registry)
            .into_iter()
            .find_map(|frame| match frame.msg {
                ServiceDown::Call(call) => Some(call.call_id),
                _ => None,
            })
            .expect("the hosted service receives the call");
        assert!(bus.on_custom(
            &registry,
            &HostedUp {
                from: Some(endpoint.clone()),
                msg: ServiceUp::Result(
                    ToolResult::ok(call_id, "watching", "")
                        .with_subscription(SubscriptionRequest::new("run")),
                ),
            }
            .to_json(),
        ));
        engine.pump(0.1);
        let sub_id = bus
            .drain_down(&registry)
            .into_iter()
            .find_map(|frame| match frame.msg {
                ServiceDown::Subscribe { sub_id, .. } => Some(sub_id),
                _ => None,
            })
            .expect("the hosted service receives the subscription");
        assert_eq!(sub_id, "l44-s1");
        engine.shutdown();
        assert!(matches!(
            bus.drain_down(&registry).as_slice(),
            [HostedDown { to: Some(to), msg: ServiceDown::Unsubscribe { sub_id: ended } }]
                if to == &endpoint && ended == &sub_id
        ));
    }
}
