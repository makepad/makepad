//! Domain lifecycle routing, moved from the old wm-jit app (deleted). Client ids remain the desk's ids; domain and
//! window ids belong to the HAL and are never used as client ids.
use super::*;
use crate::tile::TileHost;

#[derive(Default)]
pub(crate) struct Domains {
    pub enabled: bool,

    pending: HashMap<u32, ClientId>,
    clients: HashMap<ClientId, DomainClient>,
    windows: HashMap<(u32, LentId), ClientId>,
    outgoing: Vec<(ClientId, String)>,
}

struct DomainClient {
    domain: Option<u32>,
    window: Option<LentId>,
    status: DomainStatus,
    closing: bool,
    relaunching: bool,
}

impl Domains {
    pub fn send_message(&mut self, client: ClientId, message: String) -> bool {
        let Some(record) = self.clients.get(&client) else { return false };
        if record.closing { return false; }
        self.outgoing.push((client, message));
        true
    }

    pub fn ended(&self, client: ClientId) -> bool {
        self.clients.get(&client).is_some_and(|r| matches!(r.status, DomainStatus::Crashed | DomainStatus::Exited))
    }

    pub fn owns(&self, client: ClientId) -> bool { self.clients.contains_key(&client) }
}

impl App {
    pub(crate) fn flush_domain_messages(&mut self, cx: &mut Cx) {
        let mut pending = Vec::new();
        for (client, message) in self.domains.outgoing.drain(..) {
            let Some(record) = self.domains.clients.get(&client) else { continue };
            if record.closing { continue; }
            let (Some(domain), Some(window)) = (record.domain, record.window) else {
                // Services can register during Startup, before their first window is announced.
                pending.push((client, message));
                continue;
            };
            let data = makepad_hal::Range { off: 0, len: message.len() as u32 };
            cx.hal_forward(domain, window, &makepad_hal::RawEvent::app_message(makepad_hal::Id::NONE, data), message.as_bytes());
        }
        self.domains.outgoing = pending;
    }

    pub(crate) fn start_domain_client(&mut self, cx: &mut Cx, app: &clients::AppDef,
        extra: &[String], pane: bool) -> ClientId {
        let client = self.next_id;
        self.next_id += 1;
        let args: Vec<String> = app.args.iter().chain(extra).cloned().collect();
        let request = cx.hal_start_domain(&app.package, &args);
        self.domains.pending.insert(request, client);
        self.domains.clients.insert(client, DomainClient {
            domain: None, window: None, status: DomainStatus::Compiling,
            closing: false, relaunching: false,
        });
        let mut slot = clients::ClientSlot::module(client, &app.id, &app.label);
        slot.ready = false;
        slot.pane = pane;
        slot.status = "compiling…".into();
        self.state_mut().clients.insert(client, slot);
        if pane {
            self.with_ai_pane(cx, |cx, p| p.set_domain_client(cx, client));
        } else if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() {
            desk.mark_domain(client);
        }
        self.with_domain_view(cx, client, |cx, v| v.set_status_line(cx, "compiling…"));
        client
    }

    fn with_domain_view<R>(&mut self, cx: &mut Cx, client: ClientId,
        f: impl FnOnce(&mut Cx, &mut crate::domain_view::MpDomainView) -> R) -> Option<R> {
        if self.state.as_ref()?.clients.get(&client)?.pane {
            self.with_ai_pane(cx, |cx, p| p.with_domain_view(cx, f)).flatten()
        } else {
            self.desk(cx).borrow_mut::<WmDesk>()?.with_domain_view(cx, client, f)
        }
    }

    pub(crate) fn launch_domain(&mut self, cx: &mut Cx, app: &clients::AppDef) {
        let client = self.start_domain_client(cx, app, &[], false);
        self.seat_domain(cx, client);
    }

    pub(crate) fn seat_domain(&mut self, cx: &mut Cx, client: ClientId) {
        let area = self.desk_area(cx);
        let state = self.state_mut();
        let gap = state.gap;
        state.layout_mut().insert(client, area, gap);
        self.activate_client(cx, client);
        self.redraw_all(cx);
    }

    pub(crate) fn close_domain(&mut self, cx: &mut Cx, client: ClientId) -> bool {
        let Some(record) = self.domains.clients.get(&client) else { return false };
        if record.closing { return true; }
        let domain = record.domain;
        let clients: Vec<_> = self.domains.clients.iter().filter(|(id, r)|
            **id == client || domain.is_some() && r.domain == domain).map(|(id, _)| *id).collect();
        if let Some(domain) = domain { cx.hal_stop_domain(domain, false); }
        for client in clients {
            let record = self.domains.clients.get_mut(&client).unwrap();
            record.closing = true;
            let ended = matches!(record.status, DomainStatus::Exited | DomainStatus::Crashed);
            if ended {
                self.remove_client(cx, client);
            } else {
                // A close during compilation still owes QUIT when DomainStart answers.
                if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                    slot.closing = Some(host::now());
                }
                self.state_mut().layout_of_mut(client).remove(client);
                if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() { desk.remove_client(client); }
            }
        }
        self.focus_after_layout(cx);
        self.update_bar(cx);
        self.redraw_all(cx);
        true
    }

    pub(crate) fn relaunch_domain(&mut self, cx: &mut Cx, client: ClientId) -> bool {
        let Some(record) = self.domains.clients.get_mut(&client) else { return false };
        let Some(domain) = record.domain else { return true };
        if record.relaunching { return true; }
        let clients: Vec<_> = self.domains.clients.iter().filter(|(_, r)| r.domain == Some(domain)).map(|(id, _)| *id).collect();
        self.domains.windows.retain(|(child, _), _| *child != domain);
        for client in clients {
            let record = self.domains.clients.get_mut(&client).unwrap();
            record.relaunching = true;
            record.window = None;
            self.with_domain_view(cx, client, |cx, v| {
                v.clear_window(cx);
                v.set_status_line(cx, "relaunching…");
            });
        }
        cx.hal_relaunch_domain(domain);
        true
    }

    pub(crate) fn forget_domain(&mut self, client: ClientId) {
        self.domains.clients.remove(&client);
        self.domains.windows.retain(|_, c| *c != client);
        self.domains.pending.retain(|_, c| *c != client);
        self.domains.outgoing.retain(|(c, _)| *c != client);
    }

    /// A focus that could not land at launch (the tile had no window yet) lands once the
    /// domain's window or frame has arrived -- what the first frame of a process client does.
    /// Not when the person has gone to another space since.
    fn land_pending_focus(&mut self, cx: &mut Cx, client: ClientId) {
        if self.pending_focus != Some(client) { return; }
        let shown = {
            let l = self.state_mut().layout_of(client);
            l.workspace_of(client) == Some(l.active)
        };
        if shown { self.focus_client(cx, client); } else { self.pending_focus = None; }
    }

    pub(crate) fn domain_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::ChildMessage { child, message } => {
                let client = self.domains.clients.iter().filter(|(_, r)| r.domain == Some(*child) && !r.closing).map(|(id, _)| *id).min();
                if let Some(client) = client { self.on_app_msg(cx, client, AppToStudio::Custom(message.clone())); }
            }
            Event::DomainState(e) => {
                let pending = self.domains.pending.remove(&e.request);
                if let Some(client) = pending {
                    if let Some(record) = self.domains.clients.get_mut(&client) {
                        record.domain = Some(e.domain);
                    }
                }
                let clients: Vec<_> = self.domains.clients.iter()
                    .filter(|(_, r)| r.domain == Some(e.domain)).map(|(c, _)| *c).collect();
                for client in clients {
                    let record = self.domains.clients.get_mut(&client).unwrap();
                    if record.relaunching && e.status == DomainStatus::Exited { continue; }
                    if matches!(e.status, DomainStatus::Compiling | DomainStatus::Running) { record.relaunching = false; }
                    record.status = e.status;
                    let closing = record.closing;
                    let ended = matches!(e.status, DomainStatus::Crashed | DomainStatus::Exited);
                    if ended {
                        record.window = None;
                        self.domains.windows.retain(|_, c| *c != client);
                    }
                    if pending == Some(client) && closing { cx.hal_stop_domain(e.domain, false); }
                    if closing {
                        if ended { self.remove_client(cx, client); }
                        continue;
                    }
                    let name = self.state_mut().clients.get(&client).map(|s| s.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| "app".into());
                    let text = match e.status {
                        DomainStatus::Compiling => format!("{name}\n{}", e.message),
                        DomainStatus::Running => format!("{name}\n{}", if e.message.is_empty() { "starting app" } else { e.message.as_str() }),
                        DomainStatus::Crashed => format!("crashed: {}", e.message),
                        DomainStatus::Exited => format!("exited {}", e.message),
                    };
                    if let Some(slot) = self.state_mut().clients.get_mut(&client) { slot.status = text.clone(); }
                    if !self.state_mut().clients[&client].pane {
                        // A crashed or exited app draws nothing: its card gets the desk's title bar to
                        // close it by, until the app runs (and draws its own) again.
                        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() { desk.set_own_chrome(cx, client, !ended); }
                    }
                    self.with_domain_view(cx, client, |cx, v| {
                        v.set_domain(e.domain);
                        if ended { v.clear_window(cx); }
                        v.set_status_line(cx, &text);
                        v.set_progress(if matches!(e.status, DomainStatus::Compiling) { (e.percent as f32 / 100.0).clamp(0.0, 1.0) } else { 1.0 });
                    });
                }
            }
            Event::ChildWindow(e) => {
                let first_window = !self.domains.windows.contains_key(&(e.child, e.window));
                let client = if let Some(client) = self.domains.windows.get(&(e.child, e.window)) {
                    *client
                } else {
                    let free = self.domains.clients.iter().filter(|(_, r)|
                        r.domain == Some(e.child) && r.window.is_none() && !r.closing).map(|(c, _)| *c).min();
                    let client = if let Some(client) = free { client } else {
                        let Some((&owner, _)) = self.domains.clients.iter().find(|(_, r)| r.domain == Some(e.child) && !r.closing) else { return };
                        let app = self.state_mut().clients[&owner].app.clone();
                        let client = self.next_id;
                        self.next_id += 1;
                        self.state_mut().clients.insert(client, clients::ClientSlot::module(client, &app, &e.title));
                        self.domains.clients.insert(client, DomainClient { domain: Some(e.child), window: None,
                            status: DomainStatus::Running, closing: false, relaunching: false });
                        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() { desk.mark_domain(client); }
                        self.seat_domain(cx, client);
                        client
                    };
                    self.domains.clients.get_mut(&client).unwrap().window = Some(e.window);
                    self.domains.windows.insert((e.child, e.window), client);
                    client
                };
                if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                    slot.title = e.title.clone();
                    slot.ready = true;
                }
                self.with_domain_view(cx, client, |cx, v| v.child_window(cx, e));
                self.land_pending_focus(cx, client);
                if first_window {
                    let theme_splash = theme::theme_splash_path(&self.state_mut().theme_name).to_string_lossy().into_owned();
                    self.send_wm_event(client, WmEvent::Hosted { theme_splash });
                    self.send_desktop_style(client);
                    self.resend_screens_event(client);
                    if self.ai_bus.is_pane(client) { self.replay_bus_to_pane(); }
                }
                if let Some(path) = self.preview_cache.take_pending(client) {
                    self.send_wm_event(client, WmEvent::PreviewFile { path });
                }
                self.update_bar(cx);
            }
            Event::ChildWindowAction(e) => {
                let Some(&client) = self.domains.windows.get(&(e.child, e.window)) else { return };
                if self.state_mut().clients[&client].pane {
                    if matches!(&e.action, ChildWindowAction::State(makepad_hal::window_state::CLOSE)) { self.request_close(cx, client); }
                    return;
                }
                match &e.action {
                    // Every makepad window draws its own chrome (own_chrome.rs): nothing to hear.
                    ChildWindowAction::OwnChrome(_) => {}
                    ChildWindowAction::DragAnswer { request, response } => {
                        let start = self.desk(cx).borrow_mut::<WmDesk>().and_then(|mut desk|
                            desk.with_domain_view(cx, client, |cx, v| v.take_caption_press(cx, *request, *response)).flatten());
                        if let Some(abs) = start {
                            self.begin_drag_on(cx, client, abs, false, false);
                        }
                    }
                    ChildWindowAction::State(state) => {
                        use makepad_hal::window_state as ws;
                        match *state {
                            ws::CLOSE => self.request_close(cx, client),
                            ws::MINIMIZE => self.minimize_desktop(cx, client),
                            ws::MAXIMIZE => {
                                if self.state_mut().layout_of(client).is_fullscreen_space(client) {
                                    self.fullscreen_request(cx, client, false);
                                } else {
                                    let maximised = self.state_mut().layout_of(client).is_maximised(client);
                                    self.fullscreen_request(cx, client, !maximised);
                                }
                            }
                            ws::RESTORE => {
                                if self.state_mut().layout_of(client).is_fullscreen_space(client) {
                                    self.fullscreen_request(cx, client, false);
                                } else if self.state_mut().layout_of(client).is_maximised(client) { self.fullscreen_request(cx, client, false); }
                            }
                            ws::FULLSCREEN => self.fullscreen_request(cx, client, true),
                            _ => {}
                        }
                    }
                }
            }
            Event::ChildFrame(e) => {
                if let Some(&client) = self.domains.windows.get(&(e.child, e.window)) {
                    if self.domains.clients.get(&client).is_none_or(|r| r.closing) { return; }
                    self.with_domain_view(cx, client, |cx, v| v.child_frame(cx, e.texture));
                    if self.state_mut().phone.tiles.is_tile_client(client) {
                        // a home tile's domain: the frame belongs to the face whose viewport it matches
                        let size = self.with_domain_view(cx, client, |_, v| v.child_size()).unwrap_or_default();
                        let tiles = &mut self.state_mut().phone.tiles;
                        let face = tiles.note_frame(client, size);
                        if face.is_some() {
                            if let Some(app) = tiles.get(client).map(|t| t.app.clone()) { tiles.note_healthy(&app); }
                        }
                        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() { desk.note_client_frame(client, face); }
                        self.redraw_scene_content(cx);
                    }
                    self.land_pending_focus(cx, client);
                }
            }
            Event::ChildWindowClosed(e) => {
                if let Some(client) = self.domains.windows.remove(&(e.child, e.window)) {
                    if let Some(record) = self.domains.clients.get_mut(&client) {
                        record.window = None;
                        if record.closing { return; }
                    }
                    self.with_domain_view(cx, client, |cx, v| {
                        v.clear_window(cx);
                        v.set_status_line(cx, "window closed");
                    });
                }
            }
            Event::Shutdown => {
                let mut domains: Vec<_> = self.domains.clients.values().filter_map(|r| r.domain).collect();
                domains.sort_unstable();
                domains.dedup();
                for domain in domains { cx.hal_stop_domain(domain, false); }
            }
            _ => {}
        }
    }
}
