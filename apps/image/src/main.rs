//! image: a dumb picture viewer. `image <path>` opens fit-to-window,
//! centered on a dark makepad_wm_theme background. `image --preview <path>` sizes
//! the window as a small popup instead.

pub use makepad_widgets;
use makepad_widgets::*;
use makepad_image::preview::{PreviewAction, PreviewState};
use makepad_image::widget::{MpImageAction, MpImageView};
use std::path::PathBuf;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1200, 800)
                window.title: "image"
                pass +: { clear_color: theme.color_bg_app }
                body +: {
                    root := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 0
                        padding: 0

                        viewer := MpImageView{
                            width: Fill
                            height: Fill
                        }

                        status_row := RectView{
                            width: Fill
                            height: 22
                            align: Align{y: 0.5}
                            padding: Inset{left: 10 right: 10 top: 0 bottom: 0}
                            draw_bg +: { color: theme.color_bg_container }

                            status := Label{
                                width: Fill
                                max_lines: 1
                                text_overflow: TextOverflow.Ellipsis
                                text: ""
                                draw_text +: {
                                    color: theme.color_text_meta
                                    text_style: theme.font_regular{font_size: 9}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    /// The Quick Look v2 warm-viewer state: whether this run is a preview
    /// panel, and what it currently shows. See `preview.rs`.
    #[rust]
    preview: PreviewState,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let mut preview = false;
        let mut path: Option<PathBuf> = None;
        for arg in std::env::args().skip(1) {
            if arg == "--preview" {
                preview = true;
            } else if arg.starts_with("--") {
                // --remote, --stdin-loop, or any other flag every Makepad app
                // already understands elsewhere: not ours to parse, ignore.
            } else if path.is_none() {
                path = Some(PathBuf::from(arg));
            }
        }

        // A theme.splash background (makepad_wm_theme::apply already retinted
        // theme.color_bg_app from it, if MAKEPAD_WM_THEME_SPLASH is set); patch the
        // pass clear color explicitly too so the dark letterbox is right even
        // if the stock theme default ever drifts.
        if let Some(bg_hex) = makepad_wm_theme::current().map(|p| p.hex("background", "#1a1b26")) {
            if let Some(color) = parse_hex_color(&bg_hex) {
                let mut window = self.ui.window(cx, ids!(main_window));
                script_apply_eval!(cx, window, {
                    pass +: { clear_color: #(color) }
                });
            }
        }

        self.preview = PreviewState::new(preview, path.as_deref());

        if let Some(mut viewer) = self.ui.widget(cx, ids!(viewer)).borrow_mut::<MpImageView>() {
            viewer.set_preview(preview);
            if let Some(path) = &path {
                viewer.open(cx, path);
            }
        }

        self.set_title(cx);
        // A --stdin-loop child is tiled by the compositor, which owns the
        // window size; a standalone --preview run gets the popup size.
        if preview && std::env::var("STUDIO_HOST").is_err() {
            self.ui
                .window(cx, ids!(main_window))
                .resize(cx, dvec2(900.0, 700.0));
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            if let MpImageAction::Status { name, width, height, zoom_pct } = widget_action.cast() {
                let text = if name.is_empty() {
                    // An unloaded warm viewer has nothing to say.
                    String::new()
                } else if width > 0 && height > 0 {
                    format!("{}    {}\u{00d7}{}    {}%", name, width, height, zoom_pct)
                } else {
                    format!("{}    loading\u{2026}", name)
                };
                self.ui.label(cx, ids!(status)).set_text(cx, &text);
            }
        }
    }
}

impl App {
    /// Quick Look v2: `StudioToApp::Custom` from wm reaches a hosted app
    /// as `Event::Custom(json)`; this is what the viewer half of the
    /// protocol does with it. `PreviewUnload` never ends the process.
    fn handle_wm_event(&mut self, cx: &mut Cx, event: &makepad_wm_api::WmEvent) {
        match self.preview.on_wm_event(event) {
            PreviewAction::Show(path) => {
                if let Some(mut viewer) =
                    self.ui.widget(cx, ids!(viewer)).borrow_mut::<MpImageView>()
                {
                    viewer.open(cx, &path);
                }
                self.set_title(cx);
            }
            PreviewAction::Unload => {
                if let Some(mut viewer) =
                    self.ui.widget(cx, ids!(viewer)).borrow_mut::<MpImageView>()
                {
                    viewer.unload(cx);
                }
                self.set_title(cx);
            }
            PreviewAction::Close => cx.quit(),
            PreviewAction::Ignore => {}
        }
    }

    /// The window's own caption, and the title the WM shows for the tile.
    fn set_title(&mut self, cx: &mut Cx) {
        let title = self.preview.title("image");
        self.ui.window(cx, ids!(main_window)).set_title(cx, &title);
        makepad_wm_api::set_title(cx, &title);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        makepad_image::widget::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Custom(json) = event {
            if let Some(wm) = makepad_wm_api::WmEvent::parse(json) {
                self.handle_wm_event(cx, &wm);
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

/// Parses a `#rrggbb` (or `#rgb`) hex string into a `Vec4f`, alpha 1.0.
fn parse_hex_color(value: &str) -> Option<Vec4f> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    let rgba = match digits.len() {
        3 => {
            let mut expanded = String::with_capacity(8);
            for c in digits.chars() {
                expanded.push(c);
                expanded.push(c);
            }
            expanded.push_str("ff");
            u32::from_str_radix(&expanded, 16).ok()?
        }
        6 => u32::from_str_radix(digits, 16).ok()?.checked_shl(8)? | 0xff,
        8 => u32::from_str_radix(digits, 16).ok()?,
        _ => return None,
    };
    Some(Vec4f::from_u32(rgba))
}
