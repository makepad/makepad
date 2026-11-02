//! Compile/resolve misses: cargo supplies the verdict, the master remembers which JIT targets
//! to skip until their miss is closed. The JSONL ledger has one current row per diagnostic.

use makepad_strict_json::{obj, s, Value};
use std::path::Path;
use std::sync::Mutex;

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Diagnostics may come from a coloured cargo launcher, the JIT's short format, or JSON.
pub(crate) fn plain(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.clone().next() == Some('[') {
            chars.next();
            for c in chars.by_ref() { if ('@'..='~').contains(&c) { break; } }
        } else { out.push(c); }
    }
    out
}

pub(crate) fn running_tests(line: &str) -> bool {
    let mut words = line.split_whitespace();
    words.next() == Some("running") && words.next().is_some_and(|n| n.parse::<u64>().is_ok())
        && matches!(words.next(), Some("test" | "tests")) && words.next().is_none()
}

/// Only output outside a target's harness window (`running N tests` .. `test result:`) can
/// be a compile miss. A package run builds and runs one target after another, so one target's
/// compile error counts even when other targets ran. Diagnostics in a failing test's captured
/// output inside its window never turn that test into a JIT miss.
pub(crate) fn compile_errors(log: &str) -> Vec<String> {
    let log = plain(log);
    let mut errors: Vec<String> = Vec::new();
    let mut summary = Vec::new();
    let mut harness = false;
    for line in log.lines().map(str::trim) {
        if running_tests(line) { harness = true; continue; }
        if harness {
            if line.starts_with("test result:") { harness = false; }
            continue;
        }
        if let Ok(v) = makepad_strict_json::parse(line.as_bytes()) {
            if text(&v, "severity") == "error" {
                let file = text(&v, "file");
                let message = text(&v, "message").lines().next().unwrap_or("");
                errors.push(if file.is_empty() { message.to_string() } else {
                    format!("{file}:{}:{}: {message}", v.get("line").and_then(Value::as_u64).unwrap_or(0), v.get("col").and_then(Value::as_u64).unwrap_or(0))
                });
                continue;
            }
        }
        if let Some(location) = line.strip_prefix("--> ") {
            if let Some(error) = errors.last_mut().filter(|e| e.starts_with("error")) { *error = format!("{location}: {error}"); }
            continue;
        }
        if line.starts_with("error: could not compile") || line.starts_with("error: aborting")
            || line.starts_with("makepad-run:") && (line.contains("unresolved imports") || line.contains("signature errors") || line.contains("load error")) {
            summary.push(line.to_string());
        } else if line.starts_with("error: test failed") || line.contains("panicked at") || line.contains("internal compiler error") || line.contains("makepad run bug") || line.contains("jit panic") || line.contains("jit ice") {
            continue;
        } else if line.starts_with("error:") || line.starts_with("error[")
            || line.contains(": error:") || line.contains(": error[")
            || line.split_once(".rs:").is_some_and(|(_, rest)| {
                let Some((line, message)) = rest.split_once(':') else { return false };
                line.parse::<u64>().is_ok() && !message.trim().is_empty()
                    && message.trim().trim_end_matches(':').parse::<u64>().is_err()
            }) {
            errors.push(line.to_string());
        }
    }
    if errors.is_empty() { summary } else { errors }
}

/// The spelling distinguishes `--lib` from a test target actually named `lib`.
pub(crate) fn target(args: &[String]) -> String {
    let select = &args[..args.iter().position(|a| a == "--").unwrap_or(args.len())];
    for (i, arg) in select.iter().enumerate() {
        match arg.as_str() {
            "--lib" | "--tests" => return arg.clone(),
            "--test" => return format!("--test {}", select.get(i + 1).map_or("", String::as_str)),
            _ => {}
        }
    }
    "default".into()
}

struct Miss {
    id: String,
    pkg: String,
    target: String,
    error: Vec<String>,
    lane: String,
    pair: crate::state::Pair,
    first_ms: u64,
    last_ms: u64,
    count: u64,
    closed_ms: u64,
}

impl Miss {
    fn value(&self) -> Value {
        obj(vec![
            ("id", s(&self.id)), ("pkg", s(&self.pkg)), ("target", s(&self.target)),
            ("error", Value::Arr(self.error.iter().map(|e| s(e)).collect())),
            ("lane", s(&self.lane)), ("pair", obj(vec![("work", s(&self.pair.work)), ("main", s(&self.pair.main))])),
            ("first_ms", Value::Int(self.first_ms as i64)), ("last_ms", Value::Int(self.last_ms as i64)),
            ("count", Value::Int(self.count as i64)), ("closed_ms", Value::Int(self.closed_ms as i64)),
        ])
    }

    fn parse(v: &Value) -> Result<Self, String> {
        let id = text(v, "id");
        let errors: Vec<String> = v.get("error").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect();
        if id.strip_prefix('M').and_then(|s| s.parse::<u64>().ok()).is_none() || errors.is_empty() {
            return Err("jit misses: invalid ledger row".into());
        }
        let n = |k| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        let pair = v.get("pair").map(|p| crate::state::Pair::new(text(p, "work"), text(p, "main"))).unwrap_or_default();
        Ok(Self { id: id.into(), pkg: text(v, "pkg").into(), target: text(v, "target").into(), error: errors, lane: text(v, "lane").into(), pair, first_ms: n("first_ms"), last_ms: n("last_ms"), count: n("count"), closed_ms: n("closed_ms") })
    }

    fn touch(&mut self, req: &Value, now: u64) -> Result<(), String> {
        let pin = crate::lease::Pin::of_rev(text(req, "rev")).ok_or("jit misses: missing commit pair")?;
        self.pair = crate::state::Pair::new(&pin.makepad, pin.commercial.as_deref().unwrap_or(""));
        self.lane = text(req, "lane").into();
        self.last_ms = now;
        self.count += 1;
        self.closed_ms = 0;
        Ok(())
    }
}

pub(crate) fn notice(v: &Value) -> String {
    let error = v.get("error").and_then(Value::as_arr).and_then(|a| a.first()).and_then(Value::as_str).unwrap_or("");
    format!("devhub test: JIT miss on {} {} (recorded {}): {error}; this verdict is cargo's", text(v, "pkg"), text(v, "target"), text(v, "id"))
}

/// The old CI refusal seeds a miss once. A closed row overrides the stale mark, allowing the
/// next attempt to reach the JIT; a real miss on that attempt reopens its diagnostic's row.
fn ci_error(home: &Path, pkg: &str) -> Option<Vec<String>> {
    crate::state::Store::open_quiet(home, "").read(|d| {
        d.ci.timing(&format!("failing:{pkg}"))?;
        let id = format!("test:{pkg}");
        let shard = d.ci.run.iter().chain(d.ci.sweep_run.iter()).chain(d.ci.runs.iter().rev())
            .flat_map(|r| &r.shards).find(|s| s.state == "fail" && (s.id == id || s.id.starts_with(&format!("{id}:"))));
        // a crate whose tests merely fail is no JIT miss: only compile or resolve errors seed one
        // (any failing mark did, and `devhub test` then answered with cargo's verdict)
        let compile = compile_errors(&shard?.errors.join("\n"));
        if compile.is_empty() { return None; }
        let mut errors = vec![format!("CI marks failing:{pkg}: the JIT could not build its tests")];
        errors.extend(compile.into_iter().take(4));
        Some(errors)
    })
}

/// Called only on the master. Serialize read/modify/rename, including across simultaneous
/// clients; a failed write leaves the old ledger intact and is reported to the caller.
pub(crate) fn serve(home: &Path, line: &str, now: u64, event: &dyn Fn(&str, &Value)) -> Result<String, String> {
    static LEDGER: Mutex<()> = Mutex::new(());
    let _guard = LEDGER.lock().unwrap();
    let path = home.join("jit_misses.jsonl");
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut entries: Vec<Miss> = contents.lines().filter(|l| !l.trim().is_empty()).map(|l| {
        let v = makepad_strict_json::parse(l.as_bytes()).map_err(|e| format!("{}: {e:?}", path.display()))?;
        Miss::parse(&v)
    }).collect::<Result<_, String>>()?;
    let rest = line.strip_prefix("misses").unwrap_or(line).trim();
    if rest.is_empty() || rest == "--all" {
        let list: Vec<_> = entries.iter().filter(|m| rest == "--all" || m.closed_ms == 0).collect();
        let mut out = String::new();
        for m in &list {
            out.push_str(&format!("{} {} {} {} ({} times, lane {}, pair {}, first {}, last {}): {}\n", m.id, if m.closed_ms == 0 { "open" } else { "closed" }, m.pkg, m.target, m.count, m.lane, m.pair.short(), m.first_ms, m.last_ms, m.error[0]));
        }
        if list.is_empty() { out.push_str("no JIT misses\n"); }
        return Ok(obj(vec![("text", s(&out)), ("misses", Value::Arr(list.iter().map(|m| m.value()).collect()))]).to_json());
    }
    let (verb, arg) = rest.split_once(' ').ok_or("jit misses [--all] | close <id>")?;
    let (index, action) = match verb {
        "close" => {
            let i = entries.iter().position(|m| m.id == arg.trim()).ok_or_else(|| format!("no JIT miss {}", arg.trim()))?;
            entries[i].closed_ms = now;
            (i, "closed")
        }
        "lookup" | "record" => {
            let req = makepad_strict_json::parse(arg.as_bytes()).map_err(|_| "jit misses: bad request")?;
            let (pkg, target) = (text(&req, "pkg"), text(&req, "target"));
            if pkg.is_empty() || target.is_empty() { return Err("jit misses: pkg and target required".into()); }
            let existing = entries.iter().position(|m| m.pkg == pkg && m.target == target && m.closed_ms == 0);
            let errors = if verb == "lookup" {
                if let Some(i) = existing { entries[i].error.clone() }
                else if entries.iter().any(|m| m.pkg == pkg && m.target == target) { return Ok("null".into()); }
                else if let Some(errors) = ci_error(home, pkg) { errors }
                else { return Ok("null".into()); }
            } else {
                req.get("error").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).flat_map(str::lines).take(5).map(str::to_string).collect()
            };
            if errors.is_empty() { return Err("jit misses: error lines required".into()); }
            let i = entries.iter().position(|m| m.pkg == pkg && m.target == target && m.error.first() == errors.first()).unwrap_or_else(|| {
                let id = entries.iter().filter_map(|m| m.id[1..].parse::<u64>().ok()).max().unwrap_or(0) + 1;
                entries.push(Miss { id: format!("M{id}"), pkg: pkg.into(), target: target.into(), error: errors.clone(), lane: String::new(), pair: Default::default(), first_ms: now, last_ms: now, count: 0, closed_ms: 0 });
                entries.len() - 1
            });
            entries[i].touch(&req, now)?;
            entries[i].error = errors;
            (i, if verb == "lookup" && existing.is_some() { "used" } else { "recorded" })
        }
        _ => return Err("jit misses [--all] | close <id>".into()),
    };
    let tmp = path.with_extension("jsonl.new");
    let contents: String = entries.iter().map(|m| format!("{}\n", m.value().to_json())).collect();
    std::fs::create_dir_all(home).and_then(|_| std::fs::write(&tmp, contents)).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))?;
    let value = entries[index].value();
    drop(_guard);
    event(action, &value);
    Ok(value.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_that_fails_to_compile_between_passing_targets_is_a_miss() {
        let error = "/home/me/devhub/ci-tree/apps/commercial/scope/deps/code_atlas/tests/relief.rs:160:62: E0277: the trait bound `HashMap<Arc<str>, Arc<str>, RandomState>: FromIterator<<Map<Iter<String, FrozenFile>, closure#825> as Iterator>::Item>` is not satisfied";
        let log = format!("running 2 tests
test a ... ok
test b ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.68s

{error}
error: could not compile `relief` (test) due to 1 previous error: no test ran (`makepad test` fails as `cargo test` does on code rustc rejects; if `cargo test` builds it, this is a makepad run bug)

running 10 tests
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.83s
");
        assert_eq!(compile_errors(&log), vec![error.to_string()]);
        let log = "running 1 test
test t ... FAILED

failures:

---- t stdout ----
src/a.rs:1:2: E0308: mismatched types
error: could not compile `a` (lib) due to 1 previous error

failures:
    t

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";
        assert!(compile_errors(log).is_empty());
    }
}
