//! code — the window. Layout lives in `ui` and `ide`; the agent core is
//! the makepad-code-agent crate.
pub use makepad_widgets;
use makepad_widgets::*;

mod ide;
mod pictures;
mod ui;

use makepad_code_agent::event::{AgentConfig, ApprovalMode};
use std::path::PathBuf;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Makepad Code"
                window.inner_size: vec2(1440, 900)
                pass +: {clear_color: theme.color_bg_app}
                body +: {
                    padding: 0 margin: 0 spacing: 0
                    ide := IdeShell{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

/// `makepad-app-code [FOLDER] [--approve ask|allowlist|auto] [--context N]`:
/// the folder defaults to the current directory.
fn config_from_args() -> AgentConfig {
    if makepad_code_agent::mock::presentation() {
        return AgentConfig {
            root: PathBuf::from("demo"),
            context_tokens: 16384,
            model: None,
            approval: ApprovalMode::Allowlist,
            session_dir: None,
        };
    }
    let mut root: Option<PathBuf> = None;
    let mut approval = ApprovalMode::Allowlist;
    let mut context_tokens = 16384u32;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--approve" => {
                approval = match args.next().as_deref() {
                    Some("ask") => ApprovalMode::Ask,
                    Some("auto") => ApprovalMode::Auto,
                    _ => ApprovalMode::Allowlist,
                }
            }
            "--context" => context_tokens = args.next().and_then(|v| v.parse().ok()).unwrap_or(context_tokens),
            other if !other.starts_with("--") => root = Some(other.into()),
            _ => {}
        }
    }
    let root = root.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
    let session_dir = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".makepad/code/sessions"));
    AgentConfig {
        root: root.canonicalize().unwrap_or(root),
        context_tokens,
        model: None,
        approval,
        session_dir,
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        pictures::install();
        // The chat tab starts the agent from the workspace when it is first
        // drawn; the panels read the root and the file index from it.
        ide::Workspace::install(cx, config_from_args());
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_app_terminal::widget::script_mod(vm);
        ui::script_mod(vm);
        ide::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // Apps started from the window do not outlive it.
        if let Event::Shutdown = event {
            makepad_code_agent::runs::RunManager::global().shutdown();
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
