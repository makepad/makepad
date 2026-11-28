//! A review of sized, infallible growth in the VM. A key is the file, the Rust
//! function the call is written in, and the call's name with its size argument
//! only, so `vec![x; n]` is `vec!(n)` and `.resize_with(n, f)` is
//! `.resize_with(n)`: the fill, the element and any closure are not reviewed.
//! Keys are compared as tokens, so moved lines and reformatting leave them alone.
//! Two such calls in one function need a count of two.
use std::{collections::{BTreeMap, BTreeSet}, fs, path::{Path, PathBuf}};

// The workspace's Rust lexer handles literals, nested comments, lifetimes and
// joint punctuation.
use relax_lex::lexer::{self, T, Tok};

#[derive(Debug)]
enum Reason {
    Source,   // Linear in the source or its parsed representation.
    Constant, // The requested size has a fixed upper bound.
    Host,     // Host setup or inspection, outside document evaluation.
    Guarded,  // A dominating fallible reserve covers this physical allocation.
    Gap(&'static str), // A document-controlled allocation still to be guarded.
}

use Reason::*;
type Entry = (&'static str, &'static str, &'static str, usize, Reason);
const REVIEWED: &[Entry] = &[
    // set_array_index_impl reserves the active storage before delegating to set_index.
    ("array.rs", "ScriptArrayStorage::set_index", ".resize(index + 1)", 5, Guarded),
    ("array_heap.rs", "ScriptHeap::new_array_u8_with_preflight", "Vec::with_capacity(len)", 1, Gap("base64-decode")),
    // preflight_gc reserves live_bodies for the whole body table before marking starts.
    ("gc.rs", "Marker<'_,STATIC>::mark_transient_bodies", ".resize(transient.len())", 1, Guarded),
    ("gen_index.rs", "GenVec<T>::with_capacity", "Vec::with_capacity(capacity)", 1, Host),
    ("heap.rs", "size_marks", ".resize(words)", 1, Guarded),
    ("heap.rs", "reserve_free_list", ".reserve(slots - free.len())", 1, Gap("leaf-free-lists")),
    ("inspect_query.rs", "targets_of", "Vec::with_capacity(n)", 1, Host),
    ("inspect_registry.rs", "json_str", "String::with_capacity(s.len() + 2)", 1, Host),
    ("literal.rs", "SiteSeen::first", ".resize_with(body + 1)", 1, Source),
    ("literal.rs", "SiteSeen::first", ".resize(word + 1)", 1, Source),
    ("literal.rs", "NameReads::record", ".resize_with(b + 1)", 1, Source),
    ("literal.rs", "NameReads::record", ".resize(i + 1)", 1, Source),
    ("mod_regex.rs", "define_regex_module", "vec!(num_slots)", 1, Gap("capture-slots")),
    // One row per native type compiled into the binary.
    ("native.rs", "ScriptNative::ensure_type_table_capacity", ".resize_with(ty_redux.to_index() + 1)", 3, Constant),
    ("native.rs", "ScriptNative::ensure_type_table_capacity", ".resize(ty_redux.to_index() + 1)", 1, Constant),
    ("object_heap.rs", "ScriptHeap::set_value_index", ".resize(index + 1)", 1, Guarded),
    ("opcodes_vars.rs", "ScriptVm<'a>::handle_use", "Vec::with_capacity(obj_data.map.len() + obj_data.vec.len())", 1, Gap("glob-import")),
    ("parser.rs", "ScriptParser::slot_body_close", "vec!(frame as usize)", 2, Source),
    ("parser.rs", "ScriptParser::insert_code_with_source", ".resize(self.opcodes.len())", 1, Source),
    ("pod_heap.rs", "ScriptHeap::new_pod_impl", ".resize(pod_ty.ty.size_of_impl().next_multiple_of(4) >> 2)", 2, Gap("pod-layout")),
    ("pod_heap.rs", "ScriptHeap::pod_pop_to_me", ".resize(no_u32 + 1)", 1, Gap("pod-builder")),
    ("pod_heap.rs", "ScriptHeap::pod_pop_to_me", ".resize(no_u32)", 1, Gap("pod-builder")),
    ("pod_heap.rs", "ScriptHeap::pod_pop_to_me", ".resize(1)", 5, Constant),
    ("pod_heap.rs", "ScriptHeap::pod_write_field", ".resize(start_u32 + elem_count_u32)", 1, Gap("pod-tail")),
    // Returning a closure can retain an object with a high heap slot index.
    ("roots.rs", "ScriptRoots::add", ".resize(i + 1)", 1, Gap("root-counts")),
    ("shader_pack.rs", "pod_to_pack", "Vec::with_capacity(fields.len())", 1, Host),
    ("shader_pack.rs", "pack_entry_from_desc", "vec!(su.slots)", 1, Host),
    ("shader_pack.rs", "write_pack", "Vec::with_capacity(entries.len())", 1, Host),
    ("shader_pack.rs", "PackReader<'a>::entry", "Vec::with_capacity(n.min(1024))", 8, Constant),
    ("shader_pack.rs", "read_strings", "Vec::with_capacity(n.min(1 << 16))", 1, Constant),
    ("shader_pack.rs", "read_index", "Vec::with_capacity(n.min(1 << 16))", 1, Constant),
    ("string.rs", "regex_find_all", "vec!(num_slots)", 1, Gap("capture-slots")),
    ("string.rs", "regex_exec_first", "vec!(num_slots)", 1, Gap("capture-slots")),
    ("string_heap.rs", "ScriptHeap::new_string_with_preflight", ".reserve_exact(max_len)", 1, Gap("string-output")),
    ("string_heap.rs", "ScriptHeap::temp_string_with_preflight", ".reserve_exact(max_len)", 1, Gap("string-output")),
    ("string_heap.rs", "ScriptHeap::cast_to_owned_string", ".reserve_exact(len)", 1, Gap("string-output")),
    ("suggest.rs", "levenshtein", "vec!(b_len + 1)", 1, Gap("property-suggestion")),
    // The structural diff sizes its per-child tables from the children it compares.
    ("syntax/diff.rs", "match_children", "vec!(old.len())", 1, Source),
    ("syntax/diff.rs", "stationary", "vec!(matched.len())", 2, Source),
    // Each surviving bare child occupies one slot and one insertion gap in its type's rank tree.
    ("syntax/diff.rs", "BareRanks::new", "vec!(2 * original.len() + 2)", 1, Source),
    ("syntax/diff.rs", "diff_pair", "vec!(old.len() + 2)", 1, Source),
    ("syntax/edit.rs", "Syntax::apply", "String::with_capacity(self.source.len())", 1, Source),
    ("syntax/mod.rs", "Syntax::print", "String::with_capacity(self.source.len())", 1, Source),
    ("syntax/parse.rs", "Parser<'a>::new", "vec!(tokens.len())", 1, Source),
    ("syntax/quantity.rs", "evaluate", "String::with_capacity(text.len())", 1, Source),
    ("syntax/write.rs", "ColorStyle::rewrite", "String::with_capacity(text.len())", 1, Source),
    ("syntax/write.rs", "quote", "String::with_capacity(text.len() + 2)", 1, Source),
    ("thread.rs", "ScriptThread::new", "Vec::with_capacity(64)", 3, Constant),
    ("thread.rs", "ScriptThread::new", "Vec::with_capacity(16)", 1, Constant),
    ("thread.rs", "ScriptThread::new", "Vec::with_capacity(1024)", 1, Constant),
    ("thread.rs", "ScriptThread::new", "Vec::with_capacity(256)", 1, Constant),
    ("thread.rs", "ScriptThread::grow_slots", ".resize(len)", 1, Guarded),
    // With the host census enabled, a document definition can live at a high slot.
    ("touch.rs", "ScriptTouch::set_pending", ".resize(w + 1)", 1, Gap("touch-bitmap")),
    // The argument count is checked against the variant's declared arity first.
    ("typed_value.rs", "ScriptVm<'_>::typed_variant", "Vec::with_capacity(args.len())", 1, Constant),
    ("value_map.rs", "empty_buckets", ".resize(len)", 1, Guarded),
    // Object splat charges for a merge, then inserts into this map without reserving its spill.
    ("value_map.rs", "ValueMap<K,V>::reserve", ".reserve(self.entries_for(additional))", 1, Gap("merged-map")),
    ("value_map.rs", "ValueMap<K,V>::reserve", "vec!(SpillIndex::bucket_count(want))", 1, Gap("merged-map")),
    ("value_map.rs", "ValueMap<K,V>::insert_absent", ".reserve(1)", 1, Gap("merged-map")),
    // Host bindings applying an array inherit the length supplied by the document.
    ("vec_prims.rs", "ScriptApply for Vec<T>::script_apply", ".resize_with(len)", 2, Gap("apply-vec")),
    ("vec_prims.rs", "ScriptApply for Vec<u8>::script_apply", ".resize(vm.array_len(arr))", 1, Gap("apply-vec")),
    ("vm.rs", "ScriptCode::fn_tokens_placed", "Vec::with_capacity(last + 1 - first)", 1, Source),
    ("vm.rs", "ScriptCode::fn_image", "Vec::with_capacity(last + 1 - first)", 1, Source),
];

// Keep token boundaries in the key: e.g. `n << 1` and `n < < 1` differ.
fn tokens(source: &str) -> Vec<Tok> {
    let mut tokens = Vec::new();
    lexer::lex(source.as_bytes(), 2021, &mut tokens)
        .unwrap_or_else(|e| panic!("cannot scan source at byte {}: {}", e.pos, e.msg));
    tokens.retain(|t| !t.kind.is_trivia() && t.kind != T::Eof);
    tokens
}

type Key = (String, String, Vec<String>);
type Sites = BTreeMap<Key, Vec<(usize, String)>>;

fn reviewed_key(file: &str, owner: &str, call: &str) -> Key {
    let spelling = tokens(call).iter().map(|t| t.text(call).to_owned()).collect();
    (file.to_owned(), owner.to_owned(), spelling)
}

struct Scan<'a> {
    source: &'a str,
    file: &'a str,
    tokens: Vec<Tok>,
    lines: Vec<u32>,
    // The other end of every (), [] and {} group, in either direction.
    mate: Vec<usize>,
}

impl<'a> Scan<'a> {
    fn new(file: &'a str, source: &'a str) -> Self {
        let tokens = tokens(source);
        let mut mate: Vec<usize> = (0..tokens.len()).collect();
        let mut stack = Vec::new();
        for (i, token) in tokens.iter().enumerate() {
            match token.kind {
                T::OpenParen | T::OpenBracket | T::OpenBrace => stack.push(i),
                T::CloseParen | T::CloseBracket | T::CloseBrace => {
                    let open = stack.pop().expect("balanced Rust delimiters");
                    mate[open] = i;
                    mate[i] = open;
                }
                _ => {}
            }
        }
        assert!(stack.is_empty(), "unclosed delimiter in {file}");
        Self { source, file, tokens, mate, lines: lexer::line_starts(source.as_bytes()) }
    }

    fn text(&self, i: usize) -> &'a str {
        self.tokens.get(i).map_or("", |t| t.text(self.source))
    }

    fn kind(&self, i: usize) -> T {
        self.tokens.get(i).map_or(T::Eof, |t| t.kind)
    }

    // One sibling token or one whole delimited group.
    fn next(&self, i: usize) -> usize {
        self.mate[i].max(i) + 1
    }

    fn spell(&self, start: usize, end: usize) -> Vec<String> {
        self.tokens[start..end].iter().map(|t| t.text(self.source).to_owned()).collect()
    }

    // Find the body of an item, skipping parameter lists, attributes and const
    // generic arguments. A declaration ending in a semicolon has no body.
    fn body(&self, mut i: usize, end: usize) -> usize {
        let mut angles = 0usize;
        while i < end {
            match self.kind(i) {
                T::Lt => angles += 1,
                T::Gt => angles = angles.saturating_sub(1),
                T::Shr => angles = angles.saturating_sub(2),
                T::OpenBrace | T::Semi if angles == 0 => break,
                _ => {}
            }
            i = self.next(i);
        }
        i
    }

    fn owner(&self, i: usize, body: usize) -> String {
        if self.kind(i) != T::KwImpl {
            return self.text(i + 1).to_owned();
        }
        // Qualify same-named methods by trait and self type. Generic bounds
        // on the impl are outside its name.
        let mut first = i + 1;
        if self.kind(first) == T::Lt {
            let mut depth = 1usize;
            first += 1;
            while first < body && depth > 0 {
                match self.kind(first) {
                    T::Lt => depth += 1,
                    T::Gt => depth -= 1,
                    T::Shr => depth = depth.saturating_sub(2),
                    _ => {}
                }
                first = self.next(first);
            }
        }
        let last = (first..body).find(|&j| self.kind(j) == T::KwWhere).unwrap_or(body);
        let mut name = String::new();
        for j in first..last {
            if self.kind(j) == T::KwFor { name.push_str(" for "); }
            else { name.push_str(self.text(j)); }
        }
        name
    }

    // Read outer attributes of a module (including any pub visibility).
    fn module_attrs(&self, decl: usize) -> (bool, Option<&str>) {
        let mut j = decl;
        if j > 0 && self.kind(j - 1) == T::CloseParen { j = self.mate[j - 1]; }
        if j > 0 && self.kind(j - 1) == T::KwPub { j -= 1; }
        let mut test = false;
        let mut path = None;
        while j > 0 && self.kind(j - 1) == T::CloseBracket {
            let open = self.mate[j - 1];
            if open == 0 || self.kind(open - 1) != T::Pound { break; }
            if self.text(open + 1) == "cfg" && self.kind(open + 2) == T::OpenParen
                && self.text(open + 3) == "test" && j == open + 6 {
                test = true;
            }
            if self.text(open + 1) == "path" && self.kind(open + 2) == T::Eq {
                let literal = self.text(open + 3);
                // Rust source paths normally use plain or raw string literals.
                let first = literal.find('"');
                let last = literal.rfind('"');
                path = first.zip(last).filter(|(a, b)| a < b).map(|(a, b)| &literal[a + 1..b]);
            }
            j = open - 1;
        }
        (test, path)
    }

    // The key of a sized call: its name, then its size argument in parentheses.
    fn sized(&self, name: (usize, usize), size: (usize, usize)) -> Vec<String> {
        let mut key = self.spell(name.0, name.1);
        key.push("(".to_owned());
        key.extend(self.spell(size.0, size.1));
        key.push(")".to_owned());
        key
    }

    // The end of the first argument of the call group opened at `open`.
    fn first_arg(&self, open: usize) -> usize {
        let mut j = open + 1;
        while j < self.mate[open] && self.kind(j) != T::Comma { j = self.next(j); }
        j
    }

    // Where a sized call starts, and its key.
    fn call(&self, i: usize) -> Option<(usize, Vec<String>)> {
        let name = self.text(i).strip_prefix("r#").unwrap_or(self.text(i));
        if name == "vec" && self.kind(i + 1) == T::Not && self.kind(i + 2) == T::OpenBracket {
            let end = self.mate[i + 2];
            let mut j = i + 3;
            while j < end {
                if self.kind(j) == T::Semi { return Some((i, self.sized((i, i + 2), (j + 1, end)))); }
                j = self.next(j);
            }
            return None;
        }
        if self.kind(i + 1) != T::OpenParen { return None; }
        let prev = i.checked_sub(1).map(|j| self.kind(j));
        if prev == Some(T::Dot) && matches!(name, "reserve" | "reserve_exact" | "resize" | "resize_with" | "repeat" | "with_capacity") {
            return Some((i - 1, self.sized((i - 1, i + 1), (i + 2, self.first_arg(i + 1)))));
        }
        if !matches!(name, "with_capacity" | "reserve_exact") || prev == Some(T::KwFn) { return None; }
        let mut start = i;
        while start >= 2 && self.kind(start - 1) == T::PathSep {
            let j = start - 2;
            if matches!(self.kind(j), T::Ident | T::RawIdent | T::KwSelfValue | T::KwSelfType | T::KwCrate | T::KwSuper) {
                start = j;
            } else if matches!(self.kind(j), T::Gt | T::Shr) {
                // Include explicit type arguments / qualified type in the path.
                let mut at = j;
                let mut depth = 0usize;
                loop {
                    match self.kind(at) {
                        T::Gt => depth += 1,
                        T::Shr => depth += 2,
                        T::Lt => { depth -= 1; if depth == 0 { break; } }
                        _ => {}
                    }
                    if at == 0 { break; }
                    at = self.mate[at].min(at).saturating_sub(1);
                }
                start = at;
            } else { break; }
        }
        if start > 0 && self.kind(start - 1) == T::PathSep { start -= 1; }
        Some((start, self.sized((start, i + 1), (i + 2, self.first_arg(i + 1)))))
    }

    fn walk(&self, mut i: usize, end: usize, owner: &str, module: &Path, mut test: bool, found: &mut Review) {
        while i < end {
            let kind = self.kind(i);
            if kind == T::Pound && self.kind(i + 1) == T::Not && self.kind(i + 2) == T::OpenBracket
                && self.text(i + 3) == "cfg" && self.kind(i + 4) == T::OpenParen
                && self.text(i + 5) == "test" && self.mate[i + 2] == i + 7 {
                test = true; // This module has an inner #![cfg(test)].
            }
            if kind == T::KwMod && self.kind(i + 1).is_ident_like() {
                let name = self.text(i + 1).trim_start_matches("r#");
                let (cfg_test, path) = self.module_attrs(i);
                let dir = module.join(name);
                let child = i + 2;
                if self.kind(child) == T::Semi {
                    let mut edge = |child| found.modules.push((PathBuf::from(self.file), child, test || cfg_test));
                    if let Some(path) = path {
                        let base = if owner.is_empty() { Path::new(self.file).parent().unwrap() } else { module };
                        edge(base.join(path));
                    } else {
                        edge(dir.with_extension("rs"));
                        edge(dir.join("mod.rs"));
                    }
                    i = child + 1;
                    continue;
                }
                if self.kind(child) == T::OpenBrace {
                    let path = if owner.is_empty() { name.to_owned() } else { format!("{owner}::{name}") };
                    self.walk(child + 1, self.mate[child], &path, &dir, test || cfg_test, found);
                    i = self.next(child);
                    continue;
                }
            }
            if kind == T::KwImpl || (matches!(kind, T::KwFn | T::KwTrait)
                && matches!(self.kind(i + 1), T::Ident | T::RawIdent)) {
                let body = self.body(i + 1, end);
                if self.kind(body) == T::OpenBrace && body < end {
                    let name = self.owner(i, body);
                    let path = if owner.is_empty() { name } else { format!("{owner}::{name}") };
                    self.walk(body + 1, self.mate[body], &path, module, test, found);
                    i = self.next(body);
                    continue;
                }
            }
            if !test && matches!(kind, T::Ident | T::RawIdent) {
                if let Some((start, call)) = self.call(i) {
                    let (line, _) = lexer::line_col_at(&self.lines, self.source.len(), self.tokens[start].lo);
                    let offset = self.lines[line - 1] as usize;
                    let snippet = self.source[offset..].lines().next().unwrap().trim();
                    let key = (self.file.to_owned(), owner.to_owned(), call);
                    found.sites.entry(key).or_default().push((line, snippet.to_owned()));
                }
            }
            if self.mate[i] > i { self.walk(i + 1, self.mate[i], owner, module, test, found); }
            i = self.next(i);
        }
    }
}

#[derive(Default)]
struct Review {
    sites: Sites,
    files: BTreeSet<PathBuf>,
    // Parent, declared child, whether the declaration is test-only.
    modules: Vec<(PathBuf, PathBuf, bool)>,
}

impl Review {
    fn production_files(&self) -> BTreeSet<PathBuf> {
        // Unreferenced files are scanned too. A file shared by an ordinary and
        // a test module is production if any ordinary path reaches it.
        let mut live = self.files.clone();
        for (_, child, _) in &self.modules { live.remove(child); }
        live.retain(|p| p.file_name().is_some_and(|n| n != "tests.rs"));
        loop {
            let before = live.len();
            for (parent, child, test) in &self.modules {
                if !test && live.contains(parent) && self.files.contains(child)
                    && child.file_name().is_some_and(|n| n != "tests.rs") {
                    live.insert(child.clone());
                }
            }
            if live.len() == before { return live; }
        }
    }
}

fn scan_dir(root: &Path, dir: &Path, found: &mut Review) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { scan_dir(root, &path, found); }
        else if path.extension().is_some_and(|x| x == "rs") {
            let file = path.strip_prefix(root).unwrap().to_str().unwrap().replace('\\', "/");
            let source = fs::read_to_string(&path).unwrap();
            let scan = Scan::new(&file, &source);
            let module = match path.file_name().and_then(|n| n.to_str()) {
                Some("mod.rs" | "lib.rs") => Path::new(&file).parent().unwrap().to_owned(),
                _ => Path::new(&file).with_extension(""),
            };
            found.files.insert(PathBuf::from(&file));
            scan.walk(0, scan.tokens.len(), "", &module, path.file_name().unwrap() == "tests.rs", found);
        }
    }
}

#[test]
fn sized_growth_has_a_reviewed_owner_and_reason() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Review::default();
    scan_dir(&root, &root, &mut found);
    let production = found.production_files();
    let mut sites = found.sites;
    sites.retain(|(file, _, _), _| production.contains(Path::new(file)));
    let mut failures = Vec::new();
    for &(file, owner, call, expected, ref reason) in REVIEWED {
        assert!(expected > 0, "empty review entry for {file} {owner} {call}");
        if let Gap(tag) = reason { assert!(!tag.is_empty(), "missing follow-up for {file} {owner}"); }
        let key = reviewed_key(file, owner, call);
        let found = sites.remove(&key).unwrap_or_default();
        if found.len() != expected {
            failures.push(format!("{file} {owner} {call} ({reason:?}): expected {expected}, found {}", found.len()));
            for (line, snippet) in found {
                failures.push(format!("{file}:{line} {owner} [{call}] on {snippet}"));
            }
        }
    }
    for ((file, owner, call), rows) in sites {
        for (line, snippet) in rows {
            failures.push(format!("{file}:{line} {owner} unreviewed [{}] on {snippet}", call.join(" ")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn growth_belongs_to_its_enclosing_function() {
    let source = r###"
        impl First {
            fn build() {
                let _ = || Vec /* capacity */ :: with_capacity(n);
                xs.resize(n, 0);
                xs.resize(n, NIL);
                ys.resize_with(n, || { error!("a message, {}", n) });
                fn inner() { f!(xs.reserve(n)); }
                let _ = [Vec::<u8>::with_capacity(n), Vec::with_capacity(n)];
                let _ = vec![{ let n = 1; n }, r#"vec![x; n]"#];
                let _ = buf.try_reserve(n);
                let _ = buf.try_reserve_exact(n);
                let _ = Vec::try_with_capacity(n);
            }
        }
        #[cfg(test)] pub(crate) mod fixtures { fn test() { vec![x; n]; } }
        #[cfg(test)] mod external_fixtures;
        mod unit { #![cfg(test)] fn also_test() { xs.resize(n, 0) } }
        impl Second { fn build() { let _ = vec![vec![x; n]; n]; } }
    "###;
    let scan = Scan::new("example.rs", source);
    let mut found = Review::default();
    scan.walk(0, scan.tokens.len(), "", Path::new("example"), false, &mut found);
    for (owner, call, count) in [
        ("First::build", "Vec::with_capacity(n)", 2),
        ("First::build", "Vec::<u8>::with_capacity(n)", 1),
        ("First::build::inner", ".reserve(n)", 1),
        ("First::build", ".resize(n)", 2),
        ("First::build", ".resize_with(n)", 1),
        ("Second::build", "vec!(n)", 2),
    ] {
        let key = reviewed_key("example.rs", owner, call);
        assert_eq!(found.sites.remove(&key).unwrap_or_default().len(), count, "{owner} must own {call}");
    }
    assert!(found.sites.is_empty(), "test modules, comments, literals, fallible calls and list separators: {:?}", found.sites);
    found.files.extend(["example.rs", "example/external_fixtures.rs", "example/standalone.rs"].map(PathBuf::from));
    let live = found.production_files();
    assert!(!live.contains(Path::new("example/external_fixtures.rs")), "out-of-line test modules");
    assert!(live.contains(Path::new("example/standalone.rs")), "unreferenced source files still need review");
}
