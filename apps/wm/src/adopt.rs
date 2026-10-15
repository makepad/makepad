//! Apps that come to the WM by themselves (Linux). The WM publishes its socket
//! (`$XDG_RUNTIME_DIR/makepad-wm-<n>`, `makepad_hal::wm_socket`) and hands its path to every
//! child as `MAKEPAD_WM_SOCKET`. An app started anywhere in the session -- from a terminal,
//! from Code's run panel, from a script -- asks it first: a WM speaking its hosted wire gives
//! it a client id on the hub, and the app connects as that client exactly like an app the WM
//! launched. Its window becomes a tile like any other; the process stays its launcher's (no
//! child handle here), so the tile goes when the app's hub socket closes.
use super::*;
use makepad_widgets::makepad_platform::thread::ThreadOptions;
use makepad_widgets::makepad_platform::makepad_hal::wm_socket::{self, Admission, WmSocket};
use std::sync::mpsc::{channel, Receiver};

/// Ids the socket hands out: far above the WM's own counter, so the two never meet.
const FIRST_ADOPTED_CLIENT: ClientId = 1 << 32;

pub(crate) struct Adopted {
    pub client: ClientId,
    pub pid: u32,
    pub crate_name: String,
}

pub(crate) struct Adoptions {
    pub path: std::path::PathBuf,
    rx: Receiver<Adopted>,
}

impl Adoptions {
    /// Publish the socket and serve it on a worker of its own; children learn its path.
    pub fn start(cx: &mut Cx, hub_port: u16) -> Option<Adoptions> {
        let Some(dir) = wm_socket::runtime_dir() else {
            log!("wm: XDG_RUNTIME_DIR is not set: apps started outside the WM cannot find it");
            return None;
        };
        // A hidden WM (tests, agents) never takes the path apps find by default.
        let first = if std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some() { 1 } else { 0 };
        let socket = match WmSocket::bind(&dir, first) {
            Ok(socket) => socket,
            Err(e) => {
                log!("wm: cannot publish the WM socket in {}: {e}", dir.display());
                return None;
            }
        };
        let path = socket.path.clone();
        host::set_child_env(wm_socket::SOCKET_ENV, path.as_os_str());
        let (tx, rx) = channel();
        let worker = cx.thread_spawner().spawn_worker(ThreadOptions { name: Some("wm-socket".into()), ..Default::default() }, move || {
            let mut next = FIRST_ADOPTED_CLIENT;
            for stream in socket.listener.incoming() {
                let Ok(stream) = stream else { continue };
                let served = WmSocket::serve(stream, makepad_studio_protocol::WIRE, |request| {
                    let client = next;
                    next += 1;
                    tx.send(Adopted { client, pid: request.pid, crate_name: request.crate_name.clone() })
                        .map_err(|_| "the WM is shutting down".to_string())?;
                    SignalToUI::set_ui_signal();
                    Ok(Admission { port: hub_port, client })
                });
                if let Err(e) = served {
                    log!("wm: socket: {e}");
                }
            }
        });
        match worker {
            Ok(worker) => worker.detach(),
            Err(e) => {
                log!("wm: cannot serve the WM socket: {e:?}");
                return None;
            }
        }
        log!("wm: published {} for apps started in this session", path.display());
        Some(Adoptions { path, rx })
    }
}

impl App {
    /// Make a tile for every app the socket admitted. Runs before the hub's events, so the
    /// client exists when its websocket connects.
    pub(crate) fn drain_adoptions(&mut self, cx: &mut Cx) {
        let Some(adoptions) = self.adoptions.as_ref() else { return };
        let adopted: Vec<Adopted> = adoptions.rx.try_iter().collect();
        for Adopted { client, pid, crate_name } in adopted {
            let entry = makepad_app_registry::apps().iter().find(|a| a.package == crate_name);
            let (app, label) = entry.map_or((crate_name.clone(), crate_name.clone()), |e| (e.id.to_string(), e.label.to_string()));
            log!("wm: {label} (pid {pid}) came to the WM by itself; client {client}");
            let slot = clients::ClientSlot::adopted(client, &app);
            let area = self.desk_area(cx);
            let state = self.state_mut();
            state.clients.insert(client, slot);
            let gap = state.gap;
            state.layout_mut().insert(client, area, gap);
            let hub_port = state.hub_port;
            self.desk(cx)
                .borrow_mut::<WmDesk>()
                .map(|mut d| d.with_run_view(cx, client, |cx, v| v.set_run_target(cx, client, 0, hub_port)));
            self.activate_client(cx, client);
            self.redraw_all(cx);
        }
    }
}
