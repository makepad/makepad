//! The module contract (aicontrol.md §3): what an app crate exports when it
//! can be hosted IN-PROCESS — a tile in the desktop window manager, a tile
//! of the web superbuild, the mobile WM later — with every instance in its
//! own splash isolate.
//!
//! Three things make up the contract, and only types live here:
//!
//! - [`AppModule`]: the crate's static description of itself — id, label,
//!   how to register its widget families into an isolate, what `create`
//!   accepts ([`OpenSchema`]), the host capabilities it needs — and
//!   `create`, which builds one instance inside an isolate the host has
//!   already prepared. Everything that touches the isolate happens inside
//!   the host's `with_script_vm_id` call; a module never sees a second
//!   `&mut Cx` beside the VM, the heap-corrupting mistake the isolate API
//!   warns about.
//! - [`InstanceHandles`]: what the host OWNS and hands the instance — the
//!   scope token every long-lived operation carries, the storage jail, the
//!   viewport, the reply sink for calls that finish later. Never a path,
//!   never a socket, never a process.
//! - [`InstanceParts`]: what an instance IS to the host — its root widget,
//!   its [`ServiceExecutor`] (the tools the assistant may call, addressed
//!   on their own because a widget cannot lend out the state route's tools
//!   need), and its shutdown.
//!
//! Hosting one is the host's job (the WM's `module_host.rs`): allocate the
//! isolate, apply the theme, `register`, `create`, seat the root in a tile,
//! bridge the executor onto the AI bus, and tear the instance down in the
//! order §3 lists. A module is trusted native code: the isolate bounds its
//! SCRIPT, the host's grant bounds its capabilities, and the rule that it
//! never touches the filesystem, processes, sockets or threads directly is
//! what makes one module run in every host, the web included.

pub use makepad_ai_services;
pub use makepad_widgets;

use makepad_ai_services::wire::{ServiceCall, ServiceManifest, ToolResult};
use makepad_strict_json as json;
use makepad_widgets::makepad_platform::storage::StorageHandle;
use makepad_widgets::*;
use std::fmt;
use std::sync::mpsc::{channel, Receiver, Sender};

/// What an app crate exports when it can be hosted in-process.
pub trait AppModule: Sync + 'static {
    /// The registry id: `sheets`, `photos`, `files`.
    fn id(&self) -> &'static str;
    /// What a person calls it.
    fn label(&self) -> &'static str;
    /// Register this module's widget families and root type into the
    /// isolate. The common widget universe and the host's theme are ALREADY
    /// there when this runs; a module never re-runs the widgets' own
    /// `script_mod`.
    fn register(&self, vm: &mut ScriptVm);
    /// The versioned schema of what [`create`](Self::create) accepts. The
    /// host validates raw arguments against it (see [`OpenSchema::validate`])
    /// and hands over a [`ValidatedOpen`]; a module never sees argv or a raw
    /// path — a file arrives as a [`ScopedHandle`] the host issued.
    fn open_schema(&self) -> OpenSchema;
    /// Build one instance INSIDE the isolate `vm` is (the host calls this
    /// under `with_script_vm_id`). `handles` are owned host handles; keep
    /// what the instance needs, drop the rest.
    fn create(&self, vm: &mut ScriptVm, open: ValidatedOpen, handles: InstanceHandles) -> InstanceParts;
    /// What this app needs from the host: `storage`, `audio.output`,
    /// `location`, `clipboard`, `net`… A trusted-code declaration the
    /// host's grant checks; the isolate does not enforce it.
    fn capabilities(&self) -> &'static [&'static str];
}

/// The owner token of one instance. Every lease the host opens for the
/// instance — a timer, a network request, an audio lane, a native layer —
/// carries it, so teardown can find and end them and a late callback can be
/// told apart from a live one. The generation makes a reused id unequal to
/// the instance that had it before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InstanceScope {
    id: u64,
    generation: u64,
}

impl InstanceScope {
    pub fn new(id: u64, generation: u64) -> Self {
        InstanceScope { id, generation }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl fmt::Display for InstanceScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "i{}g{}", self.id, self.generation)
    }
}

/// The tile the instance draws in, as the host knows it: what
/// `AdaptiveView` should size against instead of the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport {
    pub size: DVec2,
}

/// Where an executor that answered [`ExecOutcome::Pending`] sends the
/// result when it has it. Cloneable, so a worker can hold one; the host
/// drains the other end on its own events.
#[derive(Clone)]
pub struct ReplySink {
    tx: Sender<ToolResult>,
}

impl ReplySink {
    pub fn pair() -> (ReplySink, Receiver<ToolResult>) {
        let (tx, rx) = channel();
        (ReplySink { tx }, rx)
    }

    /// Hand a finished result to the host. Nothing happens when the host
    /// side is gone (the instance was torn down).
    pub fn reply(&self, result: ToolResult) {
        let _ = self.tx.send(result);
    }
}

/// Everything the host hands one instance at `create`. Owned handles, no
/// `Cx`: the VM the host is running `create` in already carries it.
pub struct InstanceHandles {
    pub scope: InstanceScope,
    /// The instance's storage jail: a namespace of the Cx storage API, on
    /// disk natively and in the browser's store on the web.
    pub storage: StorageHandle,
    pub viewport: Viewport,
    pub replies: ReplySink,
}

/// How a call ended, from the executor's side.
pub enum ExecOutcome {
    Done(ToolResult),
    /// The work continues (a worker, a network round trip); the result
    /// reaches the host through the [`ReplySink`] it gave the instance.
    Pending,
}

/// The instance's tools, as the assistant calls them. Executors are
/// addressed on their own — separately from the root widget — because the
/// tools of an app like route need simultaneous mutable access to trip,
/// map and marker state a generic widget reference cannot lend out.
pub trait ServiceExecutor {
    /// The manifest the host registers on the AI bus for this instance.
    fn manifest(&self) -> ServiceManifest;
    /// Run one call. `cx` is the host's; the executor may borrow its own
    /// widgets through it but must not run isolate script outside the
    /// host's `with_script_vm_id`.
    fn execute(&mut self, cx: &mut Cx, call: &ServiceCall) -> ExecOutcome;
    /// The person or the router gave up on this call; stop if you can.
    fn cancel(&mut self, _cx: &mut Cx, _call_id: &str) {}
    /// The host's chat surface came up or went away.
    fn chat_open(&mut self, _cx: &mut Cx, _open: bool) {}
}

/// Everything one instance is, as the host keeps it.
pub struct InstanceParts {
    pub root: WidgetRef,
    pub executor: Box<dyn ServiceExecutor>,
    /// Runs inside the isolate right before the host frees it: the
    /// instance's last chance to flush state it owns. The root and the
    /// executor are dropped by the host after this.
    pub shutdown: Box<dyn FnOnce(&mut ScriptVm)>,
}

// ------------------------------------------------------------ open schema

/// The kinds an open argument may have. A file never arrives as a path:
/// `FileHandle` values are ids the host issued for files the person picked
/// or bookmarked, so the host's consent policy applies in every host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenArgKind {
    Text,
    Number,
    Bool,
    FileHandle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenArg {
    pub key: &'static str,
    pub kind: OpenArgKind,
    pub required: bool,
}

/// What [`AppModule::create`] accepts, versioned so an installed host and a
/// newer module can tell each other apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenSchema {
    pub version: u32,
    pub args: Vec<OpenArg>,
}

/// A file the host opened for the instance, by id. The host resolves it;
/// the module only ever holds the id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScopedHandle(pub String);

impl ScopedHandle {
    /// A well-formed handle id: `h` followed by digits.
    pub fn is_well_formed(id: &str) -> bool {
        id.len() > 1 && id.starts_with('h') && id[1..].bytes().all(|b| b.is_ascii_digit())
    }
}

/// Arguments the host validated against the module's schema: an object
/// with only the schema's keys, each of its kind, every file a handle the
/// host issued.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedOpen {
    pub version: u32,
    args: Vec<(String, json::Value)>,
    handles: Vec<ScopedHandle>,
}

impl OpenSchema {
    pub fn new(version: u32) -> Self {
        OpenSchema { version, args: Vec::new() }
    }

    pub fn arg(mut self, key: &'static str, kind: OpenArgKind, required: bool) -> Self {
        self.args.push(OpenArg { key, kind, required });
        self
    }

    /// The empty open: no arguments at all. Every schema accepts it when it
    /// has no required argument.
    pub fn empty_open(&self) -> Result<ValidatedOpen, String> {
        self.validate("{}", &[])
    }

    /// Check raw JSON against the schema. Unknown keys, wrong kinds, a
    /// missing required argument, a non-object, or a file handle the host
    /// did not issue are refused with the reason.
    pub fn validate(&self, raw_json: &str, issued: &[ScopedHandle]) -> Result<ValidatedOpen, String> {
        let fields = match json::parse(raw_json.as_bytes()) {
            Ok(json::Value::Obj(fields)) => fields,
            Ok(_) => return Err("open arguments must be a JSON object".into()),
            Err(e) => return Err(format!("open arguments are not JSON: {e}")),
        };
        let mut args = Vec::new();
        let mut handles = Vec::new();
        for (key, value) in fields {
            let Some(arg) = self.args.iter().find(|a| a.key == key) else {
                return Err(format!("`{key}` is not an argument this app opens with"));
            };
            let ok = match (arg.kind, &value) {
                (OpenArgKind::Text, json::Value::Str(_)) => true,
                (OpenArgKind::Number, json::Value::Int(_) | json::Value::F64(_)) => true,
                (OpenArgKind::Bool, json::Value::Bool(_)) => true,
                (OpenArgKind::FileHandle, json::Value::Str(id)) => {
                    if !ScopedHandle::is_well_formed(id) || !issued.iter().any(|h| h.0 == *id) {
                        return Err(format!("`{key}` names a file handle the host did not issue"));
                    }
                    handles.push(ScopedHandle(id.clone()));
                    true
                }
                _ => false,
            };
            if !ok {
                return Err(format!("`{key}` has the wrong kind (expected {:?})", arg.kind));
            }
            args.push((key, value));
        }
        for arg in self.args.iter().filter(|a| a.required) {
            if !args.iter().any(|(k, _)| k == arg.key) {
                return Err(format!("`{}` is required to open this app", arg.key));
            }
        }
        Ok(ValidatedOpen { version: self.version, args, handles })
    }
}

impl ValidatedOpen {
    pub fn text(&self, key: &str) -> Option<&str> {
        self.args.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.as_str())
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        self.args.iter().find(|(k, _)| k == key).and_then(|(_, v)| match v {
            json::Value::Int(i) => Some(*i as f64),
            json::Value::F64(f) => Some(*f),
            _ => None,
        })
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.args.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.as_bool())
    }

    pub fn handle(&self, key: &str) -> Option<&ScopedHandle> {
        let id = self.text(key)?;
        self.handles.iter().find(|h| h.0 == id)
    }

    /// Every file handle the open carried, in argument order.
    pub fn handles(&self) -> &[ScopedHandle] {
        &self.handles
    }

    /// The arguments back as canonical JSON (for a log line, a relaunch).
    pub fn to_json(&self) -> String {
        json::Value::Obj(self.args.clone()).to_json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> OpenSchema {
        OpenSchema::new(1)
            .arg("file", OpenArgKind::FileHandle, false)
            .arg("sheet", OpenArgKind::Text, false)
            .arg("row", OpenArgKind::Number, false)
            .arg("readonly", OpenArgKind::Bool, false)
    }

    #[test]
    fn scope_tokens_tell_generations_apart() {
        let a = InstanceScope::new(3, 1);
        let b = InstanceScope::new(3, 2);
        assert_ne!(a, b, "a reused id is a different instance");
        assert_eq!(a.id(), b.id());
        assert_eq!(a.to_string(), "i3g1");
    }

    #[test]
    fn the_schema_accepts_only_its_own_keys_of_the_right_kind() {
        let s = schema();
        let issued = vec![ScopedHandle("h7".into())];
        let open = s
            .validate(r#"{"file":"h7","sheet":"Budget","row":4,"readonly":true}"#, &issued)
            .expect("a valid open");
        assert_eq!(open.version, 1);
        assert_eq!(open.text("sheet"), Some("Budget"));
        assert_eq!(open.number("row"), Some(4.0));
        assert_eq!(open.bool("readonly"), Some(true));
        assert_eq!(open.handle("file"), Some(&ScopedHandle("h7".into())));
        assert_eq!(open.handles().len(), 1);
        assert!(open.to_json().contains(r#""sheet":"Budget""#));
        // Refusals, each with its reason.
        assert!(s.validate(r#"{"path":"/etc/passwd"}"#, &issued).unwrap_err().contains("not an argument"));
        assert!(s.validate(r#"{"row":"four"}"#, &issued).unwrap_err().contains("wrong kind"));
        assert!(s.validate(r#"[1,2]"#, &issued).unwrap_err().contains("object"));
        assert!(s.validate("nope", &issued).unwrap_err().contains("not JSON"));
        // A file must be a handle the host issued — never a path, never a guess.
        assert!(s.validate(r#"{"file":"/tmp/a.csv"}"#, &issued).unwrap_err().contains("did not issue"));
        assert!(s.validate(r#"{"file":"h8"}"#, &issued).unwrap_err().contains("did not issue"));
        // Required arguments are required; the empty open needs none here.
        let strict = OpenSchema::new(2).arg("file", OpenArgKind::FileHandle, true);
        assert!(strict.empty_open().unwrap_err().contains("required"));
        assert!(s.empty_open().is_ok());
    }

    #[test]
    fn a_reply_sink_reaches_its_receiver_and_survives_a_gone_host() {
        let (sink, rx) = ReplySink::pair();
        sink.reply(ToolResult::ok("c1", "done", ""));
        assert_eq!(rx.try_recv().unwrap().call_id, "c1");
        drop(rx);
        sink.reply(ToolResult::ok("c2", "late", ""));
    }
}
