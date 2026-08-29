//! Durable KEYED chat sessions: one append-only JSONL file per
//! `(principal, client_key, context_key)` under `<root>/chat/`.
//!
//! Layout: `<root>/chat/<principal>/<client_key>/<context_key>.jsonl`, the
//! two keys path-encoded (`[A-Za-z0-9._-]` verbatim, anything else `%XX`,
//! so `ip:10.0.0.7` is `ip%3A10.0.0.7`). Keys are validated at the route
//! to the same charset the client enforces, so a key can never spell a
//! path component that walks anywhere.
//!
//! File shape, one JSON object per line:
//! - `{"k":"h","v":1,"session":…,"provider":…,"namespace":…,"profile":…,
//!   "client_key":…,"context_key":…,"created_ms":…}` — the header, first.
//! - `{"k":"m","role":"user|assistant|tool|system","text":…,"turn":N}` —
//!   one per history message, in order, as the session feeds its provider.
//! - `{"k":"s","reason":…}` — the session SEALED (fail-closed after an
//!   ambiguous tool continuation). Appended once; a resumed session with
//!   this line comes back sealed — a restart must never un-seal.
//! - `{"k":"p","resume":…}` — a provider-native resume id, when a provider
//!   ever exposes one (none does through the threaded wrapper today; the
//!   slot exists so the file format need not change).
//!
//! Append is one `write_all` + `sync_data` per turn's new tail; a crash
//! leaves at most one torn last line, which `load` drops. A full rewrite
//! (resume with a trimmed history, a provider change) goes through a temp
//! file and a rename. Only the owning session worker ever appends; the
//! retire path deletes under the same per-session lock the worker
//! appends under, so a Clear can never be resurrected by a late append.

use super::api::principal_str;
use crate::PrincipalId;
use makepad_asset_chat::context::ClientProfile;
use makepad_asset_chat::session::SessionId;
use makepad_asset_chat::wire::{ChatMessage, ChatRole, ProviderKind};
use makepad_asset_client::json::{self, obj, s, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const FORMAT_VERSION: i64 = 1;
const DIR: &str = "chat";
const EXT: &str = "jsonl";

/// The identity a durable conversation is stored under.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub owner: PrincipalId,
    pub client_key: String,
    pub context_key: String,
}

/// The header of a session file: what the conversation was last bound to.
#[derive(Clone, Debug)]
pub struct Header {
    pub session: SessionId,
    pub provider: ProviderKind,
    pub namespace: String,
    pub profile: ClientProfile,
    pub client_key: String,
    pub context_key: String,
    pub created_ms: u64,
}

/// A loaded session file.
#[derive(Clone, Debug)]
pub struct Persisted {
    pub header: Header,
    pub history: Vec<ChatMessage>,
    /// The highest turn any message was recorded under.
    pub turn: u64,
    /// The session sealed before it was persisted; it resumes sealed.
    pub sealed: Option<String>,
}

#[derive(Clone)]
pub struct ChatStore {
    dir: PathBuf,
}

impl ChatStore {
    pub fn new(root: &Path) -> ChatStore {
        ChatStore { dir: root.join(DIR) }
    }

    pub fn path(&self, key: &SessionKey) -> PathBuf {
        self.dir
            .join(principal_str(&key.owner))
            .join(encode_component(&key.client_key))
            .join(format!("{}.{EXT}", encode_component(&key.context_key)))
    }

    /// The persisted conversation for `key`. `Ok(None)` means exactly "no
    /// conversation exists"; a file that EXISTS but cannot be read or has
    /// no readable header is `Err` — corrupt is not missing, and treating
    /// it as missing would silently overwrite whatever the file held.
    pub fn load(&self, key: &SessionKey) -> Result<Option<Persisted>, String> {
        let bytes = match fs::read(self.path(key)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("transcript unreadable: {e}")),
        };
        load_bytes(&bytes).map(Some)
    }

    /// Delete the file for `key`. True when something was removed.
    #[cfg(test)]
    pub fn wipe(&self, key: &SessionKey) -> bool {
        let path = self.path(key);
        let removed = fs::remove_file(&path).is_ok();
        prune_empty_parents(&path, &self.dir);
        removed
    }

    /// Delete every persisted conversation about `context_key`, for every
    /// principal and client. Returns how many files went.
    pub fn drop_context(&self, context_key: &str) -> usize {
        let name = format!("{}.{EXT}", encode_component(context_key));
        let mut removed = 0;
        let Ok(principals) = fs::read_dir(&self.dir) else {
            return 0;
        };
        for principal in principals.flatten() {
            let Ok(clients) = fs::read_dir(principal.path()) else {
                continue;
            };
            for client in clients.flatten() {
                let path = client.path().join(&name);
                if fs::remove_file(&path).is_ok() {
                    removed += 1;
                    prune_empty_parents(&path, &self.dir);
                }
            }
        }
        removed
    }

    /// Every persisted key on disk.
    #[cfg(test)]
    pub fn keys(&self) -> Vec<SessionKey> {
        let mut out = Vec::new();
        let Ok(principals) = fs::read_dir(&self.dir) else {
            return out;
        };
        for principal in principals.flatten() {
            let Some(owner) = principal.file_name().to_str().and_then(super::api::parse_principal) else {
                continue;
            };
            let Ok(clients) = fs::read_dir(principal.path()) else {
                continue;
            };
            for client in clients.flatten() {
                let Some(client_key) = client.file_name().to_str().and_then(decode_component)
                else {
                    continue;
                };
                let Ok(files) = fs::read_dir(client.path()) else {
                    continue;
                };
                for file in files.flatten() {
                    let name = file.file_name();
                    let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(&format!(".{EXT}")))
                    else {
                        continue;
                    };
                    if let Some(context_key) = decode_component(stem) {
                        out.push(SessionKey {
                            owner,
                            client_key: client_key.clone(),
                            context_key,
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| {
            (principal_str(&a.owner), &a.client_key, &a.context_key)
                .cmp(&(principal_str(&b.owner), &b.client_key, &b.context_key))
        });
        out
    }
}

/// The one writer of a session file: the session's worker. Holds the path
/// and how much of the session's history is already on disk. Both cursors
/// (`written`, `sealed_written`) advance ONLY after the corresponding
/// write succeeded, so a failed write is retried by the next sync rather
/// than silently skipped.
pub struct SessionFile {
    path: PathBuf,
    header: Header,
    /// Messages already written (the file's message-line count).
    written: usize,
    /// The seal line is already on disk.
    sealed_written: bool,
    /// Set by a Clear / game retire: nothing may be written again, and the
    /// file is gone.
    wiped: bool,
}

impl SessionFile {
    pub fn new(path: PathBuf, header: Header) -> SessionFile {
        SessionFile { path, header, written: 0, sealed_written: false, wiped: false }
    }

    /// Write the whole file from scratch: header + every message (+ the
    /// seal, when the session is sealed). Used on resume (trimmed history,
    /// possibly a new provider) and when a pop shortened the history.
    pub fn rewrite(
        &mut self,
        history: &[ChatMessage],
        turn: u64,
        sealed: Option<&str>,
    ) -> std::io::Result<()> {
        if self.wiped {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = String::new();
        push_line(&mut text, &header_value(&self.header));
        for m in history {
            push_line(&mut text, &message_value(m, turn));
        }
        if let Some(reason) = sealed {
            push_line(&mut text, &sealed_value(reason));
        }
        let tmp = self.path.with_extension("tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_data()?;
        }
        fs::rename(&tmp, &self.path)?;
        self.written = history.len();
        self.sealed_written = sealed.is_some();
        Ok(())
    }

    /// Bring the file up to `history` (and the seal state): append the new
    /// tail, or rewrite if the history got shorter (a refused send pops
    /// its user row). A no-op when nothing changed, so callers may retry
    /// it every pump.
    pub fn sync(
        &mut self,
        history: &[ChatMessage],
        turn: u64,
        sealed: Option<&str>,
    ) -> std::io::Result<()> {
        let seal_pending = sealed.is_some() && !self.sealed_written;
        if self.wiped || (history.len() == self.written && !seal_pending) {
            return Ok(());
        }
        if history.len() < self.written || !self.path.exists() {
            return self.rewrite(history, turn, sealed);
        }
        let mut text = String::new();
        for m in &history[self.written..] {
            push_line(&mut text, &message_value(m, turn));
        }
        if let (true, Some(reason)) = (seal_pending, sealed) {
            push_line(&mut text, &sealed_value(reason));
        }
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        f.write_all(text.as_bytes())?;
        f.sync_data()?;
        self.written = history.len();
        if seal_pending {
            self.sealed_written = true;
        }
        Ok(())
    }

    /// Clear: forget the file and refuse every later write.
    pub fn wipe(&mut self, store_dir: &Path) -> bool {
        self.wiped = true;
        let removed = fs::remove_file(&self.path).is_ok();
        let _ = fs::remove_file(self.path.with_extension("tmp"));
        prune_empty_parents(&self.path, store_dir);
        removed
    }
}

impl ChatStore {
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

// ---------------------------------------------------------------- encoding

fn header_value(h: &Header) -> Value {
    obj(vec![
        ("k", s("h")),
        ("v", Value::Int(FORMAT_VERSION)),
        ("session", s(h.session.as_str())),
        ("provider", s(h.provider.slug())),
        ("namespace", s(h.namespace.clone())),
        ("profile", s(h.profile.slug())),
        ("client_key", s(h.client_key.clone())),
        ("context_key", s(h.context_key.clone())),
        ("created_ms", Value::Int(h.created_ms.min(i64::MAX as u64) as i64)),
    ])
}

fn message_value(m: &ChatMessage, turn: u64) -> Value {
    obj(vec![
        ("k", s("m")),
        ("role", s(m.role.slug())),
        ("text", s(m.text.clone())),
        ("turn", Value::Int(turn.min(i64::MAX as u64) as i64)),
    ])
}

fn sealed_value(reason: &str) -> Value {
    obj(vec![("k", s("s")), ("reason", s(reason.to_string()))])
}

fn push_line(out: &mut String, v: &Value) {
    out.push_str(&v.to_json());
    out.push('\n');
}

/// Parse a session file. The header line is load-bearing: a file whose
/// first line is not a valid current-version header is CORRUPT (or from a
/// newer format), never "no session" — the header is written atomically
/// (temp file + rename) so a crash cannot produce a headerless file, and
/// only later appended lines may be torn. Those torn/foreign tails are
/// skipped, not fatal: the conversation up to them is still the
/// conversation.
fn load_bytes(bytes: &[u8]) -> Result<Persisted, String> {
    let mut lines = bytes.split(|b| *b == b'\n');
    let first = lines.next().unwrap_or_default();
    let header_line = json::parse(first)
        .map_err(|_| "transcript corrupt: unreadable header line".to_string())?;
    let header = parse_header(&header_line)?;
    let mut history = Vec::new();
    let mut turn = 0u64;
    let mut sealed = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Ok(v) = json::parse(line) else {
            continue;
        };
        match v.get("k").and_then(Value::as_str) {
            Some("m") => {
                let Some(role) = v.get("role").and_then(Value::as_str).and_then(ChatRole::from_slug)
                else {
                    continue;
                };
                let Some(text) = v.get("text").and_then(Value::as_str) else {
                    continue;
                };
                let msg = ChatMessage::new(role, text);
                if msg.validate().is_err() {
                    continue;
                }
                turn = turn.max(v.get("turn").and_then(Value::as_u64).unwrap_or(0));
                history.push(msg);
            }
            Some("s") => {
                sealed = Some(
                    v.get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("sealed before restart")
                        .to_string(),
                );
            }
            // `p` (a provider-native resume id) is accepted and ignored: no
            // provider exposes one through the threaded wrapper today.
            _ => {}
        }
    }
    Ok(Persisted { header, history, turn, sealed })
}

fn parse_header(v: &Value) -> Result<Header, String> {
    if v.get("k").and_then(Value::as_str) != Some("h") {
        return Err("transcript corrupt: first line is not a header".to_string());
    }
    match v.get("v").and_then(Value::as_u64) {
        Some(version) if version == FORMAT_VERSION as u64 => {}
        Some(version) => {
            return Err(format!(
                "transcript format v{version} is not this server's v{FORMAT_VERSION}"
            ));
        }
        None => return Err("transcript corrupt: header has no version".to_string()),
    }
    let field = |value: Option<&str>, what: &str| -> Result<String, String> {
        value
            .map(str::to_string)
            .ok_or_else(|| format!("transcript corrupt: header field {what}"))
    };
    let session = SessionId::parse(&field(v.get("session").and_then(Value::as_str), "session")?)
        .ok_or("transcript corrupt: header session id")?;
    let provider =
        ProviderKind::from_slug(&field(v.get("provider").and_then(Value::as_str), "provider")?)
            .ok_or("transcript corrupt: header provider")?;
    let namespace = field(v.get("namespace").and_then(Value::as_str), "namespace")?;
    let profile =
        ClientProfile::from_slug(&field(v.get("profile").and_then(Value::as_str), "profile")?)
            .ok_or("transcript corrupt: header profile")?;
    let client_key = field(v.get("client_key").and_then(Value::as_str), "client_key")?;
    let context_key = field(v.get("context_key").and_then(Value::as_str), "context_key")?;
    let created_ms = v.get("created_ms").and_then(Value::as_u64).unwrap_or(0);
    Ok(Header { session, provider, namespace, profile, client_key, context_key, created_ms })
}

/// Path-safe spelling of a key: `[A-Za-z0-9._-]` verbatim, every other
/// byte `%XX` (uppercase hex). Keys are already restricted to
/// `[A-Za-z0-9._:@-]` at the route, so in practice only `:` and `@` are
/// escaped — and a key of only dots cannot exist (one alnum is required).
pub fn encode_component(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for b in key.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
pub fn decode_component(enc: &str) -> Option<String> {
    let bytes = enc.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = enc.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Remove the (now possibly empty) client and principal directories above
/// a deleted file, stopping at the store dir. Best effort.
fn prune_empty_parents(path: &Path, stop: &Path) {
    let mut cur = path.parent();
    while let Some(dir) = cur {
        if dir == stop || !dir.starts_with(stop) {
            break;
        }
        if fs::remove_dir(dir).is_err() {
            break;
        }
        cur = dir.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("mp_chat_store_{}_{n}_{name}", std::process::id()))
    }

    fn header(sid: &str, ck: &str, xk: &str) -> Header {
        Header {
            session: SessionId::parse(sid).unwrap(),
            provider: ProviderKind::FleetQwen,
            namespace: "gen".into(),
            profile: ClientProfile::Game,
            client_key: ck.into(),
            context_key: xk.into(),
            created_ms: 7,
        }
    }

    #[test]
    fn components_encode_path_safely_and_roundtrip() {
        for key in ["ip:10.0.0.7", "ip:fe80::1", "player-42", "ast_0123", "rik@n4.io", "a"] {
            let enc = encode_component(key);
            assert!(enc.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'%')), "{enc}");
            assert!(!enc.contains('/') && !enc.contains(':'), "{enc}");
            assert_eq!(decode_component(&enc).as_deref(), Some(key));
        }
        assert_eq!(encode_component("ip:10.0.0.7"), "ip%3A10.0.0.7");
        assert_eq!(decode_component("%4"), None);
        assert_eq!(decode_component("%zz"), None);
    }

    #[test]
    fn append_sync_load_wipe_and_torn_tail() {
        let root = tmp("roundtrip");
        let store = ChatStore::new(&root);
        let owner = PrincipalId([3u8; 16]);
        let key = SessionKey { owner, client_key: "ip:10.0.0.7".into(), context_key: "ast_1".into() };
        assert!(store.load(&key).unwrap().is_none());

        let mut file = SessionFile::new(store.path(&key), header("chat_00000000000000aa", "ip:10.0.0.7", "ast_1"));
        let mut history = vec![ChatMessage::new(ChatRole::User, "hi")];
        file.rewrite(&history, 1, None).unwrap();
        history.push(ChatMessage::new(ChatRole::Assistant, "hello"));
        file.sync(&history, 1, None).unwrap();
        history.push(ChatMessage::new(ChatRole::User, "more"));
        history.push(ChatMessage::new(ChatRole::Tool, r#"{"outcome":"ok","value":{}}"#));
        file.sync(&history, 2, None).unwrap();
        // Idempotent when nothing changed.
        file.sync(&history, 2, None).unwrap();

        let loaded = store.load(&key).unwrap().unwrap();
        assert_eq!(loaded.header.session.as_str(), "chat_00000000000000aa");
        assert_eq!(loaded.header.profile, ClientProfile::Game);
        assert_eq!(loaded.history, history);
        assert_eq!(loaded.turn, 2);
        assert_eq!(store.keys(), vec![key.clone()]);

        // A torn last line (crash mid-append) costs that line only.
        let path = store.path(&key);
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(br#"{"k":"m","role":"assistant","text":"partial"#);
        fs::write(&path, &bytes).unwrap();
        let loaded = store.load(&key).unwrap().unwrap();
        assert_eq!(loaded.history.len(), 4);

        // A shorter history (a popped user row) rewrites instead of
        // appending a duplicate.
        history.pop();
        history.pop();
        file.sync(&history, 2, None).unwrap();
        assert_eq!(store.load(&key).unwrap().unwrap().history.len(), 2);

        // Wipe: the file is gone, the writer refuses forever after, and
        // the empty directories above it are pruned.
        assert!(file.wipe(store.dir()));
        assert!(!path.exists());
        history.push(ChatMessage::new(ChatRole::User, "ghost"));
        file.sync(&history, 3, None).unwrap();
        assert!(!path.exists(), "a wiped file must never come back");
        assert!(store.load(&key).unwrap().is_none());
        assert!(store.keys().is_empty());
        assert!(!store.dir().join(principal_str(&owner)).exists());
    }

    #[test]
    fn drop_context_removes_every_client_and_principal_copy() {
        let root = tmp("dropctx");
        let store = ChatStore::new(&root);
        let mk = |owner: u8, ck: &str, xk: &str| {
            let key = SessionKey { owner: PrincipalId([owner; 16]), client_key: ck.into(), context_key: xk.into() };
            let mut f = SessionFile::new(store.path(&key), header("chat_00000000000000ab", ck, xk));
            f.rewrite(&[ChatMessage::new(ChatRole::User, "x")], 1, None).unwrap();
            key
        };
        let a = mk(1, "ip:10.0.0.1", "ast_game");
        let b = mk(1, "ip:10.0.0.2", "ast_game");
        let c = mk(2, "ip:10.0.0.1", "ast_game");
        let other = mk(1, "ip:10.0.0.1", "ast_other");
        assert_eq!(store.keys().len(), 4);
        assert_eq!(store.drop_context("ast_game"), 3);
        for gone in [&a, &b, &c] {
            assert!(store.load(gone).unwrap().is_none());
        }
        assert!(store.load(&other).unwrap().is_some());
        assert_eq!(store.drop_context("ast_game"), 0);
        assert_eq!(store.drop_context("nope"), 0);
    }

    #[test]
    fn a_corrupt_or_foreign_header_is_an_error_never_a_missing_session() {
        let root = tmp("noheader");
        let store = ChatStore::new(&root);
        let key = SessionKey { owner: PrincipalId([9u8; 16]), client_key: "c".into(), context_key: "x".into() };
        let path = store.path(&key);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // A file that EXISTS but has no readable header is corrupt: the
        // caller must refuse the resume, never treat it as "no
        // conversation" and overwrite it.
        fs::write(&path, b"{\"k\":\"m\",\"role\":\"user\",\"text\":\"orphan\"}\n").unwrap();
        assert!(store.load(&key).is_err());
        fs::write(&path, b"not json at all\n").unwrap();
        assert!(store.load(&key).is_err());
        fs::write(&path, b"").unwrap();
        assert!(store.load(&key).is_err(), "an empty file is torn, not missing");
        // A NEWER format version is refused with its version named.
        fs::write(&path, b"{\"k\":\"h\",\"v\":99,\"session\":\"chat_00000000000000aa\"}\n").unwrap();
        let err = store.load(&key).unwrap_err();
        assert!(err.contains("v99"), "{err}");
        assert!(store.wipe(&key));
        assert!(!store.wipe(&key));
    }

    #[test]
    fn a_seal_persists_across_load_and_appends_exactly_once() {
        let root = tmp("sealed");
        let store = ChatStore::new(&root);
        let key = SessionKey { owner: PrincipalId([5u8; 16]), client_key: "c".into(), context_key: "x".into() };
        let mut file = SessionFile::new(store.path(&key), header("chat_00000000000000ac", "c", "x"));
        let mut history = vec![ChatMessage::new(ChatRole::User, "hi")];
        file.rewrite(&history, 1, None).unwrap();
        assert_eq!(store.load(&key).unwrap().unwrap().sealed, None);
        // The session seals: one sync writes the seal line, later syncs
        // with nothing new write nothing again.
        file.sync(&history, 1, Some("unresolved tool continuation")).unwrap();
        file.sync(&history, 1, Some("unresolved tool continuation")).unwrap();
        let loaded = store.load(&key).unwrap().unwrap();
        assert_eq!(loaded.sealed.as_deref(), Some("unresolved tool continuation"));
        assert_eq!(loaded.history.len(), 1);
        let text = fs::read_to_string(store.path(&key)).unwrap();
        assert_eq!(text.matches("\"k\":\"s\"").count(), 1, "{text}");
        // A rewrite (shrunken history) carries the seal too.
        history.push(ChatMessage::new(ChatRole::Assistant, "a"));
        file.sync(&history, 2, Some("unresolved tool continuation")).unwrap();
        history.pop();
        file.sync(&history, 2, Some("unresolved tool continuation")).unwrap();
        let loaded = store.load(&key).unwrap().unwrap();
        assert_eq!(loaded.sealed.as_deref(), Some("unresolved tool continuation"));
        assert!(file.wipe(store.dir()));
    }
}
