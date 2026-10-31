//! Discover the public items in a committed generated module without its metadata inputs.
//! Only module-scope declarations count: interface methods are not namespace functions.
use std::collections::BTreeSet;

pub(super) fn items(path: &str) -> BTreeSet<String> {
    let source = std::fs::read_to_string(path).expect("read existing bindings");
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at].is_ascii_whitespace() {
            at += 1;
        } else if bytes[at..].starts_with(b"//") {
            while at < bytes.len() && bytes[at] != b'\n' { at += 1; }
        } else if bytes[at..].starts_with(b"/*") {
            at += 2;
            let mut nesting = 1;
            while nesting != 0 && at < bytes.len() {
                if bytes[at..].starts_with(b"/*") { nesting += 1; at += 2; }
                else if bytes[at..].starts_with(b"*/") { nesting -= 1; at += 2; }
                else { at += 1; }
            }
            assert_eq!(nesting, 0, "unterminated bindings comment");
        } else if bytes[at] == b'"' {
            at += 1;
            loop {
                assert!(at < bytes.len(), "unterminated bindings string");
                let b = bytes[at]; at += 1;
                if b == b'"' { break; }
                if b == b'\\' { at += 1; }
            }
        } else {
            let start = at;
            at += 1;
            if bytes[start].is_ascii_alphanumeric() || bytes[start] == b'_' {
                while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') { at += 1; }
            }
            tokens.push(&source[start..at]);
        }
    }
    let mut result = BTreeSet::new();
    let mut modules: Vec<(&str, usize)> = Vec::new();
    let mut depth = 0;
    for i in 0..tokens.len() {
        let rest = &tokens[i..];
        let module_depth = modules.last().map_or(0, |m| m.1);
        if depth == module_depth {
            let name = match rest {
                ["pub", "mod", name, "{", ..] => {
                    modules.push((name, depth + 1));
                    None
                }
                ["pub", "struct" | "union" | "enum" | "type" | "const" | "fn", name, ..]
                | ["pub", "unsafe", "fn", name, ..] => Some(*name),
                ["pub", "use", ..] => {
                    let end = rest.iter().position(|t| *t == ";").expect("use terminator");
                    // The generator emits single-item reexports, never grouped imports.
                    Some(rest[end - 1])
                }
                _ => None,
            };
            if let Some(name) = name {
                let mut full = String::from("Windows");
                for (module, _) in &modules { full.push('.'); full.push_str(module); }
                full.push('.'); full.push_str(name);
                result.insert(full);
            }
        }
        match tokens[i] {
            "{" => depth += 1,
            "}" => {
                assert!(depth > 0, "unbalanced generated bindings");
                if modules.last().is_some_and(|m| m.1 == depth) { modules.pop(); }
                depth -= 1;
            }
            _ => (),
        }
    }
    assert_eq!(depth, 0, "unbalanced generated bindings");
    result
}
