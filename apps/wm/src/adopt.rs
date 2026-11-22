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

/// Processes the WM launched itself that will ask the socket for their window: pid -> the
/// client slot the launch already made. Their request is admitted as that client, so the
/// launch's tile is the window; anything else gets a new tile.
pub(super) static EXPECTED: std::sync::Mutex<Vec<(u32, ClientId)>> = std::sync::Mutex::new(Vec::new());

/// Scopes started by this WM, including any systemd has already collected.
static APP_SCOPES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub(super) fn record_scope(unit: String) {
    APP_SCOPES.lock().unwrap_or_else(|e| e.into_inner()).push(unit);
}

pub(super) fn stop_scopes_command(units: &[String]) -> Option<std::process::Command> {
    if units.is_empty() { return None; }
    let mut cmd = std::process::Command::new("systemctl");
    cmd.args(["--user", "stop", "--no-block"]).args(units);
    Some(cmd)
}

pub(super) fn stop_app_scopes() {
    let units = std::mem::take(&mut *APP_SCOPES.lock().unwrap_or_else(|e| e.into_inner()));
    if let Some(mut cmd) = stop_scopes_command(&units) {
        match cmd.status() {
            Ok(status) if status.success() => {}
            Ok(status) => log!("wm: systemctl stop of app scopes exited with {status}"),
            Err(e) => log!("wm: cannot stop app scopes: {e}"),
        }
    }
}

/// Keep socket admission waiting until the spawned process's pid names its launch slot.
pub(super) fn spawn_expected(client: ClientId, spawn: impl FnOnce() -> std::io::Result<std::process::Child>) -> std::io::Result<std::process::Child> {
    let mut expected = EXPECTED.lock().unwrap_or_else(|e| e.into_inner());
    let child = spawn()?;
    let pid = child.id();
    expected.retain(|(p, c)| *p != pid && *c != client);
    expected.push((pid, client));
    Ok(child)
}

/// Set once the WM has published its notice and can accept returning clients.
pub(super) static KEEP_APPS: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

pub(crate) fn remove_socket_state(socket: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_file(socket.with_added_extension("state")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Publish this WM's notice beside its socket (`<socket>.state`), replacing a crashed WM's: a
/// keeping WM's epoch and `keep=1`, else `keep=0`, and always its kind, `lane=` (empty for the
/// node's own WM, [`hub_app::host_lane`]), so a detached app joins only a WM of the kind it left.
pub(crate) fn write_socket_state(socket: &std::path::Path, keep: bool, lane: &str) -> std::io::Result<Option<String>> {
    use std::hash::{BuildHasher, RandomState};
    let epoch = keep.then(|| format!("{:016x}", RandomState::new().hash_one((std::process::id(), std::time::SystemTime::now()))));
    let text = match &epoch {
        Some(epoch) => format!("epoch={epoch}\nkeep=1\nlane={lane}\n"),
        None => format!("keep=0\nlane={lane}\n"),
    };
    let state = socket.with_added_extension("state");
    let temp = state.with_added_extension("new");
    std::fs::write(&temp, text)?;
    std::fs::rename(temp, state)?;
    Ok(epoch)
}

/// Whether this WM keeps its apps: it published `keep=1` ([`write_socket_state`]).
pub(super) fn keeping() -> bool {
    KEEP_APPS.get().is_some_and(Option::is_some)
}

/// Whether the WM, going, leaves its apps waiting for the next WM instead of closing them: the
/// node's own keeping WM (`own`: no lane's test WM, which ends with its lease and has no next)
/// going for a next WM (`for_next`, [`goes_for_next`]), unless the final stop's flag asks it to
/// close them (`close_flag`, [`take_close_flag`]). A WM that keeps nothing closes them.
pub(crate) fn leaves_apps(keeping: bool, own: bool, for_next: bool, close_flag: bool) -> bool {
    keeping && own && for_next && !close_flag
}

/// Whether the WM goes for a next WM: a termination signal (`by_signal`: SIGTERM from a deploy's
/// or `devhub wm-restart`'s unit restart, or a `systemctl stop`) or the person's "Restart desktop"
/// (`desktop_restart`, which its unit restarts), the main case: a restart to pick up an update.
/// Its session restart for its services waits for unsaved work and closes the apps.
pub(crate) fn goes_for_next(by_signal: bool, desktop_restart: bool) -> bool {
    by_signal || desktop_restart
}

/// The final stop's flag beside the socket (`<socket>.close-apps`): a stop meant to be final
/// writes it first (`touch "$XDG_RUNTIME_DIR/makepad-wm-0.close-apps"`, then stops the unit), and
/// the WM then closes its apps as it goes. Taken (removed) when read: whether it was there. A WM
/// takes it at its start too, so one a WM never read binds no later WM.
pub(crate) fn take_close_flag(socket: &std::path::Path) -> bool {
    std::fs::remove_file(socket.with_added_extension("close-apps")).is_ok()
}

/// The keeping WM goes and leaves its apps waiting (`apps`: each app and its pid): its notice stays
/// for them and says since when (`left_ms=`, wall ms, which `devhub sync status` reports while no WM
/// takes them back), which apps it left (`left=`, pids) and which had the keyboard (`focus=`, a
/// pid): the next WM reads those before it publishes its own notice ([`take_left`]). Apps read only
/// its `keep=` and `lane=` lines. The same goes, one line, into the WM's log beside its socket
/// (`<socket>.log`), which devhub and lanes read as files.
pub(crate) fn note_left(socket: &std::path::Path, left_ms: u64, apps: &[(String, u32)], focus: Option<u32>) -> std::io::Result<()> {
    let state = socket.with_added_extension("state");
    let text = std::fs::read_to_string(&state)?;
    let mut lines: Vec<String> = text.lines().filter(|l| !["left_ms=", "left=", "focus="].iter().any(|k| l.starts_with(k))).map(str::to_string).collect();
    lines.push(format!("left_ms={left_ms}"));
    lines.push(format!("left={}", apps.iter().map(|(_, pid)| pid.to_string()).collect::<Vec<_>>().join(",")));
    lines.extend(focus.map(|pid| format!("focus={pid}")));
    let temp = state.with_added_extension("new");
    std::fs::write(&temp, lines.join("\n") + "\n")?;
    std::fs::rename(temp, state)?;
    let named = apps.iter().map(|(app, pid)| format!("{app}#{pid}{}", if focus == Some(*pid) { " (had the keyboard)" } else { "" })).collect::<Vec<_>>();
    log_beside(socket, &format!("left_ms={left_ms} wm pid {} went away for a next WM, leaving {} apps waiting{}{}", std::process::id(), apps.len(), if named.is_empty() { "" } else { ": " }, named.join(", ")));
    Ok(())
}

/// One line into the WM's log beside its socket (`<socket>.log`): what a WM did with the apps it
/// keeps (left them waiting, took them back), for devhub and lanes, which cannot read a session
/// WM's journal entries. One line per WM restart and per app taken back.
pub(crate) fn log_beside(socket: &std::path::Path, line: &str) {
    use std::io::Write;
    let path = socket.with_added_extension("log");
    if let Err(e) = std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| writeln!(f, "{line}")) {
        log!("wm: cannot write {}: {e}", path.display());
    }
}

/// The apps the last WM left waiting for this one (its notice's `left=` and `focus=` pids), read
/// before this WM replaces its notice. Each app is taken once, as it comes back.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct LeftApps {
    pub pids: Vec<u32>,
    pub focus: Option<u32>,
}

static RETURNING: std::sync::Mutex<LeftApps> = std::sync::Mutex::new(LeftApps { pids: Vec::new(), focus: None });

/// [`LeftApps`] of a notice's text.
pub(crate) fn left_apps(notice: &str) -> LeftApps {
    let pids = notice.lines().find_map(|l| l.strip_prefix("left=")).map_or(Vec::new(), |v| v.split(',').filter_map(|p| p.parse().ok()).collect());
    LeftApps { pids, focus: notice.lines().find_map(|l| l.strip_prefix("focus=")?.parse().ok()) }
}

/// The apps the last WM left, to take back as they come ([`take_returning`]).
pub(crate) fn expect_returning(left: LeftApps) {
    *RETURNING.lock().unwrap_or_else(|e| e.into_inner()) = left;
}

/// Whether `pid` comes back from the last WM: `Some(true)` for the app that had the keyboard there,
/// `Some(false)` for another app it left, `None` for an app new to this WM. Taken: asked once.
pub(crate) fn take_returning(pid: u32) -> Option<bool> {
    let mut left = RETURNING.lock().unwrap_or_else(|e| e.into_inner());
    let at = left.pids.iter().position(|p| *p == pid)?;
    left.pids.remove(at);
    Some(left.focus.take_if(|f| *f == pid).is_some())
}

/// The WM started `pid` as client `client`; it will come to the socket by itself.
pub(crate) fn expect(pid: u32, client: ClientId) {
    let mut expected = EXPECTED.lock().unwrap_or_else(|e| e.into_inner());
    expected.retain(|(p, c)| *p != pid && *c != client);
    expected.push((pid, client));
}

/// The client a launch made for `pid`. Kept as long as the client's tile lives: a JIT app that
/// restarts by exec (same pid, a new image) and asks again gets its own client back, so its
/// window stays the tile it was (position, size, focus) and only its content refreshes.
fn expected_client(pid: u32) -> Option<ClientId> {
    let expected = EXPECTED.lock().unwrap_or_else(|e| e.into_inner());
    expected.iter().find(|(p, _)| *p == pid).map(|(_, c)| *c)
}

/// The pid that asked for `client`'s window (a launch's process that came to the socket).
pub(crate) fn pid_of(client: ClientId) -> Option<u32> {
    EXPECTED.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(_, c)| *c == client).map(|(p, _)| *p)
}

/// The client is gone: its launch's pid no longer names it.
pub(crate) fn forget(client: ClientId) {
    EXPECTED.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, c)| *c != client);
}

pub(crate) struct Adopted {
    pub client: ClientId,
    pub pid: u32,
    pub crate_name: String,
    /// Its slot and tile exist already: the WM launched it (`expect`), or it is a restarted
    /// image of a process that had this client.
    pub launched: bool,
    /// A lane's test instance (`lane=` on the socket).
    pub test_lane: Option<String>,
}

pub(crate) struct Adoptions {
    pub path: std::path::PathBuf,
    rx: Receiver<Adopted>,
    /// `makepad-wm launch <id> [native]` requests (devhub's `show`, a shell): the dock's
    /// launch-or-focus, its compiled build when `native`.
    launches: Receiver<(String, bool)>,
    /// `makepad-wm notify <title>\t<body>` requests (devhub's "app updated" after a sync): toasts.
    toasts: Receiver<(String, String, Option<u32>)>,
}

impl Adoptions {
    /// Publish the socket and serve it on a worker of its own; children learn its path.
    pub fn start(cx: &mut Cx, hub_port: u16) -> Option<Adoptions> {
        let Some(dir) = wm_socket::runtime_dir() else {
            log!("wm: XDG_RUNTIME_DIR is not set: apps started outside the WM cannot find it");
            return None;
        };
        // A test desktop never takes the path apps find by default: the node's next WM would be
        // second to it.
        let first = if hub_app::test_wm() { 1 } else { 0 };
        let socket = match WmSocket::bind(&dir, first) {
            Ok(socket) => socket,
            Err(e) => {
                log!("wm: cannot publish the WM socket in {}: {e}", dir.display());
                return None;
            }
        };
        let path = socket.path.clone();
        let keep = hub_app::keep_apps_at_start(&path, &dir);
        // the apps the last WM left waiting for this one, read before this WM's notice replaces its
        let left = std::fs::read_to_string(path.with_added_extension("state")).map_or_else(|_| LeftApps::default(), |t| left_apps(&t));
        if keep && !left.pids.is_empty() {
            log!("wm: the last WM left {} apps waiting for this one (pids {:?}; the keyboard was pid {:?}'s)", left.pids.len(), left.pids, left.focus);
            expect_returning(left);
        }
        let epoch = match write_socket_state(&path, keep, &hub_app::host_lane()) {
            Ok(epoch) => epoch,
            Err(e) => {
                log!("wm: cannot publish the keep policy for {}: {e}", path.display());
                return None;
            }
        };
        host::set_child_env(wm_socket::SOCKET_ENV, path.as_os_str());
        let (tx, rx) = channel();
        let (launch_tx, launches) = channel();
        let (toast_tx, toasts) = channel();
        let worker = cx.thread_spawner().spawn_worker(ThreadOptions { name: Some("wm-socket".into()), ..Default::default() }, move || {
            let mut next = FIRST_ADOPTED_CLIENT;
            for stream in socket.listener.incoming() {
                let Ok(stream) = stream else { continue };
                let served = WmSocket::serve(stream, makepad_studio_protocol::WIRE, |request| {
                    // the pid's client when it has one (a launch, or this process's earlier image):
                    // its tile exists; else a new client, which the pid names from now on
                    let launched = expected_client(request.pid);
                    let client = launched.unwrap_or_else(|| {
                        next += 1;
                        expect(request.pid, next - 1);
                        next - 1
                    });
                    tx.send(Adopted { client, pid: request.pid, crate_name: request.crate_name.clone(), launched: launched.is_some(), test_lane: request.test_lane.clone() })
                        .map_err(|_| "the WM is shutting down".to_string())?;
                    SignalToUI::set_ui_signal();
                    Ok(Admission { port: hub_port, client, version: request.version, wire: makepad_studio_protocol::WIRE })
                }, |app, native| {
                    if makepad_app_registry::find(app).is_none_or(|e| !e.wm_launchable) {
                        return Err(format!("no dock app {app}"));
                    }
                    launch_tx.send((app.to_string(), native)).map_err(|_| "the WM is shutting down".to_string())?;
                    SignalToUI::set_ui_signal();
                    Ok(())
                }, |title, body, action| {
                    // `relaunch=<pid>`: the toast offers that JIT's restart
                    let relaunch = action.and_then(|a| a.strip_prefix("relaunch=")).and_then(|p| p.parse::<u32>().ok());
                    toast_tx.send((title.to_string(), body.to_string(), relaunch)).map_err(|_| "the WM is shutting down".to_string())?;
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
                if let Err(error) = remove_socket_state(&path) {
                    log!("wm: cannot remove the state of {}: {error}", path.display());
                }
                log!("wm: cannot serve the WM socket: {e:?}");
                return None;
            }
        }
        if take_close_flag(&path) {
            log!("wm: a final stop's flag beside {} was never read by its WM: dropped", path.display());
        }
        let _ = KEEP_APPS.set(epoch);
        log!("wm: published {} for apps started in this session", path.display());
        Some(Adoptions { path, rx, launches, toasts })
    }
}

impl App {
    /// Make a tile for every app the socket admitted. Runs before the hub's events, so the
    /// client exists when its websocket connects.
    pub(crate) fn drain_adoptions(&mut self, cx: &mut Cx) {
        let Some(adoptions) = self.adoptions.as_ref() else { return };
        let adopted: Vec<Adopted> = adoptions.rx.try_iter().collect();
        let launches: Vec<(String, bool)> = adoptions.launches.try_iter().collect();
        let toasts: Vec<(String, String, Option<u32>)> = adoptions.toasts.try_iter().collect();
        for (title, body, relaunch) in toasts {
            log!("wm: socket toast: {title}: {body}{}", relaunch.map_or(String::new(), |p| format!(" [click: restart pid {p}]")));
            self.notify_with(cx, &title, &body, relaunch);
        }
        for (app, native) in launches {
            log!("wm: socket asks for the dock's {app}{}", if native { ", compiled" } else { "" });
            self.open_app(cx, &app, native, Some(makepad_app_registry::machine::Launcher::Agent("devhub show, through the WM socket".to_string())), &[]);
        }
        for Adopted { client, pid, crate_name, launched, test_lane } in adopted {
            if launched {
                // A restarted image keeps its tile only while the launch's slot still exists.
                if let Some(slot) = self.state_mut().clients.get(&client) {
                    let again = slot.ready;
                    log!("wm: {crate_name} (pid {pid}), launched as client {client}, asked for its window{}", if again { " again (a restart in place: the same tile)" } else { "" });
                    continue;
                }
                log!("wm: {crate_name} (pid {pid}), launched as client {client}, asked for its window but its launch slot was gone; adopting it anew");
            }
            self.adopt_client(cx, client, pid, crate_name, test_lane);
        }
    }

    /// A fresh admission or a vanished launch slot needs the same tile and run target.
    fn adopt_client(&mut self, cx: &mut Cx, client: ClientId, pid: u32, crate_name: String, test_lane: Option<String>) {
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
            return;
        }
        // an app the last WM left waiting: back in its tile, and only the one that had the keyboard
        // there takes it, unless something has taken the keyboard since this WM started
        let returning = take_returning(pid);
        match returning {
            Some(had_keys) => {
                log!("wm: {label} (pid {pid}) came back from the last WM; client {client}{}{}", if had_keys { ", which had the keyboard there" } else { "" }, test_lane.as_ref().map_or(String::new(), |l| format!(" (TEST instance of lane {l})")));
                if let Some(socket) = &self.adoptions {
                    log_beside(&socket.path, &format!("wm pid {} took back {app}#{pid} as client {client}{}", std::process::id(), if had_keys { ", which had the keyboard" } else { "" }));
                }
            }
            None => log!("wm: {label} (pid {pid}) came to the WM by itself; client {client}{}", test_lane.as_ref().map_or(String::new(), |l| format!(" (TEST instance of lane {l})"))),
        }
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
        let keys_taken = self.state_mut().screens.screens.iter().any(|s| s.layout.has_focus_history());
        let takes = takes_keys_back(returning, keys_taken);
        if let Some(had_keys) = returning {
            let focused = self.state_mut().screens.focused_client();
            log!("wm: client {client} (pid {pid}) {}", match (takes, had_keys) {
                (true, _) => "takes the keyboard back".to_string(),
                (false, false) => "comes back without the keyboard: it did not have it there".to_string(),
                (false, true) => format!("comes back without the keyboard: something took it since this WM started (focused now: {focused:?})"),
            });
        }
        if takes {
            self.activate_client(cx, client);
        }
        self.redraw_all(cx);
    }
}

/// Whether an adopted app is activated (shown in front, given the keyboard): an app new to this WM
/// is, as ever; one the last WM left waiting (`returning`, [`take_returning`]) comes back to its tile
/// without taking the keyboard, but the one that had it there takes it back, unless something has
/// taken the keyboard since this WM started (`keys_taken`: a click, a launch).
pub(crate) fn takes_keys_back(returning: Option<bool>, keys_taken: bool) -> bool {
    match returning {
        None => true,
        Some(had_keys) => had_keys && !keys_taken,
    }
}
