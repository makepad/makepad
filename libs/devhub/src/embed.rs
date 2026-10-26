//! What a JIT host that embeds the node (tools/run/android_host/src/devhub.rs) hands devhub
//! besides its source store: the switch and the state of its Desk Link endpoint (the deskhost
//! engine, `makepad_desk_link::host`, serving the host's own render). The node reports the state
//! in its caps (`desk_link`), so the master lists the endpoint (`devhub nodes`, the viewer's
//! `~/.makepad/devhub/desk-links`), and serves `desk on|off|state` to the master
//! (`makepad devhub desk <node> on|off`).

use std::sync::{Arc, Mutex};

/// The endpoint as devhub sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeskLinkState {
    /// Switched on (it may still be waiting, see `note`).
    pub on: bool,
    /// The port it listens on (0: not listening).
    pub port: u16,
    /// One line: listening, waiting and why, off.
    pub note: String,
}

impl DeskLinkState {
    pub fn to_json(&self) -> String {
        format!("{{\"on\":{},\"port\":{},\"note\":{}}}", self.on, self.port, makepad_remote::json_str(&self.note))
    }

    pub fn parse(v: &makepad_strict_json::Value) -> Option<DeskLinkState> {
        let on = v.get("on")?.as_bool()?;
        let port = match v.get("port") {
            Some(makepad_strict_json::Value::Int(p)) => *p as u16,
            _ => 0,
        };
        let note = v.get("note").and_then(|x| x.as_str()).unwrap_or("").to_string();
        Some(DeskLinkState { on, port, note })
    }
}

/// The host's side of the endpoint.
pub struct DeskLinkControl {
    /// Switches the endpoint on or off; Ok: what it does now (one line).
    pub set: Box<dyn Fn(bool) -> Result<String, String> + Send + Sync>,
    /// Its state now.
    pub state: Box<dyn Fn() -> DeskLinkState + Send + Sync>,
}

static DESK_LINK: Mutex<Option<Arc<DeskLinkControl>>> = Mutex::new(None);

/// Installs the host's Desk Link endpoint (once, at the host's start, before or after the node).
pub fn set_desk_link_control(control: DeskLinkControl) {
    *DESK_LINK.lock().unwrap() = Some(Arc::new(control));
}

/// The endpoint's control, on a host that has one.
pub fn desk_link_control() -> Option<Arc<DeskLinkControl>> {
    DESK_LINK.lock().unwrap().clone()
}

/// The endpoint's state, on a host that has one.
pub fn desk_link_state() -> Option<DeskLinkState> {
    desk_link_control().map(|c| (c.state)())
}

/// The node's `desk` service: `desk on`, `desk off`, `desk` / `desk state`; one JSON line.
pub fn desk_service(line: &str) -> String {
    let word = line.split_whitespace().find(|w| *w != "desk").unwrap_or("state");
    let Some(c) = desk_link_control() else {
        return "{\"err\":\"this node has no Desk Link endpoint (only JIT hosts that embed one)\"}".to_string();
    };
    let said = match word {
        "on" | "off" => match (c.set)(word == "on") {
            Ok(s) => s,
            Err(e) => return format!("{{\"err\":{}}}", makepad_remote::json_str(&e)),
        },
        "state" => String::new(),
        _ => return "{\"err\":\"desk on|off|state\"}".to_string(),
    };
    let state = (c.state)();
    format!("{{\"desk_link\":{},\"said\":{}}}", state.to_json(), makepad_remote::json_str(&said))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endpoint_switches_through_the_service() {
        assert!(desk_service("desk on").contains("no Desk Link endpoint"));
        let on = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (a, b) = (on.clone(), on.clone());
        set_desk_link_control(DeskLinkControl {
            set: Box::new(move |v| {
                a.store(v, std::sync::atomic::Ordering::Release);
                Ok(format!("switched {}", if v { "on" } else { "off" }))
            }),
            state: Box::new(move || {
                let on = b.load(std::sync::atomic::Ordering::Acquire);
                DeskLinkState { on, port: if on { 8790 } else { 0 }, note: if on { "listens on 10.0.0.201:8790".into() } else { "off".into() } }
            }),
        });
        let line = desk_service("desk on");
        assert!(line.contains("\"on\":true") && line.contains("\"port\":8790") && line.contains("switched on"), "{line}");
        let v = makepad_strict_json::parse(line.as_bytes()).unwrap();
        assert_eq!(DeskLinkState::parse(v.get("desk_link").unwrap()).unwrap().port, 8790);
        assert!(desk_service("desk").contains("\"on\":true"));
        assert!(desk_service("desk off").contains("\"on\":false"));
        assert!(desk_service("desk sideways").contains("err"));
    }
}
