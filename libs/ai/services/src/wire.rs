//! The wire between a host's engine and an app's service.
//!
//! Everything here is a bounded JSON object (micro_serde), the same way the
//! window manager's own protocol is. A service is described once by its
//! [`ServiceManifest`]; from then on the engine sends [`ServiceDown`] messages
//! (calls, cancels) and the service answers with [`ServiceUp`] messages
//! (results, progress, context). The transport is not this module's
//! business: in-process it is a channel, hosted by the window manager it
//! rides the studio protocol's `Custom` frames under the `"wm_ai"` envelope
//! key.
//!
//! Addressing. An app id (`route`) says WHAT a service is; an
//! [`EndpointId`] says WHICH running instance it is. The host issues the
//! endpoint when a service registers and answers with
//! [`ServiceDown::Registered`]; from then on every up-frame is stamped with
//! its sender's endpoint by the host (never trusted from the sender) and
//! every down-frame names its target, so two instances of one app, or two
//! ports in one process, never see each other's calls. Where a service
//! lives — its parent, its tile, whether it is focused — is
//! [`InstanceMeta`], kept by the host beside the manifest.
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
//! confirm) and keeps its own floors per service, since a declaration is
//! self-reported; the app stays its own security boundary regardless — a
//! closed match over tool names, typed arguments, path jails, bounded output.
//!
//! Bounds are enforced where a frame ARRIVES: [`HostedUp::parse`] refuses a
//! frame over [`MAX_FRAME_BYTES`] before deserializing anything, and every
//! variant is checked by [`ServiceUp::validate`] / [`ServiceDown::validate`]
//! on receipt — the sender's own care is not relied on.

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
/// Bytes of a result's structured data (JSON).
pub const MAX_DATA_BYTES: usize = 16 * 1024;
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
/// Longest call id or endpoint id.
pub const MAX_ID_BYTES: usize = 64;
/// Bytes of a whole manifest's text fields together.
pub const MAX_MANIFEST_BYTES: usize = 640 * 1024;
/// Bytes of one hosted frame. Anything larger is dropped unread.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
/// Bytes of an instance's display name or location line.
pub const MAX_META_BYTES: usize = 128;

/// The envelope key hosted transports use inside a studio `Custom` frame,
/// distinct from the window manager's own `"wm"` key. A protocol
/// namespace, versioned with the wire — not a product string.
pub const HOSTED_KEY: &str = "wm_ai";

/// How much a tool can break.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, SerJson, DeJson)]
pub enum Risk {
    /// Looks at something. Runs immediately.
    Read,
    /// Changes the app's own state — a route, a level, a selection. Runs
    /// immediately; the app can undo or redo it on its own terms.
    Act,
    /// Deletes, sends, spends, installs, or otherwise reaches past the
    /// app. The router parks the call until the person confirms it.
    Destructive,
}

/// One tool, as the model is told about it.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ToolDef {
    /// Short name, unique within the service: `[a-z0-9_]{1,32}`.
    pub name: String,
    /// One or two sentences: what it does and when to use it.
    pub description: String,
    /// A JSON-schema object for the arguments, verbatim. Must describe an
    /// object (`"type":"object"`).
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
        let mut total = self.brief.len() + self.label.len();
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
                Ok(makepad_strict_json::Value::Obj(fields)) => {
                    let is_object = fields
                        .iter()
                        .any(|(k, v)| k == "type" && v.as_str() == Some("object"));
                    if !is_object {
                        return Err(format!("tool '{}.{}' schema must be an argument object (\"type\":\"object\")", self.id, tool.name));
                    }
                }
                Ok(_) => return Err(format!("tool '{}.{}' schema is not a JSON object", self.id, tool.name)),
                Err(e) => return Err(format!("tool '{}.{}' schema does not parse: {e}", self.id, tool.name)),
            }
            total += tool.description.len() + tool.parameters.len() + tool.name.len();
        }
        if total > MAX_MANIFEST_BYTES {
            return Err(format!("service '{}' manifest is {} bytes; the cap is {}", self.id, total, MAX_MANIFEST_BYTES));
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

/// An opaque id: printable ASCII, no whitespace, bounded.
pub fn is_opaque_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_ID_BYTES && s.bytes().all(|b| b.is_ascii_graphic())
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

/// Which running instance a service is. Issued by the host, opaque to
/// everyone else, unique for the host's lifetime (never reused).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default, SerJson, DeJson)]
pub struct EndpointId(pub String);

impl EndpointId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where an instance lives, as the host knows it and the model is told.
#[derive(Clone, Debug, PartialEq, Default, SerJson, DeJson)]
pub struct InstanceMeta {
    /// The manifest's id (`route`).
    pub app_id: String,
    /// What the person calls it: `Route`, `Route (2)`, `Photos in Holiday Review`.
    pub display_name: String,
    /// The endpoint this instance is nested in, when it is an `AppView`
    /// inside a composed app.
    pub parent: Option<EndpointId>,
    /// One line: `workspace 1, left tile`, `inside Holiday Review`.
    pub location: String,
    /// Bumped by the host whenever this instance gains focus; the highest
    /// wins a tie when the model does not say which instance it means.
    pub focus_epoch: u64,
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
    /// The person or the router cancelled it before it finished.
    Cancelled,
    /// The service did not answer within the router's deadline.
    TimedOut,
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
            ToolOutcome::Cancelled => "cancelled",
            ToolOutcome::TimedOut => "timed_out",
        }
    }
}

/// What the model's turn does after this result lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, SerJson, DeJson)]
pub enum Disposition {
    /// The usual: the model reads the result and goes on.
    #[default]
    Continue,
    /// The turn ends here with this result as its last word — the tool
    /// handed the person somewhere else (a new level, another app).
    EndTurn,
    /// The turn ends AND the conversation state is dropped: whatever the
    /// model believed about the world is no longer true.
    ResetConversation,
}

/// One call's answer.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ToolResult {
    pub call_id: String,
    pub outcome: ToolOutcome,
    /// What the model reads. Bounded at [`MAX_RESULT_BYTES`].
    pub text: String,
    /// Structured data for the model or the panel, as JSON text; empty when
    /// there is none. Bounded at [`MAX_DATA_BYTES`].
    pub data: String,
    /// One dim transcript line: "planned Dam → Utrecht, 41 min".
    pub note: String,
    /// Show the service's live preview under this call's card.
    pub preview: bool,
    pub disposition: Disposition,
}

impl ToolResult {
    fn make(call_id: impl Into<String>, outcome: ToolOutcome, text: String, note: String) -> Self {
        ToolResult {
            call_id: call_id.into(),
            outcome,
            text,
            data: String::new(),
            note,
            preview: false,
            disposition: Disposition::Continue,
        }
    }

    pub fn ok(call_id: impl Into<String>, text: impl Into<String>, note: impl Into<String>) -> Self {
        Self::make(call_id, ToolOutcome::Ok, text.into(), note.into())
    }

    pub fn failed(call_id: impl Into<String>, message: impl Into<String>) -> Self {
        let message = message.into();
        Self::make(call_id, ToolOutcome::Failed, message.clone(), message)
    }

    pub fn refused(call_id: impl Into<String>, what: impl Into<String>) -> Self {
        let what = what.into();
        Self::make(call_id, ToolOutcome::Refused, what.clone(), what)
    }

    pub fn denied(call_id: impl Into<String>, what: impl Into<String>) -> Self {
        let what = what.into();
        Self::make(call_id, ToolOutcome::Denied, what.clone(), what)
    }

    pub fn unavailable(call_id: impl Into<String>, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self::make(call_id, ToolOutcome::Unavailable, reason.clone(), reason)
    }

    pub fn cancelled(call_id: impl Into<String>) -> Self {
        Self::make(call_id, ToolOutcome::Cancelled, "cancelled".into(), "cancelled".into())
    }

    pub fn timed_out(call_id: impl Into<String>, what: impl Into<String>) -> Self {
        let what = what.into();
        Self::make(call_id, ToolOutcome::TimedOut, what.clone(), what)
    }

    pub fn with_preview(mut self) -> Self {
        self.preview = true;
        self
    }

    pub fn with_data(mut self, json: impl Into<String>) -> Self {
        self.data = json.into();
        self
    }

    pub fn with_disposition(mut self, disposition: Disposition) -> Self {
        self.disposition = disposition;
        self
    }

    /// Enforce the caps in place. Truncated text says so at its end, so the
    /// model knows it is reading a head, not the whole. Data that does not
    /// fit is dropped whole — a truncated JSON is worse than none.
    pub fn bound(&mut self) {
        if self.text.len() > MAX_RESULT_BYTES {
            truncate_to_char_boundary(&mut self.text, MAX_RESULT_BYTES - 32);
            self.text.push_str("\n…[truncated by the router]");
        }
        if self.data.len() > MAX_DATA_BYTES {
            self.data.clear();
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
    /// Here I am. `port_tag` is the port's own nonce, so the host's
    /// [`ServiceDown::Registered`] reaches the right port when a process
    /// has several. Sending it again replaces the manifest (a re-register
    /// after a reload is fine).
    Register { manifest: ServiceManifest, port_tag: u32 },
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
    /// The host's answer to `Register`: this is who you are now. Every
    /// later frame is addressed by this endpoint.
    Registered { port_tag: u32, endpoint: EndpointId },
    Call(ServiceCall),
    /// The person or the router gave up on this call; the service should
    /// stop if it can and need not reply.
    Cancel { call_id: String },
    /// The host's chat pane is showing (or not). Informational: an
    /// embedded panel may hide itself while the desktop one is up.
    ChatOpen { open: bool },
}

impl ServiceUp {
    /// Receiver-side check of one frame, whatever transport it came by.
    /// The manifest is validated again here even though the sending app
    /// validated it: the host does not rely on the app.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            ServiceUp::Register { manifest, .. } => manifest.validate(),
            ServiceUp::Result(r) => {
                if !is_opaque_id(&r.call_id) {
                    return Err("result: bad call id".into());
                }
                if r.text.len() > MAX_RESULT_BYTES + 64 {
                    return Err("result: text over cap".into());
                }
                if r.data.len() > MAX_DATA_BYTES {
                    return Err("result: data over cap".into());
                }
                if r.note.len() > MAX_NOTE_BYTES + 4 {
                    return Err("result: note over cap".into());
                }
                Ok(())
            }
            ServiceUp::Progress { call_id, note, permille } => {
                if !is_opaque_id(call_id) {
                    return Err("progress: bad call id".into());
                }
                if note.len() > MAX_NOTE_BYTES + 4 || *permille > 1000 {
                    return Err("progress: over cap".into());
                }
                Ok(())
            }
            ServiceUp::Context(c) => {
                if c.text.len() > MAX_CONTEXT_BYTES + 4 {
                    return Err("context: over cap".into());
                }
                Ok(())
            }
            ServiceUp::Unregister => Ok(()),
        }
    }
}

impl ServiceDown {
    /// Receiver-side check of one frame.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            ServiceDown::Registered { endpoint, .. } => {
                if !is_opaque_id(endpoint.as_str()) {
                    return Err("registered: bad endpoint".into());
                }
                Ok(())
            }
            ServiceDown::Call(c) => {
                if !is_opaque_id(&c.call_id) {
                    return Err("call: bad call id".into());
                }
                if !is_ident(&c.tool, MAX_TOOL_NAME) {
                    return Err("call: bad tool name".into());
                }
                if c.args.len() > MAX_ARGS_BYTES {
                    return Err("call: args over cap".into());
                }
                Ok(())
            }
            ServiceDown::Cancel { call_id } => {
                if !is_opaque_id(call_id) {
                    return Err("cancel: bad call id".into());
                }
                Ok(())
            }
            ServiceDown::ChatOpen { .. } => Ok(()),
        }
    }
}

/// A hosted up-frame: the message and, once the host has stamped it, the
/// sender. An app leaves `from` empty; the host overwrites it with the
/// endpoint it issued to that client — a sender's own claim is never used.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct HostedUp {
    pub from: Option<EndpointId>,
    pub msg: ServiceUp,
}

/// A hosted down-frame: the message and the endpoint it is for. Only
/// `Registered` may travel without a target (the port has no endpoint yet
/// and matches on its `port_tag` instead).
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct HostedDown {
    pub to: Option<EndpointId>,
    pub msg: ServiceDown,
}

#[derive(SerJson, DeJson)]
struct UpEnvelope {
    wm_ai: HostedUp,
}

#[derive(SerJson, DeJson)]
struct DownEnvelope {
    wm_ai: HostedDown,
}

impl HostedUp {
    /// The hosted frame: `{"wm_ai": {"from": …, "msg": …}}`.
    pub fn to_json(&self) -> String {
        UpEnvelope { wm_ai: self.clone() }.serialize_json()
    }

    /// `None` for frames that are not ours (the window manager's own
    /// `"wm"` envelope, anything else), over the size cap, or invalid.
    pub fn parse(json: &str) -> Option<HostedUp> {
        if json.len() > MAX_FRAME_BYTES || !json.contains("\"wm_ai\"") {
            return None;
        }
        let up = UpEnvelope::deserialize_json(json).ok()?.wm_ai;
        up.msg.validate().ok()?;
        Some(up)
    }
}

impl HostedDown {
    pub fn to_json(&self) -> String {
        DownEnvelope { wm_ai: self.clone() }.serialize_json()
    }

    pub fn parse(json: &str) -> Option<HostedDown> {
        if json.len() > MAX_FRAME_BYTES || !json.contains("\"wm_ai\"") {
            return None;
        }
        let down = DownEnvelope::deserialize_json(json).ok()?.wm_ai;
        down.msg.validate().ok()?;
        Some(down)
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

    fn ep(s: &str) -> EndpointId {
        EndpointId(s.to_string())
    }

    #[test]
    fn a_registration_round_trips_and_the_host_stamps_the_sender() {
        let up = HostedUp { from: None, msg: ServiceUp::Register { manifest: route(), port_tag: 7 } };
        let json = up.to_json();
        assert!(json.contains("\"wm_ai\""));
        let parsed = HostedUp::parse(&json).unwrap();
        assert_eq!(parsed, up);
        let stamped = HostedUp { from: Some(ep("e1")), ..parsed };
        assert_eq!(HostedUp::parse(&stamped.to_json()), Some(stamped));
    }

    #[test]
    fn every_up_and_down_variant_round_trips() {
        let ups = vec![
            ServiceUp::Register { manifest: route(), port_tag: 1 },
            ServiceUp::Result(ToolResult::ok("c1", "41 min", "planned").with_preview().with_data(r#"{"minutes":41}"#)),
            ServiceUp::Result(ToolResult::failed("c2", "no route")),
            ServiceUp::Result(ToolResult::refused("c3", "not a tool")),
            ServiceUp::Result(ToolResult::denied("c4", "no")),
            ServiceUp::Result(ToolResult::unavailable("c5", "loading")),
            ServiceUp::Result(ToolResult::cancelled("c6")),
            ServiceUp::Result(ToolResult::timed_out("c7", "60 s").with_disposition(Disposition::EndTurn)),
            ServiceUp::Progress { call_id: "c1".into(), note: "routing".into(), permille: 500 },
            ServiceUp::Context(ServiceContext { text: "[route] gps=…".into() }),
            ServiceUp::Unregister,
        ];
        for msg in ups {
            let f = HostedUp { from: Some(ep("e9")), msg };
            let json = f.to_json();
            assert_eq!(HostedUp::parse(&json).as_ref(), Some(&f), "{json}");
        }
        let downs = vec![
            ServiceDown::Registered { port_tag: 1, endpoint: ep("e9") },
            ServiceDown::Call(ServiceCall { call_id: "c1".into(), tool: "plan".into(), args: r#"{"to":"Utrecht"}"#.into() }),
            ServiceDown::Cancel { call_id: "c1".into() },
            ServiceDown::ChatOpen { open: true },
        ];
        for msg in downs {
            let f = HostedDown { to: Some(ep("e9")), msg };
            let json = f.to_json();
            assert_eq!(HostedDown::parse(&json).as_ref(), Some(&f), "{json}");
        }
    }

    #[test]
    fn foreign_oversized_and_invalid_frames_are_refused_before_use() {
        assert_eq!(HostedUp::parse(r#"{"wm":{"Close":{}}}"#), None);
        assert_eq!(HostedDown::parse(r#"{"wm":{"Adopted":{}}}"#), None);
        assert_eq!(HostedUp::parse("not json"), None);
        let huge = format!(
            "{{\"wm_ai\":{{\"from\":null,\"msg\":{{\"Unregister\":{{}}}}}},\"pad\":\"{}\"}}",
            "x".repeat(MAX_FRAME_BYTES)
        );
        assert_eq!(HostedUp::parse(&huge), None);
        let call = HostedDown {
            to: Some(ep("e1")),
            msg: ServiceDown::Call(ServiceCall { call_id: "c1".into(), tool: "plan".into(), args: "x".repeat(MAX_ARGS_BYTES + 1) }),
        };
        assert_eq!(HostedDown::parse(&call.to_json()), None, "oversized args are dropped, not truncated");
        let bad = HostedDown { to: None, msg: ServiceDown::Call(ServiceCall { call_id: "c 1".into(), tool: "Plan".into(), args: "{}".into() }) };
        assert_eq!(HostedDown::parse(&bad.to_json()), None);
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
        let mut not_object = route();
        not_object.tools[0].parameters = r#"{"type":"string"}"#.into();
        assert!(not_object.validate().unwrap_err().contains("argument object"));
        let mut brief = route();
        brief.brief = "x".repeat(MAX_BRIEF_BYTES + 1);
        assert!(brief.validate().unwrap_err().contains("brief"));
    }

    #[test]
    fn a_result_is_bounded_without_splitting_characters() {
        let mut r = ToolResult::ok("c", "é".repeat(MAX_RESULT_BYTES), "n".repeat(MAX_NOTE_BYTES + 10))
            .with_data("d".repeat(MAX_DATA_BYTES + 1));
        r.bound();
        assert!(r.text.len() <= MAX_RESULT_BYTES);
        assert!(r.text.ends_with("[truncated by the router]"));
        assert!(r.note.len() <= MAX_NOTE_BYTES);
        assert!(r.note.ends_with('…'));
        assert!(r.data.is_empty(), "oversized data is dropped whole");
    }

    #[test]
    fn opaque_ids_are_bounded_printable_and_whitespace_free() {
        assert!(is_opaque_id("c-12_ab"));
        assert!(!is_opaque_id(""));
        assert!(!is_opaque_id("c 1"));
        assert!(!is_opaque_id(&"x".repeat(MAX_ID_BYTES + 1)));
    }
}
