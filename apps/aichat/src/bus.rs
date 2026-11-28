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
    logged_rejection: bool,
    #[cfg(not(target_arch = "wasm32"))]
    parent: Option<makepad_widgets::makepad_platform::os::hal::HalParentSender>,
}

impl ServiceBus {
    pub fn set_context(&mut self, _cx: &Cx) {
        #[cfg(not(target_arch = "wasm32"))]
        { self.parent = _cx.hal_parent_sender(); }
    }

    /// Consume an up-frame from the WM, including one the bus rejects.
    pub fn on_custom(&mut self, registry: &ServiceRegistry, json: &str) -> bool {
        let frame = match HostedUp::parse_checked(json) {
            None => return false,
            Some(Ok(frame)) => frame,
            Some(Err(rejected)) => {
                if !self.logged_rejection {
                    self.logged_rejection = true;
                    error!("AI chat rejected a frame from the window manager: {}", rejected.error);
                }
                let reply = rejected.mismatch || rejected.kind == RejectedKind::Result;
                let (true, Some(from), Some(call_id)) = (
                    reply,
                    rejected.endpoint.filter(|ep| is_opaque_id(ep)),
                    rejected.call_id.filter(|id| is_opaque_id(id)),
                ) else {
                    return true;
                };
                let mut result = if rejected.mismatch {
                    ToolResult::unavailable(call_id, HOSTED_BUILD_MISMATCH)
                } else {
                    ToolResult::failed(call_id, format!("tool reply rejected: {}", rejected.error))
                };
                result.bound();
                HostedUp {
                    from: Some(EndpointId(from)),
                    msg: ServiceUp::Result(result),
                }
            }
        };
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
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(parent) = &self.parent {
                parent.send(&frame.to_json());
                continue;
            }
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
    fn rejected_result_reaches_registry_as_failed_and_bad_progress_is_dropped() {
        let registry = ServiceRegistry::new();
        let mut bus = ServiceBus::default();
        let w4 = EndpointId("w4".into());
        let frame = |msg| HostedUp { from: Some(w4.clone()), msg }.to_json();
        let register = ServiceUp::Register { manifest: ServiceManifest::new("files", "Files", "The file browser."), port_tag: 4 };
        assert!(bus.on_custom(&registry, &frame(register)), "the bus consumes the registration");
        let result = ToolResult::ok("c1", "x".repeat(MAX_RESULT_WIRE_BYTES + 1), "");
        assert!(bus.on_custom(&registry, &frame(ServiceUp::Result(result))), "the bus consumes the oversized reply");
        assert!(matches!(registry.pump().as_slice(), [RegistryUp::Result(from, r)]
            if from == &w4 && r.call_id == "c1" && r.outcome == ToolOutcome::Failed && r.text.contains("result: payload over wire cap")),
            "the registry receives the failed result for the original endpoint and call");

        let progress = ServiceUp::Progress { call_id: "c2".into(), note: String::new(), permille: 1001, task: None };
        assert!(bus.on_custom(&registry, &frame(progress)), "the bus consumes invalid progress");
        assert!(registry.pump().is_empty(), "invalid progress leaves the call open");
    }

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
