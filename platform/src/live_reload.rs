use crate::cx::Cx;
use crate::makepad_script::{
    parser::ScriptParser, tokenizer::ScriptTokenizer, ScriptMod, ScriptModKey, ScriptSource,
    ScriptValue,
};
use makepad_live_reload_core::{
    normalize_path, normalize_path_string, normalize_relative_path_string,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use {
    crate::thread::SignalToUI,
    makepad_live_reload_core::{
        start_live_reload_watcher, LiveReloadFileChange, LiveReloadLogger, LiveReloadWatchPlan,
        LiveReloadWatcherHandle, WatchRoot,
    },
    makepad_studio_protocol::StudioToApp,
    std::sync::mpsc::channel,
};

#[derive(Clone, Debug)]
pub(crate) struct PendingLiveChange {
    file_name: String,
    content: String,
}

pub struct CxLiveReloadState {
    pub(crate) pending_files: Vec<PendingLiveChange>,
    pub script_mod_overrides: Rc<RefCell<HashMap<ScriptModKey, String>>>,
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    pub(crate) file_observer: Option<DesktopHotReloadWatcher>,
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
    /// Position of the `script_mod` identifier itself. Used to match compiled
    /// sites against file extractions — compiled `ScriptMod.line/column` come
    /// from `line!()`/`column!()` at the macro invocation site, which equal
    /// the position of `script_mod`. Other macros (`script_eval!`, `script!`)
    /// also produce `ScriptMod` runtime entries but at different positions,
    /// so this lets us ignore them.
    macro_line: usize,
    macro_column: usize,
    /// Recursive chunk tree, populated only when the body contains any
    /// `#[cfg(...)]` attribute (at any depth). Empty otherwise — callers fall
    /// back to the legacy verbatim `code` comparison path.
    chunks: Vec<ExtractedChunk>,
}

/// Mirror of the proc-macro's `Chunk` type on the extractor side.
/// `Conditional` chunks can be nested — they're discovered at any depth in
/// the source body, and their pre-order count must match the compiled-in
/// `cfg_fragments` length.
#[derive(Clone, Debug)]
enum ExtractedChunk {
    Text(String),
    Placeholder,
    Conditional(Vec<ExtractedChunk>),
}

#[derive(Clone, Debug)]
struct CompiledScriptModSite {
    key: ScriptModKey,
    file_name: String,
    original_code: String,
    values: Vec<ScriptValue>,
    /// Copy of `ScriptMod.cfg_fragments` from the compiled site. Empty when the
    /// site has no top-level cfg attributes.
    cfg_fragments: Vec<bool>,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) struct DesktopHotReloadWatcher {
    _watcher: LiveReloadWatcherHandle,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
type HotReloadWatchPlan = LiveReloadWatchPlan;

impl Default for CxLiveReloadState {
    fn default() -> Self {
        Self {
            pending_files: Vec::new(),
            script_mod_overrides: Default::default(),
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            file_observer: None,
        }
    }
}

impl CxLiveReloadState {
    pub fn queue_file_change(&mut self, file_name: String, content: String) {
        self.pending_files
            .push(PendingLiveChange { file_name, content });
    }
}

/// Result of [`Cx::handle_live_edit`], discriminating *why* a LiveEdit
/// was triggered. Callers use this to decide whether to do the heavyweight
/// follow-up work (shader cache reset, immediate Event::ScriptReapply
/// pass) that's only justified when the DSL itself just changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveEditTrigger {
    /// Nothing pending — handler should not fire `Event::LiveEdit`.
    None,
    /// A `script_mod!` block was hot-reloaded from a file change (file
    /// watcher on desktop, or `StudioToApp::LiveChange` over the studio
    /// websocket). The DSL itself changed; widget trees need a full
    /// re-walk and shader caches may be stale.
    FileChange,
    /// A platform caller invoked `cx.request_live_edit()` (e.g. iOS
    /// rotation re-baking `mod.widgets.SAFE_INSET_PAD_*`). The DSL did
    /// NOT change; we just need `script_mod` to re-evaluate so source
    /// expressions referencing changed primitives pick up new values.
    /// Shader code is unchanged, and any follow-up `ScriptReapply`
    /// triggered by the LiveEdit handler can be deferred to the next
    /// event-loop tick to keep each tick light during the animation.
    Manual,
}

impl Cx {
    pub fn start_hot_reload_file_observer_if_requested(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        self.start_desktop_hot_reload_file_observer_if_requested();
    }

    pub(crate) fn handle_live_edit(&mut self) -> LiveEditTrigger {
        handle_cx_live_edit(self)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl Cx {
    fn start_desktop_hot_reload_file_observer_if_requested(&mut self) {
        if !hot_reload_requested_from_args() {
            return;
        }
        if self.script_data.live_reload.file_observer.is_some() {
            return;
        }

        let Some(script_vm) = self.script_vm.as_ref() else {
            return;
        };
        let Some(plan) = collect_hot_reload_watch_plan(script_vm) else {
            return;
        };

        let (tx, rx) = channel::<StudioToApp>();
        let logger = LiveReloadLogger::new(
            |message| crate::log!("{}", message),
            |message| crate::error!("{}", message),
        );
        let watcher = start_live_reload_watcher(
            plan,
            move |change: LiveReloadFileChange| {
                tx.send(StudioToApp::LiveChange {
                    file_name: change.file_name,
                    content: change.content,
                })
                .map_err(|_| "channel closed".to_string())?;
                SignalToUI::set_ui_signal();
                Ok(())
            },
            logger,
        );

        match watcher {
            Ok(watcher) => {
                Cx::set_control_channel(rx);
                self.script_data.live_reload.file_observer =
                    Some(DesktopHotReloadWatcher { _watcher: watcher });
            }
            Err(err) => {
                crate::error!("hot reload watcher unavailable: {}", err);
            }
        }
    }
}

fn handle_cx_live_edit(cx: &mut Cx) -> LiveEditTrigger {
    // Process file changes first so a real DSL hot-reload always wins
    // over a (cheaper) manual request — the file path also updates the
    // `script_mod_overrides` map that subsequent `script_mod` re-runs
    // consult, and we want that done before the LiveEdit fires.
    let file_changed = handle_cx_live_edit_files(cx);
    let manual = std::mem::take(&mut cx.pending_live_edit_request);

    if file_changed {
        LiveEditTrigger::FileChange
    } else if manual {
        LiveEditTrigger::Manual
    } else {
        LiveEditTrigger::None
    }
}

fn handle_cx_live_edit_files(cx: &mut Cx) -> bool {
    let pending = std::mem::take(&mut cx.script_data.live_reload.pending_files);
    if pending.is_empty() {
        return false;
    }

    let mut latest_by_file = BTreeMap::<String, String>::new();
    for change in pending {
        latest_by_file.insert(
            normalize_path_string(Path::new(&change.file_name)),
            change.content,
        );
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

    let mut processed_files = 0usize;
    for (file_name, content) in latest_by_file {
        processed_files += 1;
        let all_compiled_sites = collect_compiled_sites_for_file(script_vm, &file_name);
        if all_compiled_sites.is_empty() {
            continue;
        }

        let extracted = match extract_script_mods_from_rust_file(&file_name, &content) {
            Ok(extracted) => extracted,
            Err(err) => {
                log_live_reload_file_error(&file_name, err);
                return false;
            }
        };

        // Filter the runtime sites to only those whose `(line, column)` came
        // from a `script_mod!` invocation in this file. Other macros that
        // produce `ScriptMod` runtime entries — `script_eval!`, `script!` —
        // sit at different positions, so we drop them; they're not
        // hot-reload targets (the file extractor only finds `script_mod!`).
        let macro_positions: HashSet<(usize, usize)> = extracted
            .iter()
            .map(|e| (e.macro_line, e.macro_column))
            .collect();
        let compiled_sites: Vec<CompiledScriptModSite> = all_compiled_sites
            .into_iter()
            .filter(|site| macro_positions.contains(&(site.key.line, site.key.column)))
            .collect();

        if compiled_sites.is_empty() {
            continue;
        }

        if extracted.len() != compiled_sites.len() {
            log_live_reload_file_error(
                &file_name,
                format!(
                    "hot reload could not match script_mod! blocks for {}: runtime has {} after filtering, file has {}",
                    file_name,
                    compiled_sites.len(),
                    extracted.len()
                ),
            );
            return false;
        }

        // Compute per-site the *effective* extracted (code, rust_value_count).
        // For sites with no top-level cfg fragments this is just the legacy
        // (`extracted.code`, `extracted.rust_value_count`). For cfg-aware sites
        // we filter the structured fragments by `site.cfg_fragments` and
        // renumber `#(N)` placeholders globally — see `compute_filtered_code`.
        let mut effective: Vec<(String, usize)> = Vec::with_capacity(compiled_sites.len());
        let mut cfg_mismatch = false;
        for (site, extracted) in compiled_sites.iter().zip(extracted.iter()) {
            if site.cfg_fragments.is_empty() {
                effective.push((extracted.code.clone(), extracted.rust_value_count));
            } else {
                match compute_filtered_code(extracted, &site.cfg_fragments) {
                    Ok(pair) => effective.push(pair),
                    Err(detail) => {
                        log_live_reload_file_error(
                            &file_name,
                            format!(
                                "hot reload could not match cfg fragments for {}: {} — rebuild required",
                                file_name, detail
                            ),
                        );
                        cfg_mismatch = true;
                        break;
                    }
                }
            }
        }
        if cfg_mismatch {
            return false;
        }

        for ((site, extracted), (effective_code, effective_value_count)) in compiled_sites
            .iter()
            .zip(extracted.iter())
            .zip(effective.iter())
        {
            if *effective_value_count != site.values.len() {
                log_live_reload_file_error(
                    &file_name,
                    format!(
                        "hot reload placeholder mismatch in {}: expected {} #(…) values, found {}",
                        file_name,
                        site.values.len(),
                        effective_value_count
                    ),
                );
                return false;
            }

            let current_effective = current_overrides
                .get(&site.key)
                .map(String::as_str)
                .unwrap_or(site.original_code.as_str());

            if effective_code == current_effective {
                continue;
            }

            if effective_code == &site.original_code {
                continue;
            }

            if !validate_extracted_script_mod_with(
                script_vm,
                site,
                extracted,
                effective_code,
                *effective_value_count,
            ) {
                crate::error!(
                    "live_reload validation failed for {}",
                    format_script_mod_site(site)
                );
                return false;
            }
        }

        for ((site, _extracted), (effective_code, _effective_value_count)) in compiled_sites
            .into_iter()
            .zip(extracted.into_iter())
            .zip(effective.into_iter())
        {
            if effective_code == site.original_code {
                next_overrides.remove(&site.key);
            } else {
                next_overrides.insert(site.key, effective_code);
            }
        }
    }

    if next_overrides == current_overrides {
        return false;
    }

    *cx.script_data.live_reload.script_mod_overrides.borrow_mut() = next_overrides;
    crate::log!(
        "hot reload applied {} override(s) from {} file change(s)",
        cx.script_data
            .live_reload
            .script_mod_overrides
            .borrow()
            .len(),
        processed_files
    );
    true
}

fn collect_compiled_sites_for_file(
    script_vm: &crate::makepad_script::ScriptVmBase,
    file_name: &str,
) -> Vec<CompiledScriptModSite> {
    let bodies = script_vm.code.bodies.borrow();
    let mut sites = Vec::new();
    let mut seen = HashSet::<ScriptModKey>::new();

    for body in bodies.iter() {
        let ScriptSource::Mod(script_mod) = &body.source else {
            continue;
        };
        let Some(compiled_file_name) = resolve_matching_script_mod_file(script_mod, file_name)
        else {
            continue;
        };
        let key = ScriptModKey::from_script_mod(script_mod);
        if !seen.insert(key.clone()) {
            continue;
        }
        sites.push(CompiledScriptModSite {
            key,
            file_name: compiled_file_name,
            original_code: script_mod.code.clone(),
            values: script_mod.values.clone(),
            cfg_fragments: script_mod.cfg_fragments.clone(),
        });
    }

    sites.sort_by_key(|site| (site.key.line, site.key.column));
    sites
}

fn validate_extracted_script_mod_with(
    script_vm: &mut crate::makepad_script::ScriptVmBase,
    site: &CompiledScriptModSite,
    extracted: &ExtractedScriptMod,
    effective_code: &str,
    _effective_value_count: usize,
) -> bool {
    let mut tokenizer = ScriptTokenizer::default();
    let mut parser = ScriptParser::default();
    tokenizer.tokenize(effective_code, &mut script_vm.heap);
    parser.parse(
        &tokenizer,
        &site.file_name,
        (extracted.first_token_line, extracted.first_token_column),
        &site.values,
    );
    !parser.had_error
}

fn format_script_mod_site(site: &CompiledScriptModSite) -> String {
    format!("{}:{}:{}", site.file_name, site.key.line, site.key.column)
}

fn log_live_reload_file_error(file_name: &str, message: String) {
    crate::log::log_with_level(file_name, 0, 0, 0, 0, message, crate::log::LogLevel::Error);
}

fn resolve_matching_script_mod_file(
    script_mod: &ScriptMod,
    changed_file_name: &str,
) -> Option<String> {
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
    if path_has_component_suffix(Path::new(&changed_file_name), Path::new(&raw_file), 3) {
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

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn hot_reload_requested_from_args() -> bool {
    std::env::args().any(|arg| arg == "--hot")
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn collect_hot_reload_watch_plan(
    script_vm: &crate::makepad_script::ScriptVmBase,
) -> Option<HotReloadWatchPlan> {
    let excluded_manifest_paths = excluded_hot_reload_manifest_paths();
    let bodies = script_vm.code.bodies.borrow();
    let mut roots = BTreeMap::<String, WatchRoot>::new();
    let mut files_by_root = HashMap::<String, Vec<String>>::new();
    let mut initial_contents = HashMap::<String, String>::new();

    for body in bodies.iter() {
        let ScriptSource::Mod(script_mod) = &body.source else {
            continue;
        };
        let Some(root) = hot_reload_root_for_script_mod(script_mod, &excluded_manifest_paths)
        else {
            continue;
        };
        let Some(file_name) = resolve_script_mod_file_for_watch(script_mod) else {
            continue;
        };

        let path = PathBuf::from(&file_name);
        if !path.is_file() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };

        roots.entry(root.clone()).or_insert_with(|| WatchRoot {
            mount: root.clone(),
            path: PathBuf::from(&root),
        });

        initial_contents.entry(file_name.clone()).or_insert(content);
        files_by_root.entry(root).or_default().push(file_name);
    }

    if initial_contents.is_empty() {
        return None;
    }

    for files in files_by_root.values_mut() {
        files.sort();
        files.dedup();
    }

    Some(HotReloadWatchPlan {
        roots: roots.into_values().collect(),
        files_by_root,
        initial_contents,
    })
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn excluded_hot_reload_manifest_paths() -> HashSet<String> {
    let platform_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    [
        platform_dir.to_path_buf(),
        platform_dir.join("script"),
        platform_dir.join("../draw"),
    ]
    .into_iter()
    .map(|path| normalize_path_string(&path))
    .collect()
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn hot_reload_root_for_script_mod(
    script_mod: &ScriptMod,
    excluded_manifest_paths: &HashSet<String>,
) -> Option<String> {
    let manifest_path = (!script_mod.cargo_manifest_path.is_empty())
        .then(|| normalize_path_string(Path::new(&script_mod.cargo_manifest_path)));
    match manifest_path {
        Some(path) if excluded_manifest_paths.contains(&path) => None,
        Some(path) => Some(path),
        None => resolve_script_mod_file_for_watch(script_mod)
            .and_then(|path| Path::new(&path).parent().map(normalize_path_string)),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn resolve_script_mod_file_for_watch(script_mod: &ScriptMod) -> Option<String> {
    let candidates = resolve_script_mod_file_candidates(script_mod);
    candidates
        .iter()
        .find(|candidate| Path::new(candidate.as_str()).is_file())
        .cloned()
        .or_else(|| candidates.into_iter().next())
}

fn push_unique_candidate(candidates: &mut Vec<String>, path: PathBuf) {
    let normalized = normalize_path_string(&path);
    if !candidates.iter().any(|candidate| candidate == &normalized) {
        candidates.push(normalized);
    }
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
                        let macro_pos = position_at_index(source, ident_start);
                        let mut block = normalize_script_mod_body(file_name, body, body_pos)?;
                        block.macro_line = macro_pos.line;
                        block.macro_column = macro_pos.column;
                        extracted.push(block);
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

    // Second pass: walk the body recursively and produce a chunk tree that
    // mirrors the proc-macro's view. `chunks` is empty when the body contains
    // no `#[cfg(...)]` attribute at any depth — consumers then use the
    // legacy verbatim `code` comparison.
    let chunks = extract_chunks(file_name, body)?;

    Ok(ExtractedScriptMod {
        code: out,
        rust_value_count,
        first_token_line: first_token.line,
        first_token_column: first_token.column,
        macro_line: 0,
        macro_column: 0,
        chunks,
    })
}

/// Second pass over the body: walk recursively and produce a chunk tree that
/// matches what the proc-macro builds (`Chunk::Text` / `Chunk::Placeholder` /
/// `Chunk::Conditional` with nested children).
///
/// When the body contains no `#[cfg(...)]` attribute at any depth, the
/// returned vec is empty — callers fall back to the legacy `code` comparison.
fn extract_chunks(file_name: &str, body: &str) -> Result<Vec<ExtractedChunk>, String> {
    let bytes = body.as_bytes();
    let mut walker = ChunkWalker {
        file_name,
        body,
        bytes,
        saw_cfg: false,
    };
    let mut i = 0usize;
    let chunks = walker.walk(&mut i, bytes.len())?;
    if !walker.saw_cfg {
        return Ok(Vec::new());
    }
    Ok(chunks)
}

struct ChunkWalker<'a> {
    file_name: &'a str,
    body: &'a str,
    bytes: &'a [u8],
    saw_cfg: bool,
}

impl<'a> ChunkWalker<'a> {
    /// Walk byte range `[i .. end)` producing a chunk list. Updates `*i` to
    /// point past the last consumed byte. Recurses into nested cfg bodies; the
    /// caller's enclosing structure (braces, parens) is preserved verbatim as
    /// Text.
    fn walk(&mut self, i: &mut usize, end: usize) -> Result<Vec<ExtractedChunk>, String> {
        let mut chunks: Vec<ExtractedChunk> = Vec::new();
        let mut buf = String::new();

        let flush = |buf: &mut String, chunks: &mut Vec<ExtractedChunk>| {
            if !buf.is_empty() {
                chunks.push(ExtractedChunk::Text(std::mem::take(buf)));
            }
        };

        while *i < end {
            if let Some(skip_end) = try_skip_passthrough(self.bytes, *i)? {
                buf.push_str(&self.body[*i..skip_end]);
                *i = skip_end;
                continue;
            }

            if self.bytes[*i] == b'#' {
                // `#[cfg(...)]` outer attribute at this depth.
                if let Some(cfg_end) = try_match_cfg_attr(self.bytes, *i)? {
                    self.saw_cfg = true;
                    // The gap whitespace from the previous accumulated token
                    // to the cfg attribute's `#` is already in `buf` (it was
                    // pushed verbatim during the byte-level walk). Flush
                    // without trimming — the gap is the DSL separator between
                    // pre-text and the conditional body.
                    flush(&mut buf, &mut chunks);

                    *i = cfg_end;
                    *i = skip_ascii_whitespace(self.bytes, *i);
                    if *i >= end {
                        return Err(format!(
                            "hot reload: #[cfg(...)] has no following item in {}",
                            self.file_name
                        ));
                    }

                    let body_chunks = if self.bytes[*i] == b'{' {
                        // Brace-grouped form: recurse into the inner range.
                        let close = find_matching_delim(self.bytes, *i, b'{', b'}')?;
                        let inner_start = *i + 1;
                        let mut inner_i = inner_start;
                        let inner_chunks = self.walk(&mut inner_i, close)?;
                        *i = close + 1;
                        inner_chunks
                    } else {
                        // Single-statement form: walk a bounded range.
                        let stmt_end = find_single_statement_end(self.bytes, *i, end)?;
                        let mut inner_i = *i;
                        let inner_chunks = self.walk(&mut inner_i, stmt_end)?;
                        *i = stmt_end;
                        inner_chunks
                    };

                    let trimmed = trim_chunks(body_chunks);
                    chunks.push(ExtractedChunk::Conditional(trimmed));
                    continue;
                }

                // `#( ... )` placeholder.
                if let Some(open_paren) = placeholder_open_paren(self.bytes, *i)? {
                    let segment_end = find_matching_delim(self.bytes, open_paren, b'(', b')')? + 1;
                    flush(&mut buf, &mut chunks);
                    chunks.push(ExtractedChunk::Placeholder);
                    *i = segment_end;
                    continue;
                }
            }

            let ch = self.body[*i..].chars().next().ok_or_else(|| {
                format!("hot reload could not decode utf-8 in {}", self.file_name)
            })?;
            let next = *i + ch.len_utf8();
            buf.push(ch);
            *i = next;
        }

        if !buf.is_empty() {
            chunks.push(ExtractedChunk::Text(buf));
        }
        Ok(chunks)
    }
}

/// Find the byte offset where a `#[cfg(...)]` single-statement scope ends:
/// either after the close of the first top-level brace group encountered, or
/// at the next depth-zero newline. Multi-line scopes without a brace group
/// are rejected up the call chain by the proc-macro (the extractor mirrors).
fn find_single_statement_end(bytes: &[u8], start: usize, max: usize) -> Result<usize, String> {
    let mut i = start;
    let mut depth: u32 = 0;
    while i < max {
        if let Some(skip_end) = try_skip_passthrough(bytes, i)? {
            i = skip_end;
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => {
                depth = depth.saturating_add(1);
                i += 1;
            }
            b')' | b']' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            b'\n' if depth == 0 => return Ok(i),
            _ => {
                let ch_len = utf8_char_len(bytes[i]);
                i += ch_len;
            }
        }
    }
    Ok(i)
}

/// Strip leading whitespace from the first leaf Text chunk and trailing
/// whitespace from the last leaf Text chunk. Matches the macro's behaviour of
/// not emitting whitespace before the first or after the last token of a
/// fragment body.
fn trim_chunks(mut chunks: Vec<ExtractedChunk>) -> Vec<ExtractedChunk> {
    if let Some(first) = chunks.first_mut() {
        trim_leading(first);
    }
    if let Some(last) = chunks.last_mut() {
        trim_trailing(last);
    }
    chunks
}

fn trim_leading(chunk: &mut ExtractedChunk) {
    match chunk {
        ExtractedChunk::Text(s) => {
            let trimmed = s.trim_start();
            if trimmed.len() != s.len() {
                *s = trimmed.to_string();
            }
        }
        ExtractedChunk::Placeholder => {}
        ExtractedChunk::Conditional(inner) => {
            if let Some(first) = inner.first_mut() {
                trim_leading(first);
            }
        }
    }
}

fn trim_trailing(chunk: &mut ExtractedChunk) {
    match chunk {
        ExtractedChunk::Text(s) => {
            let trimmed = s.trim_end();
            if trimmed.len() != s.len() {
                *s = trimmed.to_string();
            }
        }
        ExtractedChunk::Placeholder => {}
        ExtractedChunk::Conditional(inner) => {
            if let Some(last) = inner.last_mut() {
                trim_trailing(last);
            }
        }
    }
}

/// Compute the filtered DSL code for an extracted chunk tree using the site's
/// pre-order `cfg_fragments` boolean vec. Returns `(code, value_count)` so the
/// caller can also verify the placeholder count.
fn compute_filtered_code(
    extracted: &ExtractedScriptMod,
    site_cfg_fragments: &[bool],
) -> Result<(String, usize), String> {
    let total = count_conditionals(&extracted.chunks);
    if total != site_cfg_fragments.len() {
        return Err(format!(
            "file has {} cfg fragments, compiled binary has {}",
            total,
            site_cfg_fragments.len()
        ));
    }

    let mut code = String::new();
    let mut value_count: usize = 0;
    let mut cond_idx: usize = 0;
    walk_filter(
        &extracted.chunks,
        site_cfg_fragments,
        &mut cond_idx,
        true,
        &mut code,
        &mut value_count,
    );
    code.push(';');
    Ok((code, value_count))
}

fn count_conditionals(chunks: &[ExtractedChunk]) -> usize {
    let mut n = 0;
    for c in chunks {
        if let ExtractedChunk::Conditional(inner) = c {
            n += 1 + count_conditionals(inner);
        }
    }
    n
}

fn walk_filter(
    chunks: &[ExtractedChunk],
    cfg_fragments: &[bool],
    cond_idx: &mut usize,
    active: bool,
    out: &mut String,
    value_count: &mut usize,
) {
    use std::fmt::Write as _;
    for chunk in chunks {
        match chunk {
            ExtractedChunk::Text(s) => {
                if active {
                    out.push_str(s);
                }
            }
            ExtractedChunk::Placeholder => {
                if active {
                    let _ = write!(out, "#({})", *value_count);
                    *value_count += 1;
                }
            }
            ExtractedChunk::Conditional(inner) => {
                let this = cfg_fragments[*cond_idx];
                *cond_idx += 1;
                walk_filter(
                    inner,
                    cfg_fragments,
                    cond_idx,
                    active && this,
                    out,
                    value_count,
                );
            }
        }
    }
}


/// Returns the byte offset just past `#[cfg(...)]` if the bytes at `i` form an
/// outer `#[cfg(...)]` attribute. Recognised at any depth — nested cfg is
/// supported, mirroring the proc-macro. `#![...]`, `#[cfg_attr(...)]`,
/// `#[doc = ...]`, and other attribute shapes return `None`.
fn try_match_cfg_attr(bytes: &[u8], i: usize) -> Result<Option<usize>, String> {
    debug_assert_eq!(bytes[i], b'#');
    // Reject inner attribute `#![...]`.
    let mut j = i + 1;
    if bytes.get(j) == Some(&b'!') {
        return Ok(None);
    }
    j = skip_ws_and_comments(bytes, j)?;
    if bytes.get(j) != Some(&b'[') {
        return Ok(None);
    }
    let bracket_open = j;
    let bracket_close = find_matching_delim(bytes, bracket_open, b'[', b']')?;
    // Inside the brackets, the first non-ws/comment ident must be `cfg` (not
    // `cfg_attr`, not `doc`, not `derive`, etc.).
    let mut k = skip_ws_and_comments(bytes, bracket_open + 1)?;
    let ident_start = k;
    while k < bracket_close && (is_ident_continue(bytes[k]) || (k == ident_start && is_ident_start(bytes[k]))) {
        k += 1;
    }
    if k == ident_start {
        return Ok(None);
    }
    let ident = std::str::from_utf8(&bytes[ident_start..k]).map_err(|_| {
        "hot reload: invalid UTF-8 in #[...] attribute name".to_string()
    })?;
    if ident != "cfg" {
        return Ok(None);
    }
    // Expect `(` for the cfg predicate.
    k = skip_ws_and_comments(bytes, k)?;
    if bytes.get(k) != Some(&b'(') {
        return Ok(None);
    }
    let paren_close = find_matching_delim(bytes, k, b'(', b')')?;
    // After the cfg paren, only whitespace/comments before the bracket close.
    let after_paren = skip_ws_and_comments(bytes, paren_close + 1)?;
    if after_paren != bracket_close {
        return Ok(None);
    }
    Ok(Some(bracket_close + 1))
}

/// Try to skip over any non-DSL-source segment (line/block comments, string
/// literals, raw string literals, char/byte-char literals). Returns the byte
/// offset just past the segment, or `None` if the bytes at `i` are ordinary.
fn try_skip_passthrough(bytes: &[u8], i: usize) -> Result<Option<usize>, String> {
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

fn position_after_index(source: &str, index: usize) -> FilePos {
    let mut pos = FilePos { line: 1, column: 1 };
    if index < source.len() {
        bump_pos_bytes(&mut pos, &source.as_bytes()[..=index]);
    }
    pos
}

/// Position AT the given byte index (one past the previous byte). This is what
/// `line!()` / `column!()` evaluate to when emitted with the proc-macro's
/// call-site span: the position of the macro identifier's first character.
fn position_at_index(source: &str, index: usize) -> FilePos {
    let mut pos = FilePos { line: 1, column: 1 };
    if index <= source.len() {
        bump_pos_bytes(&mut pos, &source.as_bytes()[..index]);
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
            && bytes[j + 1..j + 1 + hashes]
                .iter()
                .all(|byte| *byte == b'#')
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
    fn no_cfg_attr_yields_empty_chunks() {
        let source = r#"
        script_mod! {
            use mod.prelude.widgets.*
            let X = 1
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        assert_eq!(extracted.len(), 1);
        assert!(extracted[0].chunks.is_empty());
    }

    #[test]
    fn cfg_attr_produces_chunks_tree() {
        let source = r#"
        script_mod! {
            use mod.prelude.widgets.*
            #[cfg(feature = "ai")]
            use mod.ai_widgets.*
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        assert_eq!(count_conditionals(&extracted[0].chunks), 1);
    }

    #[test]
    fn filtered_code_with_true_matches_full_body() {
        let source = r#"
        script_mod! {
            use mod.prelude.widgets.*
            #[cfg(feature = "ai")]
            use mod.ai_widgets.*
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        let (code, count) = compute_filtered_code(&extracted[0], &[true]).unwrap();
        assert!(code.contains("use mod.prelude.widgets.*"));
        assert!(code.contains("use mod.ai_widgets.*"));
        assert_eq!(count, 0);
    }

    #[test]
    fn filtered_code_with_false_omits_conditional_body() {
        let source = r#"
        script_mod! {
            use mod.prelude.widgets.*
            #[cfg(feature = "ai")]
            use mod.ai_widgets.*
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        let (code, count) = compute_filtered_code(&extracted[0], &[false]).unwrap();
        assert!(code.contains("use mod.prelude.widgets.*"));
        assert!(!code.contains("ai_widgets"), "conditional body leaked: {}", code);
        assert_eq!(count, 0);
    }

    #[test]
    fn cfg_count_mismatch_errors() {
        // Source has 2 conditionals; compiled binary had 1.
        let source = r#"
        script_mod! {
            #[cfg(feature = "a")]
            let A = 1
            #[cfg(feature = "b")]
            let B = 2
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        let err = compute_filtered_code(&extracted[0], &[true]).unwrap_err();
        assert!(err.contains("file has 2 cfg fragments, compiled binary has 1"), "{}", err);
    }

    #[test]
    fn cfg_count_mismatch_zero_to_one_errors() {
        // Source has no conditionals (fragments empty) but compiled binary
        // expected 1. The mismatch is detected up the call chain — here we
        // exercise the empty-fragments case explicitly: with no fragments,
        // compute_filtered_code reports 0 vs N.
        let source = r#"
        script_mod! {
            use mod.foo.*
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        assert!(extracted[0].chunks.is_empty());
        let err = compute_filtered_code(&extracted[0], &[true]).unwrap_err();
        assert!(err.contains("file has 0 cfg fragments, compiled binary has 1"), "{}", err);
    }

    #[test]
    fn brace_grouped_form_strips_outer_braces() {
        let source = r#"
        script_mod! {
            use mod.prelude.widgets.*
            #[cfg(feature = "ai")] {
                use mod.ai_widgets.*
                let Z = 1
            }
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        let (code, _) = compute_filtered_code(&extracted[0], &[true]).unwrap();
        // The outer `{`/`}` of the brace-grouped form must not appear at top
        // level — only the contents.
        assert!(code.contains("use mod.ai_widgets.*"));
        assert!(code.contains("let Z = 1"));
        // Specifically check no spurious `{}` at the start of a line.
        for line in code.lines() {
            let t = line.trim();
            assert!(
                t != "{" && t != "}",
                "brace-grouped outer braces leaked: line={:?} code={:?}",
                line,
                code
            );
        }
    }

    #[test]
    fn cfg_inside_string_literal_is_not_detected() {
        let source = r#"
        script_mod! {
            label: "use #[cfg(feature = \"x\")] inline"
            let X = 1
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        // The `#[cfg(...)]` inside the string literal must NOT be treated as a
        // cfg attribute — chunks stays empty.
        assert!(extracted[0].chunks.is_empty(), "chunks={:?}", extracted[0].chunks);
    }

    #[test]
    fn inner_cfg_attribute_is_not_detected() {
        // `#![cfg(...)]` is an inner attribute and should be ignored by the
        // extractor (the proc-macro rejects it at compile time, so a compiled
        // site can never carry it in `cfg_fragments`).
        let source = r#"
        script_mod! {
            #![cfg(feature = "x")]
            use mod.foo.*
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        assert!(extracted[0].chunks.is_empty());
    }

    #[test]
    fn lexical_parity_with_awkward_spacing() {
        // Whitespace and comments between `#`, `[`, `cfg`, `(...)`, `]` should
        // still be recognised.
        let source = r#"
        script_mod! {
            use mod.foo.*
            #  [  cfg  (  feature  =  "x"  )  ]
            use mod.bar.*
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        assert_eq!(count_conditionals(&extracted[0].chunks), 1);
    }

    #[test]
    fn nested_cfg_inside_group_is_recognised() {
        // Nested `#[cfg(...)]` inside a brace group must be detected. Total
        // pre-order conditional count = 1 for this body.
        let source = r#"
        script_mod! {
            ui: Root {
                #[cfg(feature = "pro")]
                pro_button := Button {}
            }
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        assert_eq!(count_conditionals(&extracted[0].chunks), 1);
    }

    #[test]
    fn nested_cfg_filters_correctly() {
        let source = r#"
        script_mod! {
            ui: Root {
                main_button := Button {}
                #[cfg(feature = "pro")]
                pro_button := Button {}
            }
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        let (code_on, _) = compute_filtered_code(&extracted[0], &[true]).unwrap();
        let (code_off, _) = compute_filtered_code(&extracted[0], &[false]).unwrap();
        assert!(code_on.contains("pro_button"));
        assert!(!code_off.contains("pro_button"), "pro_button leaked when off: {}", code_off);
        assert!(code_off.contains("main_button"));
    }

    #[test]
    fn doubly_nested_cfg_counts_in_pre_order() {
        // Two cfg attrs, one nested inside the other's brace-grouped body.
        let source = r#"
        script_mod! {
            #[cfg(feature = "outer")] {
                let A = 1
                #[cfg(feature = "inner")]
                let B = 2
            }
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        assert_eq!(count_conditionals(&extracted[0].chunks), 2);

        // When outer is true and inner is true → both A and B
        let (code, _) = compute_filtered_code(&extracted[0], &[true, true]).unwrap();
        assert!(code.contains("let A = 1"));
        assert!(code.contains("let B = 2"));

        // When outer is true and inner is false → only A
        let (code, _) = compute_filtered_code(&extracted[0], &[true, false]).unwrap();
        assert!(code.contains("let A = 1"));
        assert!(!code.contains("let B = 2"));

        // When outer is false → neither (regardless of inner)
        let (code, _) = compute_filtered_code(&extracted[0], &[false, true]).unwrap();
        assert!(!code.contains("let A = 1"));
        assert!(!code.contains("let B = 2"));
    }

    #[test]
    fn macro_position_records_ident_start() {
        // The extractor must record the (line, column) of the `script_mod`
        // identifier itself, so the apply path can match compiled-runtime
        // ScriptMod entries (whose line/column come from `line!()`/`column!()`
        // at the macro call-site) and skip entries from other macros like
        // `script_eval!` / `script!`.
        let source = "\n\n    script_mod! {\n        use mod.foo.*\n    }\n";
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        assert_eq!(extracted.len(), 1);
        // `script_mod` starts on line 3, column 5 (1-based).
        assert_eq!(extracted[0].macro_line, 3);
        assert_eq!(extracted[0].macro_column, 5);
    }

    #[test]
    fn placeholder_renumbering_in_extracted_fragments() {
        let source = r#"
        script_mod! {
            before := #(a)
            #[cfg(feature = "x")]
            middle := #(b)
            after := #(c)
        }
        "#;
        let extracted = extract_script_mods_from_rust_file("/tmp/test.rs", source).unwrap();
        // When the conditional is false, the filtered code references #(0) and
        // #(1) only — no ghost #(2).
        let (code, count) = compute_filtered_code(&extracted[0], &[false]).unwrap();
        assert_eq!(count, 2);
        assert!(code.contains("#(0)"), "{}", code);
        assert!(code.contains("#(1)"), "{}", code);
        assert!(!code.contains("#(2)"), "ghost #(2) found in {}", code);
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

    #[test]
    fn excludes_platform_script_and_draw_manifests_from_hot_reload() {
        let excluded = excluded_hot_reload_manifest_paths();
        assert!(excluded.contains(&normalize_path_string(Path::new(env!(
            "CARGO_MANIFEST_DIR"
        )))));
        assert!(excluded.contains(&normalize_path_string(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("script")
        )));
        assert!(excluded.contains(&normalize_path_string(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../draw")
        )));
    }
}
