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
        let mut gone: Vec<EndpointId> = Vec::new();
        for (endpoint, host) in &self.hosts {
            loop {
                match host.down.try_recv() {
                    Ok(mut frame) => {
                        // `Registered` travels without a target; the WM
                        // routes it by the endpoint we register as.
                        if frame.to.is_none() {
                            frame.to = Some(endpoint.clone());
                        }
                        Cx::send_studio_message(AppToStudio::Custom(frame.to_json()));
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
    }

    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }
}
