//! Apps that come to the WM by themselves (Linux). The WM publishes its socket
//! (`$XDG_RUNTIME_DIR/makepad-wm-<n>`, `makepad_hal::wm_socket`) and hands its path to every
//! child as `MAKEPAD_WM_SOCKET`. An app started anywhere in the session -- from a terminal,
//! from Code's run panel, from a script -- asks it first: a WM speaking its hosted wire gives
//! it a client id on the hub, and the app connects as that client exactly like an app the WM
//! launched. Its window becomes a tile like any other; the process stays its launcher's (no
//! child handle here), so the tile goes when the app's hub socket closes, when the same pid
//! asks again, or when the pid is gone (`App::reap_exited`).
use super::*;
use makepad_widgets::makepad_platform::thread::ThreadOptions;
use makepad_widgets::makepad_platform::makepad_hal::wm_socket::{self, Admission, WmSocket};
use std::sync::mpsc::{channel, Receiver};

/// Ids the socket hands out: far above the WM's own counter, so the two never meet.
const FIRST_ADOPTED_CLIENT: ClientId = 1 << 32;

/// Processes the WM launched itself that will ask the socket for their window (a JIT run:
/// `makepad run`'s HAL finds the WM by its socket, it is not a `--stdin-loop` child): pid -> the
/// client slot the launch already made. Their request is admitted as that client, so the
/// launch's tile is the window; anything else gets a new tile.
static EXPECTED: std::sync::Mutex<Vec<(u32, ClientId)>> = std::sync::Mutex::new(Vec::new());

/// The WM started `pid` as client `client`; it will come to the socket by itself.
pub(crate) fn expect(pid: u32, client: ClientId) {
    let mut expected = EXPECTED.lock().unwrap_or_else(|e| e.into_inner());
    expected.retain(|(p, c)| *p != pid && *c != client);
    expected.push((pid, client));
}

/// The client a launch made for `pid`, once.
fn expected_client(pid: u32) -> Option<ClientId> {
    let mut expected = EXPECTED.lock().unwrap_or_else(|e| e.into_inner());
    let at = expected.iter().position(|(p, _)| *p == pid)?;
    Some(expected.remove(at).1)
}

pub(crate) struct Adopted {
    pub client: ClientId,
    pub pid: u32,
    pub crate_name: String,
    /// The WM launched it (`expect`): its slot and tile exist already.
    pub launched: bool,
    /// A lane's test instance (`lane=` on the socket).
    pub test_lane: Option<String>,
}

pub(crate) struct Adoptions {
    pub path: std::path::PathBuf,
    rx: Receiver<Adopted>,
    /// `makepad-wm launch <id>` requests (devhub's `show`, a shell): the dock's launch-or-focus.
    launches: Receiver<String>,
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
        let (launch_tx, launches) = channel();
        let worker = cx.thread_spawner().spawn_worker(ThreadOptions { name: Some("wm-socket".into()), ..Default::default() }, move || {
            let mut next = FIRST_ADOPTED_CLIENT;
            for stream in socket.listener.incoming() {
                let Ok(stream) = stream else { continue };
                let served = WmSocket::serve(stream, makepad_studio_protocol::WIRE, |request| {
                    let launched = expected_client(request.pid);
                    let client = launched.unwrap_or_else(|| {
                        next += 1;
                        next - 1
                    });
                    tx.send(Adopted { client, pid: request.pid, crate_name: request.crate_name.clone(), launched: launched.is_some(), test_lane: request.test_lane.clone() })
                        .map_err(|_| "the WM is shutting down".to_string())?;
                    SignalToUI::set_ui_signal();
                    Ok(Admission { port: hub_port, client })
                }, |app| {
                    if makepad_app_registry::find(app).is_none_or(|e| !e.wm_launchable) {
                        return Err(format!("no dock app {app}"));
                    }
                    launch_tx.send(app.to_string()).map_err(|_| "the WM is shutting down".to_string())?;
                    SignalToUI::set_ui_signal();
                    Ok(())
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
        Some(Adoptions { path, rx, launches })
    }
}

impl App {
    /// Make a tile for every app the socket admitted. Runs before the hub's events, so the
    /// client exists when its websocket connects.
    pub(crate) fn drain_adoptions(&mut self, cx: &mut Cx) {
        let Some(adoptions) = self.adoptions.as_ref() else { return };
        let adopted: Vec<Adopted> = adoptions.rx.try_iter().collect();
        let launches: Vec<String> = adoptions.launches.try_iter().collect();
        for app in launches {
            log!("wm: socket asks for the dock's {app}");
            self.launch_app(cx, &app);
        }
        for Adopted { client, pid, crate_name, launched, test_lane } in adopted {
            if launched {
                log!("wm: {crate_name} (pid {pid}), launched as client {client}, asked for its window");
                continue;
            }
            // The same process asking again (a detached app reattaching after its hub connect
            // failed): its earlier admission is void, and its tile with it.
            let earlier: Vec<ClientId> = self.state_mut().clients.iter()
                .filter(|(id, slot)| **id != client && slot.external == Some(pid))
                .map(|(id, _)| *id)
                .collect();
            for id in earlier {
                log!("wm: pid {pid} asked again; client {id} was its earlier admission");
                self.remove_client(cx, id);
            }
            let entry = makepad_app_registry::apps().iter().find(|a| a.package == crate_name);
            let (app, label) = entry.map_or((crate_name.clone(), crate_name.clone()), |e| (e.id.to_string(), e.label.to_string()));
            // The node's AI Hub app, started by this WM or an earlier one (hub_app): a service
            // with a tray and a dropdown (dropdown.rs), no tile. Its run view takes the GPU
            // descriptor channel; nothing lays it out.
            #[cfg(not(target_arch = "wasm32"))]
            if (app == clients::HUB_SERVICE_APP && hub_app::is_hub_pid(pid)) || (app == clients::DEVHUB_SERVICE_APP && hub_app::is_service_pid(&hub_app::DEVHUB, pid)) {
                log!("wm: {label} (pid {pid}) came to the WM by itself as this node's service; client {client}: no tile");
                hub_app::note_adopted(pid);
                self.state_mut().clients.insert(client, clients::ClientSlot::adopted_service(client, &app, pid));
                let hub_port = self.state_mut().hub_port;
                self.desk(cx)
                    .borrow_mut::<WmDesk>()
                    .map(|mut d| d.with_run_view(cx, client, |cx, v| v.set_run_target(cx, client, 0, hub_port)));
                continue;
            }
            log!("wm: {label} (pid {pid}) came to the WM by itself; client {client}{}", test_lane.as_ref().map_or(String::new(), |l| format!(" (TEST instance of lane {l})")));
            let mut slot = clients::ClientSlot::adopted(client, &app, pid);
            if let Some(lane) = test_lane {
                slot.test_lane = Some(lane);
                slot.set_title(&label);
            }
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
