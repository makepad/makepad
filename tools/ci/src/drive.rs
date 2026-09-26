//! Deterministic Splash host. Natives never use model output to choose input.
use crate::{
    process::{ChildLog, Result},
    remote::{Failure, Remote},
    report::{safe_name, Run, Update},
    smoke::{Script, Target},
    uihub::Judge,
    watch::Config,
};
use makepad_script::*;
use makepad_strict_json::{self as json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

struct AppHandle {
    child: ChildLog,
    remote: Option<Remote>,
    interrupted: bool,
    closed: bool,
    name: String,
}
struct Runtime {
    run: Run,
    config: Config,
    hub: Judge,
    apps: Vec<AppHandle>,
    images: Vec<PathBuf>,
    group: String,
    step: Option<usize>,
    suppressed: bool,
    fatal: bool,
    skipped: bool,
    validate: bool,
    validated_steps: Vec<String>,
    host: crate::cargo::Host,
    permit: Option<crate::runner::Permit>,
    warning: String,
    manifest: Option<(String, PathBuf)>,
}
impl Runtime {
    fn fail(&mut self, text: &str) {
        self.suppressed = true;
        if let Some(i) = self.step {
            self.run.end(i, "failed", text);
        } else {
            self.run.fail(&format!("{}: runtime", self.group), text);
        }
    }
    fn allowed(&mut self) -> bool {
        if self.suppressed || self.fatal {
            return false;
        }
        if let Err(e) = self.run.control.check() {
            self.fatal = true;
            self.fail(&e);
            return false;
        }
        true
    }
    fn request(&mut self, i: usize, route: &str, mutate: bool) -> Result<String> {
        if !route.starts_with('/')
            || route
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
            || route.contains("if_user_seq")
        {
            return Err("invalid route (or attempt to replace the pinned user sequence)".into());
        }
        let app = self.apps.get_mut(i).ok_or("invalid app handle")?;
        if app.interrupted {
            return Err("human intervention; app is no longer owned".into());
        }
        if app.closed {
            return Err(format!("{} already closed", app.name));
        }
        let remote = app.remote.as_ref().ok_or("remote unavailable")?;
        self.run.log(&format!("{} GET {route}", app.name));
        // A grab the bridge could not place, or a grab budget that is full,
        // ends in "; retry": ask again. An input is never asked again: the
        // app applied it when it took the request, whatever it answered
        // about the frame after it, and a second request is a second key.
        let mut attempt = 0;
        let result = loop {
            let result = remote.request(route, mutate);
            let again = !applies_input(route)
                && matches!(&result, Err(e) if e.to_string().contains("; retry"));
            if !again || attempt == 5 {
                break result;
            }
            attempt += 1;
            std::thread::sleep(Duration::from_millis(300));
        };
        if matches!(result, Err(Failure::Interrupted(_))) {
            app.interrupted = true;
            self.fatal = true;
            self.run.held_pids.push(app.child.child.id());
        }
        let text = result.map_err(|e| e.to_string())?;
        if let Ok(v) = json::parse_depth(text.as_bytes(), 64) {
            if let Some(e) = v.get("err") {
                return Err(format!("GET {route}: {}", e.to_json()));
            }
        }
        Ok(text)
    }
    fn request_json(&mut self, i: usize, route: &str, mutate: bool) -> Result<Value> {
        json::parse_depth(self.request(i, route, mutate)?.as_bytes(), 64).map_err(str::to_string)
    }
    fn drain_app(&mut self, i: usize) -> Result<()> {
        let run = &mut self.run;
        self.apps[i].child.drain(&mut |s| run.log(s))?;
        if self.apps[i]
            .child
            .text
            .contains("[makepad-remote] user closed")
            || self.apps[i].child.text.contains("app exit: user closed")
        {
            self.apps[i].interrupted = true;
            self.fatal = true;
            let pid = self.apps[i].child.child.id();
            if !self.run.held_pids.contains(&pid) {
                self.run.held_pids.push(pid);
            }
            return Err("user closed window; leaving the instance alone".into());
        }
        Ok(())
    }
    fn alive(&mut self, i: usize) -> Result<bool> {
        self.drain_app(i)?;
        let alive = self.apps[i].child.alive()?;
        if !alive && !self.apps[i].closed {
            self.fatal = true;
        }
        Ok(alive)
    }
    fn launch(&mut self, binary: PathBuf, cwd: PathBuf, app_env: &[(String, String)]) -> Result<usize> {
        let binary = self.run.root.join(binary);
        let name = binary
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if !binary.is_file() {
            self.fatal = true;
            return Err(format!("missing binary: {}", binary.display()));
        }
        let cwd = inside(&self.run.root, &cwd)?;
        let path = self.run.path(&format!("{}-stdout.log", name));
        let mut command = Command::new(&binary);
        command
            .current_dir(cwd)
            .arg("--remote")
            .env("MAKEPAD_HIDE_WINDOWS", "1")
            .env_remove("MAKEPAD_FOCUS")
            .env("SANDBOX_MUTE", "1")
            .env("CARGO_TARGET_DIR", self.run.root.join("target"))
            .envs(app_env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let child = ChildLog::spawn(&mut command, path)?;
        self.run
            .log(&format!("owned {} pid={}", name, child.child.id()));
        let index = self.apps.len();
        self.apps.push(AppHandle {
            child,
            remote: None,
            interrupted: false,
            closed: false,
            name: name,
        });
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            self.run.control.check()?;
            self.drain_app(index)?;
            let app = &mut self.apps[index];
            let pid = app.child.child.id();
            if let Some(port) = app
                .child
                .text
                .lines()
                .find_map(|line| parse_startup(line, pid))
            {
                app.remote = Some(Remote {
                    port,
                    user_seq: None,
                });
                break;
            }
            if !app.child.alive()? {
                self.fatal = true;
                return Err(format!(
                    "{} exited before remote startup\n{}",
                    app.name,
                    crate::process::error_lines(&app.child.text)
                ));
            }
            if Instant::now() >= deadline {
                return Err("remote startup timed out after 60s".into());
            }
            self.run.control.sleep(0.1)?;
        }
        loop {
            self.run.control.check()?;
            if !self.alive(index)? {
                return Err("app died before ready".into());
            }
            if let Ok(v) = self.request_json(index, "/s", false) {
                if v.get("w")
                    .and_then(Value::as_arr)
                    .is_some_and(|w| !w.is_empty())
                {
                    break;
                }
            }
            if self.fatal {
                return Err("human intervention during startup".into());
            }
            if Instant::now() >= deadline {
                return Err("/s readiness timed out".into());
            }
            self.run.control.sleep(0.2)?;
        }
        let activity = self.request_json(index, "/activity", false)?;
        let seq = activity
            .get("user_seq")
            .and_then(Value::as_u64)
            .ok_or("missing activity user_seq")?;
        self.apps[index].remote.as_mut().unwrap().user_seq = Some(seq);
        if activity.get("user_active").and_then(Value::as_bool) == Some(true) {
            self.apps[index].interrupted = true;
            self.fatal = true;
            self.run.held_pids.push(self.apps[index].child.child.id());
            return Err("human active at startup; leaving app running".into());
        }
        let help = self.request(index, "/", false)?;
        let path = self.run.path("remote-help.txt");
        fs::write(path, help).map_err(|e| e.to_string())?;
        Ok(index)
    }
    fn key(&mut self, i: usize, code: &str, mods: &str) -> Result<()> {
        self.run.control.check()?;
        self.request_json(
            i,
            &format!("/k?k=down&c={}{mods}&wait=1", encode(code)),
            true,
        )?;
        // Always release a successfully pressed key, including on Stop.
        std::thread::sleep(Duration::from_millis(180));
        self.request_json(i, &format!("/k?k=up&c={}{mods}&wait=1", encode(code)), true)?;
        self.run.control.check()
    }
    fn type_text(&mut self, i: usize, text: &str) -> Result<()> {
        for c in text.chars() {
            let (code, shift) = character_key(c)?;
            self.key(i, &code, if shift { "&shift=1" } else { "" })?;
        }
        Ok(())
    }
    fn log_text(&mut self, i: usize) -> Result<String> {
        self.drain_app(i)?;
        let v = self.request_json(i, "/log?n=10000", false)?;
        let mut lines = Vec::new();
        collect_log(&v, &mut lines);
        Ok(lines.join("\n"))
    }
    fn no_errors(&mut self, i: usize) -> Result<()> {
        if !self.alive(i)? {
            return Err("app process is dead".into());
        }
        let log = format!("{}\n{}", self.log_text(i)?, self.apps[i].child.text);
        let errors: Vec<_> = log
            .lines()
            .filter(|line| {
                let line = line.trim_start();
                let error = line.starts_with("[E]") || line.contains("panicked");
                let allowed = line.contains("drag_anywhere")
                    && self
                        .config
                        .allowed_errors
                        .iter()
                        .any(|entry| !entry.is_empty() && line.contains(entry));
                error && !allowed
            })
            .take(40)
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
    fn wait_log(&mut self, i: usize, text: &str, secs: f64) -> Result<()> {
        if !secs.is_finite() || !(0.0..=3600.0).contains(&secs) {
            return Err("invalid log timeout".into());
        }
        let end = Instant::now() + Duration::from_secs_f64(secs);
        loop {
            if !self.alive(i)? {
                return Err("app died while waiting for log".into());
            }
            if log_has(&self.log_text(i)?, text) {
                return Ok(());
            }
            if Instant::now() >= end {
                return Err(format!(
                    "{}: log did not contain {text:?} within {secs}s",
                    self.apps[i].name
                ));
            }
            self.run.control.sleep(0.2)?;
        }
    }
    fn grab(&mut self, i: usize, name: &str) -> Result<usize> {
        let value = self.request_json(i, "/g", false)?;
        let mut paths = Vec::new();
        png_paths(&value, &mut paths);
        let source = paths.first().ok_or("grab did not return a PNG path")?;
        let bytes = fs::read(source).map_err(|e| e.to_string())?;
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("grab is not PNG".into());
        }
        let sub = self.run.dir.join(safe_name(&self.group));
        fs::create_dir_all(&sub).map_err(|e| e.to_string())?;
        let path = sub.join(format!("{}.png", safe_name(name)));
        if path.exists() {
            return Err(format!("duplicate grab name: {name}"));
        }
        fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        self.run.grabs.push(path.display().to_string());
        (self.run.notify)(Update::Grab(path.clone(), Arc::new(bytes)));
        let index = self.images.len();
        self.images.push(path);
        Ok(index)
    }
    fn quit(&mut self, i: usize) -> Result<()> {
        if self.apps[i].interrupted {
            return Err("human intervention; app left running".into());
        }
        if self.apps[i].closed {
            return Ok(());
        }
        if !self.apps[i].child.alive()? {
            self.apps[i].closed = true;
            return Ok(());
        }
        // A failed launch can have an endpoint but not yet have pinned ownership.
        if self.apps[i]
            .remote
            .as_ref()
            .is_some_and(|r| r.user_seq.is_none())
        {
            let a = self.request_json(i, "/activity", false)?;
            let seq = a
                .get("user_seq")
                .and_then(Value::as_u64)
                .ok_or("cleanup missing user_seq")?;
            self.apps[i].remote.as_mut().unwrap().user_seq = Some(seq);
            if a.get("user_active").and_then(Value::as_bool) == Some(true) {
                self.apps[i].interrupted = true;
                self.run.held_pids.push(self.apps[i].child.child.id());
                return Err("human active; app left running".into());
            }
        }
        let mut error = None;
        if self.apps[i].remote.is_some() {
            if let Err(e) = self.request_json(i, "/gq", true) {
                if self.apps[i].interrupted {
                    return Err(e);
                }
                self.run.log(&format!("/gq failed: {e}; trying /quit"));
                error = Some(e);
                if let Err(e) = self.request(i, "/quit", true) {
                    if self.apps[i].interrupted {
                        return Err(e);
                    }
                }
            }
        }
        let end = Instant::now() + Duration::from_secs(10);
        while self.apps[i].child.alive()? && Instant::now() < end {
            self.drain_app(i)?;
            std::thread::sleep(Duration::from_millis(100));
        }
        if self.apps[i].child.alive()? {
            if self.apps[i].remote.is_some() {
                if let Err(e) = self.request(i, "/activity", false) {
                    if self.apps[i].interrupted {
                        return Err(e);
                    }
                }
            }
            let pid = self.apps[i].child.child.id();
            self.apps[i].child.kill_exact()?;
            self.run.log(&format!(
                "killed exact owned pid={pid} after graceful shutdown timeout"
            ));
            error = Some(format!("pid={pid} required forced cleanup"));
        }
        self.apps[i].child.child.wait().map_err(|e| e.to_string())?;
        let run = &mut self.run;
        self.apps[i].child.finish_output(&mut |s| run.log(s))?;
        self.apps[i].closed = true;
        error.map_or(Ok(()), Err)
    }
}
fn parse_startup(line: &str, pid: u32) -> Option<u16> {
    if !line.starts_with("[makepad-remote] listening on 127.0.0.1:") {
        return None;
    }
    let fields: Vec<_> = line.split_whitespace().collect();
    let actual = fields
        .iter()
        .find_map(|s| s.strip_prefix("pid="))
        .and_then(|s| s.parse::<u32>().ok())?;
    if actual != pid {
        return None;
    }
    fields.get(3)?.rsplit(':').next()?.parse().ok()
}
fn character_key(c: char) -> Result<(String, bool)> {
    match c {
        'a'..='z' | 'A'..='Z' => Ok((
            format!("Key{}", c.to_ascii_uppercase()),
            c.is_ascii_uppercase(),
        )),
        '0'..='9' => Ok((format!("Key{c}"), false)),
        ' ' => Ok(("Space".into(), false)),
        _ => Err(format!("type_text does not support character {c:?}")),
    }
}
/// The routes that apply an input to the app when it takes the request
/// (keys, pointer, text, a dropped file, the tweaker, the AI pane): a
/// second request is a second input, so these are never asked again.
fn applies_input(route: &str) -> bool {
    ["/k?", "/m?", "/t?", "/click?", "/drop?", "/tweak?", "/ai?"]
        .iter()
        .any(|prefix| route.starts_with(prefix))
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn collect_log(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Str(s) => out.push(s.clone()),
        Value::Arr(a) => {
            for x in a {
                collect_log(x, out)
            }
        }
        Value::Obj(a) => {
            for (k, v) in a {
                if matches!(
                    k.as_str(),
                    "l" | "lines" | "log" | "entries" | "text" | "message" | "msg" | "s"
                ) {
                    collect_log(v, out);
                }
            }
        }
        _ => {}
    }
}
fn png_paths(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Obj(a) => {
            for (k, v) in a {
                if k == "png" {
                    match v {
                        Value::Str(s) => out.push(s.clone()),
                        Value::Arr(a) => {
                            out.extend(a.iter().filter_map(Value::as_str).map(str::to_string))
                        }
                        _ => {}
                    }
                } else {
                    png_paths(v, out)
                }
            }
        }
        Value::Arr(a) => {
            for v in a {
                png_paths(v, out)
            }
        }
        _ => {}
    }
}
fn inside(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = root.join(path).canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(&root) {
        return Err("cwd must remain inside checkout".into());
    }
    Ok(path)
}
fn rt<'a>(vm: &'a mut ScriptVm) -> &'a mut Runtime {
    vm.host.as_any_mut().downcast_mut::<Runtime>().unwrap()
}
fn value(vm: &ScriptVm, args: ScriptObject, key: LiveId) -> ScriptValue {
    vm.bx.heap.value(args, key.into(), NoTrap)
}
fn text(vm: &mut ScriptVm, v: ScriptValue) -> Result<String> {
    vm.string_with(v, |_, s| s.to_string())
        .ok_or("expected string".into())
}
fn str_arg(vm: &mut ScriptVm, args: ScriptObject, key: LiveId) -> Result<String> {
    let v = value(vm, args, key);
    text(vm, v)
}
fn num_arg(vm: &ScriptVm, args: ScriptObject, key: LiveId) -> Result<f64> {
    value(vm, args, key)
        .as_number()
        .filter(|v| v.is_finite())
        .ok_or_else(|| {
            format!(
                "expected finite number for {}: {:?}",
                if key == id!(image) {
                    "image"
                } else if key == id!(secs) {
                    "secs"
                } else {
                    "coordinate"
                },
                value(vm, args, key)
            )
        })
}
fn object_field(vm: &ScriptVm, v: ScriptValue, key: LiveId) -> ScriptValue {
    v.as_object()
        .map(|o| value(vm, o, key))
        .filter(|v| !v.is_err())
        .unwrap_or(NIL)
}
fn json_value(vm: &mut ScriptVm, v: Value) -> ScriptValue {
    match v {
        Value::Null => NIL,
        Value::Bool(v) => v.into(),
        Value::Int(v) => (v as f64).into(),
        Value::F64(v) => v.into(),
        Value::Str(v) => vm.new_string_with(|_, out| out.push_str(&v)),
        v => {
            let mut parser = makepad_script::json::JsonParserThread::default();
            parser.read_json(&v.to_json(), &mut vm.bx.heap)
        }
    }
}
fn result(vm: &mut ScriptVm, r: Result<Value>) -> ScriptValue {
    match r {
        Ok(v) => json_value(vm, v),
        Err(e) => {
            rt(vm).fail(&e);
            NIL
        }
    }
}
fn allow(vm: &mut ScriptVm) -> bool {
    if vm
        .bx
        .captured_errors
        .as_ref()
        .is_some_and(|a| !a.is_empty())
    {
        let e = vm.take_errors().join("\n");
        vm.bx.captured_errors = Some(Vec::new());
        rt(vm).fail(&e);
        return false;
    }
    rt(vm).allowed()
}

#[path = "drive_natives.rs"]
mod natives;

fn register(vm: &mut ScriptVm, target: Option<&Target>) {
    let ci = vm.new_module(id!(ci));
    natives::register(vm, ci);
    let v = if let Some(t) = target {
        json_value(
            vm,
            json::obj(vec![
                ("package", json::s(&t.package)),
                ("binary", json::s(&t.binary)),
                ("cwd", json::s(t.cwd.display().to_string())),
                ("manifest", json::s(t.manifest.display().to_string())),
            ]),
        )
    } else {
        NIL
    };
    vm.bx.heap.set_value_def(ci, id!(target).into(), v);
    vm.add_method(
        ci,
        id_lut!(step),
        script_args!(name = NIL, body = NIL),
        |vm, args| {
            let name = match str_arg(vm, args, id!(name)) {
                Ok(v) => v,
                Err(e) => {
                    rt(vm).fail(&e);
                    return NIL;
                }
            };
            let body = value(vm, args, id!(body));
            if rt(vm).step.is_some() {
                rt(vm).fail("nested ci.step is not supported");
                return NIL;
            }
            let before = vm.take_errors();
            vm.bx.captured_errors = Some(Vec::new());
            if !before.is_empty() {
                rt(vm).fail(&before.join("\n"));
            }
            let runtime = rt(vm);
            let i = runtime.run.plan(&format!("{} / {name}", runtime.group));
            if runtime.fatal || runtime.run.control.stopped() {
                runtime
                    .run
                    .end(i, "skipped", "app dead, human intervention, or run stopped");
                return NIL;
            }
            runtime.suppressed = false;
            runtime.skipped = false;
            runtime.warning.clear();
            runtime.step = Some(i);
            runtime.run.begin(i, "");
            runtime.validated_steps.push(name);
            let answer = vm.call(body, &[]);
            let errors = vm.take_errors();
            vm.bx.captured_errors = Some(Vec::new());
            if !errors.is_empty() {
                rt(vm).fail(&errors.join("\n"));
            }
            if answer.is_err() && !rt(vm).suppressed {
                rt(vm).fail("step callback returned an error");
            }
            let runtime = rt(vm);
            if !runtime.suppressed {
                runtime.run.end(
                    i,
                    if runtime.skipped || !runtime.warning.is_empty() {
                        "warning"
                    } else {
                        "passed"
                    },
                    if runtime.skipped {
                        "skipped (model not installed)"
                    } else {
                        &runtime.warning
                    },
                );
            }
            runtime.step = None;
            runtime.suppressed = runtime.fatal;
            NIL
        },
    );
    vm.add_method(
        ci,
        id_lut!(launch),
        script_args!(target = NIL),
        |vm, args| {
            if !allow(vm) {
                // Stopped or already failed: the script still holds an app,
                // an inert one, so its later calls are not errors on nil.
                return app_object(vm, usize::MAX);
            }
            let v = value(vm, args, id!(target));
            let r = natives::launch(vm, v);
            match r {
                Ok(i) => app_object(vm, i),
                Err(e) => {
                    rt(vm).fatal = true;
                    rt(vm).fail(&e);
                    // A script keeps its shape after a failed launch: the app
                    // it holds is inert (the run is fatal, so every method
                    // returns before acting) instead of nil, which would add
                    // a "method not found on nil" error per later call.
                    app_object(vm, usize::MAX)
                }
            }
        },
    );
    vm.add_method(ci, id_lut!(sleep), script_args!(secs = NIL), |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let r = num_arg(vm, args, id!(secs))
            .and_then(|n| {
                if rt(vm).validate {
                    Ok(())
                } else {
                    rt(vm).run.control.sleep(n)
                }
            })
            .map(|_| Value::Null);
        result(vm, r)
    });
    vm.add_method(
        ci,
        id_lut!(check),
        script_args!(condition = NIL, message = NIL),
        |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            if value(vm, args, id!(condition)).as_bool() != Some(true) {
                let message = str_arg(vm, args, id!(message)).unwrap_or_else(|e| e);
                rt(vm).fail(&message);
            }
            NIL
        },
    );
    vm.add_method(
        ci,
        id_lut!(fail),
        script_args!(message = NIL),
        |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let s = str_arg(vm, args, id!(message)).unwrap_or_else(|e| e);
            rt(vm).fail(&s);
            NIL
        },
    );
    vm.add_method(ci, id_lut!(log), script_args!(text = NIL), |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let r = str_arg(vm, args, id!(text)).map(|s| {
            rt(vm).run.log(&s);
            Value::Null
        });
        result(vm, r)
    });
    for (name, accept) in [(id_lut!(judge), false), (id_lut!(accept), true)] {
        vm.add_method(
            ci,
            name,
            script_args!(image = NIL, text = NIL),
            move |vm, args| {
                if !allow(vm) {
                    return NIL;
                }
                let r = (|| {
                    let prompt = str_arg(vm, args, id!(text))?;
                    let index = num_arg(vm, args, id!(image))? as usize;
                    if rt(vm).validate {
                        return Ok(json::obj(vec![
                            ("yes", Value::Bool(true)),
                            ("reasons", Value::Arr(vec![])),
                            ("raw", json::s("VERDICT: YES")),
                        ]));
                    }
                    let runtime = rt(vm);
                    let image = runtime
                        .images
                        .get(index)
                        .ok_or("invalid image handle")?
                        .clone();
                    let verdict = runtime.hub.judge(
                        &image,
                        &prompt,
                        &runtime.run.control,
                        &runtime.run.notify,
                    )?;
                    runtime
                        .run
                        .log(&format!("judge {}: {}", image.display(), verdict.raw));
                    let record = json::obj(vec![
                        ("group", json::s(&runtime.group)),
                        (
                            "step",
                            json::s(
                                runtime
                                    .step
                                    .map(|i| runtime.run.stages[i].name.as_str())
                                    .unwrap_or("judge"),
                            ),
                        ),
                        ("prompt", json::s(&prompt)),
                        ("image", json::s(image.display().to_string())),
                        ("verdict", verdict.json()),
                        (
                            "peak_rss_bytes",
                            crate::uihub::peak_rss_bytes()
                                .map(|n| Value::Int(n as i64))
                                .unwrap_or(Value::Null),
                        ),
                    ]);
                    runtime.run.evidence.push(record);
                    runtime.skipped |= verdict.skipped;
                    if verdict.skipped && runtime.step.is_none() {
                        let i = runtime.run.plan(&format!("{} / vision", runtime.group));
                        runtime.run.end(i, "warning", &verdict.raw);
                    }
                    if accept && !verdict.yes {
                        // The raw answer already contains both reasons and verdict.
                        return Err(if verdict.parse_error {
                            format!("vision parse failed\n{}", verdict.raw)
                        } else {
                            verdict.raw
                        });
                    }
                    Ok(verdict.json())
                })();
                result(vm, r)
            },
        );
    }
    vm.add_method(ci, id_lut!(ask), script_args!(text = NIL), |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let r = (|| {
            let prompt = str_arg(vm, args, id!(text))?;
            if rt(vm).validate {
                return Ok(json::s("VERDICT: YES"));
            }
            let r = rt(vm);
            let answer = r.hub.ask(&prompt, &r.run.control, &r.run.notify)?;
            r.skipped |= answer == "skipped (model not installed)";
            if r.skipped && r.step.is_none() {
                let i = r.run.plan(&format!("{} / ask", r.group));
                r.run.end(i, "warning", &answer);
            }
            r.run.log(&format!("ask: {answer}"));
            r.run.evidence.push(json::obj(vec![
                ("ask", json::s(prompt)),
                ("answer", json::s(&answer)),
            ]));
            Ok(json::s(answer))
        })();
        result(vm, r)
    });
    vm.add_method(
        ci,
        id_lut!(run),
        script_args!(program = NIL, args = NIL, options = NIL),
        |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let r = (|| {
                let program = str_arg(vm, args, id!(program))?;
                let av = value(vm, args, id!(args));
                let array = av.as_array().ok_or("run args must be an array")?;
                let mut argv = Vec::new();
                for i in 0..vm.bx.heap.array_len(array) {
                    let v = vm.bx.heap.array_index(array, i, NoTrap);
                    argv.push(text(vm, v)?);
                }
                let options = value(vm, args, id!(options));
                let cwd = object_field(vm, options, id!(cwd));
                let cwd = if cwd.is_nil() {
                    ".".into()
                } else {
                    text(vm, cwd)?
                };
                let timeout = object_field(vm, options, id!(timeout_secs))
                    .as_number()
                    .unwrap_or(600.0);
                if !timeout.is_finite() || !(1.0..=86400.0).contains(&timeout) {
                    return Err("invalid timeout_secs".into());
                }
                let envobj = object_field(vm, options, id!(env));
                let mut env = Vec::new();
                if let Some(obj) = envobj.as_object() {
                    let fields: Vec<_> = vm
                        .bx
                        .heap
                        .map_ref(obj)
                        .iter()
                        .map(|(k, v)| (*k, v.value))
                        .collect();
                    for (k, v) in fields {
                        let mut key = String::new();
                        vm.bx.heap.cast_to_string(k, &mut key);
                        env.push((key, text(vm, v)?));
                    }
                }
                if rt(vm).validate {
                    return Ok(json::obj(vec![
                        ("code", Value::Int(0)),
                        ("out", json::s("")),
                    ]));
                }
                let cwd = inside(&rt(vm).run.root, Path::new(&cwd))?;
                if program == "cargo" {
                    env.retain(|(k, _)| k != "CARGO_TARGET_DIR");
                    env.push((
                        "CARGO_TARGET_DIR".into(),
                        rt(vm).run.root.join("target").display().to_string(),
                    ));
                }
                if let Some(i) = rt(vm).step {
                    rt(vm).run.stages[i].command = crate::report::display_command(&program, &argv);
                    rt(vm).run.publish();
                }
                let output = rt(vm)
                    .run
                    .command(&program, &argv, &cwd, &env, timeout as u64)?;
                Ok(json::obj(vec![
                    ("code", Value::Int(output.code as i64)),
                    ("out", json::s(output.out)),
                ]))
            })();
            result(vm, r)
        },
    );
}

fn app_object(vm: &mut ScriptVm, index: usize) -> ScriptValue {
    let obj = vm.bx.heap.new_object();
    vm.add_method(
        obj,
        id_lut!(key),
        script_args!(code = NIL, options = NIL),
        move |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let r = (|| {
                let code = str_arg(vm, args, id!(code))?;
                let options = value(vm, args, id!(options));
                let mut mods = String::new();
                for (k, s) in [
                    (id!(cmd), "cmd"),
                    (id!(shift), "shift"),
                    (id!(ctrl), "ctrl"),
                    (id!(alt), "alt"),
                ] {
                    if object_field(vm, options, k).as_bool() == Some(true) {
                        mods.push_str(&format!("&{s}=1"));
                    }
                }
                if !rt(vm).validate {
                    rt(vm).key(index, &code, &mods)?;
                }
                Ok(Value::Null)
            })();
            result(vm, r)
        },
    );
    vm.add_method(
        obj,
        id_lut!(type_text),
        script_args!(text = NIL),
        move |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let r = str_arg(vm, args, id!(text)).and_then(|s| {
                if rt(vm).validate {
                    for c in s.chars() {
                        character_key(c)?;
                    }
                } else {
                    rt(vm).type_text(index, &s)?;
                }
                Ok(Value::Null)
            });
            result(vm, r)
        },
    );
    for (method, scroll) in [(id_lut!(click), false), (id_lut!(scroll), true)] {
        vm.add_method(
            obj,
            method,
            script_args!(x = NIL, y = NIL, dx = 0.0, dy = 0.0),
            move |vm, args| {
                if !allow(vm) {
                    return NIL;
                }
                let r = (|| {
                    let x = num_arg(vm, args, id!(x))?;
                    let y = num_arg(vm, args, id!(y))?;
                    let route = if scroll {
                        format!(
                            "/m?k=scroll&x={x}&y={y}&dx={}&dy={}&wait=1",
                            num_arg(vm, args, id!(dx))?,
                            num_arg(vm, args, id!(dy))?
                        )
                    } else {
                        format!("/click?x={x}&y={y}&wait=1")
                    };
                    if !rt(vm).validate {
                        rt(vm).request_json(index, &route, true)?;
                    }
                    Ok(Value::Null)
                })();
                result(vm, r)
            },
        );
    }
    vm.add_method(
        obj,
        id_lut!(get),
        script_args!(route = NIL),
        move |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let r = str_arg(vm, args, id!(route)).and_then(|s| {
                if rt(vm).validate {
                    Ok(json::s("{}"))
                } else {
                    rt(vm).request(index, &s, true).map(json::s)
                }
            });
            result(vm, r)
        },
    );
    vm.add_method(
        obj,
        id_lut!(snap),
        script_args!(query = NIL),
        move |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let r = str_arg(vm, args, id!(query)).and_then(|s| {
                if rt(vm).validate {
                    Ok(Value::Arr(vec![]))
                } else {
                    rt(vm)
                        .request_json(index, &format!("/snap?q={}", encode(&s)), false)
                        .map(|v| {
                            v.get("s")
                                .or_else(|| v.get("entries"))
                                .or_else(|| v.get("items"))
                                .cloned()
                                .unwrap_or(v)
                        })
                }
            });
            result(vm, r)
        },
    );
    vm.add_method(
        obj,
        id_lut!(wait_log),
        script_args!(substring = NIL, secs = NIL),
        move |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let r = (|| {
                let s = str_arg(vm, args, id!(substring))?;
                let secs = num_arg(vm, args, id!(secs))?;
                if !rt(vm).validate {
                    rt(vm).wait_log(index, &s, secs)?;
                }
                Ok(Value::Null)
            })();
            result(vm, r)
        },
    );
    vm.add_method(obj, id_lut!(log), script_args!(), move |vm, _| {
        if !allow(vm) {
            return NIL;
        }
        let r = if rt(vm).validate {
            Ok(json::s(""))
        } else {
            rt(vm).log_text(index).map(json::s)
        };
        result(vm, r)
    });
    vm.add_method(obj, id_lut!(no_errors), script_args!(), move |vm, _| {
        if !allow(vm) {
            return NIL;
        }
        let r = if rt(vm).validate {
            Ok(Value::Null)
        } else {
            rt(vm).no_errors(index).map(|_| Value::Null)
        };
        result(vm, r)
    });
    vm.add_method(obj, id_lut!(alive), script_args!(), move |vm, _| {
        if !allow(vm) {
            return NIL;
        }
        let r = if rt(vm).validate {
            Ok(Value::Bool(true))
        } else {
            rt(vm).alive(index).map(Value::Bool)
        };
        result(vm, r)
    });
    vm.add_method(
        obj,
        id_lut!(grab),
        script_args!(name = NIL),
        move |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let r = str_arg(vm, args, id!(name)).and_then(|s| {
                if rt(vm).validate {
                    Ok(Value::Int(0))
                } else {
                    rt(vm).grab(index, &s).map(|i| Value::Int(i as i64))
                }
            });
            result(vm, r)
        },
    );
    vm.add_method(obj, id_lut!(quit), script_args!(), move |vm, _| {
        if !allow(vm) {
            return NIL;
        }
        let r = if rt(vm).validate {
            Ok(Value::Null)
        } else {
            rt(vm).quit(index).map(|_| Value::Null)
        };
        result(vm, r)
    });
    obj.into()
}

pub fn run_script(
    run: Run,
    config: Config,
    hub: Judge,
    script: &Script,
    permit: Option<crate::runner::Permit>,
) -> Run {
    execute(run, config, hub, script, false, permit).0
}
fn execute(
    run: Run,
    config: Config,
    hub: Judge,
    script: &Script,
    validate: bool,
    permit: Option<crate::runner::Permit>,
) -> (Run, Vec<String>) {
    let host = if validate {
        crate::cargo::Host {
            target: "host".into(),
            targets: vec!["wasm32-unknown-unknown".into()],
            nightly: None,
        }
    } else {
        let shared = run.cargo_cache.clone();
        let mut cache = shared.lock().unwrap();
        let detected = match &cache.host {
            Some(host) => Ok(host.clone()),
            None => crate::cargo::Host::detect(&run),
        };
        match detected {
            Ok(h) => { cache.host = Some(h.clone()); h },
            Err(e) => {
                let mut run = run;
                run.fail("host discovery", &e);
                return (run, Vec::new());
            }
        }
    };
    let manifest_path = script
        .target
        .as_ref()
        .map(|t| t.manifest.clone())
        .unwrap_or_else(|| {
            script
                .path
                .parent()
                .unwrap_or(Path::new("."))
                .join("Cargo.toml")
        });
    let manifest = fs::read_to_string(run.root.join(&manifest_path))
        .ok()
        .and_then(|text| makepad_toml_parser::parse_toml(&text).ok())
        .and_then(|doc| {
            doc.get_path(&["package", "name"])
                .and_then(|v| v.as_str())
                .map(|name| (name.to_string(), manifest_path))
        });
    let source = fs::read_to_string(run.root.join(&script.path));
    let mut host = ScriptVmHost::new(
        Runtime {
            run,
            config,
            hub,
            apps: Vec::new(),
            images: Vec::new(),
            group: script.name.clone(),
            step: None,
            suppressed: false,
            fatal: false,
            skipped: false,
            validate,
            validated_steps: Vec::new(),
            host,
            permit,
            warning: String::new(),
            manifest,
        },
        (),
    );
    match source {
        Err(e) => host
            .host
            .run
            .fail(&script.name, &format!("{}: {e}", script.path.display())),
        Ok(code) => {
            let mut vm = ScriptVm {
                host: &mut host,
                bx: Box::new(ScriptVmBase::new()),
            };
            register(&mut vm, script.target.as_ref());
            vm.bx.captured_errors = Some(Vec::new());
            let answer = vm.with_instruction_limit(5_000_000, |vm| {
                vm.eval(ScriptMod {
                    file: script.path.display().to_string(),
                    code,
                    ..Default::default()
                })
            });
            let errors = vm.take_errors();
            if !errors.is_empty() {
                rt(&mut vm).fail(&errors.join("\n"));
            }
            if answer.is_err() {
                rt(&mut vm).fail("script returned an error");
            }
        }
    }
    let runtime = &mut host.host;
    for i in 0..runtime.apps.len() {
        let stage = runtime.run.plan(&format!(
            "{} / cleanup {}",
            runtime.group, runtime.apps[i].name
        ));
        runtime.run.begin(stage, "GET /gq");
        match runtime.quit(i) {
            Ok(()) => runtime.run.end(stage, "passed", ""),
            Err(e) => runtime.run.end(stage, "failed", &e),
        }
    }
    (host.host.run, host.host.validated_steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{process::Control, report::Notify};
    #[test]
    fn an_input_route_is_never_asked_twice_a_grab_is() {
        for route in ["/k?k=down&c=KeyB&wait=1", "/m?k=click&x=1&y=2", "/t?t=browser", "/click?x=1&y=2", "/drop?path=/a&x=1&y=1"] {
            assert!(applies_input(route), "{route}");
        }
        for route in ["/g", "/gseq?n=8", "/log?n=10", "/gq", "/quit", "/activity", "/snap?q=x"] {
            assert!(!applies_input(route), "{route}");
        }
    }
    #[test]
    fn contract_scripts_execute_on_real_vm_without_io() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let config = Config {
            remote: "origin".into(),
            branches: vec!["work".into()],
            poll_secs: 60,
            checkout: root.clone(),
            skip_apps: vec![],
            model: "qwen3.5-4b-vision".into(),
            targets: vec![],
            allowed_errors: vec![],
            no_vision: true,
            parallel: 2,
            deep_tests: false,
            machines: Default::default(),
        };
        let judge = crate::uihub::JudgeWorker::start(&config.model, true).unwrap();
        // The three contract scripts with their pinned step counts, then every
        // app script in the tree: each follows the default shape, two steps.
        let mut scripts: Vec<(String, usize)> = vec![
            ("ci.splash".into(), 3),
            ("apps/wm/ci.splash".into(), 6),
            ("tools/ci/default.ci.splash".into(), 2),
        ];
        let mut apps: Vec<_> = fs::read_dir(root.join("apps"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|app| app != "wm" && root.join("apps").join(app).join("ci.splash").is_file())
            .collect();
        apps.sort();
        assert!(apps.len() >= 20, "the app scripts are missing: {apps:?}");
        // The terminal, and director which embeds one, also build the pty
        // helper a terminal starts its shell through.
        scripts.extend(apps.into_iter().map(|app| {
            let steps = if app == "terminal" || app == "director" { 3 } else { 2 };
            (format!("apps/{app}/ci.splash"), steps)
        }));
        for (path, count) in scripts.iter().map(|(path, count)| (path.as_str(), *count)) {
            let notify: Notify = Arc::new(|_| {});
            let run = Run::new(
                &root,
                root.clone(),
                "vm-contract",
                "test",
                Control::default(),
                notify,
            )
            .unwrap();
            let dir = run.dir.clone();
            let script = Script {
                path: path.into(),
                name: "contract".into(),
                target: Some(Target {
                    package: "test".into(),
                    binary: "test".into(),
                    cwd: ".".into(),
                    manifest: PathBuf::new(),
                    name: "test".into(),
                }),
            };
            let mut run = run;
            run.app_targets = Arc::new(script.target.iter().cloned().collect());
            let (run, steps) = execute(
                run,
                config.clone(),
                judge.judge.clone(),
                &script,
                true,
                None,
            );
            assert!(!run.failed, "{} failed: {:?}", path, run.stages);
            assert_eq!(steps.len(), count);
            drop(run);
            fs::remove_dir_all(dir).unwrap();
        }
        judge.finish();
    }
    #[test]
    fn remote_log_envelope_preserves_errors_and_wm_launch_lines() {
        let value=json::parse_depth(br#"{"n":7,"pool":"not a log line","l":["[E] shader failed","wm: launched terminal as client 2"]}"#,32).unwrap();
        let mut lines = Vec::new();
        collect_log(&value, &mut lines);
        assert_eq!(
            lines,
            vec!["[E] shader failed", "wm: launched terminal as client 2"]
        );
    }
    #[test]
    fn startup_requires_owned_pid_and_type_text_rejects_unsupported_input() {
        assert_eq!(
            parse_startup(
                "[makepad-remote] listening on 127.0.0.1:1234 pid=7 app=test",
                7
            ),
            Some(1234)
        );
        assert_eq!(
            parse_startup("[makepad-remote] listening on 127.0.0.1:1234 pid=8", 7),
            None
        );
        assert!(character_key('é').is_err());
        assert_eq!(character_key('A').unwrap(), ("KeyA".into(), true));
    }
}

/// What a script's `wait_log` text matches. Plain text is a substring of the
/// log. A `*` is a gap inside ONE line: the parts on either side must appear
/// on the same line in order, which is how a script waits for a line that
/// carries a number it cannot know (`terminal client * first frame`).
fn log_has(log: &str, text: &str) -> bool {
    if !text.contains('*') {
        return log.contains(text);
    }
    let parts: Vec<&str> = text.split('*').filter(|part| !part.is_empty()).collect();
    log.lines().any(|line| {
        let mut rest = line;
        parts.iter().all(|part| match rest.find(part) {
            Some(at) => {
                rest = &rest[at + part.len()..];
                true
            }
            None => false,
        })
    })
}

#[cfg(test)]
mod log_has_tests {
    use super::log_has;

    #[test]
    fn a_star_is_a_gap_inside_one_line_and_never_across_lines() {
        let log = "wm: launched terminal as client 3\nwm: terminal client 3 first frame in 812 ms (cold)\nwm: browser client 7 launched";
        assert!(log_has(log, "launched terminal"));
        assert!(log_has(log, "terminal client * first frame"));
        assert!(!log_has(log, "browser client * first frame"), "the browser has not drawn yet");
        assert!(!log_has(log, "launched terminal * first frame"), "the parts are on different lines");
        assert!(!log_has(log, "first frame * terminal client"), "order matters");
    }
}
