use makepad_code_editor;
use makepad_widgets::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::audio::{self, get_audio_state};
use crate::ws::stdio_bridge::StdioBridge;
use crate::ws::types::CanvasCommand;

// ============================================================================
// UI — Visual output canvas for Claude Code
// ============================================================================

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*


    // Override Markdown with code_block for syntax highlighting.
    // This works because set_type_default now marks objects as auto,
    // so vec entries (named children) are copied to instances.
    mod.widgets.Markdown = set_type_default() do mod.widgets.MarkdownBase{
        width: Fill height: Fit
        flow: Flow.Right{wrap: true}
        padding: theme.mspace_1
        font_size: theme.font_size_p
        font_color: theme.color_label_inner
        paragraph_spacing: 16
        pre_code_spacing: 8
        inline_code_padding: theme.mspace_1
        inline_code_margin: theme.mspace_1
        heading_base_scale: 1.8
        draw_text +: { color: theme.color_label_inner }
        text_style_normal: theme.font_regular{ font_size: theme.font_size_p }
        text_style_italic: theme.font_italic{ font_size: theme.font_size_p }
        text_style_bold: theme.font_bold{ font_size: theme.font_size_p }
        text_style_bold_italic: theme.font_bold_italic{ font_size: theme.font_size_p }
        text_style_fixed: theme.font_code{ font_size: theme.font_size_p }
        code_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: 10}
        }
        code_walk: Walk{width: Fill height: Fit}
        quote_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: theme.space_2}
        }
        quote_walk: Walk{width: Fill height: Fit}
        list_item_layout: Layout{ flow: Flow.Right{wrap: true} padding: theme.mspace_1 }
        list_item_walk: Walk{ height: Fit width: Fill }
        sep_walk: Walk{ width: Fill height: 4. margin: theme.mspace_v_1 }
        draw_block +: {
            line_color: theme.color_label_inner
            sep_color: theme.color_shadow
            quote_bg_color: theme.color_bg_highlight
            quote_fg_color: theme.color_label_inner
            code_color: theme.color_bg_highlight
            selection_color: theme.color_selection_focus
            space_1: uniform(theme.space_1)
            space_2: uniform(theme.space_2)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                match self.block_type {
                    FlowBlockType.Quote => {
                        sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                        sdf.fill(self.quote_bg_color)
                        sdf.box(self.space_1 self.space_1 self.space_1 self.rect_size.y-self.space_2 1.5)
                        sdf.fill(self.quote_fg_color)
                        return sdf.result
                    }
                    FlowBlockType.Sep => {
                        sdf.box(0. 1. self.rect_size.x-1. self.rect_size.y-2. 2.)
                        sdf.fill(self.sep_color)
                        return sdf.result
                    }
                    FlowBlockType.Code => {
                        sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                        sdf.fill(self.code_color)
                        return sdf.result
                    }
                    FlowBlockType.InlineCode => {
                        sdf.box(1. 1. self.rect_size.x-2. self.rect_size.y-2. 2.)
                        sdf.fill(self.code_color)
                        return sdf.result
                    }
                    FlowBlockType.Underline => {
                        sdf.box(0. self.rect_size.y-2. self.rect_size.x 2.0 0.5)
                        sdf.fill(self.line_color)
                        return sdf.result
                    }
                    FlowBlockType.Strikethrough => {
                        sdf.box(0. self.rect_size.y * 0.45 self.rect_size.x 2.0 0.5)
                        sdf.fill(self.line_color)
                        return sdf.result
                    }
                    FlowBlockType.Selection => {
                        return vec4(self.selection_color.rgb * self.selection_color.a, self.selection_color.a)
                    }
                }
                return #f00
            }
        }
        link := mod.widgets.MarkdownLink{}
        use_code_block_widget: true
        code_block := View{width: Fill height: Fit flow: Overlay
            code_view := CodeView{
                editor +: {
                    height: Fit
                    draw_bg +: { color: #x1a1a2e }
                }
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1200, 800)
                window.transparent: true
                show_caption_bar: false
                pass.clear_color: #x0a0a1840
                body +: {
                    View{width: Fill height: Fill flow: Right

                        // Sidebar
                        sidebar := SolidView{width: 220 height: Fill draw_bg.color: #x12121e90 flow: Down padding: Inset{left: 12. right: 12. top: 10. bottom: 10.} spacing: 6 new_batch: true
                            // Toggle button
                            sidebar_toggle := Button{text: "<<" width: Fit height: Fit draw_bg.color: #x22223366 draw_text.color: #x8888aa padding: Inset{left: 8. right: 8. top: 4. bottom: 4.} draw_bg.radius: 4.}

                            // Section: Examples
                            Label{text: "Examples" draw_text.color: #x666688 draw_text.text_style.font_size: 9}
                            example_pomodoro := Button{text: "Pomodoro Timer" width: Fill draw_bg.color: #x1e1e3050 draw_text.color: #xaaaacc draw_text.text_style.font_size: 10 padding: Inset{left: 10. right: 10. top: 6. bottom: 6.} draw_bg.radius: 4.}
                            example_music := Button{text: "Music Player" width: Fill draw_bg.color: #x1e1e3050 draw_text.color: #xaaaacc draw_text.text_style.font_size: 10 padding: Inset{left: 10. right: 10. top: 6. bottom: 6.} draw_bg.radius: 4.}
                            example_dashboard := Button{text: "Token Dashboard" width: Fill draw_bg.color: #x1e1e3050 draw_text.color: #xaaaacc draw_text.text_style.font_size: 10 padding: Inset{left: 10. right: 10. top: 6. bottom: 6.} draw_bg.radius: 4.}
                            example_monitor := Button{text: "Claude Monitor" width: Fill draw_bg.color: #x1e1e3050 draw_text.color: #xaaaacc draw_text.text_style.font_size: 10 padding: Inset{left: 10. right: 10. top: 6. bottom: 6.} draw_bg.radius: 4.}

                            // Separator
                            SolidView{width: Fill height: 1. draw_bg.color: #x333355}

                            // Section: History
                            Label{text: "History" draw_text.color: #x666688 draw_text.text_style.font_size: 9}
                            ScrollYView{width: Fill height: Fill
                                history_list := Splash{width: Fill height: Fit}
                            }
                        }

                        // Main content area
                        View{width: Fill height: Fill flow: Down

                            // Status bar (semi-transparent glass)
                            SolidView{width: Fill height: Fit draw_bg.color: #x1a1a2e60 padding: Inset{left: 16. right: 16. top: 8. bottom: 8.} flow: Right spacing: 16 align: Align{y: 0.5} new_batch: true
                                sidebar_open := Button{text: ">>" width: Fit height: Fit draw_bg.color: #x22223366 draw_text.color: #x8888aa padding: Inset{left: 8. right: 8. top: 4. bottom: 4.} draw_bg.radius: 4. visible: false}
                                Label{text: "Makepad Canvas" draw_text.color: #x8888bb draw_text.text_style.font_size: 10}
                                Filler{}
                                reload_btn := Button{text: "Reload" width: Fit height: Fit draw_bg.color: #x22223366 draw_text.color: #x8888aa padding: Inset{left: 8. right: 8. top: 4. bottom: 4.} draw_bg.radius: 4.}
                                status_conn := Label{text: "Waiting..." draw_text.color: #xccaa55 draw_text.text_style.font_size: 10}
                            }

                            // Splash rendering panel
                            View{width: Fill height: Fill flow: Overlay
                                SolidView{width: Fill height: Fill draw_bg.color: #x0e0e2230}
                                // Audio visualizer layer (behind Splash)
                                audio_vis := SpectrumBars{width: Fill height: Fill visible: false}
                                // Splash on top (transparent bg lets shader show through)
                                ScrollYView{width: Fill height: Fill
                                    splash_panel := Splash{width: Fill height: Fit visible: false}
                                }
                                placeholder := View{width: Fill height: Fill align: Center
                                    Label{text: "Waiting for Claude Code..." draw_text.color: #x333344 draw_text.text_style.font_size: 18}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// App
// ============================================================================

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    signal: SignalToUI,
    #[rust]
    bridge: Option<Arc<StdioBridge>>,
    #[rust]
    initialized: bool,
    #[rust]
    uid_map: HashMap<WidgetUid, String>,
    #[rust]
    uid_map_dirty: bool,
    #[rust]
    widget_names: Vec<String>,
    #[rust]
    stream_code: String,
    #[rust]
    audio_signal: SignalToUI,
    #[rust]
    audio_initialized: bool,
    #[rust]
    sidebar_visible: bool,
    #[rust]
    history: Vec<(String, String)>,
    #[rust]
    history_counter: u32,
    #[rust]
    current_code: String,
    #[rust]
    buttons_hidden: bool,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;

        self.sidebar_visible = true;

        let bridge = Arc::new(StdioBridge::new(self.signal.clone()));
        bridge.start();
        self.bridge = Some(bridge);

        // Initialize audio output
        let audio_state = get_audio_state();
        audio::start_audio_output(cx, audio_state, self.audio_signal.clone());
        self.audio_initialized = true;

        // Load history from disk
        self.load_history();
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        let default_output = devices.default_output();
        if !default_output.is_empty() {
            cx.use_audio_outputs(&default_output);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if let Some(wa) = action.as_widget_action() {
                if let Some(ButtonAction::Clicked(_)) = wa.action.downcast_ref::<ButtonAction>() {
                    let name = self
                        .uid_map
                        .get(&wa.widget_uid)
                        .cloned()
                        .unwrap_or_default();

                    // Generic audio control: buttons named audio_* route to audio service
                    match name.as_str() {
                        "audio_toggle" | "play_btn" => {
                            get_audio_state().toggle();
                            continue;
                        }
                        "audio_stop" => {
                            get_audio_state().stop();
                            continue;
                        }
                        _ => {}
                    }

                    // Sidebar toggle (close)
                    if wa.widget_uid == self.ui.widget(cx, ids!(sidebar_toggle)).widget_uid() {
                        self.sidebar_visible = false;
                        self.ui.widget(cx, ids!(sidebar)).set_visible(cx, false);
                        self.ui.widget(cx, ids!(sidebar_open)).set_visible(cx, true);
                        self.ui.redraw(cx);
                        continue;
                    }
                    // Sidebar open
                    if wa.widget_uid == self.ui.widget(cx, ids!(sidebar_open)).widget_uid() {
                        self.sidebar_visible = true;
                        self.ui.widget(cx, ids!(sidebar)).set_visible(cx, true);
                        self.ui
                            .widget(cx, ids!(sidebar_open))
                            .set_visible(cx, false);
                        self.ui.redraw(cx);
                        continue;
                    }
                    // Reload current Splash
                    if wa.widget_uid == self.ui.widget(cx, ids!(reload_btn)).widget_uid() {
                        let code = self.current_code.clone();
                        if !code.is_empty() {
                            self.apply_splash_code(cx, &code);
                        }
                        continue;
                    }

                    // Example buttons
                    if wa.widget_uid == self.ui.widget(cx, ids!(example_pomodoro)).widget_uid() {
                        self.load_example(cx, "pomodoro.splash");
                        continue;
                    }
                    if wa.widget_uid == self.ui.widget(cx, ids!(example_music)).widget_uid() {
                        self.load_example(cx, "music-player.splash");
                        continue;
                    }
                    if wa.widget_uid == self.ui.widget(cx, ids!(example_dashboard)).widget_uid() {
                        self.load_example(cx, "token-dashboard.splash");
                        continue;
                    }
                    if wa.widget_uid == self.ui.widget(cx, ids!(example_monitor)).widget_uid() {
                        self.load_example(cx, "claude-monitor.splash");
                        continue;
                    }

                    // History item buttons (check prefix match on uid_map or use history index)
                    if let Some(idx) = self.find_history_button_index(cx, wa.widget_uid) {
                        if let Some((_, code)) = self.history.get(idx).cloned() {
                            self.apply_splash_code(cx, &code);
                        }
                        continue;
                    }

                    if let Some(bridge) = &self.bridge {
                        let event_name = if !name.is_empty() {
                            name
                        } else {
                            format!("uid_{:?}", wa.widget_uid)
                        };
                        bridge.send_event(&event_name);
                    }
                }
            }
        }
    }

    fn handle_signal(&mut self, cx: &mut Cx) {
        // Handle audio signal (spectrum/position updates)
        if self.audio_signal.check_and_clear() {
            let audio_state = get_audio_state();
            let amplitude = audio_state.amplitude.get();
            let spectrum: [f32; 16] = if let Ok(bands) = audio_state.spectrum.lock() {
                *bands
            } else {
                [0.0; 16]
            };

            // Inject spectrum data into VM globals so Splash code can read them
            let band_ids = [
                id!(_b0),
                id!(_b1),
                id!(_b2),
                id!(_b3),
                id!(_b4),
                id!(_b5),
                id!(_b6),
                id!(_b7),
                id!(_b8),
                id!(_b9),
                id!(_b10),
                id!(_b11),
                id!(_b12),
                id!(_b13),
                id!(_b14),
                id!(_b15),
            ];
            cx.with_vm(|vm| {
                for (i, &bid) in band_ids.iter().enumerate() {
                    vm.set_injected_global(bid, ScriptValue::from(spectrum[i] as f64));
                }
                vm.set_injected_global(id!(_amp), ScriptValue::from(amplitude));
                vm.set_injected_global(
                    id!(_playing),
                    ScriptValue::from(audio_state.is_playing.load(Ordering::Relaxed)),
                );
                vm.set_injected_global(
                    id!(_pos),
                    ScriptValue::from(audio_state.position_secs.get()),
                );
                vm.set_injected_global(
                    id!(_dur),
                    ScriptValue::from(audio_state.duration_secs.get()),
                );
            });

            // Call on_audio() in Splash scope if defined
            if let Some(mut splash) = self
                .ui
                .widget(cx, ids!(splash_panel))
                .borrow_mut::<Splash>()
            {
                splash.call_fn(cx, id!(on_audio));
            }

            // Broadcast to WS clients
            if let Some(bridge) = &self.bridge {
                let is_playing = audio_state.is_playing.load(Ordering::Relaxed);
                let position = audio_state.position_secs.get();
                let duration = audio_state.duration_secs.get();
                let msg = serde_json::json!({
                    "audio_state": {
                        "playing": is_playing,
                        "position": position,
                        "duration": duration,
                        "amplitude": amplitude,
                        "spectrum": spectrum,
                    }
                });
                bridge.broadcast_message(&msg.to_string());
            }
            self.ui.redraw(cx);
        }

        if !self.signal.check_and_clear() {
            return;
        }

        let bridge = match &self.bridge {
            Some(b) => b.clone(),
            None => return,
        };

        let commands: Vec<CanvasCommand> = {
            let mut q = bridge.commands.lock().unwrap();
            q.drain(..).collect()
        };

        for cmd in commands {
            match cmd {
                CanvasCommand::ConnectionState { connected } => {
                    let text = if connected { "Connected" } else { "Waiting..." };
                    self.ui.label(cx, ids!(status_conn)).set_text(cx, text);
                }

                CanvasCommand::SplashRender { code } => {
                    if code.is_empty() {
                        if let Some(mut splash) = self
                            .ui
                            .widget(cx, ids!(splash_panel))
                            .borrow_mut::<Splash>()
                        {
                            splash.view.set_visible(cx, false);
                        }
                        self.ui.widget(cx, ids!(placeholder)).set_visible(cx, true);
                        self.widget_names.clear();
                        self.uid_map.clear();
                    } else {
                        // Extract named widgets (foo := Button{...}) for event routing
                        self.widget_names = extract_widget_names(&code);
                        self.uid_map_dirty = true;

                        if let Some(mut splash) = self
                            .ui
                            .widget(cx, ids!(splash_panel))
                            .borrow_mut::<Splash>()
                        {
                            splash.set_text(cx, &code);
                            // set_text may replace self.view, so set visible AFTER
                            splash.view.set_visible(cx, true);
                        }
                        self.ui.widget(cx, ids!(placeholder)).set_visible(cx, false);

                        // Save to history only if this is a genuinely new app.
                        // Detect data refreshes by comparing extracted title with
                        // the last history entry's title (same title = same app, just updated data).
                        let new_title = extract_app_title(&code);
                        let is_refresh = self.history.last().map_or(false, |(old_name, _)| {
                            new_title.as_deref() == Some(old_name.as_str())
                        });
                        if is_refresh {
                            // Update the last history entry in-place (no new sidebar item)
                            if let Some(last) = self.history.last_mut() {
                                last.1 = code.clone();
                            }
                        } else {
                            self.add_to_history(cx, &code);
                        }
                    }
                }

                CanvasCommand::SplashStreamBegin => {
                    self.stream_code.clear();
                    if let Some(mut splash) = self
                        .ui
                        .widget(cx, ids!(splash_panel))
                        .borrow_mut::<Splash>()
                    {
                        splash.view.set_visible(cx, true);
                        splash.stream_begin(cx);
                    }
                    self.ui.widget(cx, ids!(placeholder)).set_visible(cx, false);
                }

                CanvasCommand::SplashStreamAppend { code } => {
                    self.stream_code.push_str(&code);
                    if let Some(mut splash) = self
                        .ui
                        .widget(cx, ids!(splash_panel))
                        .borrow_mut::<Splash>()
                    {
                        splash.stream_append(cx, &code);
                    }
                }

                CanvasCommand::SplashStreamEnd => {
                    self.widget_names = extract_widget_names(&self.stream_code);
                    self.uid_map_dirty = true;

                    // Save streamed code to history
                    if !self.stream_code.is_empty() {
                        let code = self.stream_code.clone();
                        self.add_to_history(cx, &code);
                    }
                }

                CanvasCommand::SplashEval { .. } => {
                    // Reserved for future use
                }

                CanvasCommand::AudioPlay { url } => {
                    let audio_state = get_audio_state();
                    audio::download_and_decode(url, audio_state, self.audio_signal.clone());
                    // Show visualizer
                    if let Some(mut v) = self
                        .ui
                        .widget(cx, ids!(audio_vis))
                        .borrow_mut::<crate::visualizer::Visualizer>()
                    {
                        v.visible = true;
                    }
                }
                CanvasCommand::AudioPause => {
                    get_audio_state().toggle();
                }
                CanvasCommand::AudioStop => {
                    get_audio_state().stop();
                    // Hide visualizer
                    if let Some(mut v) = self
                        .ui
                        .widget(cx, ids!(audio_vis))
                        .borrow_mut::<crate::visualizer::Visualizer>()
                    {
                        v.visible = false;
                    }
                }
                CanvasCommand::AudioToggle => {
                    get_audio_state().toggle();
                }
                CanvasCommand::SaveApp { name } => {
                    self.save_app(cx, &name);
                }
            }
        }

        self.ui.redraw(cx);
    }
}

/// Extract named widget identifiers from Splash code.
/// Pattern: `name := Widget{...}` → extracts "name"
fn extract_widget_names(code: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut i = 0;
    let bytes = code.as_bytes();
    while i < bytes.len() {
        // Look for `:=` pattern
        if i + 1 < bytes.len() && bytes[i] == b':' && bytes[i + 1] == b'=' {
            // Walk backwards to find the name
            let mut end = i;
            while end > 0 && bytes[end - 1] == b' ' {
                end -= 1;
            }
            let mut start = end;
            while start > 0
                && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
            {
                start -= 1;
            }
            if start < end {
                if let Ok(name) = std::str::from_utf8(&bytes[start..end]) {
                    names.push(name.to_string());
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    names
}

// ============================================================================
// Sidebar & History helpers
// ============================================================================

impl App {
    /// Directory for persisting saved apps.
    fn apps_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let dir = PathBuf::from(home).join(".canvas-apps");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Find the examples directory relative to the current executable or cwd.
    fn examples_dir() -> Option<PathBuf> {
        // Try relative to cwd first (typical cargo run scenario)
        let cwd = std::env::current_dir().ok()?;
        let candidate = cwd.join("tools/canvas/examples");
        if candidate.is_dir() {
            return Some(candidate);
        }
        // Try relative to executable
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let candidate = parent.join("examples");
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    /// Load an example .splash file by filename and render it.
    fn load_example(&mut self, cx: &mut Cx, filename: &str) {
        if let Some(dir) = Self::examples_dir() {
            let path = dir.join(filename);
            if let Ok(code) = std::fs::read_to_string(&path) {
                self.apply_splash_code(cx, &code);
            } else {
                log!("Canvas: failed to read example {}", path.display());
            }
        } else {
            log!("Canvas: examples directory not found");
        }
    }

    /// Apply splash code to the panel (shared by example loading and history recall).
    fn apply_splash_code(&mut self, cx: &mut Cx, code: &str) {
        self.current_code = code.to_string();
        self.widget_names = extract_widget_names(code);
        self.uid_map_dirty = true;

        if let Some(mut splash) = self
            .ui
            .widget(cx, ids!(splash_panel))
            .borrow_mut::<Splash>()
        {
            splash.set_text(cx, &code);
            splash.view.set_visible(cx, true);
        }
        self.ui.widget(cx, ids!(placeholder)).set_visible(cx, false);
        self.ui.redraw(cx);
    }

    /// Add code to history, save to disk, and update the sidebar list.
    fn add_to_history(&mut self, cx: &mut Cx, code: &str) {
        self.history_counter += 1;
        let name =
            extract_app_title(code).unwrap_or_else(|| format!("App #{}", self.history_counter));
        self.history.push((name.clone(), code.to_string()));

        // Persist to disk
        let dir = Self::apps_dir();
        let filename = format!("{:04}_{}.splash", self.history_counter, slug(&name));
        let _ = std::fs::write(dir.join(&filename), code);

        // Keep at most 50 entries
        while self.history.len() > 50 {
            self.history.remove(0);
        }

        self.rebuild_history_ui(cx);
    }

    /// Save current app with an explicit name.
    fn save_app(&mut self, cx: &mut Cx, name: &str) {
        if self.current_code.is_empty() {
            return;
        }
        let app_name = if name.is_empty() {
            extract_app_title(&self.current_code)
                .unwrap_or_else(|| format!("App #{}", self.history_counter + 1))
        } else {
            name.to_string()
        };

        self.history_counter += 1;
        self.history
            .push((app_name.clone(), self.current_code.clone()));

        let dir = Self::apps_dir();
        let filename = format!("{:04}_{}.splash", self.history_counter, slug(&app_name));
        let _ = std::fs::write(dir.join(&filename), &self.current_code);

        self.rebuild_history_ui(cx);
        log!("Canvas: saved app '{}'", app_name);
    }

    /// Rebuild the history list buttons in the sidebar.
    fn rebuild_history_ui(&mut self, cx: &mut Cx) {
        // Build Splash code for the history list dynamically
        // Prefix with flow: Down so items stack vertically in the sidebar
        let mut items = String::from("flow: Down spacing: 4\n");
        for (i, (name, _)) in self.history.iter().enumerate().rev() {
            // Each history item is a button with a unique name
            items.push_str(&format!(
                "hist_{i} := Button{{text: \"{name}\" width: Fill draw_bg.color: #x1a1a3050 draw_text.color: #x9999bb draw_text.text_style.font_size: 9.5 padding: Inset{{left: 10. right: 10. top: 5. bottom: 5.}} draw_bg.radius: 3.}}\n"
            ));
        }

        if let Some(mut history_list) = self
            .ui
            .widget(cx, ids!(history_list))
            .borrow_mut::<Splash>()
        {
            if items.len() <= 25 {
                // Only the prefix, no actual items
                history_list.set_text(cx, "Label{text: \"No history yet\" draw_text.color: #x444466 draw_text.text_style.font_size: 9}");
            } else {
                history_list.set_text(cx, &items);
            }
            history_list.view.set_visible(cx, true);
        }
    }

    /// Find which history index a button UID corresponds to.
    fn find_history_button_index(&self, cx: &mut Cx, uid: WidgetUid) -> Option<usize> {
        let history_list = self.ui.widget(cx, ids!(history_list));
        // History is displayed in reverse order in the UI
        let display_count = self.history.len();
        for (display_idx, (i, _)) in self.history.iter().enumerate().rev().enumerate() {
            let btn_name = format!("hist_{}", i);
            let live_id = LiveId::from_str(&btn_name);
            let child = history_list.widget(cx, &[live_id]);
            if child.widget_uid() == uid && uid != WidgetUid(0) {
                // display_idx is the reversed index; convert back to history index
                return Some(display_count - 1 - display_idx);
            }
        }
        None
    }

    /// Load saved apps from disk on startup.
    fn load_history(&mut self) {
        let dir = Self::apps_dir();
        if !dir.is_dir() {
            return;
        }

        let mut entries: Vec<_> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(_) => return,
        };
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("splash") {
                continue;
            }
            if let Ok(code) = std::fs::read_to_string(&path) {
                self.history_counter += 1;
                let name = extract_app_title(&code)
                    .unwrap_or_else(|| format!("App #{}", self.history_counter));
                self.history.push((name, code));
            }
        }
    }
}

/// Extract a title from Splash code by finding the first Label with font_size >= 16.
/// Skips Labels that look like data values (pure numbers, time formats like "25:00").
fn extract_app_title(code: &str) -> Option<String> {
    // Try extracting from Markdown body: "# Title" heading
    if let Some(title) = extract_markdown_title(code) {
        return Some(title);
    }

    // Try extracting from Label widgets with large font size
    for line in code.lines() {
        let trimmed = line.trim();
        if !trimmed.contains("Label") {
            continue;
        }
        // Check for font_size >= 12 (skip tiny labels like font_size: 9 or 10)
        let has_title_font = trimmed.contains("font_size: 12")
            || trimmed.contains("font_size: 13")
            || trimmed.contains("font_size: 14")
            || trimmed.contains("font_size: 15")
            || trimmed.contains("font_size: 16")
            || trimmed.contains("font_size: 17")
            || trimmed.contains("font_size: 18")
            || trimmed.contains("font_size: 19")
            || trimmed.contains("font_size: 2")
            || trimmed.contains("font_size: 3")
            || trimmed.contains("font_size: 4")
            || trimmed.contains("font_size: 5")
            || trimmed.contains("font_size: 6");
        if !has_title_font {
            continue;
        }
        // Extract text: "..."
        if let Some(start) = trimmed.find("text: \"") {
            let after = &trimmed[start + 7..];
            if let Some(end) = after.find('"') {
                let title = &after[..end];
                // Skip values that look like data (numbers, time, etc.)
                if title.is_empty() {
                    continue;
                }
                let is_data = title.chars().all(|c| {
                    c.is_ascii_digit()
                        || c == ':'
                        || c == '.'
                        || c == '%'
                        || c == '$'
                        || c == ','
                        || c == ' '
                });
                if is_data {
                    continue;
                }
                return Some(title.to_string());
            }
        }
    }
    None
}

/// Extract title from Markdown body content (first # heading).
fn extract_markdown_title(code: &str) -> Option<String> {
    // Look for body: "..." containing a markdown heading
    let body_start = code.find("body: \"")?;
    let after = &code[body_start + 7..];
    let body_end = after.find('"')?;
    let body = &after[..body_end];

    // Unescape \n and find first # heading
    for segment in body.split("\\n") {
        let trimmed = segment.trim();
        if trimmed.starts_with("# ") {
            let title = trimmed[2..].trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

/// Create a simple slug from a name for filenames.
fn slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_code_editor::script_mod(vm);
        crate::visualizer::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        // Build UID map after draw when widget tree is ready
        if matches!(event, Event::Draw(_)) && self.uid_map_dirty && !self.widget_names.is_empty() {
            let splash = self.ui.widget(cx, ids!(splash_panel));
            self.uid_map.clear();
            for name in &self.widget_names {
                let live_id = LiveId::from_str(name);
                let child = splash.widget(cx, &[live_id]);
                let uid = child.widget_uid();
                if uid != WidgetUid(0) {
                    self.uid_map.insert(uid, name.clone());
                }
            }
            self.uid_map_dirty = false;
        }

        // Hide traffic light buttons after window is created (first draw)
        if matches!(event, Event::Draw(_)) && !self.buttons_hidden {
            self.buttons_hidden = true;
            cx.push_unique_platform_op(CxOsOp::HideWindowButtons(WindowId(0, 0)));
        }
    }
}
