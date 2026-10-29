//! Hosted process endpoints. The shell and domains stay in the local HAL mirror;
//! each process keeps its own IDs, snapshots and action completion stream.
use makepad_app_host::{a11y::RemoteUi, hub::send_to_app, ClientId};
use makepad_remote::{json_str, Out, Req};
use makepad_studio_protocol::AppToStudio;
use std::{collections::BTreeMap, sync::{mpsc::Sender, Mutex, OnceLock}};

#[derive(Clone)]
struct Child { name: String, ui: RemoteUi, sender: Sender<Vec<u8>> }
fn children() -> &'static Mutex<BTreeMap<ClientId, Child>> {
    static CHILDREN: OnceLock<Mutex<BTreeMap<ClientId, Child>>> = OnceLock::new();
    CHILDREN.get_or_init(Default::default)
}
pub fn connected(id: ClientId, name: &str, sender: Sender<Vec<u8>>) {
    disconnected(id);
    children().lock().unwrap().insert(id, Child { name: name.into(), ui: RemoteUi::default(), sender });
}
pub fn disconnected(id: ClientId) {
    let old = children().lock().unwrap().remove(&id);
    if let Some(mut old) = old { old.ui.reset(); }
}
pub fn apply(id: ClientId, msg: &AppToStudio) -> bool {
    if !matches!(msg, AppToStudio::A11yUpdate { .. } | AppToStudio::A11yActionDone { .. } | AppToStudio::A11yReadDone { .. }) { return false; }
    let ui = children().lock().unwrap().get(&id).map(|c| c.ui.clone());
    if let Some(ui) = ui { ui.apply(msg); }
    true
}
pub fn flush() {
    let children: Vec<_> = children().lock().unwrap().values().map(|c| (c.ui.clone(), c.sender.clone())).collect();
    for (ui, sender) in children { send_to_app(&sender, ui.drain()); }
}
pub fn dispatch(req: &Req, out: &mut Out) -> bool {
    if req.get("machine").is_some() { return false; }
    let children: Vec<_> = children().lock().unwrap().iter().map(|(id,c)| (*id,c.clone())).collect();
    if let Some(app) = req.get("app") {
        if app == "shell" { return makepad_hal::a11y::dispatch_local(req, out); }
        let matches: Vec<_> = children.iter().filter(|(id,c)| id.to_string() == app || c.name == app).collect();
        match matches.as_slice() {
            [(_, child)] => { child.ui.dispatch(req, out); }
            [] => out.err("no such hosted app; ui lists connected apps (app=shell selects local sessions)"),
            _ => out.err("ambiguous app name; use app=CLIENT_ID from ui"),
        }
        return true;
    }
    // Window selectors and commands with state/action scope address the local tree by default.
    // A child is always selected explicitly before a mutation, so repeated IDs cannot collide.
    if req.get("w").is_some() || !matches!(req.pos.first().map_or("", String::as_str), "" | "find" | "lint" | "speak") { return false; }
    makepad_hal::a11y::dispatch_local(req, out);
    for (id, child) in children {
        if req.get("json") == Some("1") {
            out.line(&format!("{{\"app\":\"{id}\",\"name\":{}}}", json_str(&child.name)));
        } else { out.line(&format!("app={id} {}", json_str(&child.name))); }
        child.ui.dispatch(req, out);
    }
    true
}
