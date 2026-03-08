use crate::cx::Cx;
use crate::makepad_script::{
    parser::ScriptParser,
    tokenizer::ScriptTokenizer,
    ScriptMod,
    ScriptModKey,
    ScriptSource,
    ScriptValue,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub(crate) struct PendingLiveChange {
    file_name: String,
    content: String,
}

#[derive(Clone, Debug, Default)]
pub struct CxLiveReloadState {
    pub(crate) pending_files: Vec<PendingLiveChange>,
    pub script_mod_overrides: Rc<RefCell<HashMap<ScriptModKey, String>>>,
}

#[derive(Clone, Copy, Debug)]
struct FilePos {
    line: usize,
    column: usize,
}

#[derive(Clone, Debug)]
struct ExtractedScriptMod {
    code: String,
    rust_value_count: usize,
    first_token_line: usize,
    first_token_column: usize,
}

#[derive(Clone, Debug)]
struct CompiledScriptModSite {
    key: ScriptModKey,
    file_name: String,
    original_code: String,
    values: Vec<ScriptValue>,
}

impl CxLiveReloadState {
    pub fn queue_file_change(&mut self, file_name: String, content: String) {
        self.pending_files.push(PendingLiveChange { file_name, content });
    }
}

impl Cx {
    pub fn handle_live_edit(&mut self) -> bool {
        handle_cx_live_edit(self)
    }
}

fn handle_cx_live_edit(cx: &mut Cx) -> bool {
    let pending = std::mem::take(&mut cx.script_data.live_reload.pending_files);
    if pending.is_empty() {
        return false;
    }

    let mut latest_by_file = BTreeMap::<String, String>::new();
    for change in pending {
        latest_by_file.insert(normalize_path_string(Path::new(&change.file_name)), change.content);
    }

    let Some(script_vm) = cx.script_vm.as_mut() else {
        crate::error!("live_reload no script VM available");
        return false;
    };

    let current_overrides = cx
        .script_data
        .live_reload
        .script_mod_overrides
        .borrow()
        .clone();
    let mut next_overrides = current_overrides.clone();

    for (file_name, content) in latest_by_file {
        let compiled_sites = collect_compiled_sites_for_file(script_vm, &file_name);
        if compiled_sites.is_empty() {
            continue;
        }

        let extracted = match extract_script_mods_from_rust_file(&file_name, &content) {
            Ok(extracted) => extracted,
            Err(err) => {
                log_live_reload_file_error(&file_name, err);
                return false;
            }
        };

        if extracted.len() != compiled_sites.len() {
            log_live_reload_file_error(
                &file_name,
                format!(
                    "hot reload could not match script_mod! blocks for {}: runtime has {}, file has {}",
                    file_name,
                    compiled_sites.len(),
                    extracted.len()
                ),
            );
            return false;
        }

        for (site, extracted) in compiled_sites.iter().zip(extracted.iter()) {
            if extracted.rust_value_count != site.values.len() {
                log_live_reload_file_error(
                    &file_name,
                    format!(
                        "hot reload placeholder mismatch in {}: expected {} #(…) values, found {}",
                        file_name,
                        site.values.len(),
                        extracted.rust_value_count
                    ),
                );
                return false;
            }

            let current_effective = current_overrides
                .get(&site.key)
                .map(String::as_str)
                .unwrap_or(site.original_code.as_str());

            if extracted.code == current_effective {
                continue;
            }

            if extracted.code == site.original_code {
                continue;
            }

            if !validate_extracted_script_mod(script_vm, site, extracted) {
                crate::error!(
                    "live_reload validation failed for {}",
                    format_script_mod_site(site)
                );
                return false;
            }
        }

        for (site, extracted) in compiled_sites.into_iter().zip(extracted.into_iter()) {
            if extracted.code == site.original_code {
                next_overrides.remove(&site.key);
            } else {
                next_overrides.insert(site.key, extracted.code);
            }
        }
    }

    if next_overrides == current_overrides {
        return false;
    }

    *cx.script_data
        .live_reload
        .script_mod_overrides
        .borrow_mut() = next_overrides;
    true
}

fn collect_compiled_sites_for_file(
    script_vm: &crate::makepad_script::ScriptVmBase,
    file_name: &str,
) -> Vec<CompiledScriptModSite> {
    let bodies = script_vm.code.bodies.borrow();
    let mut sites = Vec::new();

    for body in bodies.iter() {
        let ScriptSource::Mod(script_mod) = &body.source else {
            continue;
        };
        let Some(compiled_file_name) = resolve_matching_script_mod_file(script_mod, file_name) else {
            continue;
        };
        sites.push(CompiledScriptModSite {
            key: ScriptModKey::from_script_mod(script_mod),
            file_name: compiled_file_name,
            original_code: script_mod.code.clone(),
            values: script_mod.values.clone(),
        });
    }

    sites.sort_by_key(|site| (site.key.line, site.key.column));
    sites
}

fn validate_extracted_script_mod(
    script_vm: &mut crate::makepad_script::ScriptVmBase,
    site: &CompiledScriptModSite,
    extracted: &ExtractedScriptMod,
) -> bool {
    let mut tokenizer = ScriptTokenizer::default();
    let mut parser = ScriptParser::default();
    tokenizer.tokenize(&extracted.code, &mut script_vm.heap);
    parser.parse(
        &tokenizer,
        &site.file_name,
        (extracted.first_token_line, extracted.first_token_column),
        &site.values,
    );
    !parser.had_error
}

fn format_script_mod_site(site: &CompiledScriptModSite) -> String {
    format!(
        "{}:{}:{}",
        site.file_name, site.key.line, site.key.column
    )
}

fn log_live_reload_file_error(file_name: &str, message: String) {
    crate::log::log_with_level(
        file_name,
        0,
        0,
        0,
        0,
        message,
        crate::log::LogLevel::Error,
    );
}

fn resolve_matching_script_mod_file(script_mod: &ScriptMod, changed_file_name: &str) -> Option<String> {
    let changed_file_name = normalize_path_string(Path::new(changed_file_name));
    if script_mod.file.is_empty() {
        return None;
    }

    let raw_file = normalize_relative_path_string(Path::new(&script_mod.file));
    if raw_file == changed_file_name {
        return Some(changed_file_name);
    }

    if resolve_script_mod_file_candidates(script_mod)
        .into_iter()
        .any(|candidate| candidate == changed_file_name)
    {
        return Some(changed_file_name);
    }

    // `file!()` can be workspace-relative under cargo builds, so allow the
    // absolute Studio path to match a sufficiently-specific path suffix.
    if path_has_component_suffix(
        Path::new(&changed_file_name),
        Path::new(&raw_file),
        3,
    ) {
        return Some(changed_file_name);
    }

    // For crate-relative paths like `src/main.rs`, anchor the suffix with the
    // crate directory name so we do not match every `src/main.rs` in the repo.
    if !script_mod.cargo_manifest_path.is_empty() {
        let manifest_path = Path::new(&script_mod.cargo_manifest_path);
        if let Some(crate_dir) = manifest_path.file_name() {
            let anchored = PathBuf::from(crate_dir).join(Path::new(&raw_file));
            if path_has_component_suffix(Path::new(&changed_file_name), &anchored, 3) {
                return Some(changed_file_name);
            }
        }
    }

    None
}

fn resolve_script_mod_file_candidates(script_mod: &ScriptMod) -> Vec<String> {
    if script_mod.file.is_empty() {
        return Vec::new();
    }
    let file_path = Path::new(&script_mod.file);
    let mut candidates = Vec::new();

    if file_path.is_absolute() {
        push_unique_candidate(&mut candidates, file_path.to_path_buf());
        return candidates;
    }

    if let Ok(cwd) = std::env::current_dir() {
        push_unique_candidate(&mut candidates, cwd.join(file_path));
    }

    if !script_mod.cargo_manifest_path.is_empty() {
        let manifest_path = Path::new(&script_mod.cargo_manifest_path);
        for ancestor in manifest_path.ancestors() {
            push_unique_candidate(&mut candidates, ancestor.join(file_path));
        }
    } else {
        push_unique_candidate(&mut candidates, file_path.to_path_buf());
    }

    candidates
}

fn push_unique_candidate(candidates: &mut Vec<String>, path: PathBuf) {
    let normalized = normalize_path_string(&path);
    if !candidates.iter().any(|candidate| candidate == &normalized) {
        candidates.push(normalized);
    }
}

fn normalize_relative_path_string(path: &Path) -> String {
    normalize_path(path).to_string_lossy().replace('\\', "/")
}

fn normalize_path_string(path: &Path) -> String {
    let path = if path.exists() {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    normalize_path(&path).to_string_lossy().replace('\\', "/")
}

fn path_has_component_suffix(path: &Path, suffix: &Path, min_components: usize) -> bool {
    let path_components = normalized_path_components(path);
    let suffix_components = normalized_path_components(suffix);
    if suffix_components.len() < min_components || suffix_components.len() > path_components.len() {
        return false;
    }
    path_components[path_components.len() - suffix_components.len()..] == suffix_components
}

fn normalized_path_components(path: &Path) -> Vec<String> {
    normalize_path(path)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Prefix(prefix) => {
                Some(prefix.as_os_str().to_string_lossy().to_string())
            }
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            std::path::Component::ParentDir => Some("..".to_string()),
            std::path::Component::RootDir | std::path::Component::CurDir => None,
        })
        .collect()
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            std::path::Component::RootDir => out.push(comp.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            std::path::Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn extract_script_mods_from_rust_file(
    file_name: &str,
    source: &str,
) -> Result<Vec<ExtractedScriptMod>, String> {
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut extracted = Vec::new();

    while i < bytes.len() {
        if let Some(end) = skip_non_code_segment(bytes, i)? {
            i = end;
            continue;
        }

        if is_ident_start(bytes[i]) {
            let ident_start = i;
            i += 1;
            while i < bytes.len() && is_ident_continue(bytes[i]) {
                i += 1;
            }

            if &source[ident_start..i] == "script_mod" {
                let mut j = skip_ws_and_comments(bytes, i)?;
                if bytes.get(j) == Some(&b'!') {
                    j += 1;
                    j = skip_ws_and_comments(bytes, j)?;
                    if bytes.get(j) == Some(&b'{') {
                        let end = find_matching_delim(bytes, j, b'{', b'}')?;
                        let body_start = j + 1;
                        let body = &source[body_start..end];
                        let body_pos = position_after_index(source, j);
                        extracted.push(normalize_script_mod_body(file_name, body, body_pos)?);
                        i = end + 1;
                        continue;
                    }
                }
            }
            continue;
        }

        i += utf8_char_len(bytes[i]);
    }

    Ok(extracted)
}

fn normalize_script_mod_body(
    file_name: &str,
    body: &str,
    start_pos: FilePos,
) -> Result<ExtractedScriptMod, String> {
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut pos = start_pos;
    let mut out = String::with_capacity(body.len() + 1);
    let mut rust_value_count = 0;
    let mut first_token = None;

    while i < bytes.len() {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            let end = skip_line_comment(bytes, i);
            push_comment_whitespace(&mut out, &bytes[i..end]);
            bump_pos_bytes(&mut pos, &bytes[i..end]);
            i = end;
            continue;
        }

        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let end = skip_block_comment(bytes, i)?;
            push_comment_whitespace(&mut out, &bytes[i..end]);
            bump_pos_bytes(&mut pos, &bytes[i..end]);
            i = end;
            continue;
        }

        if let Some((prefix_len, hashes)) = raw_string_prefix(bytes, i) {
            let segment_end = skip_raw_string(bytes, i, prefix_len, hashes)?;
            if first_token.is_none() {
                first_token = Some(pos);
            }
            out.push_str(&body[i..segment_end]);
            bump_pos_bytes(&mut pos, &bytes[i..segment_end]);
            i = segment_end;
            continue;
        }

        if bytes[i] == b'b' && bytes.get(i + 1) == Some(&b'"') {
            let segment_end = skip_quoted(bytes, i, 1, b'"')?;
            if first_token.is_none() {
                first_token = Some(pos);
            }
            out.push_str(&body[i..segment_end]);
            bump_pos_bytes(&mut pos, &bytes[i..segment_end]);
            i = segment_end;
            continue;
        }

        if bytes[i] == b'"' {
            let segment_end = skip_quoted(bytes, i, 0, b'"')?;
            if first_token.is_none() {
                first_token = Some(pos);
            }
            out.push_str(&body[i..segment_end]);
            bump_pos_bytes(&mut pos, &bytes[i..segment_end]);
            i = segment_end;
            continue;
        }

        if bytes[i] == b'b' && bytes.get(i + 1) == Some(&b'\'') {
            if let Some(segment_end) = char_literal_end(bytes, i, 1) {
                if first_token.is_none() {
                    first_token = Some(pos);
                }
                out.push_str(&body[i..segment_end]);
                bump_pos_bytes(&mut pos, &bytes[i..segment_end]);
                i = segment_end;
                continue;
            }
        }

        if let Some(segment_end) = char_literal_end(bytes, i, 0) {
            if first_token.is_none() {
                first_token = Some(pos);
            }
            out.push_str(&body[i..segment_end]);
            bump_pos_bytes(&mut pos, &bytes[i..segment_end]);
            i = segment_end;
            continue;
        }

        if bytes[i] == b'#' {
            if let Some(open_paren) = placeholder_open_paren(bytes, i)? {
                let segment_end = find_matching_delim(bytes, open_paren, b'(', b')')? + 1;
                if first_token.is_none() {
                    first_token = Some(pos);
                }
                out.push_str(&format!("#({rust_value_count})"));
                rust_value_count += 1;
                bump_pos_bytes(&mut pos, &bytes[i..segment_end]);
                i = segment_end;
                continue;
            }
        }

        let ch = body[i..]
            .chars()
            .next()
            .ok_or_else(|| format!("hot reload could not decode utf-8 in {}", file_name))?;
        if first_token.is_none() && !ch.is_whitespace() {
            first_token = Some(pos);
        }
        out.push(ch);
        let next = i + ch.len_utf8();
        bump_pos_bytes(&mut pos, &bytes[i..next]);
        i = next;
    }

    out.push(';');
    let first_token = first_token.unwrap_or(start_pos);

    Ok(ExtractedScriptMod {
        code: out,
        rust_value_count,
        first_token_line: first_token.line,
        first_token_column: first_token.column,
    })
}

fn position_after_index(source: &str, index: usize) -> FilePos {
    let mut pos = FilePos { line: 1, column: 1 };
    if index < source.len() {
        bump_pos_bytes(&mut pos, &source.as_bytes()[..=index]);
    }
    pos
}

fn bump_pos_bytes(pos: &mut FilePos, bytes: &[u8]) {
    for &byte in bytes {
        if byte == b'\n' {
            pos.line += 1;
            pos.column = 1;
        } else {
            pos.column += 1;
        }
    }
}

fn push_comment_whitespace(out: &mut String, bytes: &[u8]) {
    for &byte in bytes {
        if byte == b'\n' {
            out.push('\n');
        } else {
            out.push(' ');
        }
    }
}

fn placeholder_open_paren(bytes: &[u8], index: usize) -> Result<Option<usize>, String> {
    let mut i = index + 1;
    loop {
        i = skip_ascii_whitespace(bytes, i);
        if i >= bytes.len() {
            return Ok(None);
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            i = skip_line_comment(bytes, i);
            continue;
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i = skip_block_comment(bytes, i)?;
            continue;
        }
        return Ok((bytes[i] == b'(').then_some(i));
    }
}

fn skip_ws_and_comments(bytes: &[u8], mut i: usize) -> Result<usize, String> {
    loop {
        i = skip_ascii_whitespace(bytes, i);
        if bytes.get(i) == Some(&b'/') && bytes.get(i + 1) == Some(&b'/') {
            i = skip_line_comment(bytes, i);
            continue;
        }
        if bytes.get(i) == Some(&b'/') && bytes.get(i + 1) == Some(&b'*') {
            i = skip_block_comment(bytes, i)?;
            continue;
        }
        return Ok(i);
    }
}

fn skip_ascii_whitespace(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn find_matching_delim(bytes: &[u8], mut i: usize, open: u8, close: u8) -> Result<usize, String> {
    let mut depth = 0usize;
    while i < bytes.len() {
        if let Some(end) = skip_non_code_segment(bytes, i)? {
            i = end;
            continue;
        }
        if bytes[i] == open {
            depth += 1;
            i += 1;
            continue;
        }
        if bytes[i] == close {
            depth -= 1;
            if depth == 0 {
                return Ok(i);
            }
            i += 1;
            continue;
        }
        i += utf8_char_len(bytes[i]);
    }
    Err("hot reload hit an unclosed delimiter while scanning Rust source".to_string())
}

fn skip_non_code_segment(bytes: &[u8], i: usize) -> Result<Option<usize>, String> {
    if i >= bytes.len() {
        return Ok(None);
    }
    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
        return Ok(Some(skip_line_comment(bytes, i)));
    }
    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
        return Ok(Some(skip_block_comment(bytes, i)?));
    }
    if let Some((prefix_len, hashes)) = raw_string_prefix(bytes, i) {
        return Ok(Some(skip_raw_string(bytes, i, prefix_len, hashes)?));
    }
    if bytes[i] == b'b' && bytes.get(i + 1) == Some(&b'"') {
        return Ok(Some(skip_quoted(bytes, i, 1, b'"')?));
    }
    if bytes[i] == b'"' {
        return Ok(Some(skip_quoted(bytes, i, 0, b'"')?));
    }
    if bytes[i] == b'b' && bytes.get(i + 1) == Some(&b'\'') {
        if let Some(end) = char_literal_end(bytes, i, 1) {
            return Ok(Some(end));
        }
    }
    if let Some(end) = char_literal_end(bytes, i, 0) {
        return Ok(Some(end));
    }
    Ok(None)
}

fn raw_string_prefix(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    if i >= bytes.len() {
        return None;
    }

    let (mut j, prefix_len) = if bytes[i] == b'r' && bytes.get(i + 1) == Some(&b'b') {
        (i + 2, 2)
    } else if bytes[i] == b'b' && bytes.get(i + 1) == Some(&b'r') {
        (i + 2, 2)
    } else if bytes[i] == b'r' {
        (i + 1, 1)
    } else {
        return None;
    };

    let mut hashes = 0usize;
    while bytes.get(j) == Some(&b'#') {
        hashes += 1;
        j += 1;
    }
    if bytes.get(j) != Some(&b'"') {
        return None;
    }
    Some((prefix_len + 1 + hashes + 1, hashes))
}

fn skip_raw_string(
    bytes: &[u8],
    i: usize,
    prefix_len: usize,
    hashes: usize,
) -> Result<usize, String> {
    let mut j = i + prefix_len;
    while j < bytes.len() {
        if bytes[j] == b'"'
            && j + hashes < bytes.len()
            && bytes[j + 1..j + 1 + hashes].iter().all(|byte| *byte == b'#')
        {
            return Ok(j + 1 + hashes);
        }
        j += 1;
    }
    Err("hot reload hit an unterminated raw string".to_string())
}

fn skip_quoted(bytes: &[u8], i: usize, prefix_len: usize, quote: u8) -> Result<usize, String> {
    let mut j = i + prefix_len + 1;
    while j < bytes.len() {
        if bytes[j] == b'\\' {
            j += 1;
            if j < bytes.len() {
                j += 1;
            }
            continue;
        }
        if bytes[j] == quote {
            return Ok(j + 1);
        }
        j += 1;
    }
    Err("hot reload hit an unterminated string literal".to_string())
}

fn char_literal_end(bytes: &[u8], i: usize, prefix_len: usize) -> Option<usize> {
    let quote_index = i + prefix_len;
    if quote_index >= bytes.len() || bytes[quote_index] != b'\'' {
        return None;
    }

    let mut j = quote_index + 1;
    if j >= bytes.len() {
        return None;
    }

    if bytes[j] == b'\\' {
        j += 1;
        if j >= bytes.len() {
            return None;
        }
        if bytes[j] == b'u' && bytes.get(j + 1) == Some(&b'{') {
            j += 2;
            while j < bytes.len() && bytes[j] != b'}' && bytes[j] != b'\n' {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] != b'}' {
                return None;
            }
            j += 1;
        } else {
            j += 1;
        }
    } else {
        if bytes[j] == b'\'' || bytes[j] == b'\n' {
            return None;
        }
        j += utf8_char_len(bytes[j]);
    }

    (bytes.get(j) == Some(&b'\'')).then_some(j + 1)
}

fn skip_line_comment(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i] != b'\n' {
        i += 1;
    }
    i
}

fn skip_block_comment(bytes: &[u8], mut i: usize) -> Result<usize, String> {
    let mut depth = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            depth += 1;
            i += 2;
            continue;
        }
        if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return Ok(i);
            }
            continue;
        }
        i += 1;
    }
    Err("hot reload hit an unterminated block comment".to_string())
}

fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_ident_continue(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn utf8_char_len(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte & 0b1110_0000 == 0b1100_0000 {
        2
    } else if byte & 0b1111_0000 == 0b1110_0000 {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_multiple_script_mods() {
        let source = r#"
        script_mod! {
            use mod.widgets.*
        }

        fn helper() {}

        script_mod!{
            mod.widgets.Button = Button{}
        }
        "#;

        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        assert_eq!(extracted.len(), 2);
        assert!(extracted[0].code.contains("use mod.widgets.*"));
        assert!(extracted[1].code.contains("mod.widgets.Button = Button{}"));
    }

    #[test]
    fn rewrites_rust_values_but_keeps_colors() {
        let source = r#"
        script_mod! {
            value: #(foo(bar))
            color: #x2ecc71
            color2: #fff
            other: # (baz)
        }
        "#;

        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        let code = &extracted[0].code;
        assert!(code.contains("value: #(0)"));
        assert!(code.contains("color: #x2ecc71"));
        assert!(code.contains("color2: #fff"));
        assert!(code.contains("other: #(1)"));
        assert_eq!(extracted[0].rust_value_count, 2);
    }

    #[test]
    fn ignores_comments_and_strings_when_finding_macros() {
        let source = r#"
        // script_mod! { ignored }
        let _ = "script_mod! { also_ignored }";
        script_mod! {
            text: "/* not a comment */"
            /* comment with script_mod! { ignored } */
            value: #(foo)
        }
        "#;

        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        assert_eq!(extracted.len(), 1);
        let code = &extracted[0].code;
        assert!(code.contains("text: \"/* not a comment */\""));
        assert!(code.contains("value: #(0)"));
        assert!(!code.contains("ignored"));
    }

    #[test]
    fn matches_workspace_relative_runtime_file_against_absolute_change_path() {
        let script_mod = ScriptMod {
            cargo_manifest_path: "/Users/admin/makepad/makepad/examples/shader".to_string(),
            file: "examples/shader/src/main.rs".to_string(),
            ..Default::default()
        };

        let matched = resolve_matching_script_mod_file(
            &script_mod,
            "/Users/admin/makepad/makepad/examples/shader/src/main.rs",
        );
        assert_eq!(
            matched.as_deref(),
            Some("/Users/admin/makepad/makepad/examples/shader/src/main.rs")
        );
    }

    #[test]
    fn matches_crate_relative_runtime_file_against_absolute_change_path() {
        let script_mod = ScriptMod {
            cargo_manifest_path: "/Users/admin/makepad/makepad/examples/shader".to_string(),
            file: "src/main.rs".to_string(),
            ..Default::default()
        };

        let matched = resolve_matching_script_mod_file(
            &script_mod,
            "/Users/admin/makepad/makepad/examples/shader/src/main.rs",
        );
        assert_eq!(
            matched.as_deref(),
            Some("/Users/admin/makepad/makepad/examples/shader/src/main.rs")
        );
    }

    #[test]
    fn does_not_match_short_unanchored_suffixes() {
        let script_mod = ScriptMod {
            file: "main.rs".to_string(),
            ..Default::default()
        };

        assert_eq!(
            resolve_matching_script_mod_file(
                &script_mod,
                "/Users/admin/makepad/makepad/examples/shader/src/main.rs",
            ),
            None
        );
    }
}
