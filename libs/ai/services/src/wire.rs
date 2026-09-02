//! The wire between a host's engine and an app's service.
//!
//! Everything here is a bounded JSON object (micro_serde), the same way the
//! window manager's own protocol is. A service is described once by its
//! [`ServiceManifest`]; from then on the engine sends [`ServiceDown`] messages
//! (calls, cancels) and the service answers with [`ServiceUp`] messages
//! (results, progress, context). The transport is not this module's
//! business: in-process it is a channel, hosted by mpwm it rides the
//! studio protocol's `Custom` frames under the `"mpwm_ai"` envelope key.
//!
//! Names. A tool is known to the model by its canonical dotted name,
//! `<service>.<tool>` (`route.plan`, `files.list_dir`); native function
//! calling APIs forbid dots, so the same tool is `<service>__<tool>` there.
//! [`canonical_name`] / [`api_name`] / [`split_name`] are the one place that
//! mapping lives.
//!
//! Risk. Every tool declares how much it can break: reading, acting on the
//! app's own state, or destroying something outside the app's undo reach.
//! The ROUTER enforces the gate (a destructive call waits for the person to
//! confirm); the app stays its own security boundary regardless — a closed
//! match over tool names, typed arguments, path jails, bounded output.

use makepad_micro_serde::*;

/// Bytes a service brief may occupy in the system prompt.
pub const MAX_BRIEF_BYTES: usize = 4 * 1024;
/// Bytes of a tool description.
pub const MAX_DESCRIPTION_BYTES: usize = 512;
/// Bytes of one tool's JSON-schema text.
pub const MAX_PARAMETERS_BYTES: usize = 8 * 1024;
/// Tools one service may declare.
pub const MAX_TOOLS: usize = 64;
/// Bytes of a result's model-facing text; the router truncates past this.
pub const MAX_RESULT_BYTES: usize = 16 * 1024;
/// Bytes of a result's transcript note.
pub const MAX_NOTE_BYTES: usize = 256;
/// Bytes of a service's volatile per-turn context.
pub const MAX_CONTEXT_BYTES: usize = 2 * 1024;
/// Bytes of a call's argument JSON.
pub const MAX_ARGS_BYTES: usize = 16 * 1024;
/// Longest service id.
pub const MAX_SERVICE_ID: usize = 24;
/// Longest tool short name.
pub const MAX_TOOL_NAME: usize = 32;

/// The envelope key hosted transports use inside a studio `Custom` frame,
/// distinct from the window manager's own `"mpwm"` key.
pub const HOSTED_KEY: &str = "mpwm_ai";

/// How much a tool can break.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum Risk {
    /// Looks at something. Runs immediately.
    Read,
    /// Changes the app's own state — a route, a level, a selection. Runs
    /// immediately; the app can undo or redo it on its own terms.
    Act,
    /// Deletes, sends, spends, or otherwise reaches past the app. The
    /// router parks the call until the person confirms it.
    Destructive,
}

/// One tool, as the model is told about it.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ToolDef {
    /// Short name, unique within the service: `[a-z0-9_]{1,32}`.
    pub name: String,
    /// One or two sentences: what it does and when to use it.
    pub description: String,
    /// A JSON-schema object for the arguments, verbatim.
    pub parameters: String,
    pub risk: Risk,
    /// A successful call wants a live preview of the app under its card.
    pub preview: bool,
}

impl ToolDef {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: impl Into<String>,
        risk: Risk,
    ) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: description.into(),
            parameters: parameters.into(),
            risk,
            preview: false,
        }
    }

    /// The same tool, asking for a preview card when it succeeds.
    pub fn with_preview(mut self) -> ToolDef {
        self.preview = true;
        self
    }
}

/// Who a service is and what it will do.
#[derive(Clone, Debug, PartialEq, Default, SerJson, DeJson)]
pub struct ServiceManifest {
    /// `[a-z0-9_]{1,24}`: `route`, `files`, `game`, `os`.
    pub id: String,
    /// Shown on the chip: `Route`, `Files`.
    pub label: String,
    /// The doctrine paragraph the model reads about this app.
    pub brief: String,
    pub tools: Vec<ToolDef>,
}

impl ServiceManifest {
    pub fn new(id: impl Into<String>, label: impl Into<String>, brief: impl Into<String>) -> Self {
        ServiceManifest {
            id: id.into(),
            label: label.into(),
            brief: brief.into(),
            tools: Vec::new(),
        }
    }

    pub fn with_tool(mut self, tool: ToolDef) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn tool(&self, name: &str) -> Option<&ToolDef> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// Refuse anything the caps do not allow, naming the first problem.
    pub fn validate(&self) -> Result<(), String> {
        if !is_ident(&self.id, MAX_SERVICE_ID) {
            return Err(format!("service id '{}' is not [a-z0-9_]{{1,{}}}", self.id, MAX_SERVICE_ID));
        }
        if self.label.trim().is_empty() || self.label.len() > 48 {
            return Err(format!("service '{}' label must be 1..48 bytes", self.id));
        }
        if self.brief.len() > MAX_BRIEF_BYTES {
            return Err(format!("service '{}' brief is {} bytes; the cap is {}", self.id, self.brief.len(), MAX_BRIEF_BYTES));
        }
        if self.tools.len() > MAX_TOOLS {
            return Err(format!("service '{}' declares {} tools; the cap is {}", self.id, self.tools.len(), MAX_TOOLS));
        }
        for (i, tool) in self.tools.iter().enumerate() {
            if !is_ident(&tool.name, MAX_TOOL_NAME) {
                return Err(format!("service '{}' tool '{}' is not [a-z0-9_]{{1,{}}}", self.id, tool.name, MAX_TOOL_NAME));
            }
            if self.tools[..i].iter().any(|t| t.name == tool.name) {
                return Err(format!("service '{}' declares '{}' twice", self.id, tool.name));
            }
            if tool.description.trim().is_empty() || tool.description.len() > MAX_DESCRIPTION_BYTES {
                return Err(format!("tool '{}.{}' description must be 1..{} bytes", self.id, tool.name, MAX_DESCRIPTION_BYTES));
            }
            if tool.parameters.len() > MAX_PARAMETERS_BYTES {
                return Err(format!("tool '{}.{}' schema is {} bytes; the cap is {}", self.id, tool.name, tool.parameters.len(), MAX_PARAMETERS_BYTES));
            }
            match makepad_strict_json::parse(tool.parameters.as_bytes()) {
                Ok(makepad_strict_json::Value::Obj(_)) => {}
                Ok(_) => return Err(format!("tool '{}.{}' schema is not a JSON object", self.id, tool.name)),
                Err(e) => return Err(format!("tool '{}.{}' schema does not parse: {e}", self.id, tool.name)),
            }
        }
        Ok(())
    }
}

/// `[a-z0-9_]{1,max}`, starting with a letter.
pub fn is_ident(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The name the model sees in a text tool protocol: `route.plan`.
pub fn canonical_name(service: &str, tool: &str) -> String {
    format!("{service}.{tool}")
}

/// The name a native function-calling API sees: `route__plan`.
pub fn api_name(service: &str, tool: &str) -> String {
    format!("{service}__{tool}")
}

/// `route.plan` or `route__plan` → `("route", "plan")`. A bare name has no
/// service and is returned as `("", name)` so the router can say so.
pub fn split_name(name: &str) -> (&str, &str) {
    if let Some((s, t)) = name.split_once("__") {
        return (s, t);
    }
    if let Some((s, t)) = name.split_once('.') {
        return (s, t);
    }
    ("", name)
}

/// One call, as the service receives it.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ServiceCall {
    /// The engine's id for the call; the result must carry it back.
    pub call_id: String,
    /// The tool's SHORT name (`plan`, not `route.plan`).
    pub tool: String,
    /// The argument object as JSON text. The service parses it with its
    /// own typed reader and refuses what it does not expect.
    pub args: String,
}

/// How a call ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum ToolOutcome {
    Ok,
    /// The tool ran and could not do it (no route found, file unreadable).
    Failed,
    /// The service would not do it: unknown tool, bad arguments, outside
    /// its jail.
    Refused,
    /// The person said no (a destructive call not confirmed), or the app's
    /// own policy denies AI control of this.
    Denied,
    /// Not right now: the app is busy, the model is loading, the service
    /// went away.
    Unavailable,
}

impl ToolOutcome {
    pub fn is_ok(self) -> bool {
        matches!(self, ToolOutcome::Ok)
    }

    pub fn slug(self) -> &'static str {
        match self {
            ToolOutcome::Ok => "ok",
            ToolOutcome::Failed => "failed",
            ToolOutcome::Refused => "refused",
            ToolOutcome::Denied => "denied",
            ToolOutcome::Unavailable => "unavailable",
        }
    }
}

/// One call's answer.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ToolResult {
    pub call_id: String,
    pub outcome: ToolOutcome,
    /// What the model reads. Bounded by the router at [`MAX_RESULT_BYTES`].
    pub text: String,
    /// One dim transcript line: "planned Dam → Utrecht, 41 min".
    pub note: String,
    /// Show the service's live preview under this call's card.
    pub preview: bool,
}

impl ToolResult {
    pub fn ok(call_id: impl Into<String>, text: impl Into<String>, note: impl Into<String>) -> Self {
        ToolResult {
            call_id: call_id.into(),
            outcome: ToolOutcome::Ok,
            text: text.into(),
            note: note.into(),
            preview: false,
        }
    }

    pub fn failed(call_id: impl Into<String>, message: impl Into<String>) -> Self {
        let message = message.into();
        ToolResult {
            call_id: call_id.into(),
            outcome: ToolOutcome::Failed,
            note: message.clone(),
            text: message,
            preview: false,
        }
    }

    pub fn refused(call_id: impl Into<String>, what: impl Into<String>) -> Self {
        let what = what.into();
        ToolResult {
            call_id: call_id.into(),
            outcome: ToolOutcome::Refused,
            note: what.clone(),
            text: what,
            preview: false,
        }
    }

    pub fn denied(call_id: impl Into<String>, what: impl Into<String>) -> Self {
        let what = what.into();
        ToolResult {
            call_id: call_id.into(),
            outcome: ToolOutcome::Denied,
            note: what.clone(),
            text: what,
            preview: false,
        }
    }

    pub fn unavailable(call_id: impl Into<String>, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        ToolResult {
            call_id: call_id.into(),
            outcome: ToolOutcome::Unavailable,
            note: reason.clone(),
            text: reason,
            preview: false,
        }
    }

    pub fn with_preview(mut self) -> Self {
        self.preview = true;
        self
    }

    /// Enforce the caps in place. Truncated text says so at its end, so the
    /// model knows it is reading a head, not the whole.
    pub fn bound(&mut self) {
        if self.text.len() > MAX_RESULT_BYTES {
            truncate_to_char_boundary(&mut self.text, MAX_RESULT_BYTES - 32);
            self.text.push_str("\n…[truncated by the router]");
        }
        if self.note.len() > MAX_NOTE_BYTES {
            truncate_to_char_boundary(&mut self.note, MAX_NOTE_BYTES - 3);
            self.note.push('…');
        }
    }
}

/// A service's volatile state for the next turn: "[route] gps=52.37,4.90
/// map=Utrecht z13 trip=Dam→Utrecht 41min".
#[derive(Clone, Debug, PartialEq, Default, SerJson, DeJson)]
pub struct ServiceContext {
    pub text: String,
}

/// Service → engine.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum ServiceUp {
    /// Here I am. Sent once when the service comes up; sending it again
    /// replaces the manifest (a re-register after a reload is fine).
    Register(ServiceManifest),
    Result(ToolResult),
    /// A long call is still alive. Resets the router's deadline.
    Progress { call_id: String, note: String, permille: u16 },
    Context(ServiceContext),
    /// Going away on purpose (the host also forgets a service whose
    /// transport dies).
    Unregister,
}

/// Engine → service.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum ServiceDown {
    Call(ServiceCall),
    /// The person or the router gave up on this call; the service should
    /// stop if it can and need not reply.
    Cancel { call_id: String },
    /// The host's chat pane is showing (or not). Informational: an
    /// embedded panel may hide itself while the desktop one is up.
    ChatOpen { open: bool },
}

#[derive(SerJson, DeJson)]
struct UpEnvelope {
    mpwm_ai: ServiceUp,
}

#[derive(SerJson, DeJson)]
struct DownEnvelope {
    mpwm_ai: ServiceDown,
}

impl ServiceUp {
    /// The hosted frame: `{"mpwm_ai": …}`.
    pub fn to_hosted_json(&self) -> String {
        UpEnvelope { mpwm_ai: self.clone() }.serialize_json()
    }

    /// `None` for frames that are not ours (the window manager's own
    /// `"mpwm"` envelope, anything else).
    pub fn parse_hosted(json: &str) -> Option<ServiceUp> {
        if !json.contains("\"mpwm_ai\"") {
            return None;
        }
        UpEnvelope::deserialize_json(json).ok().map(|e| e.mpwm_ai)
    }
}

impl ServiceDown {
    pub fn to_hosted_json(&self) -> String {
        DownEnvelope { mpwm_ai: self.clone() }.serialize_json()
    }

    pub fn parse_hosted(json: &str) -> Option<ServiceDown> {
        if !json.contains("\"mpwm_ai\"") {
            return None;
        }
        DownEnvelope::deserialize_json(json).ok().map(|e| e.mpwm_ai)
    }
}

/// Cut `s` to at most `max` bytes without splitting a character.
pub fn truncate_to_char_boundary(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> ServiceManifest {
        ServiceManifest::new("route", "Route", "The trip planner.")
            .with_tool(
                ToolDef::new(
                    "plan",
                    "Plan a trip.",
                    r#"{"type":"object","properties":{"to":{"type":"string"}},"required":["to"]}"#,
                    Risk::Act,
                )
                .with_preview(),
            )
            .with_tool(ToolDef::new("status", "The trip so far.", r#"{"type":"object","properties":{}}"#, Risk::Read))
    }

    #[test]
    fn a_manifest_round_trips_through_the_hosted_envelope() {
        let up = ServiceUp::Register(route());
        let json = up.to_hosted_json();
        assert!(json.contains("\"mpwm_ai\""));
        assert_eq!(ServiceUp::parse_hosted(&json), Some(up));
    }

    #[test]
    fn every_up_and_down_variant_round_trips() {
        let ups = vec![
            ServiceUp::Register(route()),
            ServiceUp::Result(ToolResult::ok("c1", "41 min", "planned").with_preview()),
            ServiceUp::Result(ToolResult::failed("c2", "no route")),
            ServiceUp::Result(ToolResult::refused("c3", "not a tool")),
            ServiceUp::Result(ToolResult::denied("c4", "no")),
            ServiceUp::Result(ToolResult::unavailable("c5", "loading")),
            ServiceUp::Progress { call_id: "c1".into(), note: "routing".into(), permille: 500 },
            ServiceUp::Context(ServiceContext { text: "[route] gps=…".into() }),
            ServiceUp::Unregister,
        ];
        for up in ups {
            let json = up.to_hosted_json();
            assert_eq!(ServiceUp::parse_hosted(&json).as_ref(), Some(&up), "{json}");
        }
        let downs = vec![
            ServiceDown::Call(ServiceCall { call_id: "c1".into(), tool: "plan".into(), args: r#"{"to":"Utrecht"}"#.into() }),
            ServiceDown::Cancel { call_id: "c1".into() },
            ServiceDown::ChatOpen { open: true },
        ];
        for down in downs {
            let json = down.to_hosted_json();
            assert_eq!(ServiceDown::parse_hosted(&json).as_ref(), Some(&down), "{json}");
        }
    }

    #[test]
    fn the_window_managers_own_frames_are_not_ours() {
        assert_eq!(ServiceUp::parse_hosted(r#"{"mpwm":{"Close":{}}}"#), None);
        assert_eq!(ServiceDown::parse_hosted(r#"{"mpwm":{"Adopted":{}}}"#), None);
        assert_eq!(ServiceUp::parse_hosted("not json"), None);
    }

    #[test]
    fn names_map_both_ways() {
        assert_eq!(canonical_name("route", "plan"), "route.plan");
        assert_eq!(api_name("route", "plan"), "route__plan");
        assert_eq!(split_name("route.plan"), ("route", "plan"));
        assert_eq!(split_name("route__plan"), ("route", "plan"));
        assert_eq!(split_name("plan"), ("", "plan"));
    }

    #[test]
    fn validation_names_the_first_problem() {
        assert!(route().validate().is_ok());
        let mut bad = route();
        bad.id = "Route".into();
        assert!(bad.validate().unwrap_err().contains("service id"));
        let mut dup = route();
        dup.tools.push(dup.tools[0].clone());
        assert!(dup.validate().unwrap_err().contains("twice"));
        let mut schema = route();
        schema.tools[0].parameters = "[]".into();
        assert!(schema.validate().unwrap_err().contains("not a JSON object"));
        let mut brief = route();
        brief.brief = "x".repeat(MAX_BRIEF_BYTES + 1);
        assert!(brief.validate().unwrap_err().contains("brief"));
    }

    #[test]
    fn a_result_is_bounded_without_splitting_characters() {
        let mut r = ToolResult::ok("c", "é".repeat(MAX_RESULT_BYTES), "n".repeat(MAX_NOTE_BYTES + 10));
        r.bound();
        assert!(r.text.len() <= MAX_RESULT_BYTES);
        assert!(r.text.ends_with("[truncated by the router]"));
        assert!(r.note.len() <= MAX_NOTE_BYTES);
        assert!(r.note.ends_with('…'));
    }
}
