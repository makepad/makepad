//! One typed action boundary for Studio's UI and the standard app AI bus.
use crate::workspace::{LayoutMode, Mode};
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef};
use makepad_strict_json::{self as json, Value};
use makepad_widgets::dock::DropPart;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Status,
    NewTerminal {
        cwd: Option<PathBuf>,
    },
    SelectTab(u64),
    CloseTab(u64),
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
    CanvasLayout(LayoutMode),
    CanvasFit,
    CanvasZoom(f64),
    MoveCard {
        id: u64,
        x: f64,
        y: f64,
    },
    ResizeCard {
        id: u64,
        width: f64,
        height: f64,
    },
    FocusCard(u64),
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
}

pub fn manifest() -> ServiceManifest {
    let mut m = ServiceManifest::new("studio", "Studio", "AI work environment with Structured tabs and a giant zoomable Canvas sharing the same live terminals and code editors. Canvas cards show system designs and observed activity, with Auto or Free layout. Read status first for current tab/card hex IDs, relationships, and state. File changes and process observations do not by themselves identify an AI owner or prove a test passed. All terminal input goes to the live PTY, with the same consequences as typing. Disk inventory and cleanup previews never delete files.");
    for (name, description, props, required, risk) in [
        ("status", "Current tabs and canvas cards (hex IDs), selections, view/layout modes, camera, relationships, observed activity, appearance, and disk summary.", "", "", Risk::Read),
        ("new_terminal", "Open and select a new live terminal; optional absolute cwd. Returns its tab ID.", r#""cwd":{"type":"string"}"#, "", Risk::Act),
        ("select_tab", "Select an existing tab by its ID from status.", r#""tab":{"type":"string"}"#, "tab", Risk::Act),
        ("close_tab", "Close a tab. For a terminal this ends its live shell and may interrupt running work.", r#""tab":{"type":"string"}"#, "tab", Risk::Destructive),
        ("dock_tab", "Move a live tab in the Structured Dock: split beside a target tab (left/right/top/bottom), join its group (center), or insert before it (tab). Preserves terminal/editor state and Canvas layout.", r#""tab":{"type":"string"},"target":{"type":"string"},"position":{"type":"string","enum":["left","right","top","bottom","center","tab"]}"#, "tab,target,position", Risk::Act),
        ("refresh_project_tree", "Refresh the project file tree in the background.", "", "", Risk::Act),
        ("reveal_project_file", "Expand the project tree to an absolute path inside the current project and select it, without opening another editor.", r#""path":{"type":"string","maxLength":4096}"#, "path", Risk::Act),
        ("open_activity", "Open the shared activity view with observed processes, source changes and recent Studio actions.", "", "", Risk::Act),
        ("open_settings", "Open Studio Settings.", "", "", Risk::Act),
        ("open_disk", "Open the disk and workspace breakdown.", "", "", Risk::Act),
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
        ("set_mode", "Switch between Structured tabs and Canvas. Both views use the same live terminals and code editors.", r#""mode":{"type":"string","enum":["structured","canvas"]}"#, "mode", Risk::Act),
        ("canvas_layout", "Choose stable Auto layout or Free layout for dragging cards. Switching preserves the independent manual arrangement.", r#""layout":{"type":"string","enum":["auto","free"]}"#, "layout", Risk::Act),
        ("canvas_fit", "Frame all current canvas cards in the viewport.", "", "", Risk::Act),
        ("canvas_zoom", "Multiply the canvas zoom by a factor from 0.1 to 10. A factor below 1 zooms out; above 1 zooms in.", r#""factor":{"type":"number","minimum":0.1,"maximum":10}"#, "factor", Risk::Act),
        ("move_card", "Move a card in Free layout using world coordinates. Auto layout must first be switched to Free; the automatic arrangement is preserved.", r#""card":{"type":"string"},"x":{"type":"number","minimum":-10000000,"maximum":10000000},"y":{"type":"number","minimum":-10000000,"maximum":10000000}"#, "card,x,y", Risk::Act),
        ("resize_card", "Resize a card in the current Auto or Free layout using world dimensions. Auto moves overlapping cards down; the other layout and live terminal/editor state are retained.", r#""card":{"type":"string"},"width":{"type":"number","minimum":160,"maximum":4096},"height":{"type":"number","minimum":120,"maximum":4096}"#, "card,width,height", Risk::Act),
        ("focus_card", "Focus an existing canvas card by its ID from status.", r#""card":{"type":"string"}"#, "card", Risk::Act),
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
    m
}

pub fn parse(call: &ServiceCall) -> Result<Action, String> {
    if call.args.len() > 140_000 {
        return Err("tool arguments exceed the size limit".into());
    }
    let args = json::parse(call.args.as_bytes()).map_err(str::to_owned)?;
    let Value::Obj(fields) = &args else {
        return Err("arguments must be an object".into());
    };
    let allowed: &[&str] = match call.tool.as_str() {
        "status"
        | "open_settings"
        | "open_activity"
        | "open_disk"
        | "refresh_disk"
        | "inspect_disk"
        | "canvas_fit"
        | "open_usage"
        | "refresh_usage"
        | "inspect_usage"
        | "refresh_project_tree" => &[],
        "new_terminal" => &["cwd"],
        "select_tab" | "close_tab" | "read_code" | "save_code" | "reload_code" => &["tab"],
        "dock_tab" => &["tab", "target", "position"],
        "read_terminal" => &["tab", "lines"],
        "type_terminal" => &["tab", "text"],
        "run" => &["tab", "command"],
        "set_appearance" => &["style", "dark"],
        "cleanup_preview" | "open_code" | "reveal_project_file" => &["path"],
        "set_layout" | "canvas_layout" => &["layout"],
        "set_mode" => &["mode"],
        "canvas_zoom" => &["factor"],
        "move_card" => &["card", "x", "y"],
        "resize_card" => &["card", "width", "height"],
        "focus_card" => &["card"],
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
    let number = |key: &str, min: f64, max: f64| -> Result<f64, String> {
        let value = match args.get(key) {
            Some(Value::Int(value)) => *value as f64,
            Some(Value::F64(value)) => *value,
            _ => return Err(format!("{key} must be a number")),
        };
        if !value.is_finite() || value < min || value > max {
            return Err(format!("{key} must be finite and between {min} and {max}"));
        }
        Ok(value)
    };
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
        "new_terminal" => Action::NewTerminal {
            cwd: if args.get("cwd").is_some() {
                Some(absolute("cwd")?)
            } else {
                None
            },
        },
        "select_tab" => Action::SelectTab(tab()?),
        "close_tab" => Action::CloseTab(tab()?),
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
            "canvas" => Mode::Canvas,
            _ => return Err("mode must be structured or canvas".into()),
        }),
        "canvas_layout" => Action::CanvasLayout(match string("layout")? {
            "auto" => LayoutMode::Auto,
            "free" => LayoutMode::Free,
            _ => return Err("layout must be auto or free".into()),
        }),
        "canvas_fit" => Action::CanvasFit,
        "canvas_zoom" => Action::CanvasZoom(number("factor", 0.1, 10.0)?),
        "move_card" => Action::MoveCard {
            id: hex_id("card")?,
            x: number("x", -10_000_000.0, 10_000_000.0)?,
            y: number("y", -10_000_000.0, 10_000_000.0)?,
        },
        "resize_card" => Action::ResizeCard {
            id: hex_id("card")?,
            width: number("width", 160.0, 4096.0)?,
            height: number("height", 120.0, 4096.0)?,
        },
        "focus_card" => Action::FocusCard(hex_id("card")?),
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
    fn canvas_tools_preserve_typed_modes_and_world_coordinates() {
        assert_eq!(
            action("set_mode", r#"{"mode":"structured"}"#).unwrap(),
            Action::Mode(Mode::Structured)
        );
        assert_eq!(
            action("set_mode", r#"{"mode":"canvas"}"#).unwrap(),
            Action::Mode(Mode::Canvas)
        );
        assert_eq!(
            action("canvas_layout", r#"{"layout":"auto"}"#).unwrap(),
            Action::CanvasLayout(LayoutMode::Auto)
        );
        assert_eq!(
            action("canvas_layout", r#"{"layout":"free"}"#).unwrap(),
            Action::CanvasLayout(LayoutMode::Free)
        );
        assert_eq!(action("canvas_fit", "{}").unwrap(), Action::CanvasFit);
        assert_eq!(
            action("canvas_zoom", r#"{"factor":0.1}"#).unwrap(),
            Action::CanvasZoom(0.1)
        );
        assert_eq!(
            action("canvas_zoom", r#"{"factor":10}"#).unwrap(),
            Action::CanvasZoom(10.0)
        );
        assert_eq!(
            action("move_card", r#"{"card":"ABcd","x":-2500.25,"y":120}"#).unwrap(),
            Action::MoveCard {
                id: 0xabcd,
                x: -2500.25,
                y: 120.0
            }
        );
        assert_eq!(
            action("focus_card", r#"{"card":"ffffffffffffffff"}"#).unwrap(),
            Action::FocusCard(u64::MAX)
        );
        assert_eq!(
            action("resize_card", r#"{"card":"abcd","width":960,"height":640}"#).unwrap(),
            Action::ResizeCard {
                id: 0xabcd,
                width: 960.0,
                height: 640.0
            }
        );
    }

    #[test]
    fn canvas_tools_reject_ambiguous_ids_nonfinite_geometry_and_wrong_shapes() {
        for (tool, args) in [
            ("set_mode", r#"{"mode":"both"}"#),
            ("set_mode", r#"{"mode":true}"#),
            ("canvas_layout", r#"{"layout":"pinned"}"#),
            ("canvas_fit", r#"{"unused":false}"#),
            ("canvas_zoom", r#"{"factor":0}"#),
            ("canvas_zoom", r#"{"factor":10.001}"#),
            ("canvas_zoom", r#"{"factor":"1"}"#),
            ("canvas_zoom", r#"{"factor":1e309}"#),
            ("move_card", r#"{"card":"1","x":10000001,"y":0}"#),
            ("move_card", r#"{"card":"1","x":0,"y":-10000001}"#),
            ("move_card", r#"{"card":"1","x":0}"#),
            ("move_card", r#"{"card":"1","x":0,"x":1,"y":0}"#),
            ("move_card", r#"{"card":"1","x":null,"y":0}"#),
            ("resize_card", r#"{"card":"1","width":0,"height":640}"#),
            ("resize_card", r#"{"card":"1","width":960,"height":1e309}"#),
            ("resize_card", r#"{"card":"1","width":960}"#),
            (
                "resize_card",
                r#"{"card":"1","width":960,"height":640,"x":1}"#,
            ),
            ("focus_card", r#"{"card":"0x12"}"#),
            ("focus_card", r#"{"card":"+12"}"#),
            ("focus_card", r#"{"card":"10000000000000000"}"#),
            ("focus_card", r#"{"card":12}"#),
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
