use makepad_rust_tokenizer::{live_id, Cursor, Delim, FullToken, LiveId, State};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    env,
    fs,
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug)]
struct VendoredCrate {
    crate_name: &'static str,
    version: &'static str,
    local_dir: &'static str,
}

const WINDOWS_CRATE: VendoredCrate = VendoredCrate {
    crate_name: "windows",
    version: "0.62.2",
    local_dir: "windows-rs",
};
const WINDOWS_COLLECTIONS_CRATE: VendoredCrate = VendoredCrate {
    crate_name: "windows-collections",
    version: "0.3.2",
    local_dir: "windows-collections",
};
const WINDOWS_CORE_CRATE: VendoredCrate = VendoredCrate {
    crate_name: "windows-core",
    version: "0.62.2",
    local_dir: "windows-core",
};
const WINDOWS_FUTURE_CRATE: VendoredCrate = VendoredCrate {
    crate_name: "windows-future",
    version: "0.3.2",
    local_dir: "windows-future",
};
const WINDOWS_NUMERICS_CRATE: VendoredCrate = VendoredCrate {
    crate_name: "windows-numerics",
    version: "0.3.1",
    local_dir: "windows-numerics",
};
const WINDOWS_THREADING_CRATE: VendoredCrate = VendoredCrate {
    crate_name: "windows-threading",
    version: "0.2.1",
    local_dir: "windows-threading",
};
const WINDOWS_LINK_CRATE: VendoredCrate = VendoredCrate {
    crate_name: "windows-link",
    version: "0.2.1",
    local_dir: "windows-link",
};
const WINDOWS_RESULT_CRATE: VendoredCrate = VendoredCrate {
    crate_name: "windows-result",
    version: "0.4.1",
    local_dir: "windows-result",
};
const WINDOWS_STRINGS_CRATE: VendoredCrate = VendoredCrate {
    crate_name: "windows-strings",
    version: "0.5.1",
    local_dir: "windows-strings",
};

const SUPPORT_CRATES: &[VendoredCrate] = &[
    WINDOWS_COLLECTIONS_CRATE,
    WINDOWS_CORE_CRATE,
    WINDOWS_FUTURE_CRATE,
    WINDOWS_NUMERICS_CRATE,
    WINDOWS_THREADING_CRATE,
    WINDOWS_LINK_CRATE,
    WINDOWS_RESULT_CRATE,
    WINDOWS_STRINGS_CRATE,
];

#[derive(Clone, Debug, PartialEq)]
pub struct TokenWithString {
    pub token: FullToken,
    pub value: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SymbolRef {
    module: Vec<String>,
    name: String,
}

#[derive(Clone, Debug)]
struct RawItem {
    order: usize,
    tokens: Vec<TokenWithString>,
    keyword: Option<String>,
    name: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct Entry {
    order: usize,
    snippets: Vec<(usize, String)>,
    deps: HashSet<SymbolRef>,
}

#[derive(Clone, Debug, Default)]
struct ModuleData {
    names: HashSet<String>,
    child_modules: HashSet<String>,
    entries: HashMap<String, Entry>,
}

#[derive(Default)]
struct ModuleNode {
    children: BTreeMap<String, ModuleNode>,
    snippets: Vec<(usize, String)>,
    seen: HashSet<String>,
}

fn parse_to_tokens(source: &str) -> Vec<TokenWithString> {
    let mut tokens = Vec::new();
    let mut total_chars = Vec::new();
    let mut state = State::default();
    let mut scratch = String::new();
    let mut last_token_start = 0;
    for line_str in source.lines() {
        let start = total_chars.len();
        total_chars.extend(line_str.chars());
        let mut cursor = Cursor::new(&total_chars[start..], &mut scratch);
        loop {
            let (next_state, full_token) = state.next(&mut cursor);
            if let Some(full_token) = full_token {
                let next_token_start = last_token_start + full_token.len;
                let value: String = total_chars[last_token_start..next_token_start]
                    .iter()
                    .collect();
                if !full_token.is_ws_or_comment() {
                    tokens.push(TokenWithString {
                        token: full_token.token,
                        value,
                    });
                } else if let Some(last) = tokens.last_mut() {
                    last.value.push_str(&value);
                }
                last_token_start = next_token_start;
            } else {
                break;
            }
            state = next_state;
        }
        if let Some(last) = tokens.last_mut() {
            last.value.push('\n');
        }
    }
    tokens
}

fn tokens_to_string(tokens: &[TokenWithString]) -> String {
    let mut out = String::new();
    for token in tokens {
        out.push_str(&token.value);
    }
    out
}

fn token_is_punct(tokens: &[TokenWithString], index: usize, punct: LiveId) -> bool {
    matches!(
        tokens.get(index),
        Some(TokenWithString {
            token: FullToken::Punct(id),
            ..
        }) if *id == punct
    )
}

fn token_ident<'a>(tokens: &'a [TokenWithString], index: usize) -> Option<&'a str> {
    match tokens.get(index) {
        Some(TokenWithString {
            token: FullToken::Ident(_),
            value,
        }) => Some(value.trim()),
        _ => None,
    }
}

fn ident_eq(tokens: &[TokenWithString], index: usize, value: &str) -> bool {
    token_ident(tokens, index) == Some(value)
}

fn is_colon_colon(tokens: &[TokenWithString], index: usize) -> bool {
    token_is_punct(tokens, index, live_id!(::))
}

fn is_semicolon(tokens: &[TokenWithString], index: usize) -> bool {
    token_is_punct(tokens, index, live_id!(;))
}

fn is_comma(tokens: &[TokenWithString], index: usize) -> bool {
    token_is_punct(tokens, index, live_id!(,))
}

fn is_star(tokens: &[TokenWithString], index: usize) -> bool {
    token_is_punct(tokens, index, live_id!(*))
}

fn is_ampersand(tokens: &[TokenWithString], index: usize) -> bool {
    token_is_punct(tokens, index, live_id!(&))
}

fn is_hash(tokens: &[TokenWithString], index: usize) -> bool {
    token_is_punct(tokens, index, live_id!(#))
}

fn find_matching_delim(tokens: &[TokenWithString], open_index: usize, delim: Delim) -> Option<usize> {
    let mut depth = 0usize;
    for i in open_index..tokens.len() {
        match tokens[i].token {
            FullToken::Open(d) if d == delim => {
                depth += 1;
            }
            FullToken::Close(d) if d == delim => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn skip_outer_attributes(tokens: &[TokenWithString], mut index: usize) -> usize {
    loop {
        if is_hash(tokens, index) {
            if matches!(
                tokens.get(index + 1),
                Some(TokenWithString {
                    token: FullToken::Open(Delim::Bracket),
                    ..
                })
            ) {
                if let Some(close) = find_matching_delim(tokens, index + 1, Delim::Bracket) {
                    index = close + 1;
                    continue;
                }
            }
        }
        break;
    }
    index
}

fn extract_quoted_values(line: &str, out: &mut HashSet<String>) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    if let Some(value) = line.get(start..i) {
                        out.insert(value.to_string());
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        i += 1;
    }
}

fn load_enabled_windows_features(platform_cargo_toml: &Path) -> HashSet<String> {
    let mut features = HashSet::new();
    let Ok(source) = fs::read_to_string(platform_cargo_toml) else {
        return features;
    };

    let mut in_windows_dep = false;
    let mut in_features = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_windows_dep = trimmed == "[target.'cfg(windows)'.dependencies.windows]";
            in_features = false;
            continue;
        }
        if !in_windows_dep {
            continue;
        }
        if in_features {
            extract_quoted_values(trimmed, &mut features);
            if trimmed.contains(']') {
                in_features = false;
            }
            continue;
        }
        if trimmed.starts_with("features") {
            if let Some(open) = trimmed.find('[') {
                let tail = &trimmed[open + 1..];
                extract_quoted_values(tail, &mut features);
                if !tail.contains(']') {
                    in_features = true;
                }
            }
        }
    }
    features
}

#[derive(Clone)]
struct CfgExprParser<'a> {
    source: &'a [u8],
    index: usize,
    enabled_features: &'a HashSet<String>,
}

impl<'a> CfgExprParser<'a> {
    fn new(source: &'a str, enabled_features: &'a HashSet<String>) -> Self {
        Self {
            source: source.as_bytes(),
            index: 0,
            enabled_features,
        }
    }

    fn skip_ws(&mut self) {
        while self.index < self.source.len() && self.source[self.index].is_ascii_whitespace() {
            self.index += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.index).copied()
    }

    fn consume_byte(&mut self, byte: u8) -> bool {
        self.skip_ws();
        if self.peek() == Some(byte) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn parse_ident(&mut self) -> Option<String> {
        self.skip_ws();
        let start = self.index;
        while self.index < self.source.len() {
            let b = self.source[self.index];
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.index += 1;
            } else {
                break;
            }
        }
        if self.index == start {
            return None;
        }
        String::from_utf8(self.source[start..self.index].to_vec()).ok()
    }

    fn parse_string(&mut self) -> Option<String> {
        self.skip_ws();
        if self.peek()? != b'"' {
            return None;
        }
        self.index += 1;
        let start = self.index;
        while self.index < self.source.len() {
            let b = self.source[self.index];
            if b == b'\\' {
                self.index += 2;
                continue;
            }
            if b == b'"' {
                let value = String::from_utf8(self.source[start..self.index].to_vec()).ok()?;
                self.index += 1;
                return Some(value);
            }
            self.index += 1;
        }
        None
    }

    fn skip_group(&mut self) {
        self.skip_ws();
        if self.peek() != Some(b'(') {
            return;
        }
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escape = false;
        while self.index < self.source.len() {
            let b = self.source[self.index];
            self.index += 1;
            if in_string {
                if escape {
                    escape = false;
                    continue;
                }
                if b == b'\\' {
                    escape = true;
                } else if b == b'"' {
                    in_string = false;
                }
                continue;
            }
            if b == b'"' {
                in_string = true;
                continue;
            }
            if b == b'(' {
                depth += 1;
            } else if b == b')' {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        }
    }

    fn parse_expr(&mut self) -> Option<bool> {
        let ident = self.parse_ident()?;
        match ident.as_str() {
            "all" | "any" => {
                if !self.consume_byte(b'(') {
                    return Some(true);
                }
                let mut values = Vec::new();
                loop {
                    self.skip_ws();
                    if self.consume_byte(b')') {
                        break;
                    }
                    values.push(self.parse_expr().unwrap_or(true));
                    self.skip_ws();
                    if self.consume_byte(b',') {
                        continue;
                    }
                    if self.consume_byte(b')') {
                        break;
                    }
                    return Some(true);
                }
                if ident == "all" {
                    Some(values.into_iter().all(|v| v))
                } else {
                    Some(values.into_iter().any(|v| v))
                }
            }
            "not" => {
                if !self.consume_byte(b'(') {
                    return Some(true);
                }
                let value = self.parse_expr().unwrap_or(true);
                let _ = self.consume_byte(b')');
                Some(!value)
            }
            "feature" => {
                if !self.consume_byte(b'=') {
                    return Some(true);
                }
                let value = self.parse_string()?;
                Some(self.enabled_features.contains(&value))
            }
            _ => {
                if self.consume_byte(b'=') {
                    if self.parse_string().is_none() {
                        let _ = self.parse_ident();
                    }
                } else {
                    self.skip_group();
                }
                Some(true)
            }
        }
    }
}

fn cfg_expr_is_enabled(expr: &str, enabled_features: &HashSet<String>) -> bool {
    let mut parser = CfgExprParser::new(expr, enabled_features);
    parser.parse_expr().unwrap_or(true)
}

fn item_enabled_for_features(tokens: &[TokenWithString], enabled_features: &HashSet<String>) -> bool {
    let mut index = 0usize;
    while is_hash(tokens, index) {
        let Some(TokenWithString {
            token: FullToken::Open(Delim::Bracket),
            ..
        }) = tokens.get(index + 1)
        else {
            break;
        };
        let Some(close) = find_matching_delim(tokens, index + 1, Delim::Bracket) else {
            break;
        };

        let attr_tokens = &tokens[index + 2..close];
        if ident_eq(attr_tokens, 0, "cfg")
            && matches!(
                attr_tokens.get(1),
                Some(TokenWithString {
                    token: FullToken::Open(Delim::Paren),
                    ..
                })
            )
        {
            if let Some(close_paren) = find_matching_delim(attr_tokens, 1, Delim::Paren) {
                let expr_tokens = &attr_tokens[2..close_paren];
                let expr = tokens_to_string(expr_tokens);
                if !cfg_expr_is_enabled(&expr, enabled_features) {
                    return false;
                }
            }
        }
        index = close + 1;
    }
    true
}

fn find_item_end(tokens: &[TokenWithString], start: usize) -> Option<usize> {
    let mut depth: isize = 0;
    let mut saw_block = false;
    for i in start..tokens.len() {
        match tokens[i].token {
            FullToken::Open(delim) => {
                depth += 1;
                if depth == 1 && delim == Delim::Brace {
                    saw_block = true;
                }
            }
            FullToken::Close(_) => {
                depth -= 1;
                if depth == 0 && saw_block {
                    if is_semicolon(tokens, i + 1) {
                        return Some(i + 1);
                    }
                    return Some(i);
                }
            }
            FullToken::Punct(id) if id == live_id!(;) && depth == 0 => {
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

fn extract_keyword_and_name(tokens: &[TokenWithString], start: usize) -> (Option<String>, Option<String>) {
    let mut i = start;
    if ident_eq(tokens, i, "pub") {
        i += 1;
        if matches!(
            tokens.get(i),
            Some(TokenWithString {
                token: FullToken::Open(Delim::Paren),
                ..
            })
        ) {
            if let Some(close) = find_matching_delim(tokens, i, Delim::Paren) {
                i = close + 1;
            }
        }
    }
    if ident_eq(tokens, i, "unsafe") {
        i += 1;
    }

    let (path, path_end) = parse_ident_path(tokens, i);
    if !path.is_empty() && token_is_punct(tokens, path_end, live_id!(!)) {
        let macro_name = path[path.len() - 1].as_str();
        if matches!(
            macro_name,
            "define_interface" | "interface_hierarchy" | "required_hierarchy"
        ) {
            let macro_keyword = Some(macro_name.to_string());
            let mut macro_name_arg = None;
            if matches!(
                tokens.get(path_end + 1),
                Some(TokenWithString {
                    token: FullToken::Open(Delim::Paren),
                    ..
                })
            ) {
                macro_name_arg = token_ident(tokens, path_end + 2).map(|v| v.to_string());
            }
            if macro_name == "define_interface" {
                return (macro_keyword, macro_name_arg);
            }
            return (macro_keyword, None);
        }
    }

    let keyword = token_ident(tokens, i).map(|v| v.to_string());
    let name = match keyword.as_deref() {
        Some("fn")
        | Some("const")
        | Some("type")
        | Some("struct")
        | Some("union")
        | Some("enum")
        | Some("trait")
        | Some("mod") => token_ident(tokens, i + 1).map(|v| v.to_string()),
        _ => None,
    };

    (keyword, name)
}

fn parse_top_level_items(tokens: &[TokenWithString]) -> Vec<RawItem> {
    let mut items = Vec::new();
    let mut i = 0usize;
    let mut order = 0usize;
    while i < tokens.len() {
        if is_semicolon(tokens, i) {
            i += 1;
            continue;
        }
        let start = i;
        let header_start = skip_outer_attributes(tokens, start);
        if header_start >= tokens.len() {
            break;
        }
        let Some(end) = find_item_end(tokens, header_start) else {
            i += 1;
            continue;
        };
        let (keyword, name) = extract_keyword_and_name(tokens, header_start);
        items.push(RawItem {
            order,
            tokens: tokens[start..=end].to_vec(),
            keyword,
            name,
        });
        order += 1;
        i = end + 1;
    }
    items
}

fn collect_idents(tokens: &[TokenWithString]) -> HashSet<String> {
    let mut out = HashSet::new();
    for token in tokens {
        if matches!(token.token, FullToken::Ident(_)) {
            out.insert(token.value.trim().to_string());
        }
    }
    out
}

fn parse_ident_path(tokens: &[TokenWithString], start: usize) -> (Vec<String>, usize) {
    let mut segments = Vec::new();
    let mut i = start;
    loop {
        let Some(ident) = token_ident(tokens, i) else {
            break;
        };
        segments.push(ident.to_string());
        i += 1;
        if is_colon_colon(tokens, i) && token_ident(tokens, i + 1).is_some() {
            i += 1;
            continue;
        }
        break;
    }
    (segments, i)
}

fn header_tokens(item_tokens: &[TokenWithString]) -> &[TokenWithString] {
    let mut depth = 0isize;
    for i in 0..item_tokens.len() {
        match item_tokens[i].token {
            FullToken::Open(delim) => {
                if depth == 0 && delim == Delim::Brace {
                    return &item_tokens[..i];
                }
                depth += 1;
            }
            FullToken::Close(_) => {
                depth -= 1;
            }
            _ => {}
        }
    }
    item_tokens
}

fn is_caps_type_candidate(ident: &str) -> bool {
    let bytes = ident.as_bytes();
    if bytes.len() < 3 {
        return false;
    }
    if bytes.len() >= 2 && bytes[0] == b'P' && bytes[1..].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut has_upper = false;
    for b in bytes {
        match *b {
            b'A'..=b'Z' => has_upper = true,
            b'0'..=b'9' | b'_' => {}
            _ => return false,
        }
    }
    has_upper
}

fn analyze_deps(
    item_tokens: &[TokenWithString],
    current_module: &[String],
    module_names: &HashSet<String>,
    child_modules: &HashSet<String>,
) -> HashSet<SymbolRef> {
    let mut deps = HashSet::new();
    let mut i = 0usize;
    while i < item_tokens.len() {
        if ident_eq(item_tokens, i, "crate")
            && is_colon_colon(item_tokens, i + 1)
            && ident_eq(item_tokens, i + 2, "Windows")
            && is_colon_colon(item_tokens, i + 3)
        {
            let (segments, next) = parse_ident_path(item_tokens, i + 4);
            if segments.len() >= 2 && segments[0] != "core" {
                deps.insert(SymbolRef {
                    module: segments[..segments.len() - 1].to_vec(),
                    name: segments[segments.len() - 1].clone(),
                });
            }
            i = next;
            continue;
        }

        if ident_eq(item_tokens, i, "super") {
            let mut j = i;
            let mut up = 0usize;
            loop {
                if ident_eq(item_tokens, j, "super") && is_colon_colon(item_tokens, j + 1) {
                    up += 1;
                    j += 2;
                } else {
                    break;
                }
            }
            if up > 0 {
                let (segments, next) = parse_ident_path(item_tokens, j);
                if !segments.is_empty() && current_module.len() >= up {
                    let mut module = current_module[..current_module.len() - up].to_vec();
                    let name = if segments.len() >= 2 {
                        module.extend(segments[..segments.len() - 1].iter().cloned());
                        segments[segments.len() - 1].clone()
                    } else {
                        segments[0].clone()
                    };
                    deps.insert(SymbolRef { module, name });
                }
                i = next;
                continue;
            }
        }

        if let Some(segment) = token_ident(item_tokens, i) {
            if child_modules.contains(segment) && is_colon_colon(item_tokens, i + 1) {
                let (segments, next) = parse_ident_path(item_tokens, i);
                if segments.len() >= 2 {
                    let mut module = current_module.to_vec();
                    module.extend(segments[..segments.len() - 1].iter().cloned());
                    deps.insert(SymbolRef {
                        module,
                        name: segments[segments.len() - 1].clone(),
                    });
                }
                i = next;
                continue;
            }
        }

        if token_is_punct(item_tokens, i, live_id!(:)) {
            let mut type_start = i + 1;
            while type_start < item_tokens.len() {
                if is_star(item_tokens, type_start)
                    || is_ampersand(item_tokens, type_start)
                    || ident_eq(item_tokens, type_start, "mut")
                    || ident_eq(item_tokens, type_start, "const")
                    || ident_eq(item_tokens, type_start, "dyn")
                {
                    type_start += 1;
                    continue;
                }
                break;
            }
            let (segments, _) = parse_ident_path(item_tokens, type_start);
            if segments.len() == 1 {
                let candidate = &segments[0];
                if is_caps_type_candidate(candidate) {
                    deps.insert(SymbolRef {
                        module: current_module.to_vec(),
                        name: candidate.clone(),
                    });
                }
            }
        }

        if let Some(ident) = token_ident(item_tokens, i) {
            if module_names.contains(ident) {
                deps.insert(SymbolRef {
                    module: current_module.to_vec(),
                    name: ident.to_string(),
                });
            }
        }
        i += 1;
    }
    deps
}

fn parse_module_data(
    module: &[String],
    windows_mod_root: &Path,
    enabled_features: &HashSet<String>,
) -> Option<ModuleData> {
    let mut module_dir = windows_mod_root.to_path_buf();
    for part in module {
        module_dir.push(part);
    }

    let mod_source = fs::read_to_string(module_dir.join("mod.rs")).ok()?;
    let mut parsed_sources = Vec::new();
    let mod_tokens = parse_to_tokens(&mod_source);
    let mod_raw_items = parse_top_level_items(&mod_tokens);
    parsed_sources.push(mod_raw_items);

    if mod_source.contains("core::include!(\"impl.rs\")") {
        if let Ok(impl_source) = fs::read_to_string(module_dir.join("impl.rs")) {
            let impl_tokens = parse_to_tokens(&impl_source);
            let impl_raw_items = parse_top_level_items(&impl_tokens);
            parsed_sources.push(impl_raw_items);
        }
    }

    let mut raw_items = Vec::new();
    let mut names = HashSet::new();
    let mut child_modules = HashSet::new();
    let mut order = 0usize;
    for file_raw_items in &parsed_sources {
        for item in file_raw_items {
            let mut item = item.clone();
            item.order = order;
            order += 1;
            if !item_enabled_for_features(&item.tokens, enabled_features) {
                continue;
            }

            if let Some(name) = &item.name {
                if item.keyword.as_deref() == Some("mod") {
                    child_modules.insert(name.clone());
                } else {
                    names.insert(name.clone());
                }
            }
            raw_items.push(item);
        }
    }

    let mut entries: HashMap<String, Entry> = HashMap::new();

    for item in &raw_items {
        if let Some(name) = &item.name {
            if item.keyword.as_deref() == Some("mod") {
                continue;
            }
            let snippet = tokens_to_string(&item.tokens);
            let deps = analyze_deps(&item.tokens, module, &names, &child_modules);
            let entry = entries.entry(name.clone()).or_insert_with(|| Entry {
                order: item.order,
                snippets: Vec::new(),
                deps: HashSet::new(),
            });
            entry.order = entry.order.min(item.order);
            entry.snippets.push((item.order, snippet));
            entry.deps.extend(deps);
        }
    }

    for item in &raw_items {
        if item.name.is_some() {
            continue;
        }
        let idents = if item.keyword.as_deref() == Some("impl") {
            collect_idents(header_tokens(&item.tokens))
        } else {
            collect_idents(&item.tokens)
        };
        let mut related = Vec::new();
        if idents.len() <= names.len() {
            for ident in &idents {
                if names.contains(ident) {
                    related.push(ident.clone());
                }
            }
        } else {
            for name in &names {
                if idents.contains(name) {
                    related.push(name.clone());
                }
            }
        }
        if related.is_empty() {
            continue;
        }
        let snippet = tokens_to_string(&item.tokens);
        let deps = analyze_deps(&item.tokens, module, &names, &child_modules);
        for name in related {
            if let Some(entry) = entries.get_mut(&name) {
                entry.snippets.push((item.order, snippet.clone()));
                entry.deps.extend(deps.clone());
            }
        }
    }

    for entry in entries.values_mut() {
        let mut seen = HashSet::new();
        entry.snippets.retain(|(_, snippet)| seen.insert(snippet.clone()));
        entry.snippets.sort_by_key(|(order, _)| *order);
    }

    Some(ModuleData {
        names,
        child_modules,
        entries,
    })
}

fn fallback_extract_entry(
    module: &[String],
    symbol_name: &str,
    windows_mod_root: &Path,
    module_names: &HashSet<String>,
    child_modules: &HashSet<String>,
    enabled_features: &HashSet<String>,
) -> Option<Entry> {
    let mut path = windows_mod_root.to_path_buf();
    for part in module {
        path.push(part);
    }
    path.push("mod.rs");
    let source = fs::read_to_string(path).ok()?;

    // First try a token-based extraction so we can include full definitions and related impl blocks.
    let mut sources = vec![source.clone()];
    if source.contains("core::include!(\"impl.rs\")") {
        let mut impl_path = windows_mod_root.to_path_buf();
        for part in module {
            impl_path.push(part);
        }
        impl_path.push("impl.rs");
        if let Ok(impl_source) = fs::read_to_string(impl_path) {
            sources.push(impl_source);
        }
    }

    let mut token_snippets: Vec<(usize, String)> = Vec::new();
    let mut token_deps: HashSet<SymbolRef> = HashSet::new();
    let mut order_base = 0usize;
    for source_text in &sources {
        let tokens = parse_to_tokens(source_text);
        let raw_items = parse_top_level_items(&tokens);
        for item in &raw_items {
            if !item_enabled_for_features(&item.tokens, enabled_features) {
                continue;
            }
            let mut include = item.name.as_deref() == Some(symbol_name)
                && item.keyword.as_deref() != Some("mod");
            if !include && item.name.is_none() {
                let idents = if item.keyword.as_deref() == Some("impl") {
                    collect_idents(header_tokens(&item.tokens))
                } else {
                    collect_idents(&item.tokens)
                };
                include = idents.contains(symbol_name);
            }
            if !include {
                continue;
            }

            let snippet = tokens_to_string(&item.tokens);
            let deps = analyze_deps(&item.tokens, module, module_names, child_modules);
            token_snippets.push((order_base + item.order, snippet));
            token_deps.extend(deps);
        }
        order_base += raw_items.len() + 1;
    }
    if !token_snippets.is_empty() {
        let mut seen = HashSet::new();
        token_snippets.retain(|(_, snippet)| seen.insert(snippet.clone()));
        token_snippets.sort_by_key(|(order, _)| *order);
        let order = token_snippets
            .first()
            .map(|(order, _)| *order)
            .unwrap_or(usize::MAX / 2);
        return Some(Entry {
            order,
            snippets: token_snippets,
            deps: token_deps,
        });
    }

    let patterns = [
        format!("windows_core::imp::define_interface!({}", symbol_name),
        format!("pub struct {}", symbol_name),
        format!("struct {}", symbol_name),
        format!("pub union {}", symbol_name),
        format!("pub enum {}", symbol_name),
        format!("pub trait {}", symbol_name),
        format!("pub type {}", symbol_name),
        format!("pub unsafe fn {}", symbol_name),
        format!("pub fn {}", symbol_name),
        format!("pub const {}", symbol_name),
    ];

    fn find_item_end_in_source(source: &str, start: usize) -> Option<usize> {
        let bytes = source.as_bytes();
        let mut i = start;
        let mut depth_brace = 0usize;
        let mut depth_paren = 0usize;
        let mut depth_bracket = 0usize;
        let mut saw_brace = false;
        let mut in_string = false;
        let mut in_char = false;
        let mut escape = false;

        while i < bytes.len() {
            let b = bytes[i];

            if in_string {
                if escape {
                    escape = false;
                } else if b == b'\\' {
                    escape = true;
                } else if b == b'"' {
                    in_string = false;
                }
                i += 1;
                continue;
            }
            if in_char {
                if escape {
                    escape = false;
                } else if b == b'\\' {
                    escape = true;
                } else if b == b'\'' {
                    in_char = false;
                }
                i += 1;
                continue;
            }

            match b {
                b'"' => in_string = true,
                b'\'' => in_char = true,
                b'{' => {
                    depth_brace += 1;
                    saw_brace = true;
                }
                b'}' => {
                    if depth_brace > 0 {
                        depth_brace -= 1;
                    }
                    if depth_brace == 0 && saw_brace {
                        let mut end = i;
                        let mut j = i + 1;
                        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                            j += 1;
                        }
                        if j < bytes.len() && bytes[j] == b';' {
                            end = j;
                        }
                        return Some(end);
                    }
                }
                b'(' => depth_paren += 1,
                b')' => {
                    if depth_paren > 0 {
                        depth_paren -= 1;
                    }
                }
                b'[' => depth_bracket += 1,
                b']' => {
                    if depth_bracket > 0 {
                        depth_bracket -= 1;
                    }
                }
                b';' if depth_brace == 0 && depth_paren == 0 && depth_bracket == 0 => {
                    return Some(i);
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    fn include_leading_attrs(source: &str, start: usize) -> usize {
        let mut item_start = source[..start].rfind('\n').map(|v| v + 1).unwrap_or(0);
        while item_start > 0 {
            let prev_end = item_start - 1;
            let prev_start = source[..prev_end].rfind('\n').map(|v| v + 1).unwrap_or(0);
            let prev_line = source[prev_start..prev_end].trim();
            if prev_line.is_empty() || prev_line.starts_with("#[") {
                item_start = prev_start;
                continue;
            }
            break;
        }
        item_start
    }

    fn is_ident_char(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }

    fn module_declares_symbol(source: &str, symbol: &str) -> bool {
        let patterns = [
            format!("windows_core::imp::define_interface!({}", symbol),
            format!("pub struct {}", symbol),
            format!("struct {}", symbol),
            format!("pub union {}", symbol),
            format!("pub enum {}", symbol),
            format!("pub trait {}", symbol),
            format!("pub type {}", symbol),
            format!("pub unsafe fn {}", symbol),
            format!("pub fn {}", symbol),
            format!("pub const {}", symbol),
        ];
        let bytes = source.as_bytes();
        for pattern in patterns {
            let mut search_start = 0usize;
            while let Some(rel) = source[search_start..].find(&pattern) {
                let index = search_start + rel;
                let end = index + pattern.len();
                if end == bytes.len() || !is_ident_char(bytes[end]) {
                    return true;
                }
                search_start = end;
            }
        }
        false
    }

    let source_bytes = source.as_bytes();
    let mut snippet = None;
    let mut snippet_start = 0usize;
    for pattern in patterns {
        let mut search_start = 0usize;
        while let Some(rel) = source[search_start..].find(&pattern) {
            let index = search_start + rel;
            let pattern_end = index + pattern.len();
            if pattern_end < source_bytes.len() && is_ident_char(source_bytes[pattern_end]) {
                search_start = pattern_end;
                continue;
            }
            let start = include_leading_attrs(&source, index);
            if let Some(end) = find_item_end_in_source(&source, start) {
                snippet = Some(source[start..=end].to_string());
                snippet_start = start;
                break;
            }
            search_start = index + 1;
        }
        if snippet.is_some() {
            break;
        }
    }
    let snippet = snippet?;
    let mut snippets = Vec::new();
    let mut deps = HashSet::new();

    let tail_source = &source[snippet_start..];
    let tail_tokens = parse_to_tokens(tail_source);
    let tail_raw_items = parse_top_level_items(&tail_tokens);
    for item in &tail_raw_items {
        if !item_enabled_for_features(&item.tokens, enabled_features) {
            continue;
        }
        let mut include = item.name.as_deref() == Some(symbol_name)
            && item.keyword.as_deref() != Some("mod");
        if !include && item.name.is_none() {
            let idents = if item.keyword.as_deref() == Some("impl") {
                collect_idents(header_tokens(&item.tokens))
            } else {
                collect_idents(&item.tokens)
            };
            include = idents.contains(symbol_name);
        }
        if !include {
            continue;
        }

        let snippet = tokens_to_string(&item.tokens);
        deps.extend(analyze_deps(&item.tokens, module, module_names, child_modules));
        snippets.push((usize::MAX / 2 + item.order, snippet));
    }
    if snippets.is_empty() {
        let snippet_tokens = parse_to_tokens(&snippet);
        deps.extend(analyze_deps(
            &snippet_tokens,
            module,
            module_names,
            child_modules,
        ));
        snippets.push((usize::MAX / 2, snippet));
    }

    let pub_struct_pattern = format!("pub struct {}", symbol_name);
    let struct_pattern = format!("struct {}", symbol_name);
    for (_, snippet) in &mut snippets {
        let trimmed = snippet.trim_start();
        let is_struct = trimmed.starts_with(&pub_struct_pattern) || trimmed.starts_with(&struct_pattern);
        let has_attrs = trimmed.starts_with("#[");
        if !is_struct || has_attrs {
            continue;
        }
        let struct_index = source
            .find(&pub_struct_pattern)
            .or_else(|| source.find(&struct_pattern));
        let Some(struct_index) = struct_index else {
            continue;
        };
        let attr_start = include_leading_attrs(&source, struct_index);
        if attr_start < struct_index {
            let attrs = &source[attr_start..struct_index];
            if !attrs.trim().is_empty() {
                *snippet = format!("{}{}", attrs, snippet);
            }
        }
    }

    for (_, snippet) in &snippets {
        let snippet_tokens = parse_to_tokens(snippet);

        if (ident_eq(&snippet_tokens, 0, "pub") && ident_eq(&snippet_tokens, 1, "const"))
            || ident_eq(&snippet_tokens, 0, "const")
        {
            let mut i = 0usize;
            while i < snippet_tokens.len() {
                if token_is_punct(&snippet_tokens, i, live_id!(:)) {
                    let (segments, _) = parse_ident_path(&snippet_tokens, i + 1);
                    if segments.len() == 1 {
                        let candidate = &segments[0];
                        if candidate != symbol_name && module_declares_symbol(&source, candidate) {
                            deps.insert(SymbolRef {
                                module: module.to_vec(),
                                name: candidate.clone(),
                            });
                        }
                    }
                    break;
                }
                i += 1;
            }
        }
    }

    let mut seen = HashSet::new();
    snippets.retain(|(_, snippet)| seen.insert(snippet.clone()));
    snippets.sort_by_key(|(order, _)| *order);

    Some(Entry {
        order: snippets
            .first()
            .map(|(order, _)| *order)
            .unwrap_or(usize::MAX / 2),
        snippets,
        deps,
    })
}

fn normalize_windows_path(path: &[String]) -> Option<Vec<String>> {
    for i in 0..path.len() {
        if path[i] == "windows" {
            if i > 0 && path[i - 1] == "os" {
                continue;
            }
            let rest = path[i + 1..].to_vec();
            if rest.is_empty() {
                return None;
            }
            return Some(rest);
        }
    }
    None
}

fn parse_use_tokens(tokens: &[TokenWithString]) -> (Vec<Vec<String>>, Vec<Vec<String>>) {
    let mut explicit = Vec::new();
    let mut globs = Vec::new();
    let mut stack: Vec<Vec<String>> = vec![Vec::new()];
    let mut current: Vec<String> = Vec::new();
    let mut pending = false;
    let mut skip_alias = false;
    let mut i = 0usize;

    while i < tokens.len() {
        if let Some(ident) = token_ident(tokens, i) {
            if skip_alias {
                skip_alias = false;
                i += 1;
                continue;
            }
            if ident == "as" {
                skip_alias = true;
                i += 1;
                continue;
            }
            current.push(ident.to_string());
            pending = true;
            i += 1;
            continue;
        }

        if is_colon_colon(tokens, i) {
            i += 1;
            continue;
        }

        if is_star(tokens, i) {
            if !current.is_empty() {
                globs.push(current.clone());
            }
            pending = false;
            i += 1;
            continue;
        }

        if matches!(
            tokens.get(i),
            Some(TokenWithString {
                token: FullToken::Open(Delim::Brace),
                ..
            })
        ) {
            stack.push(current.clone());
            pending = false;
            i += 1;
            continue;
        }

        if matches!(
            tokens.get(i),
            Some(TokenWithString {
                token: FullToken::Close(Delim::Brace),
                ..
            })
        ) {
            if pending {
                explicit.push(current.clone());
            }
            stack.pop();
            current = stack.last().cloned().unwrap_or_default();
            pending = false;
            i += 1;
            continue;
        }

        if is_comma(tokens, i) {
            if pending {
                explicit.push(current.clone());
            }
            current = stack.last().cloned().unwrap_or_default();
            pending = false;
            i += 1;
            continue;
        }

        i += 1;
    }

    if pending {
        explicit.push(current);
    }

    (explicit, globs)
}

fn collect_imports_from_file(
    file_path: &Path,
) -> (HashSet<SymbolRef>, Vec<(Vec<String>, HashSet<String>)>) {
    let source = fs::read_to_string(file_path).unwrap();
    let tokens = parse_to_tokens(&source);
    let idents = collect_idents(&tokens);

    let mut explicit = HashSet::new();
    let mut globs = Vec::new();

    let mut depth = 0isize;
    let mut i = 0usize;
    while i < tokens.len() {
        match tokens[i].token {
            FullToken::Open(_) => {
                depth += 1;
                i += 1;
                continue;
            }
            FullToken::Close(_) => {
                depth -= 1;
                i += 1;
                continue;
            }
            _ => {}
        }

        if depth == 0 && ident_eq(&tokens, i, "use") {
            let mut j = i + 1;
            let mut local_depth = 0isize;
            while j < tokens.len() {
                match tokens[j].token {
                    FullToken::Open(_) => local_depth += 1,
                    FullToken::Close(_) => local_depth -= 1,
                    FullToken::Punct(id) if id == live_id!(;) && local_depth == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            if j <= tokens.len() {
                let (use_explicit, use_globs) = parse_use_tokens(&tokens[i + 1..j]);
                for path in use_explicit {
                    if let Some(path) = normalize_windows_path(&path) {
                        if path[0] == "core" {
                            continue;
                        }
                        if path.len() >= 1 {
                            explicit.insert(SymbolRef {
                                module: path[..path.len() - 1].to_vec(),
                                name: path[path.len() - 1].clone(),
                            });
                        }
                    }
                }
                for path in use_globs {
                    if let Some(path) = normalize_windows_path(&path) {
                        if path[0] == "core" {
                            continue;
                        }
                        globs.push((path, idents.clone()));
                    }
                }
            }
            i = j + 1;
            continue;
        }

        if ident_eq(&tokens, i, "windows") && is_colon_colon(&tokens, i + 1) {
            let (path, next) = parse_ident_path(&tokens, i);
            if let Some(path) = normalize_windows_path(&path) {
                if path[0] != "core" && !path.is_empty() {
                    explicit.insert(SymbolRef {
                        module: path[..path.len() - 1].to_vec(),
                        name: path[path.len() - 1].clone(),
                    });
                }
            }
            if next > i {
                i = next;
                continue;
            }
        }

        i += 1;
    }

    (explicit, globs)
}

fn cargo_home_candidates() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Ok(cargo_home) = env::var("CARGO_HOME") {
        if !cargo_home.is_empty() {
            homes.push(PathBuf::from(cargo_home));
        }
    }
    if let Ok(home) = env::var("HOME") {
        if !home.is_empty() {
            homes.push(PathBuf::from(home).join(".cargo"));
        }
    }
    homes.sort();
    homes.dedup();
    homes
}

fn find_crate_in_cargo_registry(crate_spec: VendoredCrate) -> Option<PathBuf> {
    let crate_dir_name = format!("{}-{}", crate_spec.crate_name, crate_spec.version);
    for cargo_home in cargo_home_candidates() {
        let registry_src = cargo_home.join("registry").join("src");
        let Ok(entries) = fs::read_dir(registry_src) else {
            continue;
        };
        let mut index_dirs = Vec::new();
        for entry in entries.flatten() {
            let index_dir = entry.path();
            if index_dir.is_dir() {
                index_dirs.push(index_dir);
            }
        }
        index_dirs.sort();
        for index_dir in index_dirs {
            let candidate = index_dir.join(&crate_dir_name);
            if candidate.join("Cargo.toml").is_file() && candidate.join("src").is_dir() {
                return Some(candidate);
            }
        }
    }
    None
}

fn resolve_windows_source_input() -> PathBuf {
    if let Some(arg) = env::args().nth(1) {
        return PathBuf::from(arg);
    }

    find_crate_in_cargo_registry(WINDOWS_CRATE).unwrap_or_else(|| {
        panic!(
            "could not locate {}-{} in the Cargo registry; run `cargo fetch --manifest-path tools/windows_strip/Cargo.toml --features fetch-windows-upstream` or pass an explicit source path",
            WINDOWS_CRATE.crate_name,
            WINDOWS_CRATE.version
        )
    })
}

fn resolve_windows_source_and_mod_root(windows_source: &Path) -> (PathBuf, PathBuf) {
    if windows_source.join("src/Windows").is_dir() {
        return (windows_source.to_path_buf(), windows_source.join("src/Windows"));
    }
    if windows_source.join("Windows").is_dir() {
        let source_root = windows_source
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return (source_root, windows_source.join("Windows"));
    }
    if windows_source
        .file_name()
        .and_then(|v| v.to_str())
        .map(|v| v == "Windows")
        .unwrap_or(false)
        && windows_source.is_dir()
    {
        let source_root = windows_source
            .parent()
            .and_then(|v| v.parent())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return (source_root, windows_source.to_path_buf());
    }
    panic!(
        "windows source path '{}' must contain 'src/Windows' or 'Windows'",
        windows_source.display()
    );
}

fn resolve_sibling_crate_source(windows_source_root: &Path, crate_spec: VendoredCrate) -> Option<PathBuf> {
    let Some(parent) = windows_source_root.parent() else {
        return None;
    };
    let candidate = parent.join(format!("{}-{}", crate_spec.crate_name, crate_spec.version));
    if candidate.join("Cargo.toml").is_file() && candidate.join("src").is_dir() {
        Some(candidate)
    } else {
        None
    }
}

fn resolve_support_crate_source(windows_source_root: &Path, crate_spec: VendoredCrate) -> PathBuf {
    if let Some(path) = resolve_sibling_crate_source(windows_source_root, crate_spec) {
        return path;
    }
    find_crate_in_cargo_registry(crate_spec).unwrap_or_else(|| {
        panic!(
            "could not locate {}-{} in the Cargo registry; run `cargo fetch --manifest-path tools/windows_strip/Cargo.toml --features fetch-windows-upstream`",
            crate_spec.crate_name, crate_spec.version
        )
    })
}

fn vendored_crate_root(crate_spec: VendoredCrate) -> PathBuf {
    Path::new("./libs").join(crate_spec.local_dir)
}

fn copy_tree(src_root: &Path, dst_root: &Path) -> io::Result<()> {
    fs::create_dir_all(dst_root)?;
    for entry in fs::read_dir(src_root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if matches!(name_str.as_ref(), ".cargo-ok" | ".cargo_vcs_info.json" | "Cargo.lock") {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst_root.join(name);
        if src_path.is_dir() {
            copy_tree(&src_path, &dst_path)?;
        } else if src_path.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn copy_crate_source(crate_spec: VendoredCrate, source_root: &Path) -> PathBuf {
    let dest_root = vendored_crate_root(crate_spec);
    if dest_root.exists() {
        fs::remove_dir_all(&dest_root).unwrap();
    }
    copy_tree(source_root, &dest_root).unwrap();
    dest_root
}

fn remove_manifest_section(manifest: &mut String, section_name: &str) {
    let header = format!("[{}]", section_name);
    let mut out = String::new();
    let mut skipping = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            skipping = trimmed == header;
        }
        if skipping {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    *manifest = out;
}

fn set_manifest_dependency_path(manifest: &mut String, section_name: &str, path: &str) {
    let header = format!("[{}]", section_name);
    let mut out = String::new();
    let mut in_section = false;
    let mut saw_section = false;
    let mut wrote_path = false;

    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_section && !wrote_path {
                out.push_str(&format!("path = \"{}\"\n", path));
            }
            in_section = trimmed == header;
            if in_section {
                saw_section = true;
                wrote_path = false;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if in_section && trimmed.starts_with("path") {
            if !wrote_path {
                out.push_str(&format!("path = \"{}\"\n", path));
                wrote_path = true;
            }
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    if in_section && !wrote_path {
        out.push_str(&format!("path = \"{}\"\n", path));
    }

    if !saw_section {
        panic!("missing dependency section [{}] in Cargo.toml", section_name);
    }

    *manifest = out;
}

fn patch_manifest_for_crate(crate_spec: VendoredCrate, crate_root: &Path) {
    let manifest_path = crate_root.join("Cargo.toml");
    let mut manifest = fs::read_to_string(&manifest_path).unwrap();

    match crate_spec.crate_name {
        "windows" => {
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-collections",
                "../windows-collections",
            );
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-core",
                "../windows-core",
            );
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-future",
                "../windows-future",
            );
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-numerics",
                "../windows-numerics",
            );
            let old_std_feature = "std = [\n    \"windows-collections/std\",\n    \"windows-core/std\",\n    \"windows-future/std\",\n    \"windows-numerics/std\",\n]\n";
            let new_std_feature =
                "std = [\n    \"windows-core/std\",\n    \"windows-future/std\",\n]\n";
            if !manifest.contains(old_std_feature) {
                panic!("windows std feature block did not match expected upstream shape");
            }
            manifest = manifest.replacen(old_std_feature, new_std_feature, 1);
        }
        "windows-core" => {
            remove_manifest_section(&mut manifest, "dependencies.windows-implement");
            remove_manifest_section(&mut manifest, "dependencies.windows-interface");
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-link",
                "../windows-link",
            );
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-result",
                "../windows-result",
            );
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-strings",
                "../windows-strings",
            );
        }
        "windows-collections" => {
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-core",
                "../windows-core",
            );
            set_manifest_dependency_path(
                &mut manifest,
                "dev-dependencies.windows-strings",
                "../windows-strings",
            );
        }
        "windows-future" => {
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-core",
                "../windows-core",
            );
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-link",
                "../windows-link",
            );
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-threading",
                "../windows-threading",
            );
        }
        "windows-numerics" => {
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-core",
                "../windows-core",
            );
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-link",
                "../windows-link",
            );
        }
        "windows-threading" | "windows-result" | "windows-strings" => {
            set_manifest_dependency_path(
                &mut manifest,
                "dependencies.windows-link",
                "../windows-link",
            );
        }
        "windows-link" => {}
        _ => panic!("unsupported vendored crate {}", crate_spec.crate_name),
    }

    fs::write(manifest_path, manifest).unwrap();
}

fn strip_windows_core_macros(crate_root: &Path) {
    let lib_path = crate_root.join("src/lib.rs");
    let mut source = fs::read_to_string(&lib_path).unwrap();
    source = source.replace("pub use windows_implement::implement;\n", "");
    source = source.replace("pub use windows_interface::interface;\n", "");
    fs::write(lib_path, source).unwrap();
}

fn strip_windows_future_implement_helpers(crate_root: &Path) {
    let lib_path = crate_root.join("src/lib.rs");
    let mut source = fs::read_to_string(&lib_path).unwrap();
    source = source.replace("#[cfg(feature = \"std\")]\nmod async_ready;\n", "");
    source = source.replace("#[cfg(feature = \"std\")]\nmod async_spawn;\n", "");
    fs::write(lib_path, source).unwrap();
}

fn load_module<'a>(
    module: &[String],
    windows_mod_root: &Path,
    cache: &'a mut HashMap<Vec<String>, Option<ModuleData>>,
    enabled_features: &HashSet<String>,
) -> Option<&'a ModuleData> {
    let key = module.to_vec();
    let data = cache
        .entry(key)
        .or_insert_with(|| parse_module_data(module, windows_mod_root, enabled_features));
    data.as_ref()
}

fn insert_snippet(node: &mut ModuleNode, module: &[String], order: usize, snippet: &str) {
    let mut cursor = node;
    for part in module {
        cursor = cursor.children.entry(part.clone()).or_default();
    }
    let key = snippet.trim();
    if !cursor.seen.contains(key) {
        let owned = snippet.to_string();
        cursor.seen.insert(key.to_string());
        cursor.snippets.push((order, owned));
    }
}

fn enqueue_symbol(queue: &mut VecDeque<SymbolRef>, enqueued: &mut HashSet<SymbolRef>, symbol: SymbolRef) {
    if enqueued.insert(symbol.clone()) {
        queue.push_back(symbol);
    }
}

fn render_module(node: &ModuleNode, out: &mut String) {
    let mut snippets = node.snippets.clone();
    snippets.sort_by_key(|(order, _)| *order);
    for (_, snippet) in snippets {
        out.push_str(&snippet);
        if !snippet.ends_with('\n') {
            out.push('\n');
        }
    }

    for (module_name, child) in &node.children {
        out.push_str(&format!("pub mod {}{{\n", module_name));
        render_module(child, out);
        out.push_str("}\n");
    }
}

fn regenerate_vendored_windows_crate(source_root: &Path, generated_mod: &str) {
    let vendored_root = copy_crate_source(WINDOWS_CRATE, source_root);
    patch_manifest_for_crate(WINDOWS_CRATE, &vendored_root);
    let mut lib_source = fs::read_to_string(source_root.join("src/lib.rs")).unwrap();
    lib_source = lib_source.replace("\nmod extensions;\n", "\n");
    lib_source = lib_source.replace("\r\nmod extensions;\r\n", "\r\n");
    fs::write(vendored_root.join("src/lib.rs"), lib_source).unwrap();
    fs::write(vendored_root.join("src/Windows/mod.rs"), generated_mod).unwrap();
}

fn regenerate_vendored_support_crates(windows_source_root: &Path) {
    for crate_spec in SUPPORT_CRATES {
        let source_root = resolve_support_crate_source(windows_source_root, *crate_spec);
        let vendored_root = copy_crate_source(*crate_spec, &source_root);
        patch_manifest_for_crate(*crate_spec, &vendored_root);
        if crate_spec.crate_name == "windows-core" {
            strip_windows_core_macros(&vendored_root);
        } else if crate_spec.crate_name == "windows-future" {
            strip_windows_future_implement_helpers(&vendored_root);
        }
    }
}

fn main() {
    let windows_source_arg = resolve_windows_source_input();
    let (windows_source_root, windows_mod_root) =
        resolve_windows_source_and_mod_root(&windows_source_arg);
    let enabled_features = load_enabled_windows_features(Path::new("./platform/Cargo.toml"));

    let mut explicit_imports = HashSet::new();
    let mut glob_imports: Vec<(Vec<String>, HashSet<String>)> = Vec::new();

    let windows_src_dir = Path::new("./platform/src/os/windows");
    for entry in fs::read_dir(windows_src_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("rs") {
            continue;
        }
        let (file_explicit, file_globs) = collect_imports_from_file(&path);
        explicit_imports.extend(file_explicit);
        glob_imports.extend(file_globs);
    }

    let mut module_cache: HashMap<Vec<String>, Option<ModuleData>> = HashMap::new();
    let mut selected: HashMap<Vec<String>, HashSet<String>> = HashMap::new();
    let mut selected_symbols: HashSet<SymbolRef> = HashSet::new();
    let mut queue: VecDeque<SymbolRef> = VecDeque::new();
    let mut enqueued: HashSet<SymbolRef> = HashSet::new();

    for symbol in explicit_imports {
        enqueue_symbol(&mut queue, &mut enqueued, symbol);
    }

    for (module, used_idents) in glob_imports {
        if let Some(module_data) = load_module(
            &module,
            &windows_mod_root,
            &mut module_cache,
            &enabled_features,
        ) {
            if used_idents.len() <= module_data.names.len() {
                for ident in &used_idents {
                    if module_data.names.contains(ident) {
                        enqueue_symbol(
                            &mut queue,
                            &mut enqueued,
                            SymbolRef {
                                module: module.clone(),
                                name: ident.clone(),
                            },
                        );
                    }
                }
            } else {
                for name in &module_data.names {
                    if used_idents.contains(name) {
                        enqueue_symbol(
                            &mut queue,
                            &mut enqueued,
                            SymbolRef {
                                module: module.clone(),
                                name: name.clone(),
                            },
                        );
                    }
                }
            }
        }
    }

    while let Some(symbol) = queue.pop_front() {
        if selected_symbols.contains(&symbol) {
            continue;
        }

        let module_key = symbol.module.clone();
        let module_data = module_cache
            .entry(module_key)
            .or_insert_with(|| {
                parse_module_data(&symbol.module, &windows_mod_root, &enabled_features)
            });
        let Some(module_data) = module_data.as_mut() else {
            continue;
        };
        if !module_data.entries.contains_key(&symbol.name) {
            if let Some(entry) = fallback_extract_entry(
                &symbol.module,
                &symbol.name,
                &windows_mod_root,
                &module_data.names,
                &module_data.child_modules,
                &enabled_features,
            ) {
                module_data.names.insert(symbol.name.clone());
                module_data.entries.insert(symbol.name.clone(), entry);
            }
        }
        let Some(entry) = module_data.entries.get(&symbol.name) else {
            continue;
        };
        let deps = entry.deps.clone();

        selected_symbols.insert(symbol.clone());
        selected
            .entry(symbol.module.clone())
            .or_default()
            .insert(symbol.name.clone());

        for dep in deps {
            if dep.module.first().map(|v| v.as_str()) == Some("core") {
                continue;
            }
            enqueue_symbol(&mut queue, &mut enqueued, dep);
        }
    }

    let mut root = ModuleNode::default();
    for (module, names) in &selected {
        let Some(module_data) = load_module(
            module,
            &windows_mod_root,
            &mut module_cache,
            &enabled_features,
        ) else {
            continue;
        };
        for name in names {
            if let Some(entry) = module_data.entries.get(name) {
                for (order, snippet) in &entry.snippets {
                    insert_snippet(&mut root, module, *order, snippet);
                }
            }
        }
    }

    let mut generated = String::new();
    render_module(&root, &mut generated);

    regenerate_vendored_windows_crate(&windows_source_root, &generated);
    regenerate_vendored_support_crates(&windows_source_root);
}
