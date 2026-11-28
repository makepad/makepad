//! Driving an app's remote commands from a command line: `makepad-remote app <pid|app|socket>
//! <command> [k=v ...]` and `makepad inspect <pid> <command> ...` are two front ends that only
//! parse argv and call [`run`]. Every command is a `cmd` request; the ones that are images are
//! answered here, client side, from the `frames` service, and the client writes the PNG files
//! (default folder `$TMPDIR/makepad-remote/<app>-<pid>/`) and prints their paths:
//!
//! - `grab [w=ID] [scale=F] [out=FILE]`, `tweak grab ...`: one frame
//! - `gseq [n=8] [every_ms=50] [scale=F] [w=ID] [out=DIR]`: n frames at a cadence
//! - `gq [scale=F] [w=ID] [if_user_seq=N]`: a frame of every window, then `quit`
//! - `tweak final`: the app's answer, plus `"png"` (the composited window) when the user drew
//! - `tools`: the app's AI services and their tools (the `app` service's `list`)
//! - `call <service>.<tool> [<instance>] [<json args>]`: one AI tool call (progress lines, then
//!   the result). Pictures are written to files and their paths come back in `image_files`.

use crate::discovery;
use crate::frames::{self, Record};
use crate::link::{Client, Error};
use makepad_strict_json::{self as json, Value};
use std::borrow::Cow;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Frames of one folder kept per window (older ones are deleted when a new one is written).
const KEPT_PER_WINDOW: usize = 64;
const KEPT_CALL_FILES: usize = 64;

/// The `app` service request (the app's AI tools) of a command line, when it is one: `tools`
/// (the service's `list`) or `call <service>.<tool> [<instance>] [<json args>]`, the arguments
/// being the rest of the line from its first `{`. Err: the usage of a `call` without its tool.
/// Every other command is the `cmd` service's (None).
pub fn app_request_of(line: &str) -> Option<Result<String, String>> {
    let (head, args) = match line.find('{') {
        Some(at) => (&line[..at], line[at..].trim()),
        None => (line, "{}"),
    };
    let mut words = head.split_whitespace();
    if matches!(words.clone().next(), Some("tools" | "call")) {
        if let Err(e) = crate::params::validate(words.clone().skip(1).filter_map(|w| w.split_once('=').map(|(k, _)| k)), "") {
            return Some(Err(e));
        }
    }
    match words.next()? {
        "tools" => Some(Ok("list".to_string())),
        "call" => {
            let Some(tool) = words.next() else {
                return Some(Err("call <service>.<tool> [<instance>] [<json args>]".into()));
            };
            let instance: Vec<&str> = words.collect();
            let head = if instance.is_empty() { format!("call {tool}") } else { format!("call {tool} {}", instance.join(" ")) };
            Some(Ok(format!("{head}\n{args}")))
        }
        _ => None,
    }
}

/// Words (the command, its sub-command) and `k=v` arguments of an argv.
pub fn split_args(argv: &[String]) -> (Vec<String>, Vec<(String, String)>) {
    let (mut words, mut kv) = (Vec::new(), Vec::new());
    for a in argv {
        match a.split_once('=') {
            Some((k, v)) if !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) => kv.push((k.trim_start_matches("--").to_string(), v.to_string())),
            _ => words.push(a.clone()),
        }
    }
    (words, kv)
}

/// The request line of a typed command's words. Code an agent passes (`eval <expr>`, `probe add
/// <hook> <code>`) keeps its text: it goes as a JSON request (`key=value` words split it
/// otherwise, and `=`/`=>` are lost). Every other command is its words.
pub fn typed_line(words: &[String]) -> String {
    if words.is_empty() {
        return "help".to_string();
    }
    let opt = |w: &str| -> Option<(String, String)> {
        let w = w.trim_start_matches("--");
        let (k, v) = w.split_once('=').unwrap_or((w, ""));
        matches!(k, "in" | "mutate" | "panic" | "domain").then(|| (k.to_string(), if v.is_empty() && k == "mutate" { "1".to_string() } else { v.to_string() }))
    };
    let json = |cmd: &str, kv: &[(String, String)]| {
        let mut s = format!("{{\"cmd\":{}", crate::json_str(cmd));
        for (k, v) in kv {
            s.push_str(&format!(",{}:{}", crate::json_str(k), crate::json_str(v)));
        }
        s.push('}');
        s
    };
    match words[0].as_str() {
        // the app service's: its JSON arguments are the rest of the line, as typed
        "call" | "tools" => words.join(" "),
        "eval" => {
            // options first (`--in path` or `in=path`, `mutate=1`, `panic=P1`), then the expression
            let mut kv = Vec::new();
            let mut i = 1;
            while i < words.len() {
                if words[i] == "--in" && i + 1 < words.len() {
                    kv.push(("in".to_string(), words[i + 1].clone()));
                    i += 2;
                } else if let Some(o) = words[i].contains('=').then(|| opt(&words[i])).flatten().or_else(|| (words[i] == "--mutate").then(|| ("mutate".to_string(), "1".to_string()))) {
                    kv.push(o);
                    i += 1;
                } else {
                    break;
                }
            }
            kv.push(("expr".to_string(), words[i..].join(" ")));
            json("eval", &kv)
        }
        "probe" if words.get(1).map(String::as_str) == Some("add") && words.len() > 3 => {
            let mut rest: Vec<String> = words[3..].to_vec();
            let mut kv = Vec::new();
            while rest.last().is_some_and(|w| w == "mutate=1" || w == "--mutate") {
                rest.pop();
                kv.push(("mutate".to_string(), "1".to_string()));
            }
            kv.push(("code".to_string(), rest.join(" ")));
            json(&format!("probe add {}", words[2]), &kv)
        }
        // a value with spaces (`q=Save as`): a plain line is split on them, a JSON one keeps them
        _ if words.iter().any(|w| w.contains('=') && w.contains(char::is_whitespace)) => {
            let (kv, cmd): (Vec<&String>, Vec<&String>) = words.iter().partition(|w| w.contains('='));
            let kv: Vec<(String, String)> = kv.iter().filter_map(|w| w.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
            json(&cmd.iter().map(|w| w.as_str()).collect::<Vec<_>>().join(" "), &kv)
        }
        _ => words.join(" "),
    }
}

/// The request line for the `cmd` service: one flat JSON object, so values keep their spaces.
pub fn request_line(words: &[String], kv: &[(String, String)]) -> String {
    let mut line = format!("{{\"cmd\":{}", crate::json_str(&words.join(" ")));
    for (k, v) in kv {
        // `cmd` is the request's own field; the Command-key modifier of an input command goes by `logo`
        let k = if k == "cmd" { "logo" } else { k };
        line.push_str(&format!(",{}:{}", crate::json_str(k), crate::json_str(v)));
    }
    line.push('}');
    line
}

fn get<'a>(kv: &'a [(String, String)], keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| kv.iter().find(|x| x.0 == *k).map(|x| x.1.as_str()))
}

/// `makepad-remote app <pid|app|socket> <command> [k=v ...]`: prints the answer's lines; exit
/// status 1 when the answer had an error, 2 when the app cannot be reached.
pub fn run_command_line(spec: &str, argv: &[String]) -> i32 {
    if let Some(("--reconnect", call)) = argv.split_first().map(|(flag, rest)| (flag.as_str(), rest)) {
        return match reconnect_call(spec, call, &mut |line| println!("{line}")) {
            Ok(false) => 0,
            Ok(true) => 1,
            Err(e) => { eprintln!("makepad-remote app: {e}"); 1 }
        }
    }
    // `<host>:<app|pid>` goes through the hub to an app on another machine (a phone that dials in)
    #[cfg(unix)]
    let routed = (spec.contains(':') && !spec.contains('/')).then(|| {
        let bin = std::env::var_os("RELAX_BIN").map(PathBuf::from);
        crate::hub::connect(bin.as_deref()).map(|hub| crate::Client::routed(&hub, spec))
    });
    #[cfg(not(unix))]
    let routed: Option<Result<crate::Client, String>> = None;
    let connected = match routed {
        Some(Ok(c)) => Ok(c),
        Some(Err(e)) => { eprintln!("makepad-remote app: {e}"); return 2; }
        None => crate::Client::connect_as(spec, &crate::discovery::Caller::command_line()),
    };
    let client = match connected {
        Ok(c) => c,
        Err(e) => {
            eprintln!("makepad-remote app: {e}");
            return 2;
        }
    };
    let argv = if argv.is_empty() { vec!["help".to_string()] } else { argv.to_vec() };
    match run_printed(&client, &argv, &mut |l| {
        println!("{l}");
        true
    }) {
        Ok(false) => 0,
        Ok(true) => 1,
        Err(e) => {
            eprintln!("makepad-remote app: {e}");
            2
        }
    }
}

/// [`run`] as a command line prints it: a tool result's inline pictures are written to files in
/// the app's grab folder and the line names them instead (`image_files`, [`call_image_line`]), so
/// a shell or a coding agent reads paths, not megabytes of base64. A library caller of [`run`]
/// (a test, the MCP bridge) keeps the inline `images`.
pub fn run_printed(client: &Client, argv: &[String], each: &mut dyn FnMut(&str) -> bool) -> Result<bool, Error> {
    let dir = std::cell::LazyCell::new(|| grab_dir(client));
    run(client, argv, &mut |l| each(&call_image_line(l, || (*dir).clone())))
}

/// Run a caller-declared replayable tool call. Only the endpoint file is stable across
/// restarts; never redial a cached socket or retry after receiving a definitive result.
pub fn reconnect_call(spec: &str, argv: &[String], print: &mut dyn FnMut(&str)) -> Result<bool, Error> {
    if !discovery::is_endpoint_file(spec) || argv.first().map(String::as_str) != Some("call") {
        return Err(Error::Io(io::Error::new(io::ErrorKind::InvalidInput, "--reconnect requires an endpoint file and a replayable call")))
    }
    let caller = discovery::Caller::command_line();
    replay_call(argv, print, || Client::connect_as(spec, &caller), || std::thread::sleep(std::time::Duration::from_secs(1)))
}

fn replay_call(argv: &[String], print: &mut dyn FnMut(&str), mut connect: impl FnMut() -> Result<Client, Error>, mut pause: impl FnMut()) -> Result<bool, Error> {
    loop {
        let run = connect().and_then(|client| {
            let mut outcome = None;
            let failed = run_printed(&client, argv, &mut |line| {
                if let Ok(value) = json::parse_depth(line.as_bytes(), json::TOOL_JSON_DEPTH) {
                    if value.get("Progress").is_some() { return true }
                    if let Some(result) = value.get("Result") {
                        let result = result.as_arr().and_then(|items| items.first()).unwrap_or(result);
                        let status = result.get("outcome");
                        if status.is_some_and(|s| s.get("Unavailable").is_some()) { return false }
                        outcome = Some(status.is_none_or(|s| s.get("Ok").is_none()));
                    }
                }
                print(line);
                outcome.get_or_insert(true);
                false
            })?;
            Ok(outcome.or(failed.then_some(true)))
        });
        match run {
            Ok(Some(failed)) => return Ok(failed),
            Ok(None) | Err(Error::LinkLost(_) | Error::Refused(crate::conn::CLOSE_LINK_LOST, _)) => pause(),
            Err(why) => return Err(why),
        }
    }
}

#[cfg(all(test, unix))]
#[test]
fn replay_a_held_call_after_disconnect_and_stop_on_its_result() {
    use crate::conn::{Open, CLOSE_NORMAL};
    use crate::endpoint::Endpoint;
    use std::{os::unix::net::UnixListener, sync::{mpsc, Arc}, time::Duration};

    let root = crate::carrier::unix::test_root().join(format!("replay-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let reference = root.join("endpoint");
    let serving = |name: &str| {
        let path = root.join(name);
        let listener = UnixListener::bind(&path).unwrap();
        let ep = Endpoint::new(name);
        let (tx, rx) = mpsc::channel();
        ep.register("app", 1, Arc::new(move |conn, open| {
            tx.send((conn.clone(), open)).unwrap();
            Ok(())
        }));
        let server = std::thread::spawn(move || {
            let sock = listener.accept().unwrap().0;
            ep.serve(crate::frame::FrameReader::new(Box::new(sock.try_clone().unwrap())),
                Box::new(sock.try_clone().unwrap()), Box::new(move || { let _ = sock.shutdown(std::net::Shutdown::Both); })).unwrap()
        });
        fs::write(&reference, path.to_str().unwrap()).unwrap();
        (server, rx)
    };
    let (first, opened) = serving("first.sock");
    let (lost, missing) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let run = |file: PathBuf, pause: &mut dyn FnMut()| {
        let mut output = Vec::new();
        let argv = ["call".into(), "sample.wait".into(), "{}".into()];
        let failed = replay_call(&argv, &mut |s| output.push(s.to_owned()), || {
            let path = fs::read_to_string(&file).map_err(|e| Error::LinkLost(e.to_string()))?;
            Client::connect(&path)
        }, pause).unwrap();
        (failed, output)
    };
    let file = reference.clone();
    let waiter = std::thread::spawn(move || run(file.clone(), &mut || {
        // The listener and reference are gone; resume only when their replacement exists.
        assert!(!file.exists());
        lost.send(()).unwrap();
        resumed.recv_timeout(Duration::from_secs(10)).unwrap();
    }));
    let (conn, request) = opened.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(request.args, "call sample.wait\n{}");
    fs::remove_file(&reference).unwrap();
    conn.close("server stopped");
    first.join().unwrap();
    missing.recv_timeout(Duration::from_secs(10)).unwrap();
    let (second, opened) = serving("second.sock");
    resume.send(()).unwrap();
    let (_, Open { channel, args, .. }) = opened.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(args, request.args);
    let success = r#"{"Result":[{"outcome":{"Ok":[]},"text":"pending line"}]}"#;
    channel.send(success.as_bytes().to_vec()).unwrap();
    channel.close(CLOSE_NORMAL, "done");
    assert_eq!(waiter.join().unwrap(), (false, vec![success.to_owned()]));
    second.join().unwrap();

    let (last, opened) = serving("last.sock");
    let file = reference.clone();
    let refused = std::thread::spawn(move || run(file, &mut || panic!("a definitive refusal was retried")));
    let (_, request) = opened.recv_timeout(Duration::from_secs(10)).unwrap();
    let ended = r#"{"Result":[{"outcome":{"Refused":[]},"text":"ended"}]}"#;
    request.channel.send(ended.as_bytes().to_vec()).unwrap();
    request.channel.close(CLOSE_NORMAL, "done");
    assert_eq!(refused.join().unwrap(), (true, vec![ended.to_owned()]));
    last.join().unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// [`run_line`] as a command line prints it ([`run_printed`]).
pub fn run_line_printed(client: &Client, line: &str, each: &mut dyn FnMut(&str) -> bool) -> Result<bool, Error> {
    let dir = std::cell::LazyCell::new(|| grab_dir(client));
    run_line(client, line, &mut |l| each(&call_image_line(l, || (*dir).clone())))
}

/// Like [`run`] for a request typed as one line (words split on spaces).
pub fn run_line(client: &Client, line: &str, each: &mut dyn FnMut(&str) -> bool) -> Result<bool, Error> {
    // a JSON request goes as it is (its values keep their spaces and `=`)
    if line.trim_start().starts_with('{') {
        let mut err = false;
        let mut lines = client.cmd(line.trim())?;
        while let Some(l) = lines.next() {
            let l = l?;
            err |= l.starts_with("{\"err\":");
            if !each(&l) {
                break;
            }
        }
        return Ok(err);
    }
    let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    run(client, &argv, each)
}

/// Runs one command: `each` gets every answer line (the `end` line left out; false stops the
/// reading). Ok(true) when the answer had an error.
pub fn run(client: &Client, argv: &[String], each: &mut dyn FnMut(&str) -> bool) -> Result<bool, Error> {
    run_inner(client, argv, each, false).map(|r| r.0)
}

/// As [`run`], also returning the `end` line of the answer (it carries the fields the app adds,
/// such as `"user_seq"`). The client-side commands have no end line of their own: the line of
/// a `ping` that follows them stands in, and `gq` has none (the app is gone).
pub fn run_end(client: &Client, argv: &[String], each: &mut dyn FnMut(&str) -> bool) -> Result<(bool, Option<String>), Error> {
    run_inner(client, argv, each, true)
}

/// Whether this side answers `argv` itself by reading the app's `frames` service (`grab`, `gseq`,
/// `rec` frames, `gq`, `tweak grab`, the picture of `tweak final`), as [`run`] does: the app has
/// no command for it, so a caller that relays commands to it runs these on a client of its own
/// ([`Client::routed`]).
pub fn client_side(argv: &[String]) -> bool {
    let (words, _) = split_args(argv);
    match (words.first().map(String::as_str), words.get(1).map(String::as_str)) {
        (Some("grab" | "gseq" | "gq"), _) | (Some("tweak"), Some("grab" | "final")) => true,
        (Some("rec"), s) => !matches!(s, Some("save" | "status" | "mark" | "on" | "off")),
        _ => false,
    }
}

fn run_inner(client: &Client, argv: &[String], each: &mut dyn FnMut(&str) -> bool, want_end: bool) -> Result<(bool, Option<String>), Error> {
    let (words, kv) = split_args(argv);
    let first = words.first().map(String::as_str).unwrap_or("help");
    let second = words.get(1).map(String::as_str);
    let reply = match (first, second) {
        ("grab", _) | ("tweak", Some("grab")) => grab(client, &kv),
        ("gseq", _) => gseq(client, &kv),
        // `rec save|status|mark|on|off` is the flight recorder (an app command); bare `rec` records frames
        ("rec", s) if !matches!(s, Some("save" | "status" | "mark" | "on" | "off")) => rec(client, &kv),
        ("gq", _) => gq(client, &kv),
        ("tweak", Some("final")) => return tweak_final(client, &words, &kv, each),
        ("tools" | "call", _) => {
            if let Err(e) = crate::params::validate(kv.iter().map(|(k, _)| k.as_str()), "") {
                return Ok((emit(Err(e), each), None));
            }
            return match app_request_of(&words.join(" ")) {
                Some(Ok(request)) => app_request(client, &request, each),
                Some(Err(usage)) => Ok((emit(Err(usage), each), None)),
                None => unreachable!("tools and call are app requests"),
            };
        }
        _ => return forward(client, &words, &kv, each),
    };
    let err = emit(reply, each);
    let end = if want_end && first != "gq" { ask_end(client, "ping") } else { None };
    Ok((err, end))
}

fn emit(reply: Result<String, String>, each: &mut dyn FnMut(&str) -> bool) -> bool {
    match reply {
        Ok(json) => {
            each(&json);
            json.starts_with("{\"err\":")
        }
        Err(msg) => {
            each(&format!("{{\"err\":{}}}", crate::json_str(&msg)));
            true
        }
    }
}

fn forward(client: &Client, words: &[String], kv: &[(String, String)], each: &mut dyn FnMut(&str) -> bool) -> Result<(bool, Option<String>), Error> {
    let line = if words.is_empty() { "help".to_string() } else { request_line(words, kv) };
    let mut err = false;
    let mut lines = client.cmd(&line)?;
    while let Some(l) = lines.next() {
        let l = l?;
        err |= l.starts_with("{\"err\":");
        if !each(&l) {
            break;
        }
    }
    Ok((err, lines.end().map(str::to_string)))
}

/// The printed form of a tool result: its inline pictures replaced with paths ([`run_printed`]).
/// Only a result with pictures needs a folder.
fn call_image_line(line: &str, dir: impl FnOnce() -> Result<PathBuf, String>) -> Cow<'_, str> {
    let Ok(Value::Obj(mut envelope)) = json::parse_depth(line.as_bytes(), json::TOOL_JSON_DEPTH) else { return Cow::Borrowed(line) };
    let result = envelope.iter_mut().find(|(key, _)| key == "Result").and_then(|(_, value)| match value {
        Value::Arr(items) => items.first_mut(),
        object => Some(object),
    });
    let Some(Value::Obj(fields)) = result else { return Cow::Borrowed(line) };
    let Some((key, Value::Arr(images))) = fields.iter_mut().find(|(key, _)| key == "images") else { return Cow::Borrowed(line) };
    if images.is_empty() {
        return Cow::Borrowed(line);
    }

    let mut files = Vec::new();
    let mut errors = Vec::new();
    if let Err(why) = dir().and_then(|dir| write_call_images(&dir, images, &mut files, &mut errors).map_err(|e| e.to_string())) {
        errors.extend((0..images.len()).map(|i| json::s(format!("image {i}: {why}"))));
    }
    *key = "image_files".into();
    *images = files;
    if !errors.is_empty() {
        fields.push(("image_errors".into(), Value::Arr(errors)));
    }
    Cow::Owned(Value::Obj(envelope).to_json())
}

fn call_file_number(path: &Path) -> Option<(u64, usize)> {
    if !matches!(path.extension()?.to_str()?, "png" | "jpg") {
        return None;
    }
    let (seq, index) = path.file_stem()?.to_str()?.strip_prefix("call-")?.split_once('-')?;
    Some((seq.parse().ok()?, index.parse().ok()?))
}

fn write_call_images(dir: &Path, images: &[Value], files: &mut Vec<Value>, errors: &mut Vec<Value>) -> io::Result<()> {
    let dir = std::path::absolute(dir)?;
    if dir.to_str().is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "picture folder path is not UTF-8"));
    }
    // Two CLI processes can receive pictures from the same app at once. Assign and prune under one lock.
    let lock = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join(".call-images.lock"))?;
    lock.lock()?;
    let mut existing = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if let Some(number) = call_file_number(&path) {
            if entry.file_type()?.is_file() {
                existing.push((number, path));
            }
        }
    }
    existing.sort_unstable();
    let last = existing.last().map_or(0, |((seq, _), _)| *seq);
    let seq = last.checked_add(1).ok_or_else(|| io::Error::other("call picture sequence exhausted"))?;
    for (i, image) in images.iter().enumerate() {
        match write_call_image(&dir, seq, i, image) {
            Ok(path) => {
                files.push(json::s(path.to_string_lossy()));
                existing.push(((seq, i), path));
            }
            Err(why) => errors.push(json::s(format!("image {i}: {why}"))),
        }
    }
    let expired = existing.len().saturating_sub(KEPT_CALL_FILES);
    for (_, path) in existing.into_iter().take(expired) {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

fn write_call_image(dir: &Path, seq: u64, index: usize, image: &Value) -> Result<PathBuf, String> {
    let data = image.as_str().ok_or("expected a base64 string")?;
    let ext = match crate::mcp::picture_media_type(data)? {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        _ => return Err("unsupported picture format".into()),
    };
    let bytes = makepad_base64::base64_decode(data.as_bytes()).map_err(|e| format!("invalid base64 ({e:?})"))?;
    let path = dir.join(format!("call-{seq:05}-{index}.{ext}"));
    let mut file = fs::File::create_new(&path).map_err(|e| format!("write {}: {e}", path.display()))?;
    if let Err(e) = file.write_all(&bytes) {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(format!("write {}: {e}", path.display()));
    }
    Ok(path)
}

/// One request of the `app` service (the app's AI tools): its answer lines, as they come. The
/// channel's end is the end of the answer; dropping it early cancels a call.
fn app_request(client: &Client, args: &str, each: &mut dyn FnMut(&str) -> bool) -> Result<(bool, Option<String>), Error> {
    use crate::conn::{Event, CLOSE_LINK_LOST, CLOSE_NORMAL};
    let ch = match client.open("app", args) {
        Ok(ch) => ch,
        Err(Error::Refused(_, why)) if why.starts_with("unsupported service") => {
            return Ok((emit(Err("this process serves no AI tools (no `app` service)".into()), each), None));
        }
        Err(e) => return Err(e),
    };
    let mut err = false;
    while let Some(ev) = ch.recv() {
        match ev {
            Event::Data(d) => {
                for line in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                    if !each(line) {
                        return Ok((err, None));
                    }
                }
            }
            Event::Closed(CLOSE_NORMAL, _) => break,
            Event::Closed(CLOSE_LINK_LOST, why) => return Err(Error::LinkLost(why)),
            Event::Closed(_, why) => {
                err = true;
                each(&format!("{{\"err\":{}}}", crate::json_str(&why)));
                break;
            }
            Event::Opened(_) => {}
        }
    }
    Ok((err, None))
}

/// The end line of a command's answer.
fn ask_end(client: &Client, line: &str) -> Option<String> {
    let mut lines = client.cmd(line).ok()?;
    for l in lines.by_ref() {
        l.ok()?;
    }
    lines.end().map(str::to_string)
}

/// The one answer line of a command (the first line), for the commands this module chains.
fn ask_one(client: &Client, line: &str) -> Result<String, String> {
    let mut lines = client.cmd(line).map_err(|e| e.to_string())?;
    let first = lines.next().transpose().map_err(|e| e.to_string())?.unwrap_or_default();
    for l in lines {
        l.map_err(|e| e.to_string())?;
    }
    Ok(first)
}

fn window_of(kv: &[(String, String)]) -> Option<usize> {
    get(kv, &["w", "window"]).and_then(|v| v.parse().ok())
}

fn scale_of(kv: &[(String, String)]) -> f64 {
    get(kv, &["scale"]).and_then(|v| v.parse().ok()).unwrap_or(1.0)
}

/// `$TMPDIR/makepad-remote/<app>-<pid>/`, created.
fn grab_dir(client: &Client) -> Result<PathBuf, String> {
    let ping = ask_one(client, "ping").unwrap_or_default();
    let f = discovery::fields(&ping);
    let field = |k: &str| f.iter().find(|x| x.0 == k).map(|x| x.1.clone());
    let app = field("app").and_then(|r| discovery::string(r.as_bytes(), &mut 0)).unwrap_or_else(|| "app".into());
    let name = match field("pid") {
        Some(pid) => format!("{app}-{pid}"),
        None => app,
    };
    // (a lease's scratch dir, which Windows' own temp dir does not follow: its holder fetches the file there)
    let dir = std::env::var_os("TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from).join("makepad-remote").join(name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("grab dir: {e}"))?;
    Ok(dir)
}

/// Writes `png` as the next `grab-w<window>-<seq>[-f<frame>-p<patch>].png` of `dir` (the frame
/// it was drawn in and the live patches applied by then, when the app says) and deletes the older
/// ones past [`KEPT_PER_WINDOW`], except the paths in `pinned`.
fn write_grab(dir: &Path, window: usize, png: &[u8], pinned: &[PathBuf], drawn: Option<(u64, u64)>) -> Result<PathBuf, String> {
    let prefix = format!("grab-w{window}-");
    let existing = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix))).collect())
            .unwrap_or_default()
    };
    let seq = |p: &Path| p.file_stem().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix(&prefix)).and_then(|n| n.split('-').next()).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
    let tag = drawn.map_or(String::new(), |(frame, patch)| format!("-f{frame}-p{patch}"));
    let mut next = existing(dir).iter().map(|p| seq(p)).max().unwrap_or(0) + 1;
    let path = loop {
        let path = dir.join(format!("{prefix}{next:05}{tag}.png"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => break path,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => next += 1,
            Err(e) => return Err(format!("grab write: {e}")),
        }
    };
    std::fs::write(&path, png).map_err(|e| format!("grab write: {e}"))?;
    let mut mine: Vec<PathBuf> = existing(dir).into_iter().filter(|p| !pinned.contains(p)).collect();
    if mine.len() > KEPT_PER_WINDOW {
        mine.sort();
        let drop = mine.len() - KEPT_PER_WINDOW;
        for old in mine.into_iter().take(drop) {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(path)
}

/// The frame a record was drawn in and the live patches applied by then, when the app says.
fn drawn(r: &Record) -> Option<(u64, u64)> {
    Some((r.num("frame")? as u64, r.num("patch")? as u64))
}

/// `{"png":path,"w":window,"sz":[w,h],"capture_ms":..,"encode_ms":..,"capture_kind":..[,"frame":n,"patch":n]}`
fn grab_json(r: &Record, path: &Path) -> String {
    format!(
        "{{\"png\":{},\"w\":{},\"sz\":[{},{}],\"capture_ms\":{:.3},\"encode_ms\":{:.3},\"capture_kind\":{}{}}}",
        crate::json_str(&path.display().to_string()),
        r.num("window").unwrap_or(0.0),
        r.num("w").unwrap_or(0.0),
        r.num("h").unwrap_or(0.0),
        r.num("capture_ms").unwrap_or(0.0),
        r.num("encode_ms").unwrap_or(0.0),
        crate::json_str(&r.text("capture_kind").unwrap_or_default()),
        drawn(r).map_or(String::new(), |(frame, patch)| format!(",\"frame\":{frame},\"patch\":{patch}")),
    )
}

/// One frame of `window`, written to `out` or the next file of the grab folder.
fn grab_one(client: &Client, window: Option<usize>, scale: f64, out: Option<&str>) -> Result<(Record, PathBuf), String> {
    let req = frames::Request { window, scale: Some(scale), raw: false, full_chroma: false, codec: None, n: Some(1), fps: None, cap: false };
    let mut got = None;
    frames::fetch(client, &req, &mut |r| {
        got = Some(r);
        Ok(())
    })?;
    let record = got.ok_or("the app sent no frame")?;
    let path = match out {
        Some(file) => {
            std::fs::write(file, &record.bytes).map_err(|e| format!("grab write: {e}"))?;
            PathBuf::from(file)
        }
        None => write_grab(&grab_dir(client)?, record.num("window").unwrap_or(0.0) as usize, &record.bytes, &[], drawn(&record))?,
    };
    Ok((record, path))
}

fn grab(client: &Client, kv: &[(String, String)]) -> Result<String, String> {
    crate::params::validate(kv.iter().map(|(k, _)| k.as_str()), crate::params::GRAB)?;
    let (record, path) = grab_one(client, window_of(kv), scale_of(kv), get(kv, &["out"]))?;
    Ok(grab_json(&record, &path))
}

/// `rec [out=DIR] [seconds=5] [fps=30] [codec=png|raw] [w=ID]`: capture by streaming. The app reads every
/// presented frame back and sends it here; this side writes `frame_<slot>.png` (or `.rgba`) into DIR and
/// counts the slots that never arrived, so a capture is checkable (`frames`, `missing`) and needs no
/// video encoder on the device. Encode the folder to a video with any encoder.
fn rec(client: &Client, kv: &[(String, String)]) -> Result<String, String> {
    crate::params::validate(kv.iter().map(|(k, _)| k.as_str()), crate::params::REC)?;
    let seconds: f64 = get(kv, &["seconds", "s"]).unwrap_or("5").parse().map_err(|_| "rec seconds must be a number".to_string())?;
    let fps: f64 = get(kv, &["fps"]).unwrap_or("30").parse().map_err(|_| "rec fps must be a number".to_string())?;
    let codec = get(kv, &["codec"]).unwrap_or("rle");
    if !matches!(codec, "rle" | "png" | "raw") { return Err("rec codec must be rle, png or raw".into()); }
    let raw = codec == "raw";
    let dir = match get(kv, &["out"]) {
        Some(d) => { std::fs::create_dir_all(d).map_err(|e| format!("rec out: {e}"))?; PathBuf::from(d) }
        None => grab_dir(client)?,
    };
    let req = frames::Request { window: window_of(kv), scale: Some(get(kv, &["scale"]).and_then(|v| v.parse().ok()).unwrap_or(0.5)), raw, full_chroma: false, codec: (codec == "rle").then_some("rle"), n: None, fps: Some(fps), cap: true };
    let started = std::time::Instant::now();
    let (mut count, mut first, mut last, mut size, mut dropped) = (0u64, None::<u64>, 0u64, (0u32, 0u32), 0u64);
    let (mut t_first, mut t_last) = (None::<f64>, 0f64);
    let result = frames::fetch(client, &req, &mut |r| {
        let slot = r.num("seq").unwrap_or(0.0) as u64;
        first.get_or_insert(slot);
        last = slot;
        size = (r.num("w").unwrap_or(0.0) as u32, r.num("h").unwrap_or(0.0) as u32);
        dropped = r.num("dropped").unwrap_or(0.0) as u64;
        let t = r.num("t_ns").unwrap_or(0.0);
        t_first.get_or_insert(t);
        t_last = t;
        let (name, bytes) = match codec {
            "rle" => (format!("frame_{slot:06}.bmp"), rle_to_bmp(size.0, size.1, &r.bytes)?),
            "raw" => (format!("frame_{slot:06}.rgba"), r.bytes.clone()),
            _ => (format!("frame_{slot:06}.png"), r.bytes.clone()),
        };
        std::fs::write(dir.join(name), bytes).map_err(|e| format!("rec write: {e}"))?;
        count += 1;
        if started.elapsed().as_secs_f64() >= seconds { return Err("\u{0}done".into()); }
        Ok(())
    });
    match result {
        Ok(()) => {}
        Err(e) if e == "\u{0}done" => {}
        Err(e) => return Err(format!("rec: {e}")),
    }
    let span = first.map_or(0, |f| last - f + 1);
    Ok(format!("{{\"dir\":{},\"frames\":{count},\"missing\":{},\"slots\":{span},\"fps\":{fps},\"sz\":[{},{}],\"device_dropped\":{dropped},\"device_ms\":{:.0}}}", crate::json_str(&dir.display().to_string()), span.saturating_sub(count), size.0, size.1, (t_last - t_first.unwrap_or(t_last)) / 1e6))
}

/// Run-length RGBA (see the app's `rle_encode`) to a 32-bit top-down BMP.
fn rle_to_bmp(w: u32, h: u32, rle: &[u8]) -> Result<Vec<u8>, String> {
    let mut px = Vec::with_capacity(w as usize * h as usize * 4);
    let mut i = 0;
    while i + 6 <= rle.len() {
        let run = u16::from_le_bytes([rle[i], rle[i + 1]]) as usize;
        let (r, g, b, a) = (rle[i + 2], rle[i + 3], rle[i + 4], rle[i + 5]);
        for _ in 0..run { px.extend_from_slice(&[b, g, r, a]); }
        i += 6;
    }
    if px.len() != w as usize * h as usize * 4 { return Err(format!("rec: a frame decoded to {} bytes, expected {}", px.len(), w as usize * h as usize * 4)); }
    let mut out = Vec::with_capacity(54 + px.len());
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + px.len()) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    out.extend_from_slice(&px);
    Ok(out)
}

fn gseq(client: &Client, kv: &[(String, String)]) -> Result<String, String> {
    crate::params::validate(kv.iter().map(|(k, _)| k.as_str()), crate::params::GSEQ)?;
    let n = match get(kv, &["n"]).unwrap_or("8").parse::<usize>() {
        Ok(n @ 1..) => n,
        _ => return Err("gseq n must be a positive integer".into()),
    };
    let every_ms = match get(kv, &["every_ms"]).unwrap_or("50").parse::<u64>() {
        Ok(ms @ 8..=60_000) => ms,
        _ => return Err("gseq every_ms must be 8..=60000".into()),
    };
    let dir = match get(kv, &["out"]) {
        Some(d) => {
            std::fs::create_dir_all(d).map_err(|e| format!("gseq out: {e}"))?;
            PathBuf::from(d)
        }
        None => grab_dir(client)?,
    };
    let req = frames::Request { window: window_of(kv), scale: Some(scale_of(kv)), raw: false, full_chroma: false, codec: None, n: Some(n), fps: Some(1000.0 / every_ms as f64), cap: false };
    let (mut paths, mut frame_json, mut pinned) = (Vec::new(), Vec::new(), Vec::new());
    frames::fetch(client, &req, &mut |r| {
        let index = frame_json.len();
        let path = write_grab(&dir, r.num("window").unwrap_or(0.0) as usize, &r.bytes, &pinned, drawn(&r))?;
        paths.push(crate::json_str(&path.display().to_string()));
        let mut frame = grab_json(&r, &path);
        frame.pop();
        frame.push_str(&format!(",\"scheduled_ms\":{},\"pixels_ms\":{:.3}}}", index as u64 * every_ms, index as f64 * every_ms as f64 + r.num("capture_ms").unwrap_or(0.0)));
        frame_json.push(frame);
        pinned.push(path);
        Ok(())
    })
    .map_err(|e| format!("gseq frame {}: {e}", frame_json.len()))?;
    Ok(format!("{{\"png\":[{}],\"n\":{n},\"every_ms\":{every_ms},\"frames\":[{}]}}", paths.join(","), frame_json.join(",")))
}

/// The windows of a `status` answer (`"w":[{"i":ID,"pass_dpi":N,...},...]`): (id, pass dpi).
fn windows_of(status: &str) -> Vec<(usize, f64)> {
    let Some(start) = status.find("\"w\":[") else { return Vec::new() };
    let b = status.as_bytes();
    let (mut i, mut depth, mut windows) = (start + 5, 1i32, Vec::new());
    while i < b.len() && depth > 0 {
        match b[i] {
            b'"' => {
                // a string: skip it (it may hold anything)
                let begin = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                // a key directly inside a window object (depth 2)
                if depth == 2 && b.get(i + 1) == Some(&b':') {
                    let number: String = status[i + 2..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                    match &status[begin..=i.min(b.len() - 1)] {
                        "\"i\"" => {
                            if let Ok(id) = number.parse() {
                                windows.push((id, 0.0));
                            }
                        }
                        "\"pass_dpi\"" => {
                            if let (Some(window), Ok(dpi)) = (windows.last_mut(), number.parse()) {
                                window.1 = dpi;
                            }
                        }
                        _ => {}
                    }
                }
            }
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    windows
}

/// The ids of the windows of a `status` answer that have drawn: one that never did (a window
/// only a headset's frames go to) has no frame to grab.
pub fn drawn_window_ids(status: &str) -> Vec<usize> {
    windows_of(status).into_iter().filter(|(_, dpi)| *dpi > 0.0).map(|(id, _)| id).collect()
}

/// A frame of every window (or `w=`), then `quit`: the last call of a session.
fn gq(client: &Client, kv: &[(String, String)]) -> Result<String, String> {
    crate::params::validate(kv.iter().map(|(k, _)| k.as_str()), crate::params::GQ)?;
    let scale = scale_of(kv);
    let targets = match window_of(kv) {
        Some(w) => vec![w],
        None => drawn_window_ids(&ask_one(client, "status").unwrap_or_default()),
    };
    let (mut paths, mut problems) = (Vec::new(), Vec::new());
    for window in targets {
        match grab_one(client, Some(window), scale, None) {
            Ok((_, path)) => paths.push(crate::json_str(&path.display().to_string())),
            Err(msg) => problems.push(format!("w{window}: {msg}")),
        }
    }
    // Capture failure still permits cleanup, but human intervention does not.
    // a driver that pinned the user sequence passes it on: quit is refused once the person intervened
    let quit = match get(kv, &["if_user_seq"]) {
        Some(seq) => request_line(&["quit".to_string()], &[("if_user_seq".to_string(), seq.to_string())]),
        None => "quit".to_string(),
    };
    let reply = ask_one(client, &quit).unwrap_or_else(|e| format!("{{\"err\":{}}}", crate::json_str(&e)));
    if reply.starts_with("{\"err\":") {
        // the person intervened (or the app refused): nothing was quit
        return Ok(reply);
    }
    let mut out = format!("{{\"png\":[{}],\"quit\":{}", paths.join(","), if reply.starts_with("{\"ok\":1") { 1 } else { 0 });
    if !problems.is_empty() {
        out.push_str(&format!(",\"err\":{}", crate::json_str(&problems.join("; "))));
    }
    out.push('}');
    Ok(out)
}

/// The app's `tweak final`; when it says the user drew (`"drew":1`) a frame of the window is
/// written too and named in the same JSON.
fn tweak_final(client: &Client, words: &[String], kv: &[(String, String)], each: &mut dyn FnMut(&str) -> bool) -> Result<(bool, Option<String>), Error> {
    let mut lines = Vec::new();
    let mut err = false;
    let mut answer = client.cmd(&request_line(words, kv))?;
    while let Some(l) = answer.next() {
        let l = l?;
        err |= l.starts_with("{\"err\":");
        lines.push(l);
    }
    let end = answer.end().map(str::to_string);
    if let Some(json) = lines.first_mut() {
        if json.contains("\"drew\":1") && json.ends_with('}') {
            json.pop();
            match grab_one(client, window_of(kv), scale_of(kv), None) {
                Ok((_, path)) => json.push_str(&format!(",\"png\":{}}}", crate::json_str(&path.display().to_string()))),
                Err(msg) => json.push_str(&format!(",\"png_err\":{}}}", crate::json_str(&msg))),
            }
        }
    }
    for l in &lines {
        if !each(l) {
            break;
        }
    }
    Ok((err, end))
}

#[cfg(test)]
mod tests {
    use super::{call_image_line, client_side, typed_line};

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn call_pictures_become_files() {
        use makepad_base64::{base64_encode, BASE64_STANDARD};
        use makepad_strict_json::{self as json, Value};
        use std::{fs, path::Path};

        let dir = std::env::temp_dir().join(format!("call_pictures_become_files-{}-{}", std::process::id(), crate::cmd::wall_ms()));
        fs::create_dir(&dir).expect("create fresh picture folder");
        for i in 0..64 {
            fs::write(dir.join(format!("call-00003-{i}.png")), []).expect("seed older call pictures");
        }
        let grab = dir.join("grab-w0-00001.png");
        fs::write(&grab, []).expect("seed a window grab");

        let png = b"\x89PNG\r\n\x1a\n";
        let jpg = b"\xff\xd8\xff\xe0\x00\x04\x00\x00\xff";
        let encode = |bytes: &[u8]| String::from_utf8(base64_encode(bytes, &BASE64_STANDARD)).unwrap();
        let line = format!(r#"{{"Result":{{"text":"rendered","call_id":"abc","images":["{}","{}"]}}}}"#, encode(png), encode(jpg));
        let output = call_image_line(&line, || Ok(dir.clone()));
        let parsed = json::parse(output.as_bytes()).expect("rewritten result is JSON");
        let result = parsed.get("Result").expect("keep result envelope");
        assert!(result.get("images").is_none(), "remove inline pictures");
        assert_eq!(result.get("text").and_then(Value::as_str), Some("rendered"), "preserve result text");
        assert_eq!(result.get("call_id").and_then(Value::as_str), Some("abc"), "preserve call ID");
        let files = result.get("image_files").and_then(Value::as_arr).expect("return a list of picture paths");
        assert_eq!(files.len(), 2, "return one path per picture");
        for (i, (ext, bytes)) in [("png", png.as_slice()), ("jpg", jpg.as_slice())].into_iter().enumerate() {
            let path = Path::new(files[i].as_str().expect("picture path is a string"));
            assert!(path.is_absolute(), "picture {i} has an absolute path");
            assert_eq!(path, dir.join(format!("call-00004-{i}.{ext}")), "picture {i} uses the next sequence, its index and its format");
            assert_eq!(fs::read(path).expect("returned picture path exists"), bytes, "picture {i} holds the decoded bytes");
        }
        assert!(!dir.join("call-00003-0.png").exists() && !dir.join("call-00003-1.png").exists(), "expire the two oldest call pictures");
        assert!(dir.join("call-00003-2.png").exists() && grab.exists(), "keep the 64th newest call picture and the window grab");

        for line in [r#"{ "Progress": {"text":"waiting"} }"#, r#"{ "Result": {"text":"done"} }"#, r#"{ "Result": [{"images":[]}] }"#, r#"{ "err": "busy" }"#, "plain text"] {
            assert_eq!(call_image_line(line, || panic!("only pictures need the folder")), line, "pass this line through byte for byte: {line}");
        }

        let bad = format!("{}!!!!", encode(jpg));
        let line = format!(r#"{{"Result":[{{"images":["{bad}","{}"]}}]}}"#, encode(png));
        let output = call_image_line(&line, || Ok(dir.clone()));
        assert!(!output.contains(&bad), "omit the invalid base64 payload from the answer");
        let parsed = json::parse(output.as_bytes()).expect("mixed success and failure is JSON");
        let result = &parsed.get("Result").and_then(Value::as_arr).expect("keep the wrapped result")[0];
        let errors = result.get("image_errors").and_then(Value::as_arr).expect("report picture errors");
        assert_eq!(errors.len(), 1, "report one error for the invalid picture");
        assert!(errors[0].as_str().unwrap().contains("base64"), "explain the decoding failure");
        let files = result.get("image_files").and_then(Value::as_arr).expect("keep the successful picture");
        assert_eq!(files.len(), 1, "return only the successful path");
        assert_eq!(Path::new(files[0].as_str().unwrap()), dir.join("call-00005-1.png"), "a failed picture preserves the next picture's index");
        fs::remove_dir_all(dir).expect("remove the test's picture folder");
    }

    #[test]
    fn typed_code_keeps_its_equals_signs() {
        assert_eq!(
            typed_line(&words("probe add f fn probe(x: u32) { let y = x; match y { 0 => (), _ => () } } mutate=1")),
            r#"{"cmd":"probe add f","mutate":"1","code":"fn probe(x: u32) { let y = x; match y { 0 => (), _ => () } }"}"#
        );
        assert_eq!(typed_line(&words("eval mutate=1 a == b")), r#"{"cmd":"eval","mutate":"1","expr":"a == b"}"#);
        assert_eq!(typed_line(&words("vm roots top=12")), "vm roots top=12");
        let spaced = ["ui".to_string(), "find".to_string(), "q=Save as".to_string(), "nth=1".to_string()];
        assert_eq!(typed_line(&spaced), r#"{"cmd":"ui find","q":"Save as","nth":"1"}"#);
    }

    #[test]
    fn the_commands_read_from_frames_are_the_clients_own() {
        for line in ["grab out=a.png", "grab w=1 scale=0.5", "gseq n=4", "gq", "rec", "rec n=30", "tweak grab", "tweak final x=1"] {
            assert!(client_side(&words(line)), "{line} is answered on the client");
        }
        for line in ["rec save", "rec status", "rec mark x", "ui tree", "tweak set x=1", "session", "tools", ""] {
            assert!(!client_side(&words(line)), "{line} is the app's own");
        }
    }
}
