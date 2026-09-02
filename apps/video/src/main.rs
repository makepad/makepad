//! video: a dumb video player. `video <path>` opens one clip
//! fit-to-window on a dark makepad_wm_theme background and plays it.
//! `video --preview <path>` sizes the window as a small popup instead —
//! that is files' Quick Look.

pub use makepad_widgets;
use makepad_widgets::*;
use makepad_video::preview::{PreviewAction, PreviewState};
use makepad_video::theme::Palette;
use makepad_video::widget::{format_time, MpVideoAction, MpVideoView};
use std::path::PathBuf;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1100, 700)
                window.title: "video"
                pass +: { clear_color: mod.mpv.bg }
                body +: {
                    root := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 0
                        padding: 0

                        player := MpVideoView{
                            width: Fill
                            height: Fill
                        }

                        status_row := RectView{
                            width: Fill
                            height: 22
                            align: Align{y: 0.5}
                            padding: Inset{left: 10 right: 10 top: 0 bottom: 0}
                            // The window background, not the transport
                            // bar's fill: the two strips sit next to each
                            // other and must not read as one.
                            draw_bg +: { color: mod.mpv.bg }

                            status := Label{
                                width: Fill
                                max_lines: 1
                                text_overflow: TextOverflow.Ellipsis
                                text: ""
                                draw_text +: {
                                    color: mod.mpv.fg
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
        let argv: Vec<String> = std::env::args().skip(1).collect();
        // Silent start for test/agent runs — see `makepad_video::wants_mute`.
        let muted = makepad_video::wants_mute(&argv, |key| std::env::var(key).ok());
        let mut preview = false;
        let mut path: Option<PathBuf> = None;
        for arg in argv.iter().cloned() {
            if arg == "--preview" {
                preview = true;
            } else if arg.starts_with("--") {
                // --mute is read above; --remote, --stdin-loop and any other
                // flag every Makepad app already understands elsewhere are not
                // ours to parse, so ignore them here.
            } else if path.is_none() {
                path = Some(PathBuf::from(arg));
            }
        }

        // Speakers: the clip's soundtrack, mixed from the decode queue.
        cx.audio_output(0, move |info, output| {
            output.zero();
            makepad_video::player::mix_into(output, info.sample_rate);
        });

        // makepad_wm_theme::apply already retinted theme.color_bg_app and
        // mod.widgets.Window.pass.clear_color from a theme.splash when one
        // is exported; patch the pass clear color explicitly too so the
        // letterbox around the picture is right either way.
        {
            let color = Palette::shared().bg_vec4();
            let mut window = self.ui.window(cx, ids!(main_window));
            script_apply_eval!(cx, window, {
                pass +: { clear_color: #(color) }
            });
        }

        self.preview = PreviewState::new(preview, path.as_deref());

        if let Some(mut player) = self.ui.widget(cx, ids!(player)).borrow_mut::<MpVideoView>() {
            player.set_muted(muted);
            player.set_preview(preview);
            if let Some(path) = &path {
                player.open(cx, path);
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

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        cx.use_audio_outputs(&devices.default_output());
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            // A raw downcast never matches a widget action: unwrap the
            // WidgetAction envelope first, then cast.
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            if let MpVideoAction::Status {
                name,
                width,
                height,
                fps,
                position_secs,
                duration_secs,
                playing,
                ended,
                volume_pct,
                muted,
                bare,
                error,
            } = widget_action.cast()
            {
                let text = if let Some(error) = error {
                    format!("{name}    cannot play \u{2014} {error}")
                } else if name.is_empty() {
                    // An unloaded warm viewer has nothing to say.
                    String::new()
                } else if width == 0 {
                    format!("{name}    opening\u{2026}")
                } else {
                    let state = if ended {
                        "ended"
                    } else if playing {
                        "playing"
                    } else {
                        "paused"
                    };
                    let sound = if muted {
                        "muted".to_string()
                    } else {
                        format!("vol {volume_pct}%")
                    };
                    format!(
                        "{name}    {width}\u{00d7}{height}    {fps:.0} fps    {state}    {} / {}    {sound}",
                        format_time(position_secs),
                        format_time(duration_secs),
                    )
                };
                self.ui.label(cx, ids!(status)).set_text(cx, &text);
                // Double-clicked into bare mode: the picture is the whole
                // window, status line included.
                self.ui.widget(cx, ids!(status_row)).set_visible(cx, !bare);
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
                if let Some(mut player) =
                    self.ui.widget(cx, ids!(player)).borrow_mut::<MpVideoView>()
                {
                    player.open(cx, &path);
                }
                self.set_title(cx);
            }
            PreviewAction::Unload => {
                if let Some(mut player) =
                    self.ui.widget(cx, ids!(player)).borrow_mut::<MpVideoView>()
                {
                    player.unload(cx);
                }
                self.set_title(cx);
            }
            PreviewAction::Close => cx.quit(),
            PreviewAction::Ignore => {}
        }
    }

    /// The window's own caption, and the title the WM shows for the tile.
    fn set_title(&mut self, cx: &mut Cx) {
        let title = self.preview.title("video");
        self.ui.window(cx, ids!(main_window)).set_title(cx, &title);
        makepad_wm_api::set_title(cx, &title);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        Palette::shared().publish(vm);
        makepad_video::widget::script_mod(vm);
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
