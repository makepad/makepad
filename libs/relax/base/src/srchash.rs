//! Source identity of a provided crate and its library dependency closure (proc macros
//! included, build dependencies excluded). Path packages read Cargo.toml, optional build.rs,
//! every file under src/, and literal include_str!/include_bytes!/include! targets named by
//! src/ Rust sources, following included .rs files transitively. Unreferenced scripts, tests,
//! examples and documentation do not count. Registry/git packages use name@version.
//! Features retain Cargo host-side and host-shims normalization. The loader embeds the hash
//! and a per-file manifest; any registered crate mismatch is a rebuild error naming inputs.
//! Shared with build scripts through #[path]: std only, with reads through crate::vfs.

/// One package of the graph: `deps` index the same slice.
pub struct SrcPkg {
    pub name: String,
    pub version: String,
    pub dir: String,
    pub features: Vec<String>,
    pub deps: Vec<usize>,
    /// a proc-macro package: it and what it reaches run at expansion time (host side)
    pub proc_macro: bool,
}

/// Features that select the shim file only (`host-shims`): the loader's build enables them,
/// a program never does, and they change no code a program calls.
const IGNORED_FEATURES: &[&str] = &["host-shims"];

struct H(u64);

impl H {
    fn bytes(&mut self, b: &[u8]) {
        let mut h = self.0 ^ (b.len() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let mut chunks = b.chunks_exact(8);
        for c in &mut chunks {
            let w = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
            h = (h ^ w).wrapping_mul(0xff51_afd7_ed55_8ccd);
            h ^= h >> 32;
        }
        for &x in chunks.remainder() {
            h = (h ^ x as u64).wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = (h ^ (h >> 29)).wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    }
}

fn immutable(dir: &str) -> bool {
    dir.contains("/.cargo/registry/") || dir.contains("/.cargo/git/")
}

/// General source-tree walk for provenance and reload tracking, not library identity.
pub fn files(dir: &std::path::Path, rel: &str, out: &mut Vec<(String, std::path::PathBuf)>) {
    let Ok(rd) = crate::vfs::read_dir(dir) else { return };
    for (name, is_dir) in rd {
        if name.starts_with('.') {
            continue;
        }
        let p = dir.join(&name);
        let r = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
        if is_dir {
            if name == "target" || crate::vfs::is_file(p.join("Cargo.toml")) {
                continue;
            }
            files(&p, &r, out);
        } else if crate::vfs::is_file(&p) {
            out.push((r, p));
        }
    }
}

/// Lex just enough Rust to recognize literal includes, ignoring comments and other literals.
#[derive(Debug)]
enum Token { Ident(String), String(String), Punct(u8), Other }

fn tokens(source: &str) -> Vec<Token> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() { i += 1; continue; }
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' { i += 1; }
            continue;
        }
        if b[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < b.len() && depth > 0 {
                if b[i..].starts_with(b"/*") { depth += 1; i += 2; }
                else if b[i..].starts_with(b"*/") { depth -= 1; i += 2; }
                else { i += 1; }
            }
            continue;
        }
        let start = i;
        let prefixed = matches!(b[i], b'b' | b'c') && i + 1 < b.len() && matches!(b[i + 1], b'"' | b'r' | b'\'');
        let literal = i + usize::from(prefixed);
        let mut quote = literal;
        if b[quote] == b'r' {
            quote += 1;
            while quote < b.len() && b[quote] == b'#' { quote += 1; }
            if quote < b.len() && b[quote] == b'"' {
                let hashes = quote - literal - 1;
                let content = quote + 1;
                i = content;
                while i < b.len() {
                    if b[i] == b'"' && b.get(i + 1..i + 1 + hashes).is_some_and(|x| x.iter().all(|c| *c == b'#')) { break; }
                    i += 1;
                }
                out.push(if prefixed { Token::Other } else { Token::String(source[content..i].into()) });
                i = (i + 1 + hashes).min(b.len());
                continue;
            }
        }
        if b[literal] == b'"' {
            i = literal + 1;
            let mut value = String::new();
            while i < b.len() && b[i] != b'"' {
                if b[i] != b'\\' {
                    let c = source[i..].chars().next().unwrap();
                    value.push(c); i += c.len_utf8(); continue;
                }
                i += 1;
                if i == b.len() { break; }
                let c = b[i]; i += 1;
                match c {
                    b'n' => value.push('\n'), b'r' => value.push('\r'), b't' => value.push('\t'), b'0' => value.push('\0'),
                    b'\\' => value.push('\\'), b'"' => value.push('"'), b'\'' => value.push('\''),
                    b'x' if i + 2 <= b.len() => {
                        if let Ok(n) = u8::from_str_radix(&source[i..i + 2], 16) { value.push(n as char); }
                        i += 2;
                    }
                    b'u' if b.get(i) == Some(&b'{') => {
                        i += 1; let from = i;
                        while i < b.len() && b[i] != b'}' { i += 1; }
                        if let Ok(n) = u32::from_str_radix(&source[from..i].replace('_', ""), 16) {
                            if let Some(c) = char::from_u32(n) { value.push(c); }
                        }
                        i = (i + 1).min(b.len());
                    }
                    b'\n' | b'\r' => { while i < b.len() && b[i].is_ascii_whitespace() { i += 1; } }
                    _ => {}
                }
            }
            i = (i + 1).min(b.len());
            out.push(if prefixed { Token::Other } else { Token::String(value) });
            continue;
        }
        if b[literal] == b'\'' {
            let mut end = literal + 1;
            if b.get(end) == Some(&b'\\') {
                end += 2;
                if b.get(end.wrapping_sub(1)) == Some(&b'u') && b.get(end) == Some(&b'{') {
                    while end < b.len() && b[end] != b'}' { end += 1; }
                    end += 1;
                } else if b.get(end.wrapping_sub(1)) == Some(&b'x') { end += 2; }
            } else if end < b.len() { end += source[end..].chars().next().unwrap().len_utf8(); }
            if b.get(end) == Some(&b'\'') { i = end + 1; out.push(Token::Other); continue; }
        }
        if b[i].is_ascii_alphabetic() || b[i] == b'_' || b[i] >= 128 {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 128) { i += 1; }
            out.push(Token::Ident(source[start..i].into()));
        } else { out.push(Token::Punct(b[i])); i += 1; }
    }
    out
}

fn includes(source: &str) -> Vec<(String, bool)> {
    let ts = tokens(source);
    let mut out = Vec::new();
    for (i, t) in ts.iter().enumerate() {
        let Token::Ident(name) = t else { continue };
        if !matches!(name.as_str(), "include" | "include_str" | "include_bytes") { continue; }
        let Some([Token::Punct(b'!'), Token::Punct(open), Token::String(path), rest @ ..]) = ts.get(i + 1..) else { continue };
        let close = match open { b'(' => b')', b'[' => b']', b'{' => b'}', _ => continue };
        let rest = if matches!(rest.first(), Some(Token::Punct(b','))) { &rest[1..] } else { rest };
        if matches!(rest.first(), Some(Token::Punct(c)) if *c == close) { out.push((path.clone(), name == "include")); }
    }
    out
}

// Lexical normalization works for VFS paths too; canonicalize would require disk files.
fn normalize(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {},
            Component::ParentDir if out.file_name().is_some_and(|n| n != "..") => { out.pop(); },
            _ => out.push(c.as_os_str()),
        }
    }
    out
}

fn relative(root: &std::path::Path, path: &std::path::Path) -> String {
    let a: Vec<_> = root.components().collect();
    let b: Vec<_> = path.components().collect();
    let common = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
    let mut rel = std::path::PathBuf::new();
    for _ in common..a.len() { rel.push(".."); }
    for c in &b[common..] { rel.push(c.as_os_str()); }
    rel.to_string_lossy().replace('\\', "/")
}

/// Library identity inputs and hashes, including transitive includes. Retain hashes, not
/// asset contents; only the current file needs to stay in memory.
fn identity_files(dir: &std::path::Path) -> Vec<(String, u64)> {
    use std::collections::{BTreeMap, BTreeSet};
    fn walk(dir: &std::path::Path, paths: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = crate::vfs::read_dir(dir) {
            for (name, is_dir) in entries {
                let p = dir.join(name);
                if is_dir { walk(&p, paths); } else if crate::vfs::is_file(&p) { paths.push(p); }
            }
        }
    }
    let root = normalize(dir);
    let mut paths = Vec::new();
    walk(&root.join("src"), &mut paths);
    let mut pending: Vec<_> = paths.into_iter().map(|p| { let rust = p.extension().is_some_and(|x| x == "rs"); (p, rust) }).collect();
    pending.push((root.join("Cargo.toml"), false));
    if crate::vfs::is_file(root.join("build.rs")) { pending.push((root.join("build.rs"), false)); }
    let mut contents = BTreeMap::new();
    let mut scanned = BTreeSet::new();
    while let Some((path, rust)) = pending.pop() {
        let path = normalize(&path);
        // A missing literal target is absent in the manifest, so deletion is reported as removed.
        if !crate::vfs::is_file(&path) { continue; }
        if contents.contains_key(&path) && (!rust || scanned.contains(&path)) { continue; }
        let bytes = crate::vfs::read(&path).unwrap_or_else(|e| panic!("source identity {}: {e}", path.display()));
        contents.entry(path.clone()).or_insert_with(|| { let mut h = H(0x5eed); h.bytes(&bytes); h.0 });
        if rust && scanned.insert(path.clone()) {
            for (name, recurse) in includes(&String::from_utf8_lossy(&bytes)) {
                let target = normalize(&path.parent().unwrap().join(name));
                let rust = recurse && target.extension().is_some_and(|x| x == "rs");
                pending.push((target, rust));
            }
        }
    }
    contents.into_iter().map(|(p, hash)| (relative(&root, &p), hash)).collect()
}

/// Sorted data records: (package including host role and version, relative file or metadata,
/// file hash or metadata value). Reserved metadata paths cannot name package files.
pub type Manifest = std::collections::BTreeMap<(String, String), String>;

pub fn closure_manifest(pkgs: &[SrcPkg], root: usize) -> Manifest {
    let mut seen = vec![[false; 2]; pkgs.len()];
    let mut stack = vec![(root, false)];
    let mut out = Manifest::new();
    while let Some((i, host)) = stack.pop() {
        if i >= pkgs.len() { continue; }
        let p = &pkgs[i];
        let host = host || p.proc_macro;
        // Metadata unifies the target relax-tokens adapter; resolver 2 does not enable it
        // in rustc's proc-macro build, which uses proc_macro tokens, never relax-lex.
        if host && p.name == "relax-lex" { continue; }
        if std::mem::replace(&mut seen[i][host as usize], true) { continue; }
        let key = format!("{}{}@{}", if host { "host:" } else { "" }, p.name.replace('-', "_"), p.version);
        let mut feats: Vec<_> = p.features.iter().map(String::as_str).filter(|f| !host && !IGNORED_FEATURES.contains(f)).collect();
        feats.sort_unstable(); feats.dedup();
        out.insert((key.clone(), "\0version".into()), p.version.clone());
        out.insert((key.clone(), "\0features".into()), feats.join(", "));
        if !immutable(&p.dir) {
            for (rel, hash) in identity_files(std::path::Path::new(&p.dir)) {
                out.insert((key.clone(), rel), format!("{hash:016x}"));
            }
        }
        stack.extend(p.deps.iter().map(|d| (*d, host)));
    }
    out
}

pub fn manifest_hash(manifest: &Manifest) -> u64 {
    let mut h = H(0x9d0);
    for ((package, path), value) in manifest {
        h.bytes(package.as_bytes()); h.bytes(path.as_bytes()); h.bytes(value.as_bytes());
    }
    h.0
}

pub fn closure_hash(pkgs: &[SrcPkg], root: usize) -> u64 {
    manifest_hash(&closure_manifest(pkgs, root))
}

/// Length-prefixed UTF-8 fields; data only, never generated Rust source.
pub fn encode_manifest(manifest: &Manifest) -> Vec<u8> {
    let mut out = Vec::new();
    for ((package, path), value) in manifest {
        for s in [package, path, value] {
            out.extend_from_slice(&(s.len() as u64).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
    }
    out
}

pub fn decode_manifest(mut bytes: &[u8]) -> Result<Manifest, String> {
    fn field(bytes: &mut &[u8]) -> Option<String> {
        let n = usize::try_from(u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?)).ok()?;
        *bytes = &bytes[8..];
        let value = std::str::from_utf8(bytes.get(..n)?).ok()?.to_owned();
        *bytes = &bytes[n..];
        Some(value)
    }
    let mut out = Manifest::new();
    while !bytes.is_empty() {
        let mut get = || field(&mut bytes).ok_or_else(|| "invalid provided source manifest".to_string());
        let package = get()?; let path = get()?; let value = get()?;
        if out.insert((package, path), value).is_some() { return Err("duplicate provided source manifest entry".into()); }
    }
    Ok(out)
}

/// Bound diagnostic output, not the work or the identity: every input still counts.
pub fn diff_message(name: &str, built: &Manifest, current: &Manifest) -> String {
    let mut files = Vec::new();
    let mut metadata = Vec::new();
    let keys: std::collections::BTreeSet<_> = built.keys().chain(current.keys()).collect();
    for key @ (package, path) in keys {
        let old = built.get(key); let new = current.get(key);
        if old == new { continue; }
        if path.starts_with('\0') {
            metadata.push(format!("  {package} {}: {} -> {}", &path[1..], old.map(String::as_str).unwrap_or("<absent>"), new.map(String::as_str).unwrap_or("<absent>")));
        } else {
            let kind = match (old, new) { (None, _) => "added", (_, None) => "removed", _ => "changed" };
            files.push(format!("  {kind} {package}: {path}"));
        }
    }
    let mut message = format!("provided crate {name} differs from the library built into makepad:\n");
    for line in files.iter().take(20).chain(metadata.iter()) { message.push_str(line); message.push('\n'); }
    if files.len() > 20 { message.push_str(&format!("  {} more files\n", files.len() - 20)); }
    message.push_str(&rebuild_hint());
    message
}

/// How to fix a `makepad` binary older than the checkout's provided crates: run it through cargo,
/// which rebuilds it first, with this process's own arguments.
pub fn rebuild_hint() -> String {
    let args: Vec<String> = std::env::args().skip(1).collect();
    format!("rebuild makepad by starting it through cargo: {}", rebuild_command(&args))
}

/// `cargo run --release -p makepad-run --bin makepad -- <args>` (arguments with spaces or quotes
/// single-quoted); `run <package>` when there are none.
pub fn rebuild_command(args: &[String]) -> String {
    let mut command = "cargo run --release -p makepad-run --bin makepad --".to_string();
    if args.is_empty() { command.push_str(" run <package>"); }
    for arg in args {
        command.push(' ');
        if arg.is_empty() || arg.contains([' ', '\'', '"', '\t']) {
            command.push('\'');
            command.push_str(&arg.replace('\'', "'\\''"));
            command.push('\'');
        } else {
            command.push_str(arg);
        }
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!("relax-srchash-{}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        fn package(&self) -> SrcPkg {
            SrcPkg { name: "fixture".into(), version: "1.0".into(), dir: self.0.to_string_lossy().into_owned(), features: vec![], deps: vec![], proc_macro: false }
        }
    }
    impl Drop for Temp { fn drop(&mut self) { std::fs::remove_dir_all(&self.0).unwrap(); } }

    #[test]
    fn library_file_set() {
        let dir = Temp::new();
        dir.write("Cargo.toml", "[package]\nname = 'fixture'\nversion = '1.0.0'\n");
        dir.write("build.rs", "fn main() {}");
        dir.write("src/lib.rs", r####"
            const TEXT: &str = include_str!("../assets/text.txt");
            const BYTES: &[u8] = include_bytes! { r#"../assets/blob"# };
            include!("../generated/one.rs");
            // include_str!("../scripts/ignored.hal")
            /* nested /* comment */ include!("../tests/ignored.rs") */
            const DECOY: &str = "include_str!(\"../docs/ignored.md\")";
            const RAW: &str = r###"include!("../examples/ignored.rs")"###;
            include!(concat!(env!("OUT_DIR"), "/generated.rs"));
        "####);
        dir.write("src/nested/mod.rs", "const X: &str = include_str!(\"../../assets/te\\x78t.txt\");");
        dir.write("src/.hidden", "all src files count");
        dir.write("src/nested/Cargo.toml", "also counts");
        dir.write("generated/one.rs", "include!(\"two.rs\");");
        dir.write("generated/two.rs", "include!(\"one.rs\"); const X: &str = include_str!(\"../assets/more.txt\");");
        for name in ["assets/text.txt", "assets/blob", "assets/more.txt", "scripts/ignored.hal", "tests/ignored.rs", "docs/ignored.md", "examples/ignored.rs", "benches/ignored.rs", "target/ignored", "README.md"] { dir.write(name, "initial"); }
        let files = identity_files(&dir.0);
        let names: Vec<_> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Cargo.toml", "assets/blob", "assets/more.txt", "assets/text.txt", "build.rs", "generated/one.rs", "generated/two.rs", "src/.hidden", "src/lib.rs", "src/nested/Cargo.toml", "src/nested/mod.rs"]);
        let pkgs = [dir.package()];
        let before = closure_hash(&pkgs, 0);
        for name in ["scripts/ignored.hal", "tests/ignored.rs", "docs/ignored.md", "examples/ignored.rs", "benches/ignored.rs", "README.md"] { dir.write(name, "changed"); }
        assert_eq!(before, closure_hash(&pkgs, 0));
        dir.write("assets/text.txt", "changed");
        assert_ne!(before, closure_hash(&pkgs, 0));
    }

    #[test]
    fn literal_tokens() {
        assert_eq!(includes(r####"
            let quote = '"'; let lifetime: &'static str = "ignored";
            let byte = b'"'; let bytes = br#"include!("wrong.rs")"#;
            include /* comment */ ! [ "a\u{2e}rs", ];
            include_str!(r##"../data.txt"##);
            include_bytes!("../binary");
            include!("not-literal.rs" + suffix);
        "####), vec![("a.rs".into(), true), ("../data.txt".into(), false), ("../binary".into(), false)]);
    }

    #[test]
    fn manifest_diff_and_data_roundtrip() {
        let dir = Temp::new();
        dir.write("Cargo.toml", "manifest");
        dir.write("src/lib.rs", "old");
        dir.write("src/removed.rs", "removed");
        let mut pkgs = [dir.package()];
        let built = closure_manifest(&pkgs, 0);
        assert_eq!(decode_manifest(&encode_manifest(&built)).unwrap(), built);
        assert!(decode_manifest(&[0]).is_err());
        dir.write("src/lib.rs", "new");
        dir.write("src/added.rs", "added");
        std::fs::remove_file(dir.0.join("src/removed.rs")).unwrap();
        pkgs[0].features.push("extra".into());
        let current = closure_manifest(&pkgs, 0);
        let message = diff_message("fixture", &built, &current);
        assert!(message.contains("provided crate fixture"));
        for line in ["changed fixture@1.0: src/lib.rs", "added fixture@1.0: src/added.rs", "removed fixture@1.0: src/removed.rs", "fixture@1.0 features:  -> extra"] { assert!(message.contains(line), "{message}"); }
        assert!(message.contains("rebuild makepad by starting it through cargo: cargo run --release -p makepad-run --bin makepad --"), "{message}");
        pkgs[0].version = "2.0".into();
        let message = diff_message("fixture", &built, &closure_manifest(&pkgs, 0));
        assert!(message.contains("version: 1.0 -> <absent>"));
        assert!(message.contains("version: <absent> -> 2.0"));
        for i in 0..25 { dir.write(&format!("src/new{i}.rs"), "new"); }
        let message = diff_message("fixture", &Manifest::new(), &closure_manifest(&pkgs, 0));
        assert_eq!(message.lines().filter(|l| l.starts_with("  added ")).count(), 20);
        assert!(message.contains("8 more files"), "{message}");
    }

    #[test]
    fn dependency_features_and_immutable_packages() {
        let dir = Temp::new();
        dir.write("Cargo.toml", "manifest");
        dir.write("src/lib.rs", "library");
        let mut root = dir.package();
        root.deps = vec![1];
        let dep = SrcPkg { name: "dep".into(), version: "1".into(), dir: "/.cargo/registry/dep".into(), features: vec![], deps: vec![], proc_macro: false };
        let mut pkgs = [root, dep];
        let built = closure_manifest(&pkgs, 0);
        assert_eq!(built.keys().filter(|(p, _)| p == "dep@1").count(), 2);
        pkgs[0].features.push("host-shims".into());
        assert_eq!(built, closure_manifest(&pkgs, 0));
        pkgs[1].features.push("enabled".into());
        assert!(diff_message("fixture", &built, &closure_manifest(&pkgs, 0)).contains("dep@1 features:  -> enabled"));
        pkgs[1].proc_macro = true;
        let host = closure_manifest(&pkgs, 0);
        pkgs[1].features.clear();
        assert_eq!(host, closure_manifest(&pkgs, 0));
    }

    #[test]
    fn the_refusal_says_the_cargo_run_that_rebuilds_makepad() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(rebuild_command(&args(&["run", "makepad-example-counter"])), "cargo run --release -p makepad-run --bin makepad -- run makepad-example-counter");
        assert_eq!(rebuild_command(&args(&["run", "x", "--", "a b", "it's"])), "cargo run --release -p makepad-run --bin makepad -- run x -- 'a b' 'it'\\''s'");
        assert_eq!(rebuild_command(&[]), "cargo run --release -p makepad-run --bin makepad -- run <package>");
    }
}
