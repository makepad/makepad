//! Parameter schemas shared by inspect consumers and their tool manifests.
//! Validation runs on the request thread, before a command can wait or mutate anything.

/// Top-level properties of a JSON-schema properties fragment. Nested properties and quoted
/// braces (descriptions, enums, anyOf) are skipped; the fragments are static trusted schemas.
pub fn properties(params: &str) -> Vec<(&str, &str)> {
    let mut fields = Vec::new();
    let (mut depth, mut quoted, mut escaped, mut start) = (0usize, false, false, 0usize);
    for (i, b) in params.bytes().enumerate() {
        if quoted {
            if escaped { escaped = false; }
            else if b == b'\\' { escaped = true; }
            else if b == b'"' { quoted = false; }
        } else {
            match b {
                b'"' => quoted = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth -= 1,
                b',' if depth == 0 => {
                    field(&params[start..i], &mut fields);
                    start = i + 1;
                }
                _ => {}
            }
        }
    }
    field(&params[start..], &mut fields);
    fields
}

fn field<'a>(s: &'a str, fields: &mut Vec<(&'a str, &'a str)>) {
    let s = s.trim();
    if let Some((key, _)) = s.strip_prefix('"').and_then(|s| s.split_once('"')) {
        fields.push((key, s));
    }
}

/// `full` belongs to the cmd envelope, which consumes it before any family dispatch.
pub fn validate<'a>(keys: impl IntoIterator<Item = &'a str>, params: &str) -> Result<(), String> {
    let properties = properties(params);
    for key in keys {
        if key != "full" && !properties.iter().any(|(known, _)| *known == key) {
            let mut known: Vec<_> = properties.iter().map(|(key, _)| *key).collect();
            if !known.contains(&"full") { known.push("full"); }
            return Err(format!("unknown parameter {key:?}; known: {}", known.join(", ")));
        }
    }
    Ok(())
}

macro_rules! params {
    ($($key:literal : $ty:literal),* $(,)?) => {
        concat!($("\"", $key, "\":{\"type\":\"", $ty, "\"},",)*)
    };
}

macro_rules! ui_common {
    () => { params!("app": "string", "machine": "string", "w": "string", "json": "boolean", "timeout_ms": "integer") };
}
macro_rules! selectors {
    () => { params!("q": "string", "node": "string", "role": "string", "class": "string", "path": "string", "state": "string", "states": "string", "text": "string", "text~": "string", "value": "string", "value~": "string", "name~": "string", "nth": "integer", "id": "string", "actions": "string") };
}

/// Each UI verb has one parameter definition, used for both validation and the manifest.
pub fn ui(verb: &str) -> Option<&'static str> {
    Some(match verb {
        "" => concat!(ui_common!(), selectors!(), params!("name": "string", "action": "string", "depth": "integer")),
        "find" => concat!(ui_common!(), selectors!(), params!("name": "string", "action": "string")),
        "act" => concat!(ui_common!(), selectors!(), params!("action": "string", "name": "string", "v": "string", "t": "string", "from": "integer", "to": "integer", "dx": "number", "dy": "number", "ms": "number", "hz": "number", "legs": "integer", "x": "number", "y": "number", "b": "integer", "mods": "string", "wait": "boolean", "if_user_seq": "integer")),
        "type" => concat!(ui_common!(), selectors!(), params!("name": "string", "action": "string", "t": "string", "from": "integer", "to": "integer", "wait": "boolean", "if_user_seq": "integer")),
        "wait" => concat!(ui_common!(), selectors!(), params!("name": "string", "action": "string", "gone": "boolean", "changed": "boolean", "count": "integer")),
        "snap" => concat!(ui_common!(), params!("name": "string")),
        "diff" => concat!(ui_common!(), params!("a": "string", "b": "string")),
        "speak" => concat!(ui_common!(), selectors!(), params!("name": "string", "action": "string", "since": "integer")),
        "lint" => concat!(ui_common!()),
        "oscheck" => concat!(ui_common!(), params!("act": "boolean")),
        _ => return None,
    }.trim_end_matches(','))
}

macro_rules! gpu_profile {
    ("frames") => { params!("n": "integer", "as": "string") };
    ("frame") => { params!("as": "string") };
    ("top") => { params!("by": "string", "frames": "string", "n": "integer", "as": "string") };
    ("ablate") => { params!("as": "string") };
    ("runs") => { params!("pass": "string", "n": "integer", "as": "string") };
    ("lanes") => { params!("width": "integer", "as": "string") };
    ("util") => { params!("frames": "string", "as": "string") };
    ("diff") => { params!("as": "string") };
}

/// HAL inspectors and the native sampler; full command words select the parameter schema.
pub fn inspect(words: &str) -> Option<&'static str> {
    Some(match words {
        "gpu" => "",
        "gpu device" => "",
        "gpu top" => params!("by": "string", "n": "integer"),
        "gpu idle" => params!("frames": "integer", "n": "integer"),
        "gpu snap" => "",
        "gpu diff" => "",
        "gpu path" => "",
        "hal pump" => "",
        "hal pace" => "",
        "hal shell" => "",
        "gpu prof start" => params!("s": "integer", "split": "integer", "hz": "number", "ablate": "string", "every": "integer", "as": "string"),
        "gpu prof stop" => params!("save": "string", "sid": "integer", "as": "string"),
        "gpu prof shader" => params!("save": "string", "as": "string"),
        "gpu prof status" => params!("as": "string"),
        "gpu prof frames" => concat!(params!("sid": "integer"), gpu_profile!("frames")),
        "gpu prof frame" => concat!(params!("sid": "integer"), gpu_profile!("frame")),
        "gpu prof top" => concat!(params!("sid": "integer"), gpu_profile!("top")),
        "gpu prof ablate" => concat!(params!("sid": "integer"), gpu_profile!("ablate")),
        "gpu prof runs" => concat!(params!("sid": "integer"), gpu_profile!("runs")),
        "gpu prof lanes" => concat!(params!("sid": "integer"), gpu_profile!("lanes")),
        "gpu prof util" => concat!(params!("sid": "integer"), gpu_profile!("util")),
        "gpu prof diff" => concat!(params!("sid": "integer"), gpu_profile!("diff")),
        "cap grab" => params!("session": "integer", "window": "integer", "timeout": "integer", "file": "string"),
        "cap list" => "",
        "cap save" => "",
        "cap scanout" => params!("png": "string", "timeout": "integer"),
        "prof" => params!("s": "integer", "hz": "integer", "n": "integer", "wall": "boolean", "ev": "string", "threads": "string", "raw": "string", "save": "boolean"),
        "why-slow" => params!("s": "integer", "n": "integer"),
        "cap frame" => params!("file": "string", "session": "integer", "window": "integer", "timeout": "integer"),
        "cap draws" => params!("file": "string", "session": "integer", "window": "integer", "timeout": "integer", "pass": "string", "owner": "string", "sort": "string", "n": "integer", "pixels": "boolean"),
        "cap draw" => params!("file": "string", "session": "integer", "window": "integer", "timeout": "integer"),
        "cap code" => params!("file": "string", "session": "integer", "window": "integer", "timeout": "integer"),
        "cap rt" => params!("file": "string", "session": "integer", "window": "integer", "timeout": "integer", "png": "string", "max": "integer", "mark": "boolean", "wire": "boolean"),
        "cap pixel" => params!("file": "string", "session": "integer", "window": "integer", "timeout": "integer", "pass": "string"),
        "cap overdraw" => params!("file": "string", "session": "integer", "window": "integer", "timeout": "integer", "png": "string", "max": "integer"),
        "cap mesh" => params!("file": "string", "session": "integer", "window": "integer", "timeout": "integer", "n": "integer"),
        "cap tex" => params!("file": "string", "session": "integer", "window": "integer", "timeout": "integer", "png": "string", "max": "integer"),
        "cap time" => params!("file": "string", "session": "integer", "window": "integer", "timeout": "integer", "n": "integer", "repeat": "integer"),
        _ => return None,
    }.trim_end_matches(','))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_reject_unknown_parameters() {
        let nested = r#""value":{"anyOf":[{"type":"string"},{"type":"number"}],"description":"braces }, comma, and \"quote\""},"count":{"type":"integer"}"#;
        assert!(validate(["value", "count", "full"], nested).is_ok());
        assert!(validate(["type"], nested).unwrap_err().contains("known: value, count, full"));
        for verb in ["", "find", "act", "type", "wait", "snap", "diff", "speak", "lint", "oscheck"] {
            let schema = ui(verb).unwrap();
            assert!(validate(["timeout_ms", "json", "w", "app", "machine"], schema).is_ok());
            assert!(validate(["timeout"], schema).unwrap_err().contains("timeout_ms"));
        }
    }

    #[test]
    fn sampler_rejects_unknown_parameters() {
        let prof = inspect("prof").unwrap();
        assert!(validate(["s", "hz", "n", "wall", "ev", "threads", "raw", "save"], prof).is_ok());
        assert!(validate(["seconds"], prof).unwrap_err().contains("known: s, hz"));
        let slow = inspect("why-slow").unwrap();
        assert!(validate(["s", "n"], slow).is_ok());
        assert!(validate(["hz"], slow).is_err());
    }
}

/// JIT-owned inspect commands. Host and program providers validate their own schemas.
pub fn jit(cmd: &str, sub: &str) -> Option<&'static str> {
    Some(match (cmd, sub) {
        ("session" | "pin" | "help" | "ping" | "threads" | "cpu" | "waits" | "vms" | "panics" | "pause" | "resume" | "rm" | "continue" | "versions" | "cache" | "frame", _) => "",
        ("inputs", _) => params!("match": "string", "of": "string"),
        ("hold", _) => params!("files": "string", "dir": "string"),
        ("why-hung" | "hang", _) => params!("probe_ms": "integer", "depth": "integer"),
        ("capture", _) => params!("why": "string"),
        ("why-slow" | "counters" | "contention", _) => params!("s": "number", "n": "integer"),
        ("prof", _) => params!("s": "number", "hz": "integer", "n": "integer", "ev": "string", "wall": "boolean", "threads": "string", "view": "string", "focus": "string", "fn": "string", "min": "number", "save": "boolean"),
        ("top", _) => params!("by": "string", "s": "number", "hz": "integer", "n": "integer", "ev": "string", "wall": "boolean", "threads": "string", "view": "string", "focus": "string", "fn": "string", "min": "number", "save": "boolean", "host": "integer", "zone": "string", "depth": "integer", "group": "string"),
        ("timeline", "start") => params!("max_s": "integer"),
        ("timeline", "frames") => params!("n": "integer"),
        ("timeline", "waits" | "contention") => params!("s": "number", "n": "integer"),
        ("timeline", _) => "",
        ("frames", _) => params!("n": "integer"),
        ("hitch", _) => params!("ms": "integer"),
        ("rec", "save") => params!("last": "string", "path": "string"),
        ("rec", _) => "",
        ("watch", _) => params!("on": "string", "since": "integer", "timeout_s": "integer"),
        ("events", _) => params!("on": "string", "since": "integer", "follow": "boolean"),
        ("engine-requests" | "live", _) => params!("after": "integer"),
        ("engine-reply", _) => params!("id": "string", "status": "string", "message": "string"),
        ("memory", _) => params!("compiler": "boolean", "n": "integer"),
        ("mem", "snap") => params!("compiler": "boolean"),
        ("mem", "timeline") => params!("from_ms": "integer", "to_ms": "integer"),
        ("mem", _) => "",
        ("diff", _) => params!("n": "integer"),
        ("heap", "start") => params!("host": "integer", "life": "integer", "rate": "integer", "ttl_s": "integer"),
        ("heap", "top") => params!("host": "integer", "by": "string", "n": "integer", "depth": "integer", "zone": "string", "group": "string"),
        ("heap", "frame" | "rate") => params!("host": "integer", "s": "number", "n": "integer"),
        ("heap", "life") => params!("host": "integer", "n": "integer", "ms": "integer", "file": "string"),
        ("heap", "diff") => params!("host": "integer", "n": "integer", "depth": "integer"),
        ("heap" | "path", _) => params!("host": "integer"),
        ("jit", "queue") => params!("n": "integer"),
        ("jit", _) => params!("ms": "integer"),
        ("owner", _) => params!("since": "integer", "n": "integer"),
        ("stacks", _) => params!("thread": "string", "depth": "integer"),
        ("count" | "break" | "restartable", _) => params!("time": "boolean", "tier": "string"),
        ("probes", _) => params!("since": "integer"),
        ("asm", "top") => params!("native": "string", "build": "boolean", "s": "number", "hz": "integer", "n": "integer", "vs": "string", "ev": "string"),
        ("asm", _) => params!("native": "string", "build": "boolean", "at": "string", "s": "number", "hz": "integer", "ev": "string", "vs": "string", "view": "string", "n": "integer", "line": "string"),
        ("vsnative", _) => params!("file": "string", "pid": "string", "native": "string", "build": "boolean", "args": "string", "s": "number", "warm_ms": "integer", "hz": "integer", "n": "integer", "ev": "string", "tree": "integer", "min": "number", "roots": "string", "save": "boolean", "threads": "string", "nthreads": "string"),
        ("ab", _) => params!("a": "string", "b": "string", "rounds": "integer", "s": "number", "settle_ms": "integer", "hz": "integer", "metric": "string", "per": "string", "ev": "string", "keep": "string"),
        ("remarks" | "code" | "stale", _) => params!("n": "integer"),
        ("why-tier", "stuck" | "uncounted") => params!("n": "integer"),
        ("why-tier", _) => "",
        ("tier", _) => params!("fresh": "boolean"),
        ("probe", "add") => params!("code": "string", "mutate": "boolean", "tier": "string"),
        ("probe", "read") => params!("n": "integer"),
        ("probe", _) => "",
        ("writes", sub) if sub.strip_prefix('W').is_some_and(|id| id.parse::<u32>().is_ok()) => params!("n": "integer"),
        ("writes", _) => "",
        ("eval", _) => params!("in": "string", "panic": "string", "mutate": "boolean", "expr": "string"),
        ("static", _) => params!("debug": "boolean"),
        ("locals", _) => params!("panic": "string", "break": "string", "thread": "string", "frame": "integer"),
        _ => return None,
    }.trim_end_matches(','))
}

/// Structural design operations, selected before queuing a UI edit.
pub const DESIGN: &[(&str, &str)] = &[
    ("open", r#""path":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("delete", r#""path":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("duplicate", r#""path":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("wrap", r#""path":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("up", r#""path":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("down", r#""path":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("out", r#""path":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("insert", r#""path":{"type":"string"},"type":{"type":"string"},"body":{"type":"string"},"place":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("move", r#""path":{"type":"string"},"to":{"type":"string"},"place":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("rename", r#""path":{"type":"string"},"name":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("set", r#""path":{"type":"string"},"key":{"type":"string"},"value":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("close", r#""if_user_seq":{"type":"integer"}"#),
    ("state", r#""#),
    ("palette", r#""#),
    ("undo", r#""if_user_seq":{"type":"integer"}"#),
    ("redo", r#""if_user_seq":{"type":"integer"}"#),
    ("reset", r#""if_user_seq":{"type":"integer"}"#),
    ("patch", r#""#),
    ("commit", r#""#),
    ("bake", r#""if_user_seq":{"type":"integer"}"#),
    ("verify", r#""#),
];

pub fn design(words: &str) -> Option<&'static str> {
    DESIGN.iter().find(|(name, _)| *name == words).map(|(_, schema)| *schema)
}

/// Tweaker callback operations; op is consumed by the platform route.
pub const TWEAK_TOGGLE: &str = r#""on":{"type":"boolean"},"annotate":{"type":"boolean"},"if_user_seq":{"type":"integer"}"#;
pub const TWEAK_STATE: &str = r#""#;
pub const TWEAK_APPLY: &str = r#""path":{"type":"string"},"splash":{"type":"string"},"prop":{"type":"string"},"value":{"type":"string"},"uid":{"type":"integer"},"const":{"type":"string"},"reset":{"type":"boolean"},"if_user_seq":{"type":"integer"}"#;
pub const TWEAK_DIFF: &str = r#""#;
pub const TWEAK_CLEAR: &str = r#""if_user_seq":{"type":"integer"}"#;

pub const TWEAK: &[(&str, &str)] = &[
    ("toggle", TWEAK_TOGGLE),
    ("perf", r#""if_user_seq":{"type":"integer"}"#),
    ("state", TWEAK_STATE),
    ("undo", r#""if_user_seq":{"type":"integer"}"#),
    ("redo", r#""if_user_seq":{"type":"integer"}"#),
    ("census", r#""if_user_seq":{"type":"integer"}"#),
    ("reach", r#""if_user_seq":{"type":"integer"}"#),
    ("diff", TWEAK_DIFF),
    ("clear", TWEAK_CLEAR),
    ("final", TWEAK_FINAL),
    ("theme", r#""name":{"type":"string"},"value":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("pulse", r#""name":{"type":"string"},"m":{"type":"number"},"if_user_seq":{"type":"integer"}"#),
    ("states", r#""phase":{"type":"string"},"if_user_seq":{"type":"integer"}"#),
    ("motion", r#""open":{"type":"boolean"},"host":{"type":"integer"},"root":{"type":"integer"},"seek":{"type":"number"},"scale":{"type":"number"},"loop":{"type":"boolean"},"yoyo":{"type":"boolean"},"pause":{"type":"boolean"},"resume":{"type":"boolean"},"restart":{"type":"boolean"},"if_user_seq":{"type":"integer"}"#),
    ("apply", TWEAK_APPLY),
];

pub fn tweak(words: &str) -> Option<&'static str> {
    TWEAK.iter().find(|(name, _)| *name == words).map(|(_, schema)| *schema)
}

/// Splash inspect provider commands, checked before scheduling VM work.
pub fn script(cmd: &str, sub: &str) -> Option<&'static str> {
    Some(match (cmd, sub) {
        ("script", _) => params!("vm": "string"),
        ("src", _) => params!("vm": "string", "wait": "integer", "line": "string", "q": "string"),
        ("vm", "roots") => params!("vm": "string", "wait": "integer", "top": "integer"),
        ("vm", "get") => params!("vm": "string", "wait": "integer", "in": "string", "depth": "integer"),
        ("vm", _) => params!("vm": "string", "wait": "integer", "in": "string"),
        ("verbs", "on") => params!("verb": "string"),
        ("verbs", "off") => "",
        ("verbs", "top") => params!("verb": "string", "n": "integer", "by": "string"),
        ("verbs", _) => params!("verb": "string", "n": "integer", "by": "string"),
        ("sprobe", "add") => params!("code": "string"),
        ("sprobe", "read") => params!("n": "integer"),
        ("sprobe", _) => "",
        _ => return None,
    }.trim_end_matches(','))
}

/// Client-side frame requests, also advertised by the platform manifest.
pub const GRAB: &str = r#""w":{"type":"integer"},"scale":{"type":"number"},"out":{"type":"string"}"#;
pub const GSEQ: &str = r#""n":{"type":"integer"},"every_ms":{"type":"integer"},"scale":{"type":"number"},"w":{"type":"integer"},"out":{"type":"string"}"#;
pub const GQ: &str = r#""scale":{"type":"number"},"w":{"type":"integer"},"if_user_seq":{"type":"integer"}"#;
pub const REC: &str = r#""out":{"type":"string"},"seconds":{"type":"number"},"fps":{"type":"number"},"codec":{"type":"string"},"w":{"type":"integer"},"scale":{"type":"number"}"#;
pub const TWEAK_FINAL: &str = r#""w":{"type":"integer"},"scale":{"type":"number"}"#;
pub const REPLAY: &str = r#""home":{"type":"string"},"until":{"type":"string"},"show":{"type":"boolean"},"dry":{"type":"boolean"}"#;

/// Profiler stream queries (the CLI adds s/save/base/hz/ev before asking the query engine).
pub fn profile(cmd: &str, sub: &str) -> Option<&'static str> {
    Some(match (cmd, sub) {
        ("summary" | "frame" | "lanes" | "why-slow" | "diff" | "brief" | "stall" | "at" | "memory" | "help", _) => "",
        ("frames", _) => params!("n": "integer", "sort": "string"),
        ("top", "gpu") => concat!(gpu_profile!("top"), params!("range": "string")),
        ("top", "jit") => params!("n": "integer", "range": "string"),
        ("top", _) => params!("n": "integer", "range": "string", "lane": "string"),
        ("hitches", _) => params!("ms": "number", "n": "integer"),
        ("timeline", _) => params!("width": "integer", "lanes": "integer"),
        ("events" | "log" | "marks", _) => params!("cat": "string", "app": "boolean", "text": "string", "a": "integer", "range": "string", "n": "integer"),
        ("input", _) => params!("n": "integer"),
        ("gpu", "") => "",
        ("gpu", sub) => gpu_profile(sub)?,
        _ => return None,
    }.trim_end_matches(','))
}

pub const PROFILE_RECORD: &str = r#""s":{"type":"number"},"save":{"type":"string"},"base":{"type":"string"},"hz":{"type":"integer"},"ev":{"type":"string"}"#;

/// Recording queries shared by saved profiles, the stream client and live HAL queries.
/// The live dispatcher adds sid to select a session; a saved file has no session selector.
pub fn gpu_profile(sub: &str) -> Option<&'static str> {
    Some(match sub {
        "frames" => gpu_profile!("frames"),
        "frame" => gpu_profile!("frame"),
        "top" => gpu_profile!("top"),
        "ablate" => gpu_profile!("ablate"),
        "runs" => gpu_profile!("runs"),
        "lanes" => gpu_profile!("lanes"),
        "util" => gpu_profile!("util"),
        "diff" => gpu_profile!("diff"),
        _ => return None,
    }.trim_end_matches(','))
}
