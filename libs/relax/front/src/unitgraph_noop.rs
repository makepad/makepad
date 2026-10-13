//! Conservative build-script elision. No package-name allowlist: a dependency
//! explicitly declares its inert exported helpers in its manifest. Anything
//! outside these small, token-checked forms goes through the ordinary runner.
use super::{BuildScript, RawUnit};
use std::ops::Range;

struct Parsed<'a> {
    lexed: Vec<crate::lexer::Tok>,
    tokens: Vec<&'a str>,
    main: Range<usize>,
}

fn parse(source: &str, edition: u16) -> Option<Parsed<'_>> {
    let mut lexed = Vec::new();
    crate::lexer::lex_with(source.as_bytes(), &crate::lexer::LexOptions {
        edition, mode: crate::lexer::Mode::Rust, trivia: false,
    }, &mut lexed).ok()?;
    lexed.pop();
    let tokens: Vec<_> = lexed.iter().map(|t| t.text(source)).collect();
    // Attributes/macros can replace main or establish a different entrypoint.
    // Explicit extern aliases could shadow the standard library in the guard.
    if tokens.iter().any(|t| matches!(*t, "#" | "extern")) { return None; }
    let mut stack = Vec::new();
    let mut main = None;
    let mut end = None;
    for (at, token) in tokens.iter().enumerate() {
        if stack.is_empty() && tokens[at..].starts_with(&["fn", "main", "(", ")", "{"]) {
            if main.replace(at + 5).is_some() { return None; }
        }
        match *token {
            "(" => stack.push(")"),
            "[" => stack.push("]"),
            "{" => stack.push("}"),
            ")" | "]" | "}" => {
                if stack.pop() != Some(*token) { return None; }
                if *token == "}" && stack.is_empty() && main.is_some() && end.is_none() {
                    end = Some(at);
                }
            }
            _ => {}
        }
    }
    let main = main? .. end?;
    if !stack.is_empty() || tokens[..main.start - 5].contains(&"!") { return None; }
    Some(Parsed { lexed, tokens, main })
}

fn statement_len(body: &[&str], expression_len: usize) -> Option<usize> {
    if body.len() == expression_len { Some(expression_len) }
    else if body.get(expression_len) == Some(&";") { Some(expression_len + 1) }
    else { None }
}

fn helper_call<'a>(body: &[&'a str]) -> Option<(usize, &'a str, &'a str)> {
    if body.len() < 5 || body[1] != "::" || body[3..5] != ["(", ")"] { return None; }
    Some((statement_len(body, 5)?, body[0], body[2]))
}

fn inert_helper(script: &BuildScript, units: &[RawUnit], dependency: &str, function: &str) -> bool {
    let Some(unit) = script.build_deps.iter().find(|(_, name)| name == dependency)
        .and_then(|(index, _)| units.get(*index)) else { return false };
    let Ok(manifest) = crate::vfs::read_to_string(format!("{}/Cargo.toml", unit.pkg.0)) else { return false };
    let Ok(manifest) = crate::toml::parse_toml(&manifest) else { return false };
    let Some(inert) = manifest.get_path(&["package", "metadata", "makepad-run", "build-script-noop"])
        .and_then(|value| value.as_array()) else { return false };
    inert.iter().any(|value| value.as_str() == Some(function))
}

fn check_cfg(body: &[&str]) -> Option<usize> {
    if body.len() < 5 || !body.starts_with(&["println", "!", "("]) || body[4] != ")" { return None; }
    let literal = body[3].strip_prefix('"')?.strip_suffix('"')?;
    // No formatting captures or escapes/newlines that could emit another cargo
    // directive. This is only cargo's lint configuration, ignored by Relax.
    if !literal.starts_with("cargo:rustc-check-cfg=") && !literal.starts_with("cargo::rustc-check-cfg=") { return None; }
    if literal.contains(['\\', '{', '}', '\n', '\r']) { return None; }
    statement_len(body, 5)
}

fn glob_import(tokens: &[&str]) -> bool {
    tokens.iter().enumerate().any(|(at, &t)| t == "use"
        && tokens[at..].iter().take_while(|&&t| t != ";").any(|&t| t == "*"))
}

fn unshadowed_println(tokens: &[&str]) -> bool {
    if glob_import(tokens) { return false; }
    let mut depth = 0usize;
    for (at, token) in tokens.iter().enumerate() {
        let word = token.strip_prefix("r#").unwrap_or(token);
        if word == "println" && tokens.get(at + 1) != Some(&"!") { return false; }
        // Other macros at item/main scope could generate an import that shadows
        // println, even if that macro occurs after the guard.
        if *token == "!" && depth <= 1 && (at == 0 || tokens[at - 1] != "println") { return false; }
        match *token {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth -= 1,
            _ => {}
        }
    }
    true
}

pub fn script(script: &BuildScript, units: &[RawUnit]) -> bool {
    let _why = crate::why::enabled().then(|| crate::why::enter(crate::why::Cause::new(&script.src_path, 1, "build-script-elision", "check inert build script")));
    let Ok(source) = crate::vfs::read_to_string(&script.src_path) else { return false };
    let Some(parsed) = parse(&source, script.edition) else { return false };
    let tokens = &parsed.tokens;
    let wrapper = parsed.main.start == 5 && parsed.main.end + 1 == tokens.len();
    let println = unshadowed_println(tokens);
    let mut body = &tokens[parsed.main];
    let mut before_guard = true;
    // With an absolute std path this expression cannot resolve to a script's
    // own env module. Only inert statements precede it; the runner sets the variable.
    let guard = ["if", "::", "std", "::", "env", "::", "var_os", "(",
        "\"MAKEPAD_RUN_BUILD_SCRIPT\"", ")", ".", "is_some", "(", ")", "{", "return", ";", "}"];
    while !body.is_empty() {
        if before_guard && body.starts_with(&guard) { return true; }
        if let Some(len) = check_cfg(body).filter(|_| println) {
            body = &body[len..];
            continue;
        }
        // Wrapper-only scripts have no imports/local definitions that could
        // shadow the dependency's extern name.
        if !wrapper { return false; }
        let Some((len, dep, function)) = helper_call(body) else { return false };
        if !inert_helper(script, units, dep, function) { return false; }
        before_guard = false;
        body = &body[len..];
    }
    wrapper
}

pub struct Rewrite {
    pub source: String,
    pub calls: Vec<(String, String)>,
}

/// A real script can still have inert helpers. Keeping their dependency loaded
/// can cost a dependency parse for a call that returns immediately.
pub fn rewrite(script: &BuildScript, units: &[RawUnit]) -> Option<Rewrite> {
    let source = crate::vfs::read_to_string(&script.src_path).ok()?;
    let parsed = parse(&source, script.edition)?;
    let tokens = &parsed.tokens;
    // Moving this source would change relative lookups or file!(). Attributes
    // (including #[path]) and extern aliases were already rejected by parse.
    if tokens.contains(&"mod") || tokens.windows(2).any(|w|
        matches!(w[0].strip_prefix("r#").unwrap_or(w[0]), "include" | "include_str" | "include_bytes" | "file") && w[1] == "!") {
        return None;
    }
    // A glob can shadow the dependency's extern name without spelling it here.
    if glob_import(tokens) { return None; }
    let mut candidates = Vec::new();
    let mut at = parsed.main.start;
    let mut depth = 0usize;
    let mut start = true;
    while at < parsed.main.end {
        if depth == 0 && start {
            if let Some((len, dep, function)) = helper_call(&tokens[at..parsed.main.end]) {
                if inert_helper(script, units, dep, function) {
                    candidates.push((at..at + len, dep, function));
                    at += len;
                    continue;
                }
            }
        }
        if depth == 0 { start = false; }
        match tokens[at] {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                depth -= 1;
                if depth == 0 && tokens[at] == "}" { start = true; }
            }
            ";" if depth == 0 => start = true,
            _ => {}
        }
        at += 1;
    }
    if candidates.is_empty() { return None; }
    let mut rewritten = source.as_bytes().to_vec();
    let mut calls = Vec::new();
    for (range, dep, function) in &candidates {
        // Every mention must be one of the calls we remove: imports, aliases,
        // nested calls and other paths all keep the dependency (raw idents too).
        let mentions = tokens.iter().filter(|t| t.strip_prefix("r#").unwrap_or(t) == *dep).count();
        if mentions != candidates.iter().filter(|(_, d, _)| d == dep).count() { continue; }
        let lo = parsed.lexed[range.start].lo as usize;
        let hi = parsed.lexed[range.end - 1].hi as usize;
        for byte in &mut rewritten[lo..hi] {
            if !matches!(*byte, b'\n' | b'\r') { *byte = b' '; }
        }
        calls.push((dep.to_string(), function.to_string()));
    }
    if calls.is_empty() { return None; }
    Some(Rewrite { source: String::from_utf8(rewritten).ok()?, calls })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: std::path::PathBuf,
        script: BuildScript,
        units: Vec<RawUnit>,
    }

    impl Fixture {
        fn new(source: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("relax-noop-{}-{seq}", std::process::id()));
            std::fs::create_dir_all(dir.join("helper")).unwrap();
            std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"script\"\nversion = \"0.0.0\"\n").unwrap();
            std::fs::write(dir.join("helper/Cargo.toml"),
                "[package]\nname = \"renamed-helper\"\n[package.metadata.makepad-run]\nbuild-script-noop = [\"build\", \"empty\"]\n").unwrap();
            let mut f = Self {
                script: BuildScript { krate: 0, src_path: dir.join("build.rs").display().to_string(), edition: 2021,
                    features: Vec::new(), build_deps: vec![(0, "helper".into())] },
                dir, units: Vec::new(),
            };
            f.units.push(f.unit("helper", Vec::new()));
            f.write(source);
            f
        }

        fn write(&self, source: &str) {
            std::fs::write(&self.script.src_path, source).unwrap();
        }

        fn unit(&self, name: &str, deps: Vec<(usize, String)>) -> RawUnit {
            let src_path = self.dir.join(format!("{name}.rs")).display().to_string();
            std::fs::write(&src_path, "// fixture\n").unwrap();
            RawUnit { name: name.into(), src_path, edition: 2021, features: Vec::new(), proc_macro: false, deps,
                run_script: false, pkg: (self.dir.join(name).display().to_string(), name.into(), "0.0.0".into()) }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
    }

    #[test]
    fn check_cfg_println_alone_among_helpers_and_before_guard() {
        let f = Fixture::new("");
        for prefix in ["cargo:", "cargo::"] {
            let line = format!("println!(\"{prefix}rustc-check-cfg=cfg(relax)\");");
            for body in [line.clone(), format!("{line} {line} helper::build()"),
                format!("helper::build(); {line} helper::empty(); {line}")] {
                f.write(&format!("fn main() {{ {body} }}"));
                assert!(script(&f.script, &f.units), "{body}");
            }
            f.write(&format!("use helper::Thing; use std::env; fn main() {{ {line} {line}
                if ::std::env::var_os(\"MAKEPAD_RUN_BUILD_SCRIPT\").is_some() {{ return; }}
                helper::real_work(); println!(\"cargo:rerun-if-changed={{}}\", env::var(\"OUT_DIR\").unwrap());
            }}"));
            assert!(script(&f.script, &f.units));
        }
    }

    #[test]
    fn other_println_forms_and_shadowed_macros_are_not_inert() {
        let f = Fixture::new("");
        for statement in [
            r#"println!("cargo:rustc-check-cfg={}", "cfg(relax)");"#,
            r#"println!("cargo:rustc-cfg=relax");"#,
            r#"println!("cargo:rustc-check-cfg={value}");"#,
            r#"println!("cargo:rustc-check-cfg=cfg(relax)\ncargo:rustc-cfg=relax");"#,
            "println!(\"cargo:rustc-check-cfg=cfg(relax)\ncargo:rustc-cfg=relax\");",
            r##"println!(r#"cargo:rustc-check-cfg=cfg(relax)"#);"##,
        ] {
            f.write(&format!("fn main() {{ {statement} }}"));
            assert!(!script(&f.script, &f.units), "{statement}");
        }
        let body = r#"fn main() { println!("cargo:rustc-check-cfg=cfg(relax)");
            if ::std::env::var_os("MAKEPAD_RUN_BUILD_SCRIPT").is_some() { return; } }"#;
        for prefix in ["use custom::println;", "use custom::*;", "use custom::r#println;",
            "macro_rules! println { ($($tt:tt)*) => {}; }"] {
            f.write(&format!("{prefix} {body}"));
            assert!(!script(&f.script, &f.units), "{prefix}");
        }
        f.write(&format!("{body} use custom::other; other!();"));
        assert!(!script(&f.script, &f.units));
    }

    #[test]
    fn platform_shaped_script_blanks_only_the_helper_statement() {
        let source = r#"use std::env;
fn main() {
    helper::build(); // keep this comment, including include! and file!
    let out = env::var("OUT_DIR").unwrap();
    std::fs::write(out + "/app_meta_gen.rs", "metadata").unwrap();
    println!("cargo:rustc-cfg=real_work");
}
"#;
        let f = Fixture::new(source);
        assert!(!script(&f.script, &f.units));
        let r = rewrite(&f.script, &f.units).unwrap();
        assert_eq!(r.source, source.replace("helper::build();", "                "));
        assert_eq!(r.source.len(), source.len());
        assert_eq!(r.calls, vec![("helper".into(), "build".into())]);
    }

    #[test]
    fn rewrites_tail_calls_and_preserves_multiline_byte_offsets() {
        let f = Fixture::new("fn main() { real_work(); helper::build() }");
        assert_eq!(rewrite(&f.script, &f.units).unwrap().source,
            format!("fn main() {{ real_work(); {} }}", " ".repeat(15)));
        let source = "fn main() {\r\n    helper:: /* café */\r\n        build(); real_work();\r\n}\r\n";
        f.write(source);
        let r = rewrite(&f.script, &f.units).unwrap();
        let lo = source.find("helper").unwrap();
        let hi = source.find("; real_work").unwrap() + 1;
        assert_eq!(r.source.len(), source.len());
        assert_eq!(&r.source[..lo], &source[..lo]);
        assert_eq!(&r.source[hi..], &source[hi..]);
        for (a, b) in source.bytes().zip(r.source.bytes()).skip(lo).take(hi - lo) {
            assert_eq!(b, if a == b'\n' || a == b'\r' { a } else { b' ' });
        }
    }

    #[test]
    fn other_dependency_mentions_and_non_statement_calls_are_not_rewritten() {
        let f = Fixture::new("");
        for source in [
            "use helper::Thing; fn main() { helper::build(); real_work(); }",
            "use r#helper::Thing; fn main() { helper::build(); real_work(); }",
            "use custom::*; fn main() { helper::build(); real_work(); }",
            "extern crate helper; fn main() { helper::build(); real_work(); }",
            "fn main() { helper::build(); helper::real_work(); }",
            "fn main() { helper::build(); if ready() { helper::empty(); } }",
            "fn main() { if ready() { helper::build(); } real_work(); }",
            "fn main() { let value = helper::build(); real_work(); }",
            "fn main() { consume(helper::build()); real_work(); }",
            "fn main() { helper::build().real_work(); }",
            "fn main() { helper::unknown(); real_work(); }",
        ] {
            f.write(source);
            assert!(rewrite(&f.script, &f.units).is_none(), "{source}");
        }
    }

    #[test]
    fn relative_source_lookups_are_not_relocated() {
        let f = Fixture::new("");
        for lookup in ["mod other;", "#[path = \"other.rs\"] mod other;", "include!(\"other.rs\");",
            "let _ = include_str!(\"other.txt\");", "let _ = include_bytes!(\"other.bin\");", "let _ = file!();"] {
            f.write(&format!("fn main() {{ helper::build(); {lookup} real_work(); }}"));
            assert!(rewrite(&f.script, &f.units).is_none(), "{lookup}");
        }
    }

    #[test]
    fn pruned_units_keep_shared_dependencies_and_hash_the_rewritten_source() {
        let source = "fn main() { helper::build(); retained::work(); }";
        let mut f = Fixture::new(source);
        f.units[0].deps = vec![(1, "shared".into()), (2, "exclusive".into())];
        f.units.push(f.unit("shared", Vec::new()));
        f.units.push(f.unit("exclusive", Vec::new()));
        f.units.push(f.unit("retained", vec![(1, "shared".into())]));
        f.script.build_deps.push((3, "retained".into()));
        let target = f.dir.join("target").display().to_string();
        let r = rewrite(&f.script, &f.units).unwrap();
        let (json, hash) = super::super::script_units(&f.script, &f.units, Some(&r), &target);
        let g = super::super::Units::from_json(json.as_bytes()).unwrap();
        assert_eq!(g.units.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["shared", "retained", "build-script-build"]);
        let root = &g.units[g.roots[0]];
        assert_eq!(root.deps, vec![(1, "retained".into())]);
        assert_eq!(g.units[1].deps, vec![(0, "shared".into())]);
        assert_eq!(root.src_path, format!("{target}/build-scripts/{hash:016x}/build.rs"));
        assert_eq!(root.pkg_id, format!("path+file://{}#build-script-build@0.0.0", f.dir.display()));
        let (original, original_hash) = super::super::script_units(&f.script, &f.units, None, &target);
        assert_ne!(hash, original_hash);
        let original = super::super::Units::from_json(original.as_bytes()).unwrap();
        assert_eq!(original.units[original.roots[0]].src_path, f.script.src_path);

        // A same-length change to an inert call or to its exclusive dependency
        // cannot affect the child; a change to retained code must invalidate it.
        f.write(&source.replace("build", "empty"));
        let r = rewrite(&f.script, &f.units).unwrap();
        std::fs::write(&f.units[0].src_path, "// changed inert helper\n").unwrap();
        assert_eq!(hash, super::super::script_units(&f.script, &f.units, Some(&r), &target).1);
        let changed = Rewrite { source: r.source.replace("work", "changed"), calls: r.calls.clone() };
        assert_ne!(hash, super::super::script_units(&f.script, &f.units, Some(&changed), &target).1);
        std::fs::write(&f.units[1].src_path, "// changed shared dependency\n").unwrap();
        assert_ne!(hash, super::super::script_units(&f.script, &f.units, Some(&r), &target).1);

        // If another root reaches the helper, only its direct edge is removed.
        f.units[3].deps.push((0, "helper".into()));
        let (json, _) = super::super::script_units(&f.script, &f.units, Some(&r), &target);
        let g = super::super::Units::from_json(json.as_bytes()).unwrap();
        assert_eq!(g.units.len(), 5);
        assert_eq!(g.units[g.roots[0]].deps, vec![(3, "retained".into())]);
    }
}
