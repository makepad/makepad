//! One typed action boundary for Studio's UI and the standard app AI bus.
use crate::workspace::Mode;
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef};
use makepad_strict_json::{self as json, Value};
use makepad_widgets::dock::DropPart;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Status,
    Flows,
    Lane {
        flow: String,
        state: crate::iteration::FlowLifecycle,
    },
    SplitLane { flow: String, item: String, title: String },
    RecoverLane {
        flow: String,
    },
    Iteration(crate::iteration_worker::Request),
    NewTerminal {
        cwd: Option<PathBuf>,
    },
    SelectTab(u64),
    CloseTab(u64),
    StopAgent(u64),
    DockTab {
        tab: u64,
        target: u64,
        position: DropPart,
    },
    RefreshProject,
    RevealProject(PathBuf),
    Settings,
    Activity,
    Disk,
    Usage,
    RefreshUsage,
    InspectUsage,
    Appearance {
        style: Option<String>,
        dark: bool,
    },
    ReadTerminal {
        tab: u64,
        lines: Option<usize>,
    },
    Input {
        tab: u64,
        text: String,
    },
    Run {
        tab: u64,
        command: String,
    },
    RefreshDisk,
    InspectDisk,
    CleanupPreview(PathBuf),
    Layout(String),
    Mode(Mode),
    OpenCode(PathBuf),
    ReadCode {
        tab: u64,
    },
    SaveCode {
        tab: u64,
    },
    ReloadCode {
        tab: u64,
    },
    AddDesign {
        title: String,
        detail: String,
        parent: Option<u64>,
        path: Option<PathBuf>,
    },
    Atlas(crate::atlas::tools::AtlasAction),
    Code(crate::atlas::tools::CodeCall),
}

pub fn manifest() -> ServiceManifest {
    let mut m = ServiceManifest::new("studio", "Studio", "AI work environment with Structured tabs, a tasks view of agent lanes with their terminals, and an Architecture map, sharing the same live terminals and code editors. Read status first for current tab hex IDs and state. File changes and process observations do not by themselves identify an AI owner or prove a test passed. All terminal input goes to the live PTY, with the same consequences as typing. Disk inventory and cleanup previews never delete files.");
    for (name, description, props, required, risk) in [
        ("status", "Current tabs (hex IDs), selections, the workspace mode, observed activity, appearance, and disk summary.", "", "", Risk::Read),
        ("open_flows", "Open the tasks view: agent lanes with their terminals, vertically stacked requirements, build checkpoints and feedback, with local/work/dev source controls. Normal wheel scrolls a lane; modifier-wheel zooms.", "", "", Risk::Act),
        ("flow_lane", "Manage a lane: active, stopped, archived, recover, split or clear_history. Split needs item/title and moves newer history with the same terminal; close apps/finish builds first. Clear history keeps terminal, current tasks, apps, files and checkpoints. Both require user authorization. Recovery saves the exact conversation before Fable /login or Codex logout/login/resume; Codex changes its shared account. Inspect status.flow_terminals for completion.", r#""flow":{"type":"string","maxLength":96},"state":{"type":"string","enum":["active","stopped","archived","recover","split","clear_history"]},"item":{"type":"string","maxLength":256},"title":{"type":"string","maxLength":240}"#, "flow,state", Risk::Destructive),
        ("new_terminal", "Open and select a new live terminal; optional absolute cwd. Returns its tab ID.", r#""cwd":{"type":"string"}"#, "", Risk::Act),
        ("select_tab", "Select an existing tab by its ID from status.", r#""tab":{"type":"string"}"#, "tab", Risk::Act),
        ("close_tab", "Close a presentation tab. Persistent agent terminals detach; their agents keep running. Use stop_agent only when explicitly asked to stop that agent.", r#""tab":{"type":"string"}"#, "tab", Risk::Act),
        ("stop_agent", "Explicitly stop the persistent agent session attached to this terminal tab. Closing Studio or a tab does not authorize stopping an agent.", r#""tab":{"type":"string"}"#, "tab", Risk::Destructive),
        ("dock_tab", "Move a live tab in the Structured Dock: split beside a target tab (left/right/top/bottom), join its group (center), or insert before it (tab). Preserves terminal/editor state and Canvas layout.", r#""tab":{"type":"string"},"target":{"type":"string"},"position":{"type":"string","enum":["left","right","top","bottom","center","tab"]}"#, "tab,target,position", Risk::Act),
        ("refresh_project_tree", "Refresh the project file tree in the background.", "", "", Risk::Act),
        ("reveal_project_file", "Expand the project tree to an absolute path inside the current project and select it, without opening another editor.", r#""path":{"type":"string","maxLength":4096}"#, "path", Risk::Act),
        ("open_activity", "Open the shared activity view with observed processes, source changes and recent Studio actions.", "", "", Risk::Act),
        ("open_settings", "Open Studio Settings.", "", "", Risk::Act),
        ("open_disk", "Switch to the Disk workspace mode: the CWD volume map filling the centre.", "", "", Risk::Act),
        ("open_usage", "Open provider usage limits and reset times from background CLI queries.", "", "", Risk::Act),
        ("refresh_usage", "Queue a coalesced background provider usage refresh. No model conversation is started.", "", "", Risk::Act),
        ("inspect_usage", "Read provider quota windows, reset times, signed-in account emails when available, query errors and freshness. Missing limits and identities remain unavailable.", "", "", Risk::Read),
        ("set_appearance", "Set standalone style (host, omarchy, macos, windows, windows-2000, nextstep, ios, android). Host follows native light/dark appearance where available; dark stores the manual preference for explicit styles. Hosted style is controlled by the window manager.", r#""style":{"type":"string"},"dark":{"type":"boolean"}"#, "style,dark", Risk::Act),
        ("read_terminal", "Read a live terminal's visible screen, or its last 1–2000 lines including scrollback.", r#""tab":{"type":"string"},"lines":{"type":"integer","minimum":1,"maximum":2000}"#, "tab", Risk::Read),
        ("type_terminal", "Type text or terminal control bytes into the live PTY (maximum 4096 bytes). No Enter is appended.", r#""tab":{"type":"string"},"text":{"type":"string","maxLength":4096}"#, "tab,text", Risk::Act),
        ("run", "Type a shell command followed by Enter into an existing live terminal. Executes for real; returns when input is accepted, not when the command finishes. Read the terminal afterward.", r#""tab":{"type":"string"},"command":{"type":"string","maxLength":4096}"#, "tab,command", Risk::Act),
        ("refresh_disk", "Queue a coalesced background disk/workspace scan. Read inspect_disk for completion and partial measurements.", "", "", Risk::Act),
        ("inspect_disk", "Read volume usage, recent history and workspace/build sizes, freshness, coverage and cleanup constraints.", "", "", Risk::Read),
        ("cleanup_preview", "Preview an exact inventory path, its measured bytes and why removal is blocked. No files are deleted. Active-use ownership must be tracked before automatic cleanup is offered.", r#""path":{"type":"string"}"#, "path", Risk::Read),
        ("set_layout", "Rearrange the current tabs by replacing the RON layout returned by status. All existing tab IDs and kinds must remain; only containers, ordering, selections and splitter positions may change.", r#""layout":{"type":"string","maxLength":131072}"#, "layout", Risk::Act),
        ("set_mode", "Switch between Structured tabs, the tasks view, the Architecture map and the Disk map. All four share the same live terminals and code editors; Architecture shows the indexed code graph with an Inspector; Disk shows the CWD volume map.", r#""mode":{"type":"string","enum":["structured","tasks","architecture","disk"]}"#, "mode", Risk::Act),
        ("open_code", "Open an absolute file path in a real code editor, reusing its existing live document when already open. Disk updates are observed without inventing AI edit attribution.", r#""path":{"type":"string","maxLength":4096}"#, "path", Risk::Act),
        ("read_code", "Read an open editor's current text and revision/conflict state using its tab ID from status.", r#""tab":{"type":"string"}"#, "tab", Risk::Read),
        ("save_code", "Save an open code editor to its file. Resolve any conflicting external change before saving.", r#""tab":{"type":"string"}"#, "tab", Risk::Act),
        ("reload_code", "Discard this editor's unsaved changes and reload its file from disk. This explicitly replaces the current buffer.", r#""tab":{"type":"string"}"#, "tab", Risk::Destructive),
        ("add_design", "Add a system design card with a title and explanation. Optionally link a known parent card and an absolute code path for drill-down. The relationship describes the supplied design; it is not inferred agent ownership.", r#""title":{"type":"string","minLength":1,"maxLength":120},"detail":{"type":"string","maxLength":8192},"parent":{"type":"string"},"path":{"type":"string","maxLength":4096}"#, "title,detail", Risk::Act),
    ] {
        let required = required.split(',').filter(|s| !s.is_empty()).map(json::s).collect();
        let schema = format!(r#"{{"type":"object","properties":{{{props}}},"required":{},"additionalProperties":false}}"#, Value::Arr(required).to_json());
        m = m.with_tool(ToolDef::new(name, description, &schema, risk));
    }
    for tool in crate::iteration_tools::tool_defs() {
        m = m.with_tool(tool);
    }
    for tool in crate::atlas::tools::tool_defs() {
        m = m.with_tool(tool);
    }
    m
}

pub fn parse(call: &ServiceCall) -> Result<Action, String> {
    if crate::iteration_tools::handles(&call.tool) {
        return crate::iteration_tools::parse(call).map(Action::Iteration);
    }
    if crate::atlas::tools::handles(&call.tool) {
        return crate::atlas::tools::parse(call).map(|parsed| match parsed {
            crate::atlas::tools::Parsed::Atlas(a) => Action::Atlas(a),
            crate::atlas::tools::Parsed::Code(c) => Action::Code(c),
        });
    }
    if call.args.len() > 140_000 {
        return Err("tool arguments exceed the size limit".into());
    }
    let args = json::parse(call.args.as_bytes()).map_err(str::to_owned)?;
    let Value::Obj(fields) = &args else {
        return Err("arguments must be an object".into());
    };
    let allowed: &[&str] = match call.tool.as_str() {
        "status"
        | "open_flows"
        | "open_settings"
        | "open_activity"
        | "open_disk"
        | "refresh_disk"
        | "inspect_disk"
        | "open_usage"
        | "refresh_usage"
        | "inspect_usage"
        | "refresh_project_tree" => &[],
        "new_terminal" => &["cwd"],
        "flow_lane" => &["flow", "state", "item", "title"],
        "select_tab" | "close_tab" | "stop_agent" | "read_code" | "save_code" | "reload_code" => {
            &["tab"]
        }
        "dock_tab" => &["tab", "target", "position"],
        "read_terminal" => &["tab", "lines"],
        "type_terminal" => &["tab", "text"],
        "run" => &["tab", "command"],
        "set_appearance" => &["style", "dark"],
        "cleanup_preview" | "open_code" | "reveal_project_file" => &["path"],
        "set_layout" => &["layout"],
        "set_mode" => &["mode"],
        "add_design" => &["title", "detail", "parent", "path"],
        _ => return Err(format!("unknown Studio tool {}", call.tool)),
    };
    if let Some((key, _)) = fields
        .iter()
        .find(|(key, _)| !allowed.contains(&key.as_str()))
    {
        return Err(format!("unknown argument {key}"));
    }
    let string = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{key} must be a string"))
    };
    let hex_id = |key: &str| -> Result<u64, String> {
        let id = string(key)?;
        if id.is_empty() || id.len() > 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("{key} must be a hex ID from status"));
        }
        u64::from_str_radix(id, 16).map_err(|_| format!("{key} must be a hex ID from status"))
    };
    let tab = || hex_id("tab");
    let absolute = |key: &str| -> Result<PathBuf, String> {
        let text = string(key)?;
        let p = PathBuf::from(text);
        if !p.is_absolute() || text.len() > 4096 || text.contains('\0') {
            return Err(format!(
                "{key} must be an absolute path of at most 4096 bytes"
            ));
        }
        Ok(p)
    };
    Ok(match call.tool.as_str() {
        "status" => Action::Status,
        "open_flows" => Action::Flows,
        "flow_lane" => {
            let flow = string("flow")?;
            if flow.is_empty()
                || flow.len() > 96
                || !flow
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err("Invalid flow ID".into());
            }
            match string("state")? {
                "clear_history" => Action::Iteration(crate::iteration_worker::parse_clear_history_request(&args)?),
                "split" => match crate::iteration_worker::parse_split_request(&args)? {
                    crate::iteration_worker::Request::SplitLane { flow, item, title } => Action::SplitLane { flow, item, title },
                    _ => return Err("Invalid split request".into()),
                },
                "recover" => Action::RecoverLane { flow: flow.into() },
                state => Action::Lane {
                    flow: flow.into(),
                    state: match state {
                        "active" => crate::iteration::FlowLifecycle::Active,
                        "stopped" => crate::iteration::FlowLifecycle::Stopped,
                        "archived" => crate::iteration::FlowLifecycle::Archived,
                        _ => return Err("Lane state must be active, stopped, archived or recover".into()),
                    },
                },
            }
        }
        "new_terminal" => Action::NewTerminal {
            cwd: if args.get("cwd").is_some() {
                Some(absolute("cwd")?)
            } else {
                None
            },
        },
        "select_tab" => Action::SelectTab(tab()?),
        "close_tab" => Action::CloseTab(tab()?),
        "stop_agent" => Action::StopAgent(tab()?),
        "dock_tab" => Action::DockTab {
            tab: tab()?,
            target: hex_id("target")?,
            position: match string("position")? {
                "left" => DropPart::Left,
                "right" => DropPart::Right,
                "top" => DropPart::Top,
                "bottom" => DropPart::Bottom,
                "center" => DropPart::Center,
                "tab" => DropPart::Tab,
                _ => return Err("position must be left, right, top, bottom, center or tab".into()),
            },
        },
        "refresh_project_tree" => Action::RefreshProject,
        "reveal_project_file" => Action::RevealProject(absolute("path")?),
        "open_settings" => Action::Settings,
        "open_activity" => Action::Activity,
        "open_disk" => Action::Disk,
        "open_usage" => Action::Usage,
        "refresh_usage" => Action::RefreshUsage,
        "inspect_usage" => Action::InspectUsage,
        "set_appearance" => {
            let style = string("style")?;
            let dark = args
                .get("dark")
                .and_then(Value::as_bool)
                .ok_or("dark must be a boolean")?;
            if style != "host"
                && !makepad_widgets::desktop_style::DesktopStyle::ALL
                    .iter()
                    .any(|s| s.id() == style)
            {
                return Err("unknown style family".into());
            }
            Action::Appearance {
                style: (style != "host").then(|| style.to_string()),
                dark,
            }
        }
        "read_terminal" => {
            let lines = match args.get("lines") {
                None => None,
                Some(Value::Int(n)) if (1..=2000).contains(n) => Some(*n as usize),
                _ => return Err("lines must be an integer from 1 to 2000".into()),
            };
            Action::ReadTerminal { tab: tab()?, lines }
        }
        "type_terminal" | "run" => {
            let is_run = call.tool == "run";
            let text = string(if is_run { "command" } else { "text" })?;
            if text.is_empty() || text.len() > 4096 {
                return Err("terminal input must contain 1–4096 bytes".into());
            }
            if is_run
                && text
                    .chars()
                    .any(|c| c.is_control() && c != '\n' && c != '\t')
            {
                return Err("command contains terminal control characters; use type_terminal for deliberate controls".into());
            }
            if is_run {
                Action::Run {
                    tab: tab()?,
                    command: text.into(),
                }
            } else {
                Action::Input {
                    tab: tab()?,
                    text: text.into(),
                }
            }
        }
        "refresh_disk" => Action::RefreshDisk,
        "inspect_disk" => Action::InspectDisk,
        "cleanup_preview" => Action::CleanupPreview(absolute("path")?),
        "set_layout" => {
            let layout = string("layout")?;
            if layout.len() > 131072 {
                return Err("layout exceeds 128 KiB".into());
            }
            Action::Layout(layout.into())
        }
        "set_mode" => Action::Mode(match string("mode")? {
            "structured" => Mode::Structured,
            "tasks" => Mode::Tasks,
            "architecture" => Mode::Architecture,
            "disk" => Mode::Disk,
            _ => return Err("mode must be structured, tasks, architecture or disk".into()),
        }),
        "open_code" => Action::OpenCode(absolute("path")?),
        "read_code" => Action::ReadCode { tab: tab()? },
        "save_code" => Action::SaveCode { tab: tab()? },
        "reload_code" => Action::ReloadCode { tab: tab()? },
        "add_design" => {
            let title = string("title")?;
            let detail = string("detail")?;
            if title.trim().is_empty() || title.len() > 120 {
                return Err("title must contain 1–120 bytes".into());
            }
            if detail.len() > 8192 {
                return Err("detail must contain at most 8192 bytes".into());
            }
            Action::AddDesign {
                title: title.into(),
                detail: detail.into(),
                parent: if args.get("parent").is_some() {
                    Some(hex_id("parent")?)
                } else {
                    None
                },
                path: if args.get("path").is_some() {
                    Some(absolute("path")?)
                } else {
                    None
                },
            }
        }
        _ => unreachable!(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn action(tool: &str, args: &str) -> Result<Action, String> {
        parse(&ServiceCall {
            call_id: "test".into(),
            tool: tool.into(),
            args: args.into(),
        })
    }
    #[test]
    fn manifest_is_valid() {
        manifest().validate().unwrap();
    }
    #[test]
    fn docking_and_project_actions_reject_invalid_targets_and_paths() {
        assert!(matches!(
            action(
                "dock_tab",
                r#"{"tab":"ab","target":"cd","position":"right"}"#
            ),
            Ok(Action::DockTab {
                tab: 0xab,
                target: 0xcd,
                position: DropPart::Right
            })
        ));
        assert!(action(
            "dock_tab",
            r#"{"tab":"ab","target":"not-an-id","position":"right"}"#
        )
        .is_err());
        assert!(action(
            "dock_tab",
            r#"{"tab":"ab","target":"cd","position":"floating"}"#
        )
        .is_err());
        assert!(action("reveal_project_file", r#"{"path":"../outside.rs"}"#).is_err());
    }
    #[test]
    fn strict_action_arguments() {
        assert!(action("status", r#"{"surprise":true}"#).is_err());
        assert!(action("run", r#"{"tab":"abcd","command":"\u001bboom"}"#).is_err());
        assert!(action("read_terminal", r#"{"tab":"abcd","lines":2001}"#).is_err());
        assert!(action("set_appearance", r#"{"style":"made-up","dark":false}"#).is_err());
        assert!(action("new_terminal", r#"{"cwd":"relative"}"#).is_err());
        assert_eq!(
            action("run", r#"{"tab":"abcd","command":"pwd"}"#).unwrap(),
            Action::Run {
                tab: 0xabcd,
                command: "pwd".into()
            }
        );
    }

    #[test]
    fn set_mode_accepts_the_three_modes_and_nothing_else() {
        assert_eq!(
            action("set_mode", r#"{"mode":"structured"}"#).unwrap(),
            Action::Mode(Mode::Structured)
        );
        assert_eq!(
            action("set_mode", r#"{"mode":"tasks"}"#).unwrap(),
            Action::Mode(Mode::Tasks)
        );
        assert_eq!(
            action("set_mode", r#"{"mode":"architecture"}"#).unwrap(),
            Action::Mode(Mode::Architecture)
        );
        assert_eq!(
            action("set_mode", r#"{"mode":"disk"}"#).unwrap(),
            Action::Mode(Mode::Disk)
        );
        for (tool, args) in [
            ("set_mode", r#"{"mode":"canvas"}"#),
            ("set_mode", r#"{"mode":"both"}"#),
            ("set_mode", r#"{"mode":true}"#),
            ("canvas_fit", "{}"),
            ("focus_card", r#"{"card":"1"}"#),
            ("open_demo", "{}"),
        ] {
            assert!(action(tool, args).is_err(), "accepted {tool} {args}");
        }
    }

    #[test]
    fn code_and_design_tools_require_explicit_paths_and_discard_action() {
        assert_eq!(
            action("open_code", r#"{"path":"/project/src/main.rs"}"#).unwrap(),
            Action::OpenCode(PathBuf::from("/project/src/main.rs"))
        );
        assert_eq!(
            action("read_code", r#"{"tab":"af"}"#).unwrap(),
            Action::ReadCode { tab: 0xaf }
        );
        assert_eq!(
            action("save_code", r#"{"tab":"af"}"#).unwrap(),
            Action::SaveCode { tab: 0xaf }
        );
        assert_eq!(
            action("reload_code", r#"{"tab":"af"}"#).unwrap(),
            Action::ReloadCode { tab: 0xaf }
        );
        assert_eq!(
            manifest().tool("reload_code").unwrap().risk,
            Risk::Destructive
        );
        assert_eq!(manifest().tool("read_code").unwrap().risk, Risk::Read);
        assert_eq!(action("add_design", r#"{"title":"Parser","detail":"Consumes tokens\nProduces syntax","parent":"ab","path":"/project/parser.rs"}"#).unwrap(),
            Action::AddDesign { title: "Parser".into(), detail: "Consumes tokens\nProduces syntax".into(), parent: Some(0xab), path: Some(PathBuf::from("/project/parser.rs")) });
        assert_eq!(
            action("add_design", r#"{"title":"System","detail":""}"#).unwrap(),
            Action::AddDesign {
                title: "System".into(),
                detail: String::new(),
                parent: None,
                path: None
            }
        );
        for (tool, args) in [
            ("open_code", r#"{"path":"relative.rs"}"#),
            ("open_code", r#"{"path":"/project/\u0000main.rs"}"#),
            ("read_code", r#"{"tab":"ab","path":"/somewhere"}"#),
            ("reload_code", r#"{"tab":null}"#),
            ("add_design", r#"{"title":" ","detail":"blank title"}"#),
            ("add_design", r#"{"title":"System"}"#),
            (
                "add_design",
                r#"{"title":"System","detail":"","parent":null}"#,
            ),
            (
                "add_design",
                r#"{"title":"System","detail":"","parent":"+1"}"#,
            ),
            (
                "add_design",
                r#"{"title":"System","detail":"","path":"relative.rs"}"#,
            ),
            (
                "add_design",
                r#"{"title":"System","detail":"","owner":"guessed"}"#,
            ),
        ] {
            assert!(action(tool, args).is_err(), "accepted {tool} {args}");
        }
        let long_title = json::obj(vec![
            ("title", json::s("a".repeat(121))),
            ("detail", json::s("")),
        ])
        .to_json();
        assert!(action("add_design", &long_title).is_err());
        let long_detail = json::obj(vec![
            ("title", json::s("System")),
            ("detail", json::s("a".repeat(8193))),
        ])
        .to_json();
        assert!(action("add_design", &long_detail).is_err());
    }
}
