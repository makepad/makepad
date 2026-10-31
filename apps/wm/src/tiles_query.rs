//! `makepad inspect <wm> tiles [remove <client>]`: the WM's client tiles as JSON lines (client,
//! app, title, pid, whether its hub socket is connected, whether its process lives), and the
//! removal of a dead one by its client id: a tile whose socket is gone, kept by mistake (a
//! restart that came back as another client before the WM kept the pid's client). A connected
//! tile is refused: closing a live app is the person's, through its window. Removing a tile
//! never touches a process.
use super::*;
use std::sync::Mutex;

/// The tiles as last listed (`App::tiles_pass`), one JSON line each.
static SNAPSHOT: Mutex<Vec<(ClientId, bool, String)>> = Mutex::new(Vec::new());
/// Removals asked for, applied by the WM at its next pass.
static REMOVE: Mutex<Vec<ClientId>> = Mutex::new(Vec::new());

pub(crate) fn register() {
    makepad_widgets::makepad_platform::remote::register_app_query(
        "tiles",
        "[remove <client>]",
        "the WM's client tiles (client, app, title, pid, connected, alive); remove <client>: drop a dead tile (socket gone); a connected one is refused",
        query,
    );
}

fn query(_cx: &mut Cx, words: &[String], _kv: &[(String, String)]) -> Vec<String> {
    let snap = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    match words.get(1).map(String::as_str) {
        None | Some("list") => snap.iter().map(|(_, _, l)| l.clone()).collect(),
        Some("remove") => {
            let Some(id) = words.get(2).and_then(|w| w.parse::<ClientId>().ok()) else { return vec!["{\"err\":\"tiles remove <client>\"}".into()] };
            match snap.iter().find(|(c, _, _)| *c == id) {
                None => vec![format!("{{\"err\":\"no tile of client {id}\"}}")],
                Some((_, true, _)) => vec![format!("{{\"err\":\"client {id} is connected: close it through its window\"}}")],
                Some(_) => {
                    REMOVE.lock().unwrap_or_else(|e| e.into_inner()).push(id);
                    SignalToUI::set_ui_signal();
                    vec![format!("{{\"remove\":{id},\"queued\":true,\"note\":\"`tiles` lists it gone once the WM took it\"}}")]
                }
            }
        }
        Some(_) => vec!["{\"err\":\"tiles [remove <client>]\"}".into()],
    }
}

impl App {
    /// Applies the removals asked for (dead tiles only); one lock when none is.
    pub(crate) fn tiles_remove_pass(&mut self, cx: &mut Cx) {
        let asked: Vec<ClientId> = std::mem::take(&mut *REMOVE.lock().unwrap_or_else(|e| e.into_inner()));
        if asked.is_empty() {
            return;
        }
        for id in asked {
            let dead = self.state_mut().clients.get(&id).is_some_and(|s| s.socket.is_none());
            if dead {
                log!("wm: tiles remove {id}: a dead tile (no socket); removing it");
                self.remove_client(cx, id);
                self.update_bar(cx);
            }
        }
        self.tiles_list();
    }

    /// Lists the tiles for `tiles` (with the reaper's pass).
    pub(crate) fn tiles_list(&mut self) {
        let Some(state) = self.state.as_mut() else { return };
        let mut lines = Vec::new();
        for (id, slot) in state.clients.iter_mut() {
            #[cfg(target_os = "linux")]
            let adopted = adopt::pid_of(*id);
            #[cfg(not(target_os = "linux"))]
            let adopted: Option<u32> = None;
            let pid = slot.external.or(adopted).or_else(|| slot.child.as_ref().map(|c| c.id()));
            let alive = pid.is_some_and(makepad_app_host::launch::process_alive);
            let connected = slot.socket.is_some();
            let line = format!(
                "{{\"client\":{id},\"app\":{},\"title\":{},\"pid\":{},\"connected\":{connected},\"alive\":{alive},\"window\":{},\"parent\":{}}}",
                makepad_remote::json_str(&slot.app),
                makepad_remote::json_str(&slot.title),
                pid.map_or("null".into(), |p| p.to_string()),
                slot.window_id,
                slot.parent.map_or("null".into(), |p| p.to_string())
            );
            lines.push((*id, connected, line));
        }
        lines.sort_by_key(|l| l.0);
        *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = lines;
    }
}
