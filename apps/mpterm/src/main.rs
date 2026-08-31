//! mpterm: a full-window terminal app. Runs standalone or inside
//! makepad-wm / Studio tiles (the shared --stdin-loop client runtime).
//!
//! Command line:
//!   --cwd <dir>        open the shell in <dir> (mpwm passes the focused
//!                      terminal's directory: Omarchy's terminal-in-cwd)
//!   --preview <path>   Quick Look mode: page the file (or list the
//!                      directory) and quit when the pager exits — mpfiles'
//!                      Space bar for text, source and everything else no
//!                      viewer app claims.

pub use makepad_widgets;
use makepad_widgets::*;
use mpterm::widget::{MpTerm, MpTermAction};
use std::path::{Path, PathBuf};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(980, 640)
                window.title: "mpterm"
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
                // The pager quit: the preview popup goes with it.
                MpTermAction::Exited if self.preview => cx.quit(),
                _ => {}
            }
        }
    }
}

impl App {
    /// Quick Look v2, viewer half: mpwm retargets a warm text preview with
    /// `PreviewFile` (restart the pager in place — no respawn, no new
    /// window) and parks it with `PreviewUnload`. `PreviewUnload` never
    /// ends the process.
    fn handle_wm_event(&mut self, cx: &mut Cx, event: &mp_wm_api::WmEvent) {
        use mp_wm_api::WmEvent;
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
                mp_wm_api::set_title(cx, &title);
            }
            WmEvent::PreviewUnload => {
                if let Some(mut term) = self.ui.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
                    term.unload(cx);
                }
            }
            WmEvent::CloseRequested => cx.quit(),
            _ => {}
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        mp_theme::apply(vm);
        mpterm::widget::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Custom(json) = event {
            if let Some(wm) = mp_wm_api::WmEvent::parse(json) {
                self.handle_wm_event(cx, &wm);
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
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
