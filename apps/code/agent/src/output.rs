//! Command output for the model: the failure patterns that end a wait early
//! (run_command's early yield), and the compact form of an output (errors,
//! failing tests, summaries) that replaces a cut of the whole text. Pure
//! text in, text out; run_command and run_output in tools.rs use it.

use makepad_regex::Regex;

/// Size of a compact result (the local models prefill slowly; keep it short).
pub const COMPACT_CHARS: usize = 4000;
/// Output this short goes to the model as it is.
const RAW_CHARS: usize = 1500;
/// Lines one diagnostic or failure block keeps.
const BLOCK_LINES: usize = 14;
/// Warnings shown when there are errors too / when there are none.
const WARNINGS_WITH_ERRORS: usize = 2;
const WARNINGS_ALONE: usize = 6;

/// What ends a wait early: the built-in failure shapes plus the model's own
/// (`early_yield_patterns`: literal text, or a regex written `/like this/`).
pub struct Patterns {
    custom: Vec<Custom>,
}

enum Custom {
    Literal(String),
    Regex(String, Regex),
}

impl Patterns {
    /// The built-ins plus `extra`; a bad regex is an error naming it.
    pub fn new(extra: &[String]) -> Result<Patterns, String> {
        let mut custom = Vec::new();
        for p in extra.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
            match p.strip_prefix('/').and_then(|r| r.strip_suffix('/')).filter(|r| !r.is_empty()) {
                Some(re) => match Regex::new(re) {
                    Ok(r) => custom.push(Custom::Regex(p.to_string(), r)),
                    Err(e) => return Err(format!("bad early_yield_patterns regex {p}: {e:?}")),
                },
                None => custom.push(Custom::Literal(p.to_string())),
            }
        }
        Ok(Patterns { custom })
    }

    /// The pattern `line` matches, as it reads in the result header.
    pub fn find(&self, line: &str) -> Option<String> {
        if let Some(p) = builtin(line) {
            return Some(p.to_string());
        }
        self.custom.iter().find_map(|c| match c {
            Custom::Literal(t) => line.contains(t.as_str()).then(|| t.clone()),
            Custom::Regex(shown, r) => r.earliest_match(line).is_some().then(|| shown.clone()),
        })
    }
}

/// The built-in failure shapes of common build and test tools.
pub fn builtin(line: &str) -> Option<&'static str> {
    let t = line.trim_start();
    if t.starts_with("error[E") {
        return Some("error[E");
    }
    // `error:` opening the line (rustc, cargo, many tools); not `my_error:`.
    if t.starts_with("error:") {
        return Some("error:");
    }
    if t.starts_with("FAIL ") || t.starts_with("FAIL\t") {
        return Some("FAIL");
    }
    if t.starts_with("--- FAIL") {
        return Some("--- FAIL");
    }
    const ANYWHERE: [&str; 6] = ["panicked at", "test result: FAILED", "AssertionError", "Traceback (most recent call last)", "BUILD FAILED", "npm ERR!"];
    if let Some(p) = ANYWHERE.iter().find(|p| line.contains(*p)) {
        return Some(p);
    }
    if has_failed_word(line) {
        return Some("FAILED");
    }
    None
}

/// `FAILED` as a word of its own (`test a ... FAILED`, `FAILED tests/x.py`),
/// not a count of none (`0 FAILED`) and not inside a word (`UNFAILED`).
fn has_failed_word(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(at) = line[from..].find("FAILED").map(|i| i + from) {
        let end = at + 6;
        let before_ok = at == 0 || !bytes[at - 1].is_ascii_alphanumeric() && bytes[at - 1] != b'_';
        let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric() && bytes[end] != b'_';
        let zero = line[..at].trim_end().rsplit(|c: char| c.is_whitespace()).next() == Some("0");
        if before_ok && after_ok && !zero {
            return true;
        }
        from = end;
    }
    false
}

/// What a line of output is, for picking the lines worth showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Error,
    Warning,
    Failure,
    Summary,
}

/// A diagnostic header: `error[E0308]: ...`, `error: ...`, `warning: ...`.
/// Cargo's own tallies (`could not compile`, `generated 3 warnings`) are
/// summaries.
fn diagnostic(t: &str) -> Option<Kind> {
    let (kind, rest) = if let Some(r) = t.strip_prefix("error") {
        (Kind::Error, r)
    } else if let Some(r) = t.strip_prefix("warning") {
        (Kind::Warning, r)
    } else {
        return None;
    };
    let msg = if let Some(r) = rest.strip_prefix('[') {
        r.split_once("]:")?.1
    } else {
        rest.strip_prefix(':')?
    };
    let msg = msg.trim();
    let tally = msg.starts_with("could not compile")
        || msg.starts_with("aborting due to")
        || msg.starts_with("test failed")
        || msg.starts_with("build failed")
        || (msg.contains(" generated ") && msg.contains("warning"))
        || msg.ends_with("warnings emitted")
        || msg.ends_with("warning emitted");
    Some(if tally { Kind::Summary } else { kind })
}

/// Lines that end or sum up a run.
fn is_summary(t: &str) -> bool {
    t.starts_with("test result:")
        || t.starts_with("Finished ")
        || t.starts_with("error: test failed")
        || t.starts_with("Tests:")
        || t.starts_with("Test Suites:")
        || (t.starts_with("ok  ") || t.starts_with("ok\t"))
        || (t.starts_with("FAIL\t") || t == "FAIL")
        || t.starts_with("BUILD ")
        || (t.starts_with('=') && t.ends_with('=') && (t.contains(" passed") || t.contains(" failed") || t.contains(" error")))
        || matches!(diagnostic(t), Some(Kind::Summary))
}

/// A line that carries on the block above it (rustc's `-->`, `|` gutters,
/// notes, indented text).
fn continues(line: &str) -> bool {
    let t = line.trim_start();
    line.starts_with(' ')
        || line.starts_with('\t')
        || t.starts_with("-->")
        || t.starts_with('|')
        || t.starts_with("= ")
        || t.starts_with("note:")
        || t.starts_with("help:")
        || t.starts_with("...")
        || t.split_once('|').is_some_and(|(n, _)| !n.trim().is_empty() && n.trim().chars().all(|c| c.is_ascii_digit()))
}

/// One block of interesting lines: its first and last line (inclusive).
#[derive(Clone, Debug)]
struct Span {
    kind: Kind,
    from: usize,
    to: usize,
}

/// The interesting blocks of `lines`, in order.
fn spans(lines: &[&str]) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_start();
        // rustc diagnostics and their location/source/help lines.
        if let Some(kind) = diagnostic(t).filter(|k| *k != Kind::Summary) {
            let mut j = i + 1;
            while j < lines.len() && continues(lines[j]) && j - i < 60 {
                j += 1;
            }
            out.push(Span { kind, from: i, to: (j - 1).min(i + BLOCK_LINES - 1) });
            i = j;
            continue;
        }
        // A test's captured output: `---- name stdout ----` up to the next
        // block; the panic and its message are what count.
        if t.starts_with("---- ") && t.ends_with(" ----") {
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim_start().starts_with("---- ") && lines[j].trim() != "failures:" {
                j += 1;
            }
            let panic = (i..j).find(|&k| lines[k].contains("panicked at"));
            let to = match panic {
                Some(p) => (p + 4).min(j - 1),
                None => (i + 6).min(j - 1),
            };
            out.push(Span { kind: Kind::Failure, from: i, to });
            i = j;
            continue;
        }
        // A panic outside a test block (a binary, a doc test).
        if line.contains("panicked at") {
            let mut j = i + 1;
            while j < lines.len() && j <= i + 3 && !lines[j].trim().is_empty() && !lines[j].starts_with("note: run with") {
                j += 1;
            }
            out.push(Span { kind: Kind::Failure, from: i, to: j - 1 });
            i = j;
            continue;
        }
        // Python tracebacks: to the exception line.
        if t.starts_with("Traceback (most recent call last)") {
            let mut j = i + 1;
            while j < lines.len() && (lines[j].starts_with(' ') || lines[j].trim().is_empty()) {
                j += 1;
            }
            let to = j.min(lines.len() - 1);
            let from = if to - i > BLOCK_LINES { to + 1 - BLOCK_LINES } else { i };
            out.push(Span { kind: Kind::Failure, from, to });
            i = to + 1;
            continue;
        }
        // Failing test lines: cargo `test a::b ... FAILED`, go `--- FAIL:`,
        // pytest `FAILED x.py::t - msg` / `E   assert ...`, jest `●`, npm ERR!.
        let failing = (t.starts_with("test ") && t.ends_with("FAILED"))
            || t.starts_with("--- FAIL")
            || t.starts_with("FAILED ")
            || t.starts_with("ERROR ")
            || t.starts_with("E   ")
            || t.starts_with("● ")
            || t.starts_with("npm ERR!")
            || t.starts_with("panic:")
            || t.contains("AssertionError");
        if failing {
            let mut j = i + 1;
            // go and jest indent the detail under the failing line.
            if t.starts_with("--- FAIL") || t.starts_with("● ") || t.starts_with("panic:") {
                while j < lines.len() && j < i + 8 && (lines[j].starts_with(' ') || lines[j].starts_with('\t')) {
                    j += 1;
                }
            }
            out.push(Span { kind: Kind::Failure, from: i, to: j - 1 });
            i = j;
            continue;
        }
        if is_summary(t) {
            out.push(Span { kind: Kind::Summary, from: i, to: i });
        }
        i += 1;
    }
    out
}

/// The compact form of a command's output for the model: short output as
/// it is; longer output as its errors (with location, source and help),
/// failing tests with their panic messages, a few warnings, the summary
/// lines and the last lines, in output order with `...` for the gaps, at
/// most `cap` chars. `more` is the pointer to the rest (`run_output 7`).
pub fn compact(text: &str, cap: usize, more: Option<&str>) -> String {
    let text = text.trim_end_matches('\n');
    if text.chars().count() <= RAW_CHARS.min(cap) {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let found = spans(&lines);
    let errors = found.iter().filter(|s| s.kind == Kind::Error).count();
    let warnings = found.iter().filter(|s| s.kind == Kind::Warning).count();
    let failures = found.iter().filter(|s| s.kind == Kind::Failure).count();
    let warn_keep = if errors + failures > 0 { WARNINGS_WITH_ERRORS } else { WARNINGS_ALONE };
    // Most important first: errors and failures, the summary lines, then
    // the last lines, then a few warnings.
    let mut ranked: Vec<Span> = Vec::new();
    ranked.extend(found.iter().filter(|s| matches!(s.kind, Kind::Error | Kind::Failure)).cloned());
    ranked.extend(found.iter().filter(|s| s.kind == Kind::Summary).cloned());
    let tail_from = lines.len().saturating_sub(4);
    ranked.push(Span { kind: Kind::Summary, from: tail_from, to: lines.len() - 1 });
    ranked.extend(found.iter().filter(|s| s.kind == Kind::Warning).take(warn_keep).cloned());
    // Take spans while they fit (dropped lines leave a gap).
    let budget = cap.saturating_sub(200);
    let mut keep = vec![false; lines.len()];
    let mut used = 0usize;
    let mut left_out = 0usize;
    for s in &ranked {
        let cost: usize = (s.from..=s.to).filter(|&k| !keep[k]).map(|k| lines[k].chars().count().min(300) + 1).sum();
        if used + cost > budget {
            left_out += 1;
            continue;
        }
        used += cost;
        for k in s.from..=s.to {
            keep[k] = true;
        }
    }
    let mut head = format!("[{} lines", lines.len());
    if errors > 0 {
        head.push_str(&format!(", {errors} error{}", if errors == 1 { "" } else { "s" }));
    }
    if warnings > 0 {
        head.push_str(&format!(", {warnings} warning{}", if warnings == 1 { "" } else { "s" }));
    }
    if failures > 0 {
        head.push_str(&format!(", {failures} failure line{}", if failures == 1 { "" } else { "s" }));
    }
    head.push_str("; showing ");
    head.push_str(if errors + failures + warnings > 0 { "the problems, summary and last lines" } else { "the summary and last lines" });
    if let Some(more) = more {
        head.push_str(&format!("; full output: {more}"));
    }
    head.push_str("]\n");
    let mut out = head;
    let mut gap = false;
    for (k, line) in lines.iter().enumerate() {
        if !keep[k] {
            gap = true;
            continue;
        }
        if gap && k > 0 {
            out.push_str("...\n");
        }
        gap = false;
        if line.chars().count() > 300 {
            out.extend(line.chars().take(300));
            out.push_str(" [line cut]\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if gap {
        out.push_str("...\n");
    }
    if left_out > 0 {
        out.push_str(&format!("[{left_out} more blocks left out for space]\n"));
    }
    out
}

/// One line about an output for a finish note: the test tally, else the
/// first error with its place, else the last line.
pub fn one_line(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let found = spans(&lines);
    let errors: Vec<&Span> = found.iter().filter(|s| s.kind == Kind::Error).collect();
    let tally = lines.iter().rev().find(|l| l.trim_start().starts_with("test result:")).map(|l| l.trim().to_string());
    let line = if let Some(first) = errors.first() {
        let head = lines[first.from].trim();
        let at = (first.from..=first.to).find_map(|k| lines[k].trim_start().strip_prefix("--> ")).map(|l| format!(" at {}", l.trim())).unwrap_or_default();
        format!("{} error{}, first: {head}{at}", errors.len(), if errors.len() == 1 { "" } else { "s" })
    } else if let Some(t) = tally {
        t
    } else if let Some(f) = found.iter().find(|s| s.kind == Kind::Failure) {
        lines[f.from].trim().to_string()
    } else {
        lines.iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).unwrap_or_else(|| "no output".into())
    };
    if line.chars().count() > 200 { line.chars().take(197).collect::<String>() + "..." } else { line }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo build --release` of a crate with one type error and a warning
    /// (captured from cargo 1.9x, paths shortened).
    const BUILD_ERROR: &str = "\
   Compiling libc v0.2.155
   Compiling cfg-if v1.0.0
   Compiling memchr v2.7.4
   Compiling probe v0.1.0 (/ws/probe)
warning: unused variable: `y`
 --> src/main.rs:2:9
  |
2 |     let y = 1;
  |         ^ help: if this is intentional, prefix it with an underscore: `_y`
  |
  = note: `#[warn(unused_variables)]` on by default

error[E0308]: mismatched types
 --> src/main.rs:3:18
  |
3 |     let x: u32 = \"five\";
  |            ---   ^^^^^^ expected `u32`, found `&str`
  |            |
  |            expected due to this

For more information about this error, try `rustc --explain E0308`.
warning: `probe` (bin \"probe\") generated 1 warning
error: could not compile `probe` (bin \"probe\") due to 1 previous error; 1 warning emitted
";

    /// `cargo test --release` with one failing test out of three.
    const TEST_FAIL: &str = "\
   Compiling probe v0.1.0 (/ws/probe)
    Finished `release` profile [optimized] target(s) in 0.61s
     Running unittests src/lib.rs (target/release/deps/probe-1f0c2a9b3d)

running 3 tests
test tests::adds ... ok
test tests::parses_numbers ... FAILED
test tests::splits ... ok

failures:

---- tests::parses_numbers stdout ----

thread 'tests::parses_numbers' panicked at src/lib.rs:21:9:
assertion `left == right` failed
  left: 41
 right: 42
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    tests::parses_numbers

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
";

    const TEST_OK: &str = "\
    Finished `release` profile [optimized] target(s) in 0.05s
     Running unittests src/lib.rs (target/release/deps/probe-1f0c2a9b3d)

running 3 tests
test tests::adds ... ok
test tests::parses_numbers ... ok
test tests::splits ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

";

    fn noisy(core: &str) -> String {
        // Many lines of progress around the core, as a big build has.
        let mut s: String = (0..300).map(|i| format!("   Compiling crate-{i} v0.1.{i} (/ws/libs/crate-{i})\n")).collect();
        s.push_str(core);
        s
    }

    #[test]
    fn built_in_patterns_hit_the_failure_shapes() {
        let hit = |l: &str| builtin(l);
        assert_eq!(hit("error[E0308]: mismatched types"), Some("error[E"));
        assert_eq!(hit("error: could not compile `x`"), Some("error:"));
        assert_eq!(hit("thread 'main' panicked at src/main.rs:2:5:"), Some("panicked at"));
        assert_eq!(hit("test result: FAILED. 2 passed; 1 failed"), Some("test result: FAILED"));
        assert_eq!(hit("test tests::parses ... FAILED"), Some("FAILED"));
        assert_eq!(hit("FAIL src/app.test.js"), Some("FAIL"));
        assert_eq!(hit("FAIL\tgithub.com/x/y\t0.01s"), Some("FAIL"));
        assert_eq!(hit("--- FAIL: TestParse (0.00s)"), Some("--- FAIL"));
        assert_eq!(hit("E   AssertionError: assert 41 == 42"), Some("AssertionError"));
        assert_eq!(hit("Traceback (most recent call last):"), Some("Traceback (most recent call last)"));
        assert_eq!(hit("BUILD FAILED in 3s"), Some("BUILD FAILED"));
        assert_eq!(hit("npm ERR! code ELIFECYCLE"), Some("npm ERR!"));
    }

    #[test]
    fn no_false_alarms() {
        for line in [
            "test result: ok. 3 passed; 0 failed; 0 ignored",
            "summary: 3 passed, 0 FAILED",
            "   Compiling terror v0.1.0",
            "let my_error: u32 = 1;",
            "log: no error: all good",
            "warning: unused variable: `x`",
            "UNFAILED_COUNT=0",
            "FAILURES: none",
            "test tests::fail_fast ... ok",
            "    Finished `release` profile [optimized] target(s) in 2.31s",
        ] {
            assert_eq!(builtin(line), None, "{line}");
        }
    }

    #[test]
    fn custom_patterns_literal_and_regex() {
        let p = Patterns::new(&["SEGV".into(), "/timeout after \\d+ ms/".into(), "  ".into()]).unwrap();
        assert_eq!(p.find("got SEGV in worker").as_deref(), Some("SEGV"));
        assert_eq!(p.find("request timeout after 300 ms").as_deref(), Some("/timeout after \\d+ ms/"));
        assert_eq!(p.find("timeout after many ms"), None);
        assert_eq!(p.find("all fine"), None);
        // Built-ins still apply.
        assert_eq!(p.find("error[E0425]: cannot find value").as_deref(), Some("error[E"));
        assert!(Patterns::new(&["/(unclosed/".into()]).is_err());
    }

    #[test]
    fn a_build_error_keeps_its_block_and_drops_the_progress() {
        let out = compact(&noisy(BUILD_ERROR), COMPACT_CHARS, Some("run_output 7"));
        assert!(out.len() <= COMPACT_CHARS, "{}", out.len());
        assert!(out.starts_with("[323 lines, 1 error, 1 warning; showing the problems, summary and last lines; full output: run_output 7]"), "{out}");
        for want in ["error[E0308]: mismatched types", " --> src/main.rs:3:18", "expected `u32`, found `&str`", "expected due to this", "could not compile `probe`", "warning: unused variable: `y`"] {
            assert!(out.contains(want), "missing {want}:\n{out}");
        }
        assert!(!out.contains("crate-10 "), "{out}");
        assert!(out.lines().count() < 40, "{out}");
        assert_eq!(one_line(BUILD_ERROR), "1 error, first: error[E0308]: mismatched types at src/main.rs:3:18");
    }

    #[test]
    fn a_failing_test_keeps_its_name_panic_and_tally() {
        let out = compact(&noisy(TEST_FAIL), COMPACT_CHARS, None);
        for want in [
            "test tests::parses_numbers ... FAILED",
            "---- tests::parses_numbers stdout ----",
            "panicked at src/lib.rs:21:9:",
            "  left: 41",
            " right: 42",
            "test result: FAILED. 2 passed; 1 failed",
            "error: test failed, to rerun pass `--lib`",
        ] {
            assert!(out.contains(want), "missing {want}:\n{out}");
        }
        assert!(!out.contains("tests::adds"), "{out}");
        assert!(out.lines().count() < 30, "{out}");
        assert_eq!(one_line(TEST_FAIL), "test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s");
    }

    #[test]
    fn success_is_short() {
        // Short output stays as it is.
        assert_eq!(compact(TEST_OK, COMPACT_CHARS, None), TEST_OK.trim_end());
        // Long successful output: the summary and the last lines.
        let out = compact(&noisy(TEST_OK), COMPACT_CHARS, None);
        assert!(out.contains("test result: ok. 3 passed"), "{out}");
        assert!(out.contains("Finished `release`"), "{out}");
        assert!(out.lines().count() < 12, "{out}");
        assert!(one_line(TEST_OK).starts_with("test result: ok. 3 passed"));
    }

    #[test]
    fn many_errors_stay_under_the_cap() {
        let block = BUILD_ERROR.replace("   Compiling", "  Compiling");
        let many: String = (0..60).map(|_| block.as_str()).collect();
        let out = compact(&many, COMPACT_CHARS, None);
        assert!(out.chars().count() <= COMPACT_CHARS, "{}", out.chars().count());
        assert!(out.contains("more blocks left out"), "{out}");
    }

    #[test]
    fn other_tools_failures() {
        let py = format!("{}\nTraceback (most recent call last):\n  File \"t.py\", line 3, in <module>\n    main()\n  File \"t.py\", line 2, in main\n    assert 1 == 2\nAssertionError\n", "x\n".repeat(900));
        let out = compact(&py, COMPACT_CHARS, None);
        assert!(out.contains("Traceback") && out.contains("assert 1 == 2") && out.contains("AssertionError"), "{out}");
        let go = format!("{}=== RUN   TestParse\n--- FAIL: TestParse (0.00s)\n    parse_test.go:12: got 41, want 42\nFAIL\nFAIL\texample.com/p\t0.002s\n", "=== RUN   TestOk\n--- PASS: TestOk (0.00s)\n".repeat(200));
        let out = compact(&go, COMPACT_CHARS, None);
        assert!(out.contains("--- FAIL: TestParse") && out.contains("got 41, want 42") && out.contains("FAIL\texample.com/p"), "{out}");
    }
}
