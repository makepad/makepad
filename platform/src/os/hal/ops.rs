//! `CxOsOp` -> HAL commands. The ops the HAL's command set holds (HAL.md section 3) become
//! `Cmd`s in the frame builder's writer; the program-side ones (HTTP) run as before; the rest are
//! logged and dropped with the phase they arrive in. Nothing is faked.

use crate::{
    cx::Cx,
    cx_api::CxOsOp,
    makepad_live_id::*,
    event::hal_intake::hal_window_id,
    event::DragItem,
    MouseCursor,
    macos_menu::MacosMenu,
    window::WindowId,
};
use super::{DialogKind, PendingDialog};
use makepad_hal::{buffers::Wire, cursor, menu_kind as menu, mobile_ui, pointer_lock as pointer, selection_handles, window_kind, window_state, Id, MenuItem};

/// `ImeShow::config`: the field's flags and its soft keyboard (each enum as its variant's index).
fn ime_config(c: &crate::ime::TextInputConfig) -> u32 {
    use makepad_hal::input::ime_config as k;
    let kb = &c.soft_keyboard;
    let mut bits = 0;
    if c.is_multiline {
        bits |= k::MULTILINE;
    }
    if c.is_secure {
        bits |= k::SECURE;
    }
    if c.submit_on_enter {
        bits |= k::SUBMIT_ON_ENTER;
    }
    if c.is_read_only {
        bits |= k::READ_ONLY;
    }
    bits | (kb.input_mode as u32) << k::INPUT_MODE_SHIFT
        | (kb.autocapitalize as u32) << k::AUTOCAPITALIZE_SHIFT
        | (kb.autocorrect as u32) << k::AUTOCORRECT_SHIFT
        | (kb.return_key_type as u32) << k::RETURN_KEY_SHIFT
        | (c.content_type as u32) << k::CONTENT_TYPE_SHIFT
}

fn flatten_menu(menu: &MacosMenu, parent: u32, out: &mut Vec<(MenuItem, String)>) {
    let push = |item: MenuItem, name: String, out: &mut Vec<(MenuItem, String)>| -> u32 {
        out.push((item, name));
        out.len() as u32
    };
    match menu {
        MacosMenu::Main { items } => {
            for i in items {
                flatten_menu(i, parent, out);
            }
        }
        MacosMenu::Sub { name, items } => {
            let me = push(MenuItem { parent, kind: self::menu::SUB, ..Default::default() }, name.clone(), out);
            for i in items {
                flatten_menu(i, me, out);
            }
        }
        MacosMenu::Item { name, command, shift, key, enabled } => {
            let mut kind = self::menu::ITEM;
            if *command == live_id!(quit) {
                kind |= self::menu::QUIT;
            }
            if *shift {
                kind |= self::menu::SHIFT;
            }
            if !*enabled {
                kind |= self::menu::DISABLED;
            }
            push(MenuItem { parent, kind, cmd: command.0, key: key.to_index(), str: 0 }, name.clone(), out);
        }
        MacosMenu::Line => {
            push(MenuItem { parent, kind: self::menu::LINE, ..Default::default() }, String::new(), out);
        }
    }
}

impl Cx {
    /// The window an op without one acts in: the focused one, else the first the HAL confirmed.
    fn hal_target_window(&self) -> Id {
        self.hal_target().map_or(Id::NONE, hal_window_id)
    }

    /// The window keyboard-ish requests act in: the focused one, else the first ready one.
    fn hal_target(&self) -> Option<WindowId> {
        self.hal.focused.filter(|w| self.windows.is_valid(*w)).or_else(|| self.windows.id_iter().find(|w| self.hal.window_ready(*w)))
    }

    /// Layout points of `window_id` (when it exists) in native window points.
    fn hal_native_points(&self, window_id: Option<WindowId>, p: crate::makepad_math::Vec2d) -> crate::makepad_math::Vec2d {
        match window_id {
            Some(w) => self.windows[w].layout_vec2d_to_native_points(p),
            None => p,
        }
    }

    fn hal_create_window(&mut self, window_id: WindowId, popup: Option<(WindowId, crate::makepad_math::Vec2d, crate::makepad_math::Vec2d)>) {
        if self.hal.hosted.is_some() {
            self.hal_hosted_create_window(window_id);
            return;
        }
        let dpi = self.hal.info.map_or(1.0, |i| i.dpi as f64);
        let id = hal_window_id(window_id);
        let window = &mut self.windows[window_id];
        let mut exact = 0;
        let (kind, parent, position, size) = match popup {
            Some((parent, position, size)) => (window_kind::POPUP, hal_window_id(parent), position, size),
            None => {
                let (position, mut size) = window.create_geom();
                // `--window WxH@scale`: the layout size at that scale whatever the display runs at,
                // an exact size that is not fitted to the displays
                if let Some(launch) = crate::remote::app_clock::launch_window_for(window_id.id()) {
                    let (w, h) = launch.native_size(dpi);
                    size = crate::makepad_math::dvec2(w, h);
                    exact = makepad_hal::window_style::EXACT_SIZE;
                }
                // no position: the window system places it (a NaN position says so)
                let nan = crate::makepad_math::dvec2(f64::NAN, f64::NAN);
                (window_kind::NORMAL, Id::NONE, position.unwrap_or(nan), size)
            }
        };
        // the geometry the app lays out against until the HAL confirms the window
        window.os_dpi_factor = Some(dpi);
        window.draws_chrome_buttons = !window.is_popup && self.hal.info.is_some_and(|info| info.flags & makepad_hal::info_flags::CLIENT_CHROME != 0);
        if cfg!(target_os = "linux") {
            window.uses_client_side_decorations = window.draws_chrome_buttons;
        }
        let placed = if position.x.is_nan() || position.y.is_nan() { crate::makepad_math::dvec2(0.0, 0.0) } else { position };
        window.window_geom = crate::event::WindowGeom { dpi_factor: dpi, position: placed, inner_size: size, outer_size: size, ..Default::default() };
        window.window_geom = window.native_window_geom_to_layout(window.window_geom.clone());
        window.is_created = true;
        let title = window.create_title.clone();
        let visuals = window.window_visuals();
        let kind = kind | exact | macos_window_style(&window.macos);
        let fullscreen = window.is_fullscreen;
        let adopt = !std::mem::replace(&mut self.hal.window_created, true);
        let fb = &mut self.hal.frames;
        fb.w.window_create(id, kind, parent, [position.x as f32, position.y as f32, size.x as f32, size.y as f32], adopt);
        if !title.is_empty() {
            let sid = fb.intern(&title);
            fb.w.window_title(id, sid);
        }
        // A window can be zoomed before it is created (or adopted from the host).
        if cfg!(target_os = "macos") {
            fb.w.window_dpi_override(id, window.dpi_override.and_then(crate::window::CxWindow::valid_dpi_factor).unwrap_or(0.0));
        }
        if fullscreen {
            fb.w.window_state(id, window_state::FULLSCREEN);
        }
        fb.w.window_visuals(id, visuals.transparent as u32 | (visuals.backdrop as u32) << 8, [visuals.backdrop_intensity, 0.0, 0.0, 0.0]);
        self.hal.windows.insert(window_id.0, super::HalWindow::default());
        // the application icon, once (the Dock's; none in a bundled app, which has its own)
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        if !self.hal.icon_sent {
            self.hal.icon_sent = true;
            if let Some(buf) = crate::app_icon::window_icon().buffers.first() {
                self.hal.frames.w.app_icon(buf.width as u32, buf.height as u32, &buf.data);
            }
        }
    }

    /// A file dialog: the settings go to the HAL, the answer comes back as `DialogResult`.
    /// The text selection's drag handles (layout points of the target window -> native).
    fn hal_selection_handles(&mut self, op: u32, start: crate::makepad_math::Vec2d, end: crate::makepad_math::Vec2d) {
        let window_id = self.hal_target();
        let w = window_id.map_or(Id::NONE, hal_window_id);
        let (start, end) = (self.hal_native_points(window_id, start), self.hal_native_points(window_id, end));
        self.hal.frames.w.selection_handles(w, op, [start.x as f32, start.y as f32], [end.x as f32, end.y as f32])
    }

    fn hal_file_dialog(&mut self, kind: DialogKind, settings: crate::file_dialogs::FileDialog) {
        // A host that draws the pickers for its children (the Linux WM: a hosted child has no
        // display of its own to show one on) gets the request, and its answer arrives as the
        // same `FileDialogAction` (hosted_relay.rs).
        if self.hal.hosted.is_some() && crate::hosted_relay::host_serves_file_dialogs() {
            let name = match kind {
                DialogKind::SelectFile => "select_file",
                DialogKind::SaveFile => "save_file",
                DialogKind::SelectFolder => "select_folder",
                DialogKind::SaveFolder => "save_folder",
            };
            let relay = crate::hosted_relay::relay_file_dialog(name, &settings);
            Self::send_studio_message(crate::studio::AppToStudio::Relay(crate::studio::ChildRelay::FileDialog(relay)));
            return;
        }
        self.hal.next_dialog += 1;
        let req = self.hal.next_dialog;
        self.hal.dialogs.insert(req, PendingDialog { kind, id: settings.id });
        let fb = &mut self.hal.frames;
        let title = fb.intern(settings.title.as_deref().unwrap_or_default());
        let location = fb.intern(&settings.location.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
        let filename = fb.intern(settings.filename.as_deref().unwrap_or_default());
        // each filter is two strings: its description and its extensions joined by commas
        let mut strs = Vec::new();
        for f in &settings.filters {
            strs.push(f.description.clone());
            strs.push(f.extensions.join(","));
        }
        // the strings as the arena holds them: u32 length, then the bytes
        let mut bytes = Vec::new();
        for s in &strs {
            bytes.extend_from_slice(&(s.len() as u32).to_le_bytes());
            bytes.extend_from_slice(s.as_bytes());
        }
        fb.w.file_dialog(req, kind as u32 | (settings.multiple as u32) << 8, title, location, filename, strs.len() as u32, &bytes);
    }

    /// Drains the platform ops into HAL commands. False when the app asked to quit.
    pub(crate) fn hal_handle_platform_ops(&mut self) -> bool {
        while let Some(op) = self.platform_ops.pop_front() {
            match op {
                CxOsOp::SetOwnChrome(own) => {
                    if let Some(window) = self.windows.id_iter().find(|w| self.windows[*w].is_created) {
                        self.hal.frames.w.window_chrome(hal_window_id(window), own);
                    }
                }
                // ---- child windows and domains
                CxOsOp::ChildGeom { child, window, size, dpi } => {
                    self.hal.frames.w.child_geom(super::domain::hal_id(window), child, [size.x as f32, size.y as f32], dpi as f32)
                }
                CxOsOp::ForwardEvent { child, window, packed } => self.hal.frames.w.forward(super::domain::hal_id(window), child, &packed),
                CxOsOp::FocusChild(child) => self.hal.frames.w.child_focus(child),
                CxOsOp::CloseChildWindow { child, window } => self.hal.frames.w.child_close(super::domain::hal_id(window), child),
                CxOsOp::StartDomain { request, app, args } => {
                    let fb = &mut self.hal.frames;
                    let (app, args) = (fb.intern(&app), fb.intern(&args));
                    fb.w.domain_start(request, app, args)
                }
                CxOsOp::StopDomain { domain, kill } => {
                    self.hal.frames.w.domain_stop(domain, if kill { makepad_hal::domain_stop::KILL } else { makepad_hal::domain_stop::QUIT })
                }
                CxOsOp::RelaunchDomain(domain) => self.hal.frames.w.domain_relaunch(domain),
                CxOsOp::ListDomains(request) => self.hal.frames.w.domain_list(request),

                CxOsOp::CreateWindow(window_id) => self.hal_create_window(window_id, None),
                CxOsOp::CreatePopupWindow { window_id, parent_window_id, position, size, grab_keyboard } => {
                    let window = &mut self.windows[window_id];
                    window.is_popup = true;
                    window.popup_parent = Some(parent_window_id);
                    window.popup_position = Some(position);
                    window.popup_size = Some(size);
                    window.popup_grab_keyboard = grab_keyboard;
                    self.hal_create_window(window_id, Some((parent_window_id, position, size)));
                }
                CxOsOp::ResizeWindow(window_id, size) => {
                    let p = self.windows[window_id].window_geom.position;
                    self.hal.frames.w.window_geom(hal_window_id(window_id), [p.x as f32, p.y as f32, size.x as f32, size.y as f32]);
                }
                // A hosted app's later windows are windows of their own on the host: what the
                // app asks of them goes to the host with the window's id; its close is local
                // (the HAL answers it with `Closed`, which tells the host).
                CxOsOp::CloseWindow(w) if self.hal_hosted_extra_window(w) => {
                    if self.windows[w].is_created {
                        self.windows[w].is_created = false;
                        self.hal.frames.w.window_close(hal_window_id(w));
                    }
                }
                CxOsOp::MinimizeWindow(w) if self.hal_hosted_extra_window(w) => Self::send_hosted_window(crate::ime::HostedWindow::new(w.id(), crate::ime::hosted_window_op::MINIMIZE)),
                CxOsOp::MaximizeWindow(w) if self.hal_hosted_extra_window(w) => Self::send_hosted_window(crate::ime::HostedWindow::new(w.id(), crate::ime::hosted_window_op::MAXIMIZE)),
                CxOsOp::FullscreenWindow(w) if self.hal_hosted_extra_window(w) => Self::send_hosted_window(crate::ime::HostedWindow::new(w.id(), crate::ime::hosted_window_op::FULLSCREEN)),
                CxOsOp::Deminiaturize(w) | CxOsOp::RestoreWindow(w) | CxOsOp::NormalizeWindow(w) if self.hal_hosted_extra_window(w) => {
                    Self::send_hosted_window(crate::ime::HostedWindow::new(w.id(), crate::ime::hosted_window_op::RESTORE))
                }
                CxOsOp::SetWindowTitle(w, title) if self.hal_hosted_extra_window(w) => {
                    Self::send_hosted_window(crate::ime::HostedWindow::title(w.id(), &title))
                }
                CxOsOp::RepositionWindow(w, pos) if self.hal_hosted_extra_window(w) => {
                    Self::send_hosted_window(crate::ime::HostedWindow { x: pos.x, y: pos.y, ..crate::ime::HostedWindow::new(w.id(), crate::ime::hosted_window_op::MOVE) })
                }
                CxOsOp::RepositionWindow(window_id, pos) => {
                    let s = self.windows[window_id].window_geom.inner_size;
                    self.hal.frames.w.window_geom(hal_window_id(window_id), [pos.x as f32, pos.y as f32, s.x as f32, s.y as f32]);
                }
                // The host owns these operations, including close. Keep the child's
                // HAL window alive until the host ends the session, as in the stdin backends.
                CxOsOp::CloseWindow(_) if self.hal.hosted.is_some() => Self::send_host_window_op(makepad_studio_protocol::window_op::CLOSE),
                CxOsOp::MinimizeWindow(_) if self.hal.hosted.is_some() => Self::send_host_window_op(makepad_studio_protocol::window_op::MINIMIZE),
                CxOsOp::MaximizeWindow(_) if self.hal.hosted.is_some() => Self::send_host_window_op(makepad_studio_protocol::window_op::MAXIMIZE),
                CxOsOp::RestoreWindow(_) if self.hal.hosted.is_some() => Self::send_host_window_op(makepad_studio_protocol::window_op::RESTORE),
                CxOsOp::CloseWindow(window_id) if self.hal_is_embedded() => {
                    self.hal.frames.w.window_state(hal_window_id(window_id), window_state::CLOSE);
                }
                CxOsOp::CloseWindow(window_id) => {
                    if self.hal.window_ready(window_id) {
                        self.windows[window_id].is_created = false;
                        self.hal.frames.w.window_close(hal_window_id(window_id));
                        break;
                    }
                }
                CxOsOp::MinimizeWindow(w) => self.hal.frames.w.window_state(hal_window_id(w), window_state::MINIMIZE),
                CxOsOp::Deminiaturize(w) | CxOsOp::RestoreWindow(w) | CxOsOp::NormalizeWindow(w) => {
                    self.hal.frames.w.window_state(hal_window_id(w), window_state::RESTORE)
                }
                CxOsOp::MaximizeWindow(w) => self.hal.frames.w.window_state(hal_window_id(w), window_state::MAXIMIZE),
                CxOsOp::FullscreenWindow(w) => self.hal.frames.w.window_state(hal_window_id(w), window_state::FULLSCREEN),
                CxOsOp::HideWindow(w) => self.hal.frames.w.window_state(hal_window_id(w), window_state::HIDE),
                CxOsOp::HideWindowButtons(w) => self.hal.frames.w.window_buttons(hal_window_id(w), false),
                CxOsOp::ShowWindowButtons(w) => self.hal.frames.w.window_buttons(hal_window_id(w), true),
                CxOsOp::CenterWindowButtons(w, y_center) => {
                    let native = self.windows[w].layout_vec2d_to_native_points(crate::makepad_math::dvec2(0.0, y_center));
                    self.hal.frames.w.window_buttons_center(hal_window_id(w), native.y as f32);
                }
                CxOsOp::SetTopmost(w, on) => self.hal.frames.w.window_topmost(hal_window_id(w), on),
                CxOsOp::SetChromelessWhenMaximized(w, on) => self.hal.frames.w.chromeless_when_max(hal_window_id(w), on),
                CxOsOp::SetWindowTitle(w, title) => {
                    let sid = self.hal.frames.intern(&title);
                    self.hal.frames.w.window_title(hal_window_id(w), sid);
                }
                CxOsOp::SetWindowVisuals(w, v) => {
                    self.hal.frames.w.window_visuals(hal_window_id(w), v.transparent as u32 | (v.backdrop as u32) << 8, [v.backdrop_intensity, 0.0, 0.0, 0.0])
                }
                CxOsOp::SetWindowDpiOverride(w, dpi_override) => {
                    self.hal.frames.w.window_dpi_override(hal_window_id(w), dpi_override.and_then(crate::window::CxWindow::valid_dpi_factor).unwrap_or(0.0));
                }
                CxOsOp::ShowInDock(show) => {
                    if self.hal.hosted.is_none() { self.hal.frames.w.show_in_dock(show); }
                }
                CxOsOp::ActivateApp => {
                    if self.hal.hosted.is_none() { self.hal.frames.w.activate_app(); }
                }
                CxOsOp::LockMousePointer(lock) => {
                    let w = self.hal_target_window();
                    self.hal.frames.w.pointer_lock(w, if lock { pointer::LOCK } else { pointer::UNLOCK })
                }
                CxOsOp::PinMousePointer(on) => {
                    let w = self.hal_target_window();
                    self.hal.frames.w.pointer_lock(w, if on { pointer::PIN } else { pointer::UNPIN })
                }
                CxOsOp::RepinMousePointer => {
                    let w = self.hal_target_window();
                    self.hal.frames.w.pointer_lock(w, pointer::REPIN)
                }
                CxOsOp::ShowTextIME(area, cursor_rect, config) => {
                    // both corners of the caret line (area-relative layout points) in native points
                    let window_id = self.get_window_id_of(&area).unwrap_or(crate::window::CxWindowPool::id_zero());
                    let area_pos = area.clipped_rect(self).pos;
                    let tl = self.windows[window_id].layout_vec2d_to_native_points(area_pos + cursor_rect.pos);
                    let br = self.windows[window_id].layout_vec2d_to_native_points(area_pos + cursor_rect.pos + cursor_rect.size);
                    self.hal.frames.w.ime_show(hal_window_id(window_id), [tl.x as f32, tl.y as f32, (br.x - tl.x) as f32, (br.y - tl.y) as f32], ime_config(&config));
                }
                CxOsOp::HideTextIME => {
                    let w = self.hal_target_window();
                    self.hal.frames.w.ime_hide(w)
                }
                CxOsOp::SyncImeState { text, selection, composition } => {
                    let w = self.hal_target_window();
                    let (cs, ce) = composition.map_or((0, 0), |c| (c.start.0 as u32, c.end.0 as u32));
                    self.hal.frames.w.ime_sync(w, selection.start.0 as u32, selection.end.0 as u32, cs, ce, text.as_bytes());
                }
                CxOsOp::SetCursor(cursor) => {
                    #[cfg(target_os = "linux")]
                    if matches!(self.os_type, crate::cx::OsType::LinuxDirect) && self.hal.direct_cursor != cursor {
                        self.hal.direct_cursor = cursor;
                        self.redraw_all();
                    }
                    if self.hal.hosted.is_some() {
                        Self::send_studio_message(crate::studio::AppToStudio::SetCursor(cursor.into()));
                    } else {
                        let w = self.hal_target_window();
                        self.hal.frames.w.cursor(w, cursor_index(cursor));
                    }
                }
                CxOsOp::StartTimer { timer_id, interval, repeats } => {
                    if let Some(hosted) = self.hal.hosted.as_mut() {
                        hosted.timers.timers.insert(timer_id, crate::os::shared_framebuf::PollTimer::new(interval, repeats));
                        hosted.demand_changed = true;
                    } else {
                        self.hal.frames.w.timer_start(timer_id as u32, (interval * 1e9) as u64, repeats);
                    }
                }
                CxOsOp::StopTimer(timer_id) => {
                    if let Some(hosted) = self.hal.hosted.as_mut() {
                        hosted.demand_changed |= hosted.timers.timers.remove(&timer_id).is_some();
                    } else {
                        self.hal.frames.w.timer_stop(timer_id as u32);
                    }
                }
                CxOsOp::Quit => {
                    #[cfg(target_os = "linux")]
                    if std::env::var_os("MAKEPAD_HAL_LOG").is_some() {
                        eprintln!("[hal-linux] CxOsOp::Quit consumed: session={} windows={} confirmed={} completed_frames={}",
                            self.hal.info.map_or(0, |i| i.session), self.hal.windows.len(),
                            self.hal.windows.values().filter(|w| w.confirmed).count(), self.hal.frames_done);
                    }
                    self.hal.frames.w.quit();
                    return false;
                }
                CxOsOp::StartDragging(items) => {
                    // the in-window drag protocol: Drag / Drop are made from the pointer events
                    self.hal.internal_drag_items = Some(std::sync::Arc::new(items));
                }
                CxOsOp::StartExternalDragging { window_id, items } => {
                    if self.hal.hosted.is_some() {
                        crate::error!("external file dragging is not available in the hosted runtime");
                        self.call_event_handler(&crate::event::Event::DragEnd);
                        continue;
                    }
                    let fb = &mut self.hal.frames;
                    let flat: Vec<makepad_hal::DragItem> = items
                        .iter()
                        .map(|i| match i {
                            DragItem::FilePath { path, .. } => makepad_hal::DragItem { kind: makepad_hal::input::drag_item::FILE, str: fb.intern(path) },
                            DragItem::String { value, .. } => makepad_hal::DragItem { kind: makepad_hal::input::drag_item::STRING, str: fb.intern(value) },
                            DragItem::VirtualFile(_) => makepad_hal::DragItem::default(),
                        })
                        .collect();
                    let mut bytes = Vec::new();
                    flat.iter().for_each(|i| i.write(&mut bytes));
                    fb.w.drag_start(hal_window_id(window_id), flat.len() as u32, &bytes);
                }
                CxOsOp::UpdateMacosMenu(menu) => {
                    if self.hal.hosted.is_some() { continue; }
                    let mut flat = Vec::new();
                    flatten_menu(&menu, 0, &mut flat);
                    let fb = &mut self.hal.frames;
                    let items: Vec<MenuItem> = flat.into_iter().map(|(mut i, name)| {
                        i.str = fb.intern(&name);
                        i
                    }).collect();
                    let mut bytes = Vec::new();
                    items.iter().for_each(|i| i.write(&mut bytes));
                    fb.w.menu_set(items.len() as u32, &bytes);
                }
                // program-side networking, as every backend does it
                CxOsOp::HttpRequest { request_id, request } => {
                    let _ = self.net.http_start(request_id, request);
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    let _ = self.net.http_cancel(request_id);
                }
                CxOsOp::ShowClipboardActions { has_selection, rect, .. } => {
                    let window_id = self.hal_target();
                    let w = window_id.map_or(Id::NONE, hal_window_id);
                    let (tl, br) = (self.hal_native_points(window_id, rect.pos), self.hal_native_points(window_id, rect.pos + rect.size));
                    self.hal.frames.w.clipboard_actions(w, true, [tl.x as f32, tl.y as f32, (br.x - tl.x) as f32, (br.y - tl.y) as f32], has_selection)
                }
                CxOsOp::HideClipboardActions => {
                    let w = self.hal_target_window();
                    self.hal.frames.w.clipboard_actions(w, false, [0.0; 4], false)
                }
                CxOsOp::CopyToClipboard(content) => {
                    if self.hal.hosted.is_some() {
                        Self::send_studio_message(crate::studio::AppToStudio::SetClipboard(content));
                    } else {
                        self.hal.frames.w.clip_set(content.as_bytes());
                    }
                }
                CxOsOp::ShareText(text) => self.hal.frames.w.share_text(text.as_bytes()),
                // a hosted child's selection is its host's primary selection
                CxOsOp::SetPrimarySelection(text) => {
                    if self.hal.hosted.is_some() {
                        Self::send_studio_message(crate::studio::AppToStudio::Relay(crate::studio::ChildRelay::SetPrimarySelection { text }));
                    } else {
                        self.hal.frames.w.selection_set(text.as_bytes());
                    }
                }

                // the mobile system UI (a desktop backend ignores it)
                CxOsOp::SetSystemBarDarkIcons(dark) => self.hal.frames.w.mobile_ui(mobile_ui::STATUS_BAR_DARK_ICONS, dark as u32),
                CxOsOp::SetScreenOrientation(orientation) => {
                    let value = match orientation {
                        crate::display_context::ScreenOrientation::Auto => mobile_ui::orientation::AUTO,
                        crate::display_context::ScreenOrientation::Portrait => mobile_ui::orientation::PORTRAIT,
                        crate::display_context::ScreenOrientation::Landscape => mobile_ui::orientation::LANDSCAPE,
                    };
                    self.hal.frames.w.mobile_ui(mobile_ui::ORIENTATION, value)
                }
                CxOsOp::SetKeepScreenOn(on) => self.hal.frames.w.mobile_ui(mobile_ui::KEEP_SCREEN_ON, on as u32),
                CxOsOp::DeferSystemGestures(edges) => {
                    use crate::cx_api::ScreenEdges;
                    let mut bits = 0;
                    for (edge, bit) in [(ScreenEdges::TOP, mobile_ui::edge::TOP), (ScreenEdges::LEFT, mobile_ui::edge::LEFT), (ScreenEdges::BOTTOM, mobile_ui::edge::BOTTOM), (ScreenEdges::RIGHT, mobile_ui::edge::RIGHT)] {
                        if edges.contains(edge) {
                            bits |= bit;
                        }
                    }
                    self.hal.frames.w.mobile_ui(mobile_ui::DEFER_SYSTEM_GESTURES, bits)
                }
                CxOsOp::ShowSelectionHandles { start, end } => self.hal_selection_handles(selection_handles::SHOW, start, end),
                CxOsOp::UpdateSelectionHandles { start, end } => self.hal_selection_handles(selection_handles::MOVE, start, end),
                CxOsOp::HideSelectionHandles => self.hal_selection_handles(selection_handles::HIDE, Default::default(), Default::default()),

                // the rest arrive as their own HAL modules (HAL.md phases 1b / 4 / 5)
                CxOsOp::SelectFileDialog(settings) => self.hal_file_dialog(DialogKind::SelectFile, settings),
                CxOsOp::SaveFileDialog(settings) => self.hal_file_dialog(DialogKind::SaveFile, settings),
                CxOsOp::SelectFolderDialog(settings) => self.hal_file_dialog(DialogKind::SelectFolder, settings),
                CxOsOp::SaveFolderDialog(settings) => self.hal_file_dialog(DialogKind::SaveFolder, settings),
                CxOsOp::AccessibilityUpdate(_) => self.hal.frames.gap("accessibility updates arrive in phase 5"),
                CxOsOp::CheckPermission { permission, request_id } => self.hal_permission(makepad_hal::media::permission_op::CHECK, permission, request_id),
                CxOsOp::RequestPermission { permission, request_id } => self.hal_permission(makepad_hal::media::permission_op::REQUEST, permission, request_id),
                CxOsOp::StartLocationUpdates => self.hal_location(makepad_hal::media::location_op::START),
                #[cfg(target_os = "android")]
                CxOsOp::HapticFeedback(kind) => self.hal_haptic(kind),
                CxOsOp::StopLocationUpdates => self.hal_location(makepad_hal::media::location_op::STOP),
                CxOsOp::PrepareVideoPlayback(video_id, source, camera_preview, _gl_handle, texture_id, autoplay, looping) => {
                    self.hal_prepare_video(video_id, source, Some(texture_id), autoplay, looping, camera_preview)
                }
                CxOsOp::PrepareAudioPlayback(video_id, source, autoplay, looping) => self.hal_prepare_video(video_id, source, None, autoplay, looping, crate::event::video_playback::CameraPreviewMode::Texture),
                CxOsOp::BeginVideoPlayback(v) => self.hal_video_control(v, makepad_hal::media::video_op::PLAY, 0),
                CxOsOp::PauseVideoPlayback(v) => self.hal_video_control(v, makepad_hal::media::video_op::PAUSE, 0),
                CxOsOp::ResumeVideoPlayback(v) => self.hal_video_control(v, makepad_hal::media::video_op::RESUME, 0),
                CxOsOp::MuteVideoPlayback(v) => self.hal_video_control(v, makepad_hal::media::video_op::MUTE, 0),
                CxOsOp::UnmuteVideoPlayback(v) => self.hal_video_control(v, makepad_hal::media::video_op::UNMUTE, 0),
                CxOsOp::CleanupVideoPlaybackResources(v) => self.hal_video_control(v, makepad_hal::media::video_op::CLEANUP, 0),
                CxOsOp::SeekVideoPlayback(v, position_ms) => self.hal_video_control(v, makepad_hal::media::video_op::SEEK, position_ms),
                CxOsOp::SetVideoVolume(v, volume) => self.hal_video_control(v, makepad_hal::media::video_op::VOLUME, (volume as f32).to_bits() as u64),
                CxOsOp::SetVideoPlaybackRate(v, rate) => self.hal_video_control(v, makepad_hal::media::video_op::RATE, (rate as f32).to_bits() as u64),
                CxOsOp::SelectVideoTrack(v, track) => self.hal_video_control(v, makepad_hal::media::video_op::SELECT_VIDEO_TRACK, track as u64),
                CxOsOp::SelectAudioTrack(v, track) => self.hal_video_control(v, makepad_hal::media::video_op::SELECT_AUDIO_TRACK, track as u64),
                op @ (CxOsOp::AttachCameraNativePreview { .. } | CxOsOp::UpdateCameraNativePreview { .. } | CxOsOp::DetachCameraNativePreview { .. }) => self.hal_camera_preview(op),
                op @ (CxOsOp::SpawnSystemBrowser { .. }
                | CxOsOp::UpdateSystemBrowser { .. }
                | CxOsOp::DetachSystemBrowser { .. }
                | CxOsOp::SetSystemBrowserUrl { .. }
                | CxOsOp::SystemBrowserHistoryGo { .. }
                | CxOsOp::CloseSystemBrowser { .. }) => self.hal_system_browser(op),
                // the macOS backend never had embedded web views either
                CxOsOp::CreateWebView { .. } | CxOsOp::UpdateWebView { .. } | CxOsOp::CloseWebView { .. } => self.hal.frames.gap("web views are not implemented on macOS"),
                CxOsOp::XrStartPresenting
                | CxOsOp::XrSetRenderScale(_)
                | CxOsOp::XrSetLocalAnchor(_)
                | CxOsOp::XrSetLocalFloor(_)
                | CxOsOp::XrAdvertiseAnchor(_)
                | CxOsOp::XrDiscoverAnchor(_)
                | CxOsOp::XrStopPresenting => self.hal.frames.gap("XR arrives in phase 5"),
            }
        }
        // A zoom queued layout geometry while handling these ops. Deliver it before the
        // next event; AppKit's moved buttons follow through the ordered Chrome/Geom pipe.
        self.handle_pending_window_geom_changes();
        true
    }
}

/// How the app wants its macOS window made, as the bits above the window kind.
fn macos_window_style(c: &crate::window::MacosWindowConfig) -> u32 {
    use crate::window::{MacosWindowChrome, MacosWindowKind, MacosWindowLevel};
    use makepad_hal::window_style as style;
    let c = c.normalized();
    let mut bits = 0;
    let mut set = |on: bool, bit: u32| {
        if on {
            bits |= bit;
        }
    };
    set(c.kind == MacosWindowKind::FloatingPanel, style::PANEL);
    set(c.chrome == MacosWindowChrome::Borderless, style::BORDERLESS);
    set(!c.closable, style::NOT_CLOSABLE);
    set(!c.miniaturizable, style::NOT_MINIATURIZABLE);
    set(!c.resizable, style::NOT_RESIZABLE);
    set(c.level == MacosWindowLevel::Floating, style::LEVEL_FLOATING);
    set(c.level == MacosWindowLevel::StatusBar, style::LEVEL_STATUS);
    set(c.non_activating, style::NON_ACTIVATING);
    set(c.becomes_key_only_if_needed, style::KEY_ONLY_IF_NEEDED);
    set(c.join_all_spaces, style::JOIN_ALL_SPACES);
    set(c.full_screen_auxiliary, style::FULLSCREEN_AUXILIARY);
    bits
}

fn cursor_index(c: MouseCursor) -> u32 {
    match c {
        MouseCursor::Hidden => cursor::HIDDEN,
        MouseCursor::Default => cursor::DEFAULT,
        MouseCursor::Crosshair => cursor::CROSSHAIR,
        MouseCursor::Hand => cursor::HAND,
        MouseCursor::Arrow => cursor::ARROW,
        MouseCursor::Move => cursor::MOVE,
        MouseCursor::Text => cursor::TEXT,
        MouseCursor::Wait => cursor::WAIT,
        MouseCursor::Help => cursor::HELP,
        MouseCursor::NotAllowed => cursor::NOT_ALLOWED,
        MouseCursor::Grab => cursor::GRAB,
        MouseCursor::Grabbing => cursor::GRABBING,
        MouseCursor::NResize => cursor::N_RESIZE,
        MouseCursor::NeResize => cursor::NE_RESIZE,
        MouseCursor::EResize => cursor::E_RESIZE,
        MouseCursor::SeResize => cursor::SE_RESIZE,
        MouseCursor::SResize => cursor::S_RESIZE,
        MouseCursor::SwResize => cursor::SW_RESIZE,
        MouseCursor::WResize => cursor::W_RESIZE,
        MouseCursor::NwResize => cursor::NW_RESIZE,
        MouseCursor::NsResize => cursor::NS_RESIZE,
        MouseCursor::NeswResize => cursor::NESW_RESIZE,
        MouseCursor::EwResize => cursor::EW_RESIZE,
        MouseCursor::NwseResize => cursor::NWSE_RESIZE,
        MouseCursor::ColResize => cursor::COL_RESIZE,
        MouseCursor::RowResize => cursor::ROW_RESIZE,
    }
}
