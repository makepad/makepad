//! terminal: a full-window terminal app. Runs standalone or inside
//! makepad-wm / Studio tiles (the shared --stdin-loop client runtime).
//!
//! Command line:
//!   --cwd <dir>        open the shell in <dir> (wm passes the focused
//!                      terminal's directory: Omarchy's terminal-in-cwd)
//!   --preview <path>   Quick Look mode: page the file (or list the
//!                      directory) and quit when the pager exits — files'
//!                      Space bar for text, source and everything else no
//!                      viewer app claims.

pub use makepad_widgets;
use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_widgets::*;
use makepad_terminal::widget::{MpTerm, MpTermAction};
use std::path::{Path, PathBuf};

mod ai;

app_main!(
    App,
    font_assets: [
        "makepad_widgets/resources/jetbrains_mono_variable.ttf",
        "makepad_widgets/resources/fa-solid-900.ttf",
    ]
);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(980, 640)
                window.title: "terminal"
                // Transparent clear: the terminal paints its own background
                // at the compositor-given opacity into the shared swapchain.
                pass +: { clear_color: vec4(0.0, 0.0, 0.0, 0.0) }
                body +: {
                    term := MpTerm{}
                }
            }
        }
    }
}

/// What the command line asked for.
#[derive(Default, Debug, PartialEq)]
struct Args {
    cwd: Option<PathBuf>,
    preview: Option<PathBuf>,
}

fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Args {
    let mut out = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cwd" => out.cwd = args.next().map(PathBuf::from),
            "--preview" => out.preview = args.next().map(PathBuf::from),
            _ => {}
        }
    }
    out
}

/// Single-quote a path for the POSIX shell.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The one-shot job a preview runs: a pager over the file, or a listing
/// of the directory, staying up until the user quits it (q / Escape).
fn preview_command(path: &Path) -> String {
    let quoted = shell_quote(&path.to_string_lossy());
    if cfg!(windows) {
        return format!("more {}", quoted);
    }
    if path.is_dir() {
        format!("ls -la -- {} | less -R", quoted)
    } else {
        format!("less -R -- {}", quoted)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    preview: bool,
    #[rust]
    ai_port: Option<AiServicePort>,
    #[rust]
    ai_context: String,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let args = parse_args(std::env::args().skip(1));
        if let Some(mut term) = self.ui.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            term.cwd = args.cwd.clone();
            if let Some(path) = &args.preview {
                term.command = Some(preview_command(path));
                if term.cwd.is_none() {
                    term.cwd = path.parent().map(Path::to_path_buf);
                }
            }
        }
        if let Some(path) = &args.preview {
            self.preview = true;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            self.ui
                .window(cx, ids!(main_window))
                .set_title(cx, &format!("{} \u{2014} preview", name));
        }
        // A warm-pool standby is not a running Terminal: its service opens
        // when the window manager adopts it into a tile (`WmEvent::Adopted`),
        // never while it is dormant — the assistant must not drive a shell
        // nobody can see.
        if !makepad_wm_api::warm_start() {
            self.open_ai_port(cx);
        }
    }


    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            // The widget emits through cx.widget_action, which wraps the
            // payload: unwrap the WidgetAction before matching.
            let Some(wa) = action.as_widget_action() else {
                continue;
            };
            match wa.cast::<MpTermAction>() {
                MpTermAction::TitleChanged(title) if !self.preview => {
                    self.ui.window(cx, ids!(main_window)).set_title(cx, &title);
                }
                MpTermAction::PwdChanged(_) => self.refresh_ai_context(cx),
                // The pager quit: the preview popup goes with it.
                MpTermAction::Exited if self.preview => cx.quit(),
                _ => {}
            }
        }
    }
}

impl App {
    fn refresh_ai_context(&mut self, cx: &mut Cx) {
        let cwd = self
            .ui
            .widget(cx, ids!(term))
            .borrow::<MpTerm>()
            .and_then(|term| term.cwd.clone());
        let Some(cwd) = cwd else {
            return;
        };
        let shown = std::env::var_os("HOME")
            .and_then(|home| cwd.strip_prefix(PathBuf::from(home)).ok().map(Path::to_path_buf))
            .map(|rest| {
                if rest.as_os_str().is_empty() {
                    "~".to_string()
                } else {
                    format!("~/{}", rest.display())
                }
            })
            .unwrap_or_else(|| cwd.display().to_string());
        let context = format!("shell in {shown}");
        if context == self.ai_context {
            return;
        }
        self.ai_context = context.clone();
        if let Some(port) = self.ai_port.as_ref() {
            port.set_context(&context);
        }
    }

    fn drain_ai_port(&mut self, cx: &mut Cx, event: &Event) {
        let events = match self.ai_port.as_mut() {
            Some(port) => port.handle_event(cx, event),
            None => return,
        };
        for event in events {
            match event {
                PortEvent::Registered(endpoint) => {
                    log!("terminal: AI service registered as {}", endpoint.as_str());
                    self.ai_context.clear();
                    self.refresh_ai_context(cx);
                }
                PortEvent::Call(call) => {
                    let result = self
                        .ui
                        .widget(cx, ids!(term))
                        .borrow_mut::<MpTerm>()
                        .map(|mut term| ai::answer(&call, &mut *term))
                        .unwrap_or_else(|| {
                            makepad_ai_services::wire::ToolResult::unavailable(
                                &call.call_id,
                                "the terminal widget is not ready",
                            )
                        });
                    if let Some(port) = self.ai_port.as_ref() {
                        port.reply(result);
                    }
                }
                PortEvent::Cancel { .. } | PortEvent::ChatOpen { .. } => {}
                PortEvent::Subscribe { .. } | PortEvent::Unsubscribe { .. } => {}
            }
        }
    }

    /// Quick Look v2, viewer half: wm retargets a warm text preview with
    /// `PreviewFile` (restart the pager in place — no respawn, no new
    /// window) and parks it with `PreviewUnload`. `PreviewUnload` never
    /// ends the process.
    fn handle_wm_event(&mut self, cx: &mut Cx, event: &makepad_wm_api::WmEvent) {
        use makepad_wm_api::WmEvent;
        match event {
            WmEvent::PreviewFile { path } => {
                let path = PathBuf::from(path);
                self.preview = true;
                if let Some(mut term) = self.ui.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
                    term.restart_with(
                        cx,
                        path.parent().map(Path::to_path_buf),
                        Some(preview_command(&path)),
                    );
                }
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                let title = format!("{} \u{2014} preview", name);
                self.ui
                    .window(cx, ids!(main_window))
                    .set_title(cx, &title);
                makepad_wm_api::set_title(cx, &title);
            }
            WmEvent::PreviewUnload => {
                if let Some(mut term) = self.ui.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
                    term.unload(cx);
                }
            }
            WmEvent::CloseRequested => cx.quit(),
            // Adopted into a real tile: now it is a running Terminal.
            WmEvent::Adopted => self.open_ai_port(cx),
            _ => {}
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        makepad_terminal::widget::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Custom(json) = event {
            if let Some(wm) = makepad_wm_api::WmEvent::parse(json) {
                self.handle_wm_event(cx, &wm);
            }
        }
        self.drain_ai_port(cx, event);
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

impl ai::TerminalTarget for MpTerm {
    fn visible_screen(&self) -> Option<ai::ScreenState> {
        let (rows, cursor_row, cursor_col) = self.ai_screen_rows(None)?;
        Some(ai::ScreenState {
            rows,
            cursor_row,
            cursor_col,
            cwd: self.cwd.as_ref().map(|path| path.display().to_string()),
        })
    }

    fn recent_screen(&self, lines: usize) -> Option<ai::ScreenState> {
        let (rows, cursor_row, cursor_col) = self.ai_screen_rows(Some(lines))?;
        Some(ai::ScreenState {
            rows,
            cursor_row,
            cursor_col,
            cwd: self.cwd.as_ref().map(|path| path.display().to_string()),
        })
    }

    fn type_bytes(&mut self, bytes: &[u8]) -> bool {
        self.ai_type_bytes(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_parse() {
        let a = parse_args(["--cwd", "/tmp", "--preview", "/tmp/a b.txt"].map(String::from));
        assert_eq!(a.cwd, Some(PathBuf::from("/tmp")));
        assert_eq!(a.preview, Some(PathBuf::from("/tmp/a b.txt")));
        assert_eq!(parse_args(Vec::<String>::new()), Args::default());
    }

    #[test]
    fn preview_quotes_the_path() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        let cmd = preview_command(Path::new("/no/such/file's.txt"));
        assert_eq!(cmd, "less -R -- '/no/such/file'\\''s.txt'");
    }
}

impl App {
    /// The service toward the assistant: at startup for a real launch, on
    /// adoption for a warm-pool standby, never twice.
    fn open_ai_port(&mut self, cx: &mut Cx) {
        if self.ai_port.is_some() {
            return;
        }
        self.ai_port = AiServicePort::open(cx, ai::manifest());
        self.refresh_ai_context(cx);
    }
}
