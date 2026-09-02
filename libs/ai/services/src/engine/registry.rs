//! The connected services: who is there, where each instance lives, what
//! it can do, and the channels to it.
//!
//! The registry is the host's authority on identity. It issues every
//! [`EndpointId`], answers each port's `Register` with `Registered`,
//! stamps nothing on trust, and forgets an instance the moment its link
//! dies. It also holds the host's risk floors: a service may declare a
//! tool riskier than the floor, never safer.
//!
//! Shared by handle (`Clone` = same registry) between the host, which
//! plugs links in and out as apps come and go, and the engine core, which
//! reads the tool table and routes calls.

use crate::port::ServiceLink;
use crate::state::ServiceInfo;
use crate::wire::*;
use crate::engine::ToolDefinition;
use makepad_platform::thread::SignalToUI;
use std::collections::HashMap;
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};

/// Instances one host will register at once.
pub const MAX_INSTANCES: usize = 32;

struct Entry {
    manifest: ServiceManifest,
    meta: InstanceMeta,
    context: String,
    link: ServiceLink,
    /// The port's nonce, once its `Register` has been seen.
    port_tag: Option<u32>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<EndpointId, Entry>,
    /// Registration order, so listings are stable.
    order: Vec<EndpointId>,
    next_endpoint: u64,
    next_focus: u64,
    generation: u64,
    /// Apps the host can start that are not running: `(id, label)`.
    launchable: Vec<(String, String)>,
    /// Per app id: the least risk any of its tools is treated as.
    risk_floors: HashMap<String, Risk>,
}

/// One frame from a service, as the core receives it.
#[derive(Clone, Debug, PartialEq)]
pub enum RegistryUp {
    Result(EndpointId, ToolResult),
    Progress { endpoint: EndpointId, call_id: String, note: String, permille: u16 },
}

#[derive(Clone, Default)]
pub struct ServiceRegistry {
    inner: Arc<Mutex<Inner>>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Plug a service in. Issues its endpoint at once (the manifest came
    /// with the link); the `Registered` answer goes down as soon as the
    /// port's `Register` frame is seen in `pump`. `location` is the host's
    /// one-line placement; `parent` the enclosing instance, if nested.
    pub fn register(&self, link: ServiceLink, location: &str, parent: Option<EndpointId>) -> Result<EndpointId, String> {
        link.manifest.validate()?;
        let mut inner = self.inner.lock().unwrap();
        if inner.entries.len() >= MAX_INSTANCES {
            return Err(format!("too many instances ({MAX_INSTANCES})"));
        }
        if let Some(p) = &parent {
            if !inner.entries.contains_key(p) {
                return Err("parent endpoint is not registered".into());
            }
        }
        inner.next_endpoint += 1;
        let endpoint = EndpointId(format!("e{}", inner.next_endpoint));
        let same_app = inner.order.iter().filter(|e| inner.entries[*e].manifest.id == link.manifest.id).count();
        let display_name = if same_app == 0 {
            link.manifest.label.clone()
        } else {
            format!("{} ({})", link.manifest.label, same_app + 1)
        };
        let mut location = location.to_string();
        truncate_to_char_boundary(&mut location, MAX_META_BYTES);
        let meta = InstanceMeta {
            app_id: link.manifest.id.clone(),
            display_name,
            parent,
            location,
            focus_epoch: 0,
        };
        inner.entries.insert(
            endpoint.clone(),
            Entry { manifest: link.manifest.clone(), meta, context: String::new(), link, port_tag: None },
        );
        inner.order.push(endpoint.clone());
        inner.generation += 1;
        Ok(endpoint)
    }

    /// Plug in a service whose endpoint ANOTHER host already issued — the
    /// window manager's bus hands the aichat child endpoints it minted, and
    /// this registry adopts them instead of minting its own.
    pub fn register_as(
        &self,
        link: ServiceLink,
        endpoint: EndpointId,
        location: &str,
        parent: Option<EndpointId>,
    ) -> Result<(), String> {
        link.manifest.validate()?;
        if !is_opaque_id(endpoint.as_str()) {
            return Err("bad endpoint".into());
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.entries.contains_key(&endpoint) {
            return Err("endpoint already registered".into());
        }
        if inner.entries.len() >= MAX_INSTANCES {
            return Err(format!("too many instances ({MAX_INSTANCES})"));
        }
        let same_app = inner.order.iter().filter(|e| inner.entries[*e].manifest.id == link.manifest.id).count();
        let display_name = if same_app == 0 {
            link.manifest.label.clone()
        } else {
            format!("{} ({})", link.manifest.label, same_app + 1)
        };
        let mut location = location.to_string();
        truncate_to_char_boundary(&mut location, MAX_META_BYTES);
        let meta = InstanceMeta { app_id: link.manifest.id.clone(), display_name, parent, location, focus_epoch: 0 };
        inner.entries.insert(
            endpoint.clone(),
            Entry { manifest: link.manifest.clone(), meta, context: String::new(), link, port_tag: None },
        );
        inner.order.push(endpoint);
        inner.generation += 1;
        Ok(())
    }

    /// Forget an instance: its link is dropped, its children with it.
    pub fn unregister(&self, endpoint: &EndpointId) {
        let mut inner = self.inner.lock().unwrap();
        Self::remove(&mut inner, endpoint);
    }

    fn remove(inner: &mut Inner, endpoint: &EndpointId) {
        if inner.entries.remove(endpoint).is_none() {
            return;
        }
        inner.order.retain(|e| e != endpoint);
        inner.generation += 1;
        let children: Vec<EndpointId> = inner
            .entries
            .iter()
            .filter(|(_, e)| e.meta.parent.as_ref() == Some(endpoint))
            .map(|(k, _)| k.clone())
            .collect();
        for child in children {
            Self::remove(inner, &child);
        }
    }

    /// The host says this instance is the focused one now.
    pub fn focus(&self, endpoint: &EndpointId) {
        let mut inner = self.inner.lock().unwrap();
        inner.next_focus += 1;
        let epoch = inner.next_focus;
        if let Some(e) = inner.entries.get_mut(endpoint) {
            e.meta.focus_epoch = epoch;
        }
    }

    pub fn set_location(&self, endpoint: &EndpointId, location: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(e) = inner.entries.get_mut(endpoint) {
            let mut location = location.to_string();
            truncate_to_char_boundary(&mut location, MAX_META_BYTES);
            e.meta.location = location;
        }
    }

    /// An app the host can start (`os.launch`) that is not running.
    pub fn mark_launchable(&self, id: &str, label: &str) {
        let mut inner = self.inner.lock().unwrap();
        if !inner.launchable.iter().any(|(i, _)| i == id) {
            inner.launchable.push((id.to_string(), label.to_string()));
            inner.generation += 1;
        }
    }

    /// The host's floor for an app's tools. A tool declared below it is
    /// treated as the floor.
    pub fn set_risk_floor(&self, app_id: &str, risk: Risk) {
        self.inner.lock().unwrap().risk_floors.insert(app_id.to_string(), risk);
    }

    pub fn risk_floor(&self, app_id: &str) -> Risk {
        self.inner.lock().unwrap().risk_floors.get(app_id).copied().unwrap_or(Risk::Read)
    }

    /// Bumps on every change that alters the tool table or the instance
    /// list; the core reconfigures the model when it moves.
    pub fn generation(&self) -> u64 {
        self.inner.lock().unwrap().generation
    }

    pub fn manifest(&self, endpoint: &EndpointId) -> Option<ServiceManifest> {
        self.inner.lock().unwrap().entries.get(endpoint).map(|e| e.manifest.clone())
    }

    pub fn meta(&self, endpoint: &EndpointId) -> Option<InstanceMeta> {
        self.inner.lock().unwrap().entries.get(endpoint).map(|e| e.meta.clone())
    }

    /// Running instances of one app, in registration order.
    pub fn instances_of(&self, app_id: &str) -> Vec<EndpointId> {
        let inner = self.inner.lock().unwrap();
        inner.order.iter().filter(|e| inner.entries[*e].manifest.id == app_id).cloned().collect()
    }

    /// The endpoint the model most likely means for an app: the one it
    /// named, else the most recently focused, else the only one.
    pub fn pick_instance(&self, app_id: &str, wanted: Option<&str>) -> Option<EndpointId> {
        let inner = self.inner.lock().unwrap();
        let mut candidates: Vec<&EndpointId> =
            inner.order.iter().filter(|e| inner.entries[*e].manifest.id == app_id).collect();
        if candidates.is_empty() {
            return None;
        }
        if let Some(w) = wanted {
            let w = w.trim();
            if let Some(hit) = candidates.iter().find(|e| {
                e.as_str() == w || inner.entries[*e].meta.display_name.eq_ignore_ascii_case(w)
            }) {
                return Some((*hit).clone());
            }
        }
        candidates.sort_by_key(|e| std::cmp::Reverse(inner.entries[*e].meta.focus_epoch));
        candidates.first().map(|e| (*e).clone())
    }

    /// Whether an app is known but not running.
    pub fn is_launchable(&self, app_id: &str) -> bool {
        self.inner.lock().unwrap().launchable.iter().any(|(i, _)| i == app_id)
    }

    /// Every app id with a running instance or a launchable entry.
    pub fn known_app_ids(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        let mut ids: Vec<String> = Vec::new();
        for e in &inner.order {
            let id = &inner.entries[e].manifest.id;
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        for (id, _) in &inner.launchable {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids
    }

    /// What the apps row shows.
    pub fn services(&self) -> Vec<ServiceInfo> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<ServiceInfo> = inner
            .order
            .iter()
            .map(|e| {
                let entry = &inner.entries[e];
                ServiceInfo {
                    id: entry.manifest.id.clone(),
                    endpoint: e.as_str().to_string(),
                    label: entry.meta.display_name.clone(),
                    parent: entry.meta.parent.as_ref().map(|p| p.as_str().to_string()),
                    location: entry.meta.location.clone(),
                    connected: true,
                    launchable: false,
                    tool_count: entry.manifest.tools.len(),
                }
            })
            .collect();
        for (id, label) in &inner.launchable {
            if !out.iter().any(|s| &s.id == id) {
                out.push(ServiceInfo {
                    id: id.clone(),
                    endpoint: String::new(),
                    label: label.clone(),
                    parent: None,
                    location: "not running".into(),
                    connected: false,
                    launchable: true,
                    tool_count: 0,
                });
            }
        }
        out
    }

    /// The tool table for the model: each app once, canonical names.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let inner = self.inner.lock().unwrap();
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for e in &inner.order {
            let m = &inner.entries[e].manifest;
            if seen.contains(&m.id) {
                continue;
            }
            seen.push(m.id.clone());
            for t in &m.tools {
                out.push(ToolDefinition {
                    name: canonical_name(&m.id, &t.name),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                });
            }
        }
        out
    }

    /// The services section of the system prompt: each app's brief once,
    /// then where its instances are.
    pub fn briefs(&self) -> String {
        let inner = self.inner.lock().unwrap();
        let mut out = String::new();
        let mut seen: Vec<String> = Vec::new();
        for e in &inner.order {
            let entry = &inner.entries[e];
            let m = &entry.manifest;
            if seen.contains(&m.id) {
                continue;
            }
            seen.push(m.id.clone());
            let instances: Vec<String> = inner
                .order
                .iter()
                .filter(|x| inner.entries[*x].manifest.id == m.id)
                .map(|x| {
                    let me = &inner.entries[x].meta;
                    if me.location.is_empty() {
                        format!("{} [{}]", me.display_name, x.as_str())
                    } else {
                        format!("{} [{}] — {}", me.display_name, x.as_str(), me.location)
                    }
                })
                .collect();
            out.push_str(&format!("## {} (tools `{}.*`)\n{}\n", m.label, m.id, m.brief.trim()));
            if instances.len() > 1 {
                out.push_str("Running instances (say which with the `instance` argument when it matters): ");
                out.push_str(&instances.join("; "));
                out.push('\n');
            } else if let Some(one) = instances.first() {
                out.push_str(&format!("Running: {one}\n"));
            }
            out.push('\n');
        }
        if !inner.launchable.is_empty() {
            let names: Vec<String> = inner
                .launchable
                .iter()
                .filter(|(id, _)| !seen.contains(id))
                .map(|(id, label)| format!("{label} (`{id}`)"))
                .collect();
            if !names.is_empty() {
                out.push_str("Not running but available through `os.launch`: ");
                out.push_str(&names.join(", "));
                out.push('\n');
            }
        }
        out
    }

    /// The volatile per-turn context every instance last sent, labelled.
    pub fn dynamic_context(&self) -> String {
        let inner = self.inner.lock().unwrap();
        let mut out = String::new();
        for e in &inner.order {
            let entry = &inner.entries[e];
            if !entry.context.trim().is_empty() {
                out.push_str(&format!("[{}] {}\n", entry.meta.display_name, entry.context.trim()));
            }
        }
        out
    }

    /// Send one frame down to an instance. False when it is gone. An
    /// in-process port drains its link on the app's events, so the UI is
    /// woken: a call must not wait for the next mouse move.
    pub fn send(&self, endpoint: &EndpointId, msg: ServiceDown) -> bool {
        let inner = self.inner.lock().unwrap();
        let sent = match inner.entries.get(endpoint) {
            Some(e) => e.link.down.send(HostedDown { to: Some(endpoint.clone()), msg }).is_ok(),
            None => false,
        };
        if sent {
            SignalToUI::set_ui_signal();
        }
        sent
    }

    /// Tell every instance whether the chat pane is showing.
    pub fn broadcast_chat_open(&self, open: bool) {
        let inner = self.inner.lock().unwrap();
        for (endpoint, e) in &inner.entries {
            let _ = e.link.down.send(HostedDown { to: Some(endpoint.clone()), msg: ServiceDown::ChatOpen { open } });
        }
        SignalToUI::set_ui_signal();
    }

    /// Drain every link. Registrations are answered, contexts stored,
    /// dead links removed; results and progress go to the caller.
    pub fn pump(&self) -> Vec<RegistryUp> {
        let mut inner = self.inner.lock().unwrap();
        let mut out = Vec::new();
        let mut dead: Vec<EndpointId> = Vec::new();
        let endpoints: Vec<EndpointId> = inner.order.clone();
        for endpoint in endpoints {
            loop {
                let frame = {
                    let entry = inner.entries.get_mut(&endpoint).unwrap();
                    match entry.link.up.try_recv() {
                        Ok(f) => f,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            dead.push(endpoint.clone());
                            break;
                        }
                    }
                };
                // A frame's own `from` is never trusted: the link IS the sender.
                if frame.msg.validate().is_err() {
                    continue;
                }
                match frame.msg {
                    ServiceUp::Register { manifest, port_tag } => {
                        let entry = inner.entries.get_mut(&endpoint).unwrap();
                        let changed = entry.manifest != manifest;
                        entry.manifest = manifest;
                        entry.port_tag = Some(port_tag);
                        let _ = entry.link.down.send(HostedDown {
                            to: None,
                            msg: ServiceDown::Registered { port_tag, endpoint: endpoint.clone() },
                        });
                        if changed {
                            inner.generation += 1;
                        }
                    }
                    ServiceUp::Result(r) => out.push(RegistryUp::Result(endpoint.clone(), r)),
                    ServiceUp::Progress { call_id, note, permille } => {
                        out.push(RegistryUp::Progress { endpoint: endpoint.clone(), call_id, note, permille })
                    }
                    ServiceUp::Context(c) => {
                        inner.entries.get_mut(&endpoint).unwrap().context = c.text;
                    }
                    ServiceUp::Unregister => dead.push(endpoint.clone()),
                }
            }
        }
        for d in dead {
            Self::remove(&mut inner, &d);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::AiServicePort;

    fn app(id: &str, label: &str, tools: &[&str]) -> ServiceManifest {
        let mut m = ServiceManifest::new(id, label, format!("The {label} app."));
        for t in tools {
            m = m.with_tool(ToolDef::new(*t, "Does it.", r#"{"type":"object","properties":{}}"#, Risk::Act));
        }
        m
    }

    #[test]
    fn registration_issues_endpoints_answers_ports_and_builds_the_table() {
        let reg = ServiceRegistry::new();
        let (mut p1, l1) = AiServicePort::in_process(app("route", "Route", &["plan", "status"])).unwrap();
        let (mut p2, l2) = AiServicePort::in_process(app("route", "Route", &["plan", "status"])).unwrap();
        let (mut p3, l3) = AiServicePort::in_process(app("files", "Files", &["list_dir"])).unwrap();
        let e1 = reg.register(l1, "workspace 1", None).unwrap();
        let e2 = reg.register(l2, "workspace 2", None).unwrap();
        let e3 = reg.register(l3, "", Some(e1.clone())).unwrap();
        assert_ne!(e1, e2);
        let gen_before = reg.generation();
        assert!(reg.pump().is_empty());
        assert_eq!(reg.generation(), gen_before, "a matching re-register does not bump");
        // Every port learned its own endpoint and nobody else's.
        // handle_event needs a Cx; the test seam drains the transport directly.
        let cx_free = |p: &mut AiServicePort| p.test_drain();
        assert_eq!(cx_free(&mut p1), vec![crate::port::PortEvent::Registered(e1.clone())]);
        assert_eq!(cx_free(&mut p2), vec![crate::port::PortEvent::Registered(e2.clone())]);
        assert_eq!(cx_free(&mut p3), vec![crate::port::PortEvent::Registered(e3.clone())]);
        // The table lists each app once; the briefs list both Route instances.
        let names: Vec<String> = reg.tool_definitions().into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["route.plan", "route.status", "files.list_dir"]);
        let briefs = reg.briefs();
        assert!(briefs.contains("Route (2)"), "{briefs}");
        assert!(briefs.contains("workspace 2"));
        // Instance choice: focus wins, a name wins over focus.
        reg.focus(&e2);
        assert_eq!(reg.pick_instance("route", None), Some(e2.clone()));
        assert_eq!(reg.pick_instance("route", Some("Route")), Some(e1.clone()));
        assert_eq!(reg.pick_instance("route", Some(e1.as_str())), Some(e1.clone()));
        assert_eq!(reg.pick_instance("photos", None), None);
        // Nested instance goes with its parent.
        reg.unregister(&e1);
        assert!(reg.manifest(&e3).is_none(), "children die with the parent");
        assert_eq!(reg.instances_of("route"), vec![e2.clone()]);
        let infos = reg.services();
        assert_eq!(infos.len(), 1);
        assert!(infos[0].connected);
    }

    #[test]
    fn a_dead_port_is_forgotten_and_launchables_are_listed() {
        let reg = ServiceRegistry::new();
        let (port, link) = AiServicePort::in_process(app("files", "Files", &["stat"])).unwrap();
        let e = reg.register(link, "", None).unwrap();
        reg.mark_launchable("route", "Route");
        assert_eq!(reg.services().len(), 2);
        assert!(reg.is_launchable("route"));
        assert!(reg.briefs().contains("os.launch"));
        drop(port);
        reg.pump();
        assert!(reg.manifest(&e).is_none());
        assert_eq!(reg.services().len(), 1);
        assert!(!reg.services()[0].connected);
    }

    #[test]
    fn results_and_context_flow_up_and_calls_flow_down() {
        let reg = ServiceRegistry::new();
        let (mut port, link) = AiServicePort::in_process(app("files", "Files", &["stat"])).unwrap();
        let e = reg.register(link, "", None).unwrap();
        reg.pump();
        port.test_drain();
        assert!(reg.send(&e, ServiceDown::Call(ServiceCall { call_id: "c1".into(), tool: "stat".into(), args: "{}".into() })));
        let ev = port.test_drain();
        assert!(matches!(&ev[0], crate::port::PortEvent::Call(c) if c.call_id == "c1"));
        port.set_context("cwd=~");
        port.progress("c1", "reading", 500);
        port.reply(ToolResult::ok("c1", "12 bytes", "stat"));
        let ups = reg.pump();
        assert_eq!(ups.len(), 2);
        assert!(matches!(&ups[0], RegistryUp::Progress { call_id, .. } if call_id == "c1"));
        assert!(matches!(&ups[1], RegistryUp::Result(ep, r) if ep == &e && r.call_id == "c1"));
        assert_eq!(reg.dynamic_context(), "[Files] cwd=~\n");
        assert!(!reg.send(&EndpointId("nope".into()), ServiceDown::ChatOpen { open: true }));
    }
}
