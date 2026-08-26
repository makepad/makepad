//! Fab product frontend over the reusable shell and feature-selected loaders.

use makepad_widgets::*;
use fab::{
    api::*, loader::{self, LoadCoordinator}, nav, render, sheets, tools, tour, ui, viewport,
};

app_main!(App);

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Fab"
                window.inner_size: vec2(1600, 1000)
                pass +: {
                    clear_color: fab.color_area
                }
                body +: {
                    flow: Down
                    spacing: 0
                    margin: 0
                    padding: 0
                    shell := FabShell{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: AppState,
    #[rust]
    loader: LoadCoordinator,
    #[rust]
    loaders_registered: bool,
    #[rust]
    started: bool,
    /// True while any `TextInput` owns key focus (outliner search, palette,
    /// drag-number entry). Tracked from `TextInputAction::KeyFocus[Lost]` in
    /// `handle_actions`; the bare viewport hotkeys (N/T/Z/Home) bail on it so
    /// typing "window" in a search field does not toggle panels (review M3).
    #[rust]
    text_input_focused: bool,
    /// Global movement keys remember the viewport that received their down
    /// event so the matching up is delivered even after focus/modal changes.
    #[rust]
    nav_keys_down: Vec<(KeyCode, usize)>,
}

impl App {
    /// Route one action through every lane hook, then the core state.
    fn dispatch(&mut self, cx: &mut Cx, action: &ShellAction) -> bool {
        let opens_modal = matches!(
            action,
            ShellAction::ShowFileBrowser(true) | ShellAction::ShowKeymapHelp(true)
        ) || matches!(action, ShellAction::ToggleCommandPalette)
            && !self.state.ui.command_palette_open;
        if opens_modal || matches!(action, ShellAction::SetWorkspace(_)) {
            self.release_nav_controls(cx);
        }
        let mut changed = false;
        changed |= loader::apply(cx, &mut self.loader, &mut self.state, action);
        changed |= self.state.apply_core(action);
        changed |= viewport::apply(cx, &mut self.state, action);
        changed |= nav::apply(cx, &mut self.state, action);
        changed |= tools::apply(cx, &mut self.state, action);
        changed |= sheets::apply(cx, &mut self.state, action);
        changed |= render::apply(cx, &mut self.state, action);
        changed |= tour::apply(cx, &mut self.state, action);
        changed |= ui::apply(cx, &mut self.state, action);
        match action {
            ShellAction::Quit => cx.quit(),
            ShellAction::Command(name) => {
                self.run_command(cx, name);
            }
            _ => {}
        }
        changed
    }

    /// The F3 palette / menus speak in command names.
    fn run_command(&mut self, cx: &mut Cx, name: &str) {
        let v = self.state.active_view;
        let n = name.trim().to_lowercase();
        let action = match n.as_str() {
            "frame all" | "view.frame_all" => Some(ShellAction::FrameAll(v)),
            "frame selected" | "view.frame_selected" => Some(ShellAction::FrameSelected(v)),
            "front" | "front view" => Some(ShellAction::PresetView(v, PresetView::Front)),
            "top" | "top view" => Some(ShellAction::PresetView(v, PresetView::Top)),
            "right" | "right view" => Some(ShellAction::PresetView(v, PresetView::Right)),
            "iso" | "isometric" => Some(ShellAction::PresetView(v, PresetView::Isometric)),
            "ortho" | "perspective" | "toggle ortho" => Some(ShellAction::ToggleOrtho(v)),
            "hide selected" => Some(ShellAction::HideSelected),
            "unhide all" => Some(ShellAction::UnhideAll),
            "isolate" | "isolate selected" => Some(ShellAction::IsolateSelected),
            "render image" | "render" => {
                // F12 and the palette: the render happens in the Render
                // workspace's `FabRenderView`, so go there first.
                cx.action(ShellAction::SetWorkspace(Workspace::Render));
                Some(ShellAction::RenderStart)
            }
            "save render" | "save png" | "export png" => {
                self.save_render(cx);
                None
            }
            "open" => Some(ShellAction::ShowFileBrowser(true)),
            "open demo" | "demo" => Some(ShellAction::OpenDemo),
            "wireframe" => Some(ShellAction::SetShading(v, Shading::Wireframe)),
            "solid" => Some(ShellAction::SetShading(v, Shading::Solid)),
            "material" => Some(ShellAction::SetShading(v, Shading::Material)),
            "realtime" => Some(ShellAction::SetShading(v, Shading::Realtime)),
            "rendered" => Some(ShellAction::SetShading(v, Shading::Rendered)),
            "hidden line" | "ink" => Some(ShellAction::SetShading(v, Shading::HiddenLine)),
            "quad view" => Some(ShellAction::ToggleQuadView),
            "keymap" | "help" => Some(ShellAction::ShowKeymapHelp(true)),
            "quit" => Some(ShellAction::Quit),
            _ => None,
        };
        match action {
            Some(a) => {
                cx.action(a);
            }
            None if matches!(n.as_str(), "save render" | "save png" | "export png") => {}
            None => {
                self.state.ui.status_message = format!("Unknown command: {name}");
            }
        }
    }

    /// Where saved renders go: `local/fab/renders/<model>-<unix seconds>.png`
    /// under the repo root (the same root the samples resolve against).
    fn render_output_path(&self) -> std::path::PathBuf {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let dir = repo_root.join("local/fab/renders");
        let _ = std::fs::create_dir_all(&dir);
        let stem: String = self
            .state
            .scene
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
            .collect();
        let stem = stem.trim_matches('-');
        let stem = if stem.is_empty() { "render" } else { stem };
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        dir.join(format!("{stem}-{secs}.png"))
    }

    /// Cmd+S / "save render": the `FabRenderView` (Render workspace) owns
    /// the tracer and answers `ExportPng` with a capture of the current
    /// accumulation. When that view is not on screen yet, the first Cmd+S
    /// starts the render there and says so; the next one saves.
    fn save_render(&mut self, cx: &mut Cx) {
        // A viewport in Rendered shading (the right pane by default) answers
        // `ExportPng` with a capture of what it shows right now; the F12
        // `FabRenderView` does the same when it is on screen.
        let rendered_pane = self.state.views.iter().any(|v| v.shading == Shading::Rendered);
        let render_view = self.state.ui.workspace == Workspace::Render && self.state.render.running;
        if !rendered_pane && !render_view {
            cx.action(ShellAction::SetWorkspace(Workspace::Render));
            cx.action(ShellAction::RenderStart);
            self.state.ui.status_message =
                "Rendering in the Render workspace — Cmd+S again to save the PNG".into();
            return;
        }
        let path = self.render_output_path();
        self.state.ui.status_message = format!("Saving {}…", path.display());
        cx.action(ShellAction::ExportPng(path));
    }

    /// Global keys, before the widget tree sees the event. Anything with a
    /// text field focused is left alone except the function keys.
    fn global_key(&mut self, cx: &mut Cx, ke: &KeyEvent) -> bool {
        let m = ke.modifiers;
        let v = self.state.active_view;
        let walking = matches!(self.state.view().nav_mode, NavMode::Walk | NavMode::Fly);

        // Escape is the pointer-safety path before every modal rule. Release
        // the OS lock and movement keys even if an overlay currently owns
        // focus, then let the navigator leave first-person mode.
        if ke.key_code == KeyCode::Escape {
            self.release_nav_controls(cx);
            if walking {
                cx.action(ShellAction::NavKey {
                    view: v,
                    key: KeyCode::Escape,
                    down: true,
                    mods: m,
                    repeat: ke.is_repeat,
                });
            }
            if self.state.ui.file_browser_open {
                cx.action(ShellAction::ShowFileBrowser(false));
            }
            if self.state.ui.keymap_help_open {
                cx.action(ShellAction::ShowKeymapHelp(false));
            }
            if self.state.ui.command_palette_open {
                cx.action(ShellAction::ToggleCommandPalette);
            }
            return walking
                || self.state.ui.file_browser_open
                || self.state.ui.keymap_help_open
                || self.state.ui.command_palette_open;
        }

        if !ke.is_repeat {
            let action = match ke.key_code {
                KeyCode::F1 => Some(ShellAction::ShowKeymapHelp(!self.state.ui.keymap_help_open)),
                KeyCode::F3 if m.shift => Some(ShellAction::TogglePerf),
                KeyCode::F3 => Some(ShellAction::ToggleCommandPalette),
                KeyCode::F12 => {
                    cx.action(ShellAction::SetWorkspace(Workspace::Render));
                    Some(ShellAction::RenderStart)
                }
                KeyCode::KeyS if m.is_primary() => {
                    self.save_render(cx);
                    return true;
                }
                KeyCode::KeyO if m.is_primary() && m.shift => Some(ShellAction::OpenDemo),
                KeyCode::KeyO if m.is_primary() => Some(ShellAction::ShowFileBrowser(true)),
                KeyCode::KeyQ if m.is_primary() => Some(ShellAction::Quit),
                KeyCode::Space if m.control => Some(ShellAction::ToggleMaximizeArea),
                _ => None,
            };
            if let Some(a) = action {
                cx.action(a);
                return true;
            }
        }

        // Viewport-context keys that must work without viewport focus — but
        // never while a text field is focused: these are bare letters and
        // would swallow typing (review M3).
        let modal_open = self.state.ui.file_browser_open
            || self.state.ui.keymap_help_open
            || self.state.ui.command_palette_open;
        if modal_open || self.text_input_focused || cx.keyboard.modifiers().logo {
            return false;
        }

        // W is both the entry command and forward movement. Post the mode
        // action first so the viewport synchronises its Navigator before the
        // same physical key-down reaches it.
        if ke.key_code == KeyCode::KeyW
            && !m.control
            && !m.alt
            && !m.shift
            && !m.logo
            && !walking
        {
            cx.action(ShellAction::SetTool(Tool::Walk));
            self.route_nav_key_down(cx, v, ke);
            return true;
        }
        if walking && is_walk_control_key(ke.key_code) && !m.logo {
            self.route_nav_key_down(cx, v, ke);
            return true;
        }
        if ke.is_repeat {
            return false;
        }
        let action = match ke.key_code {
            KeyCode::KeyN => Some(ShellAction::ToggleSidebar),
            KeyCode::KeyT => Some(ShellAction::ToggleToolbar),
            KeyCode::KeyP
                if !m.control
                    && !m.alt
                    && !m.shift
                    && !m.logo
                    && self.state.view_at(v).shading == Shading::Rendered =>
            {
                Some(ShellAction::SetRenderedPaused(
                    v,
                    !self.state.view_at(v).rendered_paused,
                ))
            }
            KeyCode::KeyZ if m.alt => Some(ShellAction::ToggleXray(v)),
            KeyCode::Home => Some(ShellAction::FrameAll(v)),
            _ => None,
        };
        if let Some(a) = action {
            cx.action(a);
            return true;
        }
        false
    }

    fn route_nav_key_down(&mut self, cx: &mut Cx, view: usize, ke: &KeyEvent) {
        let routed_view = if let Some((_, routed_view)) = self
            .nav_keys_down
            .iter()
            .find(|(key, _)| *key == ke.key_code)
        {
            *routed_view
        } else {
            self.nav_keys_down.push((ke.key_code, view));
            view
        };
        cx.action(ShellAction::NavKey {
            view: routed_view,
            key: ke.key_code,
            down: true,
            mods: ke.modifiers,
            repeat: ke.is_repeat,
        });
    }

    fn global_key_up(&mut self, cx: &mut Cx, ke: &KeyEvent) -> bool {
        let Some(i) = self
            .nav_keys_down
            .iter()
            .position(|(key, _)| *key == ke.key_code)
        else {
            return false;
        };
        let (_, view) = self.nav_keys_down.swap_remove(i);
        cx.action(ShellAction::NavKey {
            view,
            key: ke.key_code,
            down: false,
            mods: ke.modifiers,
            repeat: false,
        });
        true
    }

    /// Clear every globally routed key and release every viewport capture.
    /// This is safe to call repeatedly and is used by modal/focus transitions.
    fn release_nav_controls(&mut self, cx: &mut Cx) {
        for (key, view) in self.nav_keys_down.drain(..) {
            cx.action(ShellAction::NavKey {
                view,
                key,
                down: false,
                mods: KeyModifiers::default(),
                repeat: false,
            });
        }
        nav::walk::request_capture_release();
        cx.action(ShellAction::NavReleaseCapture);
    }

    fn open_from_args(&mut self, cx: &mut Cx) {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut path: Option<String> = None;
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a == "--open" {
                path = args.get(i + 1).cloned();
                i += 1;
            } else if let Some(p) = a.strip_prefix("--open=") {
                path = Some(p.to_string());
            } else if !a.starts_with("--") {
                path = Some(a.clone());
            }
            i += 1;
        }
        match path {
            Some(p) => self.loader.open(cx, std::path::PathBuf::from(p)),
            None => self.loader.open_demo(cx),
        }
    }

    fn register_loaders(&mut self) {
        if self.loaders_registered {
            return;
        }
        #[cfg(feature = "gltf")]
        self.loader.register(makepad_fab_loader_gltf::GltfLoader);
        self.loaders_registered = true;
    }
}

fn is_walk_control_key(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::KeyW
            | KeyCode::KeyA
            | KeyCode::KeyS
            | KeyCode::KeyD
            | KeyCode::KeyQ
            | KeyCode::KeyE
            | KeyCode::KeyF
            | KeyCode::ArrowUp
            | KeyCode::ArrowDown
            | KeyCode::ArrowLeft
            | KeyCode::ArrowRight
            | KeyCode::Shift
            | KeyCode::Control
            | KeyCode::Space
    )
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let mut changed = false;
        let (mut focus_gained, mut focus_lost) = (false, false);
        for action in actions {
            if let Some(a) = action.downcast_ref::<ShellAction>() {
                changed |= self.dispatch(cx, a);
            }
            if let Some(wa) = action.downcast_ref::<WidgetAction>() {
                match wa.action.downcast_ref::<TextInputAction>() {
                    Some(TextInputAction::KeyFocus) => focus_gained = true,
                    Some(TextInputAction::KeyFocusLost) => focus_lost = true,
                    _ => (),
                }
            }
        }
        // A focus handoff between two inputs delivers Lost + Focus in one
        // batch; the gain wins so the flag stays true across the handoff.
        if focus_gained {
            self.release_nav_controls(cx);
            self.text_input_focused = true;
        } else if focus_lost {
            self.text_input_focused = false;
        }
        if changed {
            self.ui.redraw(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        fab::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if !self.started {
            if let Event::Startup | Event::Draw(_) = event {
                self.started = true;
                self.register_loaders();
                cx.perf_monitor.set_enabled(self.state.ui.show_perf);
                self.open_from_args(cx);
            }
        }
        if let Event::KeyDown(ke) = event {
            if self.global_key(cx, ke) {
                return;
            }
        }
        if let Event::KeyUp(ke) = event {
            if self.global_key_up(cx, ke) {
                return;
            }
        }
        if matches!(
            event,
            Event::WindowLostFocus(_) | Event::Pause | Event::Background
        ) {
            self.release_nav_controls(cx);
        }
        self.match_event(cx, event);
        self.ui
            .handle_event(cx, event, &mut Scope::with_data(&mut self.state));
    }
}
