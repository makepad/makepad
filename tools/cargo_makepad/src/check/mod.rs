use crate::makepad_shell::*;
use crate::utils::*;
use makepad_micro_serde::*;
use makepad_script::{
    makepad_error_log,
    parser::ScriptParser,
    tokenizer::ScriptTokenizer,
    ScriptHeap,
    ScriptValue,
};
use makepad_toml_parser::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptCheckIssueKind {
    RuntimeError,
    ParseError,
    FallbackWarning,
}

#[derive(Debug)]
struct ScriptCheckIssue {
    kind: ScriptCheckIssueKind,
    file: String,
    line: u32,
    column: u32,
    message: String,
}

thread_local! {
    static SCRIPT_CHECK_ERRORS: RefCell<Vec<ScriptCheckIssue>> = RefCell::new(Vec::new());
}

fn script_check_capture_logger(
    file_name: &str,
    line_start: u32,
    column_start: u32,
    _line_end: u32,
    _column_end: u32,
    message: String,
    level: makepad_error_log::LogLevel,
) {
    if matches!(level, makepad_error_log::LogLevel::Error | makepad_error_log::LogLevel::Warning) {
        SCRIPT_CHECK_ERRORS.with(|cell| {
            cell.borrow_mut().push(ScriptCheckIssue {
                kind: ScriptCheckIssueKind::ParseError,
                file: file_name.to_string(),
                line: line_start + 1,
                column: column_start + 1,
                message,
            });
        });
    }
}

fn skip_rust_raw_string(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    if i >= bytes.len() {
        return None;
    }
    if bytes[i] == b'b' {
        if i + 1 >= bytes.len() || bytes[i + 1] != b'r' {
            return None;
        }
        i += 2;
    } else if bytes[i] == b'r' {
        i += 1;
    } else {
        return None;
    }

    let mut hash_count = 0usize;
    while i < bytes.len() && bytes[i] == b'#' {
        hash_count += 1;
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'"' {
        return None;
    }
    i += 1;

    while i < bytes.len() {
        if bytes[i] == b'"' {
            let mut j = i + 1;
            let mut matched = 0usize;
            while matched < hash_count && j < bytes.len() && bytes[j] == b'#' {
                matched += 1;
                j += 1;
            }
            if matched == hash_count {
                return Some(j);
            }
        }
        i += 1;
    }

    Some(bytes.len())
}

/// Maximum Rust value index in script (#(0), #(1), ... or @(0) etc). Parser expects values.len() > max.
fn max_rust_value_index(code: &str) -> usize {
    let mut max = 0usize;
    let bytes = code.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let is_hash = bytes[i] == b'#' && bytes[i + 1] == b'(';
        let is_at = bytes[i] == b'@' && bytes[i + 1] == b'(';
        if is_hash || is_at {
            i += 2;
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i > start {
                let n: usize = std::str::from_utf8(&bytes[start..i]).unwrap_or("0").parse().unwrap_or(0);
                max = max.max(n);
            }
            continue;
        }
        i += 1;
    }
    max
}

/// Replace Rust interpolation forms (`#(<rust expr>)`, `@(<rust expr>)`) with indexed placeholders
/// (`#(0)`, `#(1)`, ...) so parser-only validation can run on source extracted from Rust macros.
fn normalize_rust_interpolations(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let bytes = code.as_bytes();
    let mut i = 0usize;
    let mut next_index = 0usize;

    while i < bytes.len() {
        let c = bytes[i] as char;

        if let Some(next) = skip_rust_raw_string(bytes, i) {
            out.push_str(&code[i..next]);
            i = next;
            continue;
        }

        // Preserve strings.
        if c == '"' || c == '\'' {
            let quote = c as u8;
            out.push(c);
            i += 1;
            while i < bytes.len() {
                let b = bytes[i];
                out.push(b as char);
                i += 1;
                if b == b'\\' && i < bytes.len() {
                    out.push(bytes[i] as char);
                    i += 1;
                    continue;
                }
                if b == quote {
                    break;
                }
            }
            continue;
        }

        // Preserve line/block comments.
        if c == '/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                out.push('/');
                out.push('/');
                i += 2;
                while i < bytes.len() {
                    let b = bytes[i];
                    out.push(b as char);
                    i += 1;
                    if b == b'\n' {
                        break;
                    }
                }
                continue;
            }
            if bytes[i + 1] == b'*' {
                out.push('/');
                out.push('*');
                i += 2;
                while i + 1 < bytes.len() {
                    let b0 = bytes[i];
                    let b1 = bytes[i + 1];
                    out.push(b0 as char);
                    i += 1;
                    if b0 == b'*' && b1 == b'/' {
                        out.push('/');
                        i += 1;
                        break;
                    }
                }
                continue;
            }
        }

        // Normalize #(...) and @(...) to indexed placeholders.
        let is_hash_interp = c == '#' && i + 1 < bytes.len() && bytes[i + 1] == b'(';
        let is_at_interp = c == '@' && i + 1 < bytes.len() && bytes[i + 1] == b'(';
        if is_hash_interp || is_at_interp {
            let marker = c;
            i += 2; // skip marker and '('
            let mut depth = 1u32;
            while i < bytes.len() && depth > 0 {
                let b = bytes[i];
                if b == b'\\' {
                    i = (i + 2).min(bytes.len());
                    continue;
                }
                if b == b'(' {
                    depth += 1;
                } else if b == b')' {
                    depth -= 1;
                }
                i += 1;
            }
            out.push(marker);
            out.push('(');
            out.push_str(&next_index.to_string());
            out.push(')');
            next_index += 1;
            continue;
        }

        out.push(c);
        i += 1;
    }

    out
}

/// Run tokenize + parse on script code, capturing any parse errors via a temporary logger.
/// Returns collected errors (file, line, column, message).
fn run_script_parse(code: &str, file_name: &str, _path_for_display: &str) -> Vec<ScriptCheckIssue> {
    SCRIPT_CHECK_ERRORS.with(|cell| {
        cell.borrow_mut().clear();
    });

    let old_logger = {
        let mut guard = makepad_error_log::LOG_WITH_LEVEL.write().expect("Logger lock poisoned");
        let old = *guard;
        *guard = script_check_capture_logger;
        old
    };

    let mut heap = ScriptHeap::empty();
    let mut tokenizer = ScriptTokenizer::default();
    let mut parser = ScriptParser::default();
    let normalized = normalize_rust_interpolations(code);

    tokenizer.tokenize(&normalized, &mut heap);
    let max_idx = max_rust_value_index(&normalized);
    let values: Vec<ScriptValue> = (0..=max_idx).map(|_| ScriptValue::default()).collect();
    parser.parse(&tokenizer, file_name, (0, 0), &values);

    {
        let mut guard = makepad_error_log::LOG_WITH_LEVEL.write().expect("Logger lock poisoned");
        *guard = old_logger;
    }

    SCRIPT_CHECK_ERRORS.with(|cell| cell.take())
}

const RUNTIME_ISSUE_PREFIX: &str = "__MAKEPAD_SCRIPT_CHECK__";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptMacroKind {
    ScriptMod,
    Script,
}

#[derive(Debug, DeJson)]
struct CargoMetadata {
    packages: Vec<CargoMetadataPackage>,
    workspace_members: Vec<String>,
    workspace_default_members: Option<Vec<String>>,
}

#[derive(Debug, DeJson)]
struct CargoMetadataPackage {
    id: String,
    name: String,
    manifest_path: String,
    targets: Vec<CargoMetadataTarget>,
}

#[derive(Debug, DeJson)]
struct CargoMetadataTarget {
    name: String,
    kind: Vec<String>,
    src_path: String,
}

#[derive(Debug)]
struct RuntimeModule {
    module_path: Vec<String>,
    file_path: PathBuf,
}

#[derive(Debug)]
struct RuntimeHarnessPlan {
    package_name: String,
    crate_name: String,
    package_dir: PathBuf,
    modules: Vec<RuntimeModule>,
    needs_widgets_prelude: bool,
}

fn canonicalize_best(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn find_manifest_path(cwd: &Path) -> Option<PathBuf> {
    let mut cur = Some(cwd);
    while let Some(dir) = cur {
        let candidate = dir.join("Cargo.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

fn read_cargo_metadata(manifest_path: &Path) -> Result<CargoMetadata, String> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(manifest_path)
        .output()
        .map_err(|e| format!("Failed to run cargo metadata: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    DeJson::deserialize_json_lenient(&stdout).map_err(|e| format!("Failed to parse cargo metadata JSON: {:?}", e))
}

fn select_metadata_package<'a>(
    metadata: &'a CargoMetadata,
    manifest_path: &Path,
    cwd: &Path,
) -> Option<&'a CargoMetadataPackage> {
    let manifest_canon = canonicalize_best(manifest_path);
    if let Some(pkg) = metadata
        .packages
        .iter()
        .find(|p| canonicalize_best(Path::new(&p.manifest_path)) == manifest_canon)
    {
        return Some(pkg);
    }

    let cwd_canon = canonicalize_best(cwd);
    let mut candidates: Vec<&CargoMetadataPackage> = metadata
        .packages
        .iter()
        .filter(|p| {
            let package_dir = Path::new(&p.manifest_path)
                .parent()
                .map(canonicalize_best)
                .unwrap_or_else(|| PathBuf::from(&p.manifest_path));
            cwd_canon.starts_with(package_dir)
        })
        .collect();
    candidates.sort_by_key(|p| p.manifest_path.len());
    if let Some(pkg) = candidates.pop() {
        return Some(pkg);
    }

    if let Some(default_members) = &metadata.workspace_default_members {
        for member in default_members {
            if let Some(pkg) = metadata.packages.iter().find(|p| &p.id == member) {
                return Some(pkg);
            }
        }
    } else if let Some(member) = metadata.workspace_members.first() {
        if let Some(pkg) = metadata.packages.iter().find(|p| &p.id == member) {
            return Some(pkg);
        }
    }

    None
}

fn discover_declared_modules(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pending_cfg_attr = false;
    for raw_line in content.lines() {
        let line = raw_line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#[") {
            if line.contains("cfg(") || line.contains("cfg_attr(") {
                pending_cfg_attr = true;
            }
            continue;
        }
        if !line.ends_with(';') {
            pending_cfg_attr = false;
            continue;
        }
        let mut tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            pending_cfg_attr = false;
            continue;
        }
        let mod_index = match tokens.iter().position(|t| *t == "mod") {
            Some(idx) => idx,
            None => {
                pending_cfg_attr = false;
                continue;
            }
        };
        if pending_cfg_attr {
            pending_cfg_attr = false;
            continue;
        }
        if mod_index + 1 >= tokens.len() {
            pending_cfg_attr = false;
            continue;
        }
        let mut name = tokens[mod_index + 1].trim_end_matches(';');
        if let Some(i) = name.find('{') {
            name = &name[..i];
        }
        if !name.is_empty() && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()) {
            out.push(name.to_string());
        }
        pending_cfg_attr = false;
        tokens.clear();
    }
    out
}

fn discover_module_files(
    file: &Path,
    module_path: &[String],
    out: &mut HashMap<PathBuf, Vec<String>>,
    visited: &mut HashSet<PathBuf>,
) {
    let canon_file = canonicalize_best(file);
    if !visited.insert(canon_file.clone()) {
        return;
    }
    out.insert(canon_file.clone(), module_path.to_vec());
    let content = match std::fs::read_to_string(&canon_file) {
        Ok(c) => c,
        Err(_) => return,
    };
    let parent = match canon_file.parent() {
        Some(p) => p,
        None => return,
    };
    for name in discover_declared_modules(&content) {
        let mod_rs = parent.join(format!("{}.rs", name));
        let mod_mod_rs = parent.join(&name).join("mod.rs");
        let next_file = if mod_rs.is_file() {
            mod_rs
        } else if mod_mod_rs.is_file() {
            mod_mod_rs
        } else {
            continue;
        };
        let mut next_path = module_path.to_vec();
        next_path.push(name);
        discover_module_files(&next_file, &next_path, out, visited);
    }
}

fn discover_runtime_modules(
    lib_src_path: &Path,
    explicit_rs_paths: &HashSet<PathBuf>,
    restrict_to_explicit: bool,
) -> Vec<RuntimeModule> {
    let mut module_files = HashMap::<PathBuf, Vec<String>>::new();
    let mut visited = HashSet::new();
    discover_module_files(lib_src_path, &[], &mut module_files, &mut visited);

    let mut out = Vec::new();
    for (file, module_path) in module_files {
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if !content.contains("script_mod!") {
            continue;
        }
        if restrict_to_explicit && !explicit_rs_paths.contains(&file) {
            continue;
        }
        out.push(RuntimeModule {
            module_path,
            file_path: file,
        });
    }
    out.sort_by(|a, b| {
        let pa = if a.module_path.is_empty() {
            String::new()
        } else {
            a.module_path.join("::")
        };
        let pb = if b.module_path.is_empty() {
            String::new()
        } else {
            b.module_path.join("::")
        };
        pa.cmp(&pb)
    });
    out
}

fn build_runtime_harness_plan(
    cwd: &Path,
    explicit_rs_paths: &HashSet<PathBuf>,
    restrict_to_explicit: bool,
    needs_widgets_prelude: bool,
) -> Result<RuntimeHarnessPlan, String> {
    let manifest_path = find_manifest_path(cwd).ok_or_else(|| "No Cargo.toml found in current directory tree".to_string())?;
    let metadata = read_cargo_metadata(&manifest_path)?;
    let package = select_metadata_package(&metadata, &manifest_path, cwd)
        .ok_or_else(|| "Could not determine package for runtime script check".to_string())?;

    let lib_target = package
        .targets
        .iter()
        .find(|t| t.kind.iter().any(|k| k == "lib"))
        .ok_or_else(|| "Package has no lib target required for runtime script checking".to_string())?;

    let lib_src_path = PathBuf::from(&lib_target.src_path);
    let package_dir = Path::new(&package.manifest_path)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Invalid package manifest path".to_string())?;
    let modules = discover_runtime_modules(&lib_src_path, explicit_rs_paths, restrict_to_explicit);

    Ok(RuntimeHarnessPlan {
        package_name: package.name.clone(),
        crate_name: lib_target.name.clone(),
        package_dir,
        modules,
        needs_widgets_prelude,
    })
}

fn runtime_module_call_expr(crate_name: &str, module_path: &[String]) -> String {
    if module_path.is_empty() {
        format!("{}::script_mod", crate_name)
    } else {
        format!("{}::{}::script_mod", crate_name, module_path.join("::"))
    }
}

fn write_runtime_harness(plan: &RuntimeHarnessPlan) -> Result<PathBuf, String> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    plan.package_name.hash(&mut hasher);
    plan.crate_name.hash(&mut hasher);
    for m in &plan.modules {
        m.module_path.hash(&mut hasher);
    }
    let hash = hasher.finish();
    let dir = std::env::temp_dir().join(format!("makepad-script-check-{}", hash));
    let src_dir = dir.join("src");
    std::fs::create_dir_all(&src_dir).map_err(|e| format!("Failed to create runtime harness dir: {}", e))?;

    let script_dep = canonicalize_best(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../platform/script"));
    let platform_dep = canonicalize_best(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../platform"));
    let widgets_dep = canonicalize_best(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../widgets"));
    let package_dir = canonicalize_best(&plan.package_dir);
    let mut cargo_toml = format!(
        "[package]\nname = \"makepad-script-check-runner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nmakepad-script = {{ path = \"{}\" }}\nmakepad-platform = {{ path = \"{}\" }}\nchecked_crate = {{ package = \"{}\", path = \"{}\" }}\n",
        script_dep.display(),
        platform_dep.display(),
        plan.package_name,
        package_dir.display()
    );
    if plan.needs_widgets_prelude && plan.package_name != "makepad-widgets" {
        cargo_toml.push_str(&format!(
            "makepad-widgets = {{ path = \"{}\" }}\n",
            widgets_dep.display()
        ));
    }
    std::fs::write(dir.join("Cargo.toml"), cargo_toml)
        .map_err(|e| format!("Failed to write runtime harness Cargo.toml: {}", e))?;

    let mut bootstrap_calls = String::new();
    if plan.needs_widgets_prelude && plan.package_name != "makepad-widgets" {
        bootstrap_calls.push_str(
            "        run_module(\"makepad_widgets\", \"<makepad-widgets>\", |vm| {\n            makepad_widgets::script_mod(vm);\n        }, vm);\n",
        );
    }

    let mut calls = String::new();
    for m in &plan.modules {
        let module_name = if m.module_path.is_empty() {
            "crate_root".to_string()
        } else {
            m.module_path.join("::")
        };
        let call_expr = runtime_module_call_expr("checked_crate", &m.module_path);
        let file_name = m.file_path.display().to_string();
        calls.push_str(&format!(
            "        run_module(\"{}\", \"{}\", |vm| {{\n            {}(vm);\n        }}, vm);\n",
            module_name, file_name, call_expr
        ));
    }

    let source = format!(
        "use makepad_platform::Cx;\nuse makepad_script::{{makepad_error_log, ScriptVm}};\nuse std::sync::{{Mutex, OnceLock}};\n\nconst PREFIX: &str = \"{}\";\n\n#[derive(Clone)]\nstruct Issue {{\n    file: String,\n    line: u32,\n    column: u32,\n    message: String,\n}}\n\nstatic ISSUES: OnceLock<Mutex<Vec<Issue>>> = OnceLock::new();\n\nfn issues() -> &'static Mutex<Vec<Issue>> {{\n    ISSUES.get_or_init(|| Mutex::new(Vec::new()))\n}}\n\nfn push_issue(file: String, line: u32, column: u32, message: String) {{\n    let mut guard = issues().lock().unwrap();\n    guard.push(Issue {{ file, line, column, message }});\n}}\n\nfn capture_logger(\n    file_name: &str,\n    line_start: u32,\n    column_start: u32,\n    _line_end: u32,\n    _column_end: u32,\n    message: String,\n    level: makepad_error_log::LogLevel,\n) {{\n    if matches!(level, makepad_error_log::LogLevel::Error | makepad_error_log::LogLevel::Warning) {{\n        push_issue(file_name.to_string(), line_start + 1, column_start + 1, message);\n    }}\n}}\n\nfn panic_to_message(payload: Box<dyn std::any::Any + Send>) -> String {{\n    if let Some(s) = payload.downcast_ref::<&str>() {{\n        return (*s).to_string();\n    }}\n    if let Some(s) = payload.downcast_ref::<String>() {{\n        return s.clone();\n    }}\n    \"panic without string payload\".to_string()\n}}\n\nfn run_module<F: FnOnce(&mut ScriptVm)>(module_name: &str, file_name: &str, f: F, vm: &mut ScriptVm) {{\n    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(vm)));\n    if let Err(panic_payload) = result {{\n        push_issue(\n            file_name.to_string(),\n            1,\n            1,\n            format!(\"panic while running {{}}: {{}}\", module_name, panic_to_message(panic_payload)),\n        );\n    }}\n    vm.drain_errors();\n}}\n\nfn sanitize_message(message: &str) -> String {{\n    message.replace('\\n', \"\\\\n\").replace('\\r', \"\\\\r\").replace('\\t', \" \")\n}}\n\nfn main() {{\n    let old_logger = {{\n        let mut guard = makepad_error_log::LOG_WITH_LEVEL.write().expect(\"logger lock poisoned\");\n        let old = *guard;\n        *guard = capture_logger;\n        old\n    }};\n\n    let mut cx = Cx::new(Box::new(|_, _| {{}}));\n    cx.with_vm(|vm| {{\n{}{}\n    }});\n\n    {{\n        let mut guard = makepad_error_log::LOG_WITH_LEVEL.write().expect(\"logger lock poisoned\");\n        *guard = old_logger;\n    }}\n\n    let issues = issues().lock().unwrap();\n    for issue in issues.iter() {{\n        println!(\"{{}}\\truntime_error\\t{{}}\\t{{}}\\t{{}}\\t{{}}\", PREFIX, issue.file, issue.line, issue.column, sanitize_message(&issue.message));\n    }}\n\n    if !issues.is_empty() {{\n        std::process::exit(1);\n    }}\n}}\n",
        RUNTIME_ISSUE_PREFIX,
        bootstrap_calls,
        calls
    );

    let mut f = std::fs::File::create(src_dir.join("main.rs"))
        .map_err(|e| format!("Failed to write runtime harness source file: {}", e))?;
    f.write_all(source.as_bytes())
        .map_err(|e| format!("Failed to write runtime harness source file: {}", e))?;

    Ok(dir.join("Cargo.toml"))
}

fn parse_runtime_issue_lines(output: &str) -> Vec<ScriptCheckIssue> {
    let mut out = Vec::new();
    for line in output.lines() {
        let mut parts = line.splitn(6, '\t');
        let prefix = parts.next().unwrap_or("");
        if prefix != RUNTIME_ISSUE_PREFIX {
            continue;
        }
        let kind = parts.next().unwrap_or("");
        let file = parts.next().unwrap_or("").to_string();
        let line_num = parts.next().unwrap_or("1").parse::<u32>().unwrap_or(1);
        let col_num = parts.next().unwrap_or("1").parse::<u32>().unwrap_or(1);
        let message = parts.next().unwrap_or("").to_string();
        if kind == "runtime_error" {
            out.push(ScriptCheckIssue {
                kind: ScriptCheckIssueKind::RuntimeError,
                file,
                line: line_num,
                column: col_num,
                message,
            });
        }
    }
    out
}

fn run_runtime_harness(plan: &RuntimeHarnessPlan) -> Result<Vec<ScriptCheckIssue>, String> {
    let harness_manifest = write_runtime_harness(plan)?;
    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&harness_manifest)
        .output()
        .map_err(|e| format!("Failed to execute runtime harness: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut issues = parse_runtime_issue_lines(&stdout);
    issues.extend(parse_runtime_issue_lines(&stderr));

    if !output.status.success() && issues.is_empty() {
        let detail = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .or_else(|| stdout.lines().map(str::trim).find(|line| !line.is_empty()))
            .unwrap_or("unknown error");
        return Err(format!(
            "Runtime harness failed before reporting script issues: {}",
            detail
        ));
    }
    Ok(issues)
}

/// Extract script content from a Rust file: finds `script_mod! { ... }` and `script! { ... }` blocks.
/// Returns (line_number_1based, code, macro kind) for each block.
fn extract_script_blocks_from_rust(content: &str) -> Vec<(usize, String, ScriptMacroKind)> {
    fn is_ident_char(c: u8) -> bool {
        c == b'_' || (c as char).is_ascii_alphanumeric()
    }

    fn find_next_script_macro(content: &str, mut i: usize) -> Option<(usize, &'static str, ScriptMacroKind)> {
        let bytes = content.as_bytes();
        while i < bytes.len() {
            let c = bytes[i];
            if let Some(next) = skip_rust_raw_string(bytes, i) {
                i = next;
                continue;
            }
            if c == b'"' || c == b'\'' {
                let quote = c;
                i += 1;
                while i < bytes.len() {
                    let b = bytes[i];
                    if b == b'\\' {
                        i = (i + 2).min(bytes.len());
                        continue;
                    }
                    i += 1;
                    if b == quote {
                        break;
                    }
                }
                continue;
            }
            if c == b'/' && i + 1 < bytes.len() {
                if bytes[i + 1] == b'/' {
                    i += 2;
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                if bytes[i + 1] == b'*' {
                    i += 2;
                    while i + 1 < bytes.len() {
                        if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }
            }

            let rem = &content[i..];
            let (macro_name, macro_kind) = if rem.starts_with("script_mod!") {
                ("script_mod!", ScriptMacroKind::ScriptMod)
            } else if rem.starts_with("script!") {
                ("script!", ScriptMacroKind::Script)
            } else {
                i += 1;
                continue;
            };
            let prev_ok = i == 0 || !is_ident_char(bytes[i - 1]);
            let next = i + macro_name.len();
            let next_ok = next >= bytes.len() || !is_ident_char(bytes[next]);
            if prev_ok && next_ok {
                return Some((i, macro_name, macro_kind));
            }
            i += 1;
        }
        None
    }

    let mut blocks = Vec::new();
    let mut search = 0;
    loop {
        let (start, macro_name, macro_kind) = match find_next_script_macro(content, search) {
            Some(found) => found,
            None => break,
        };
        search = start + macro_name.len();
        // Skip to opening brace
        let open = match content[search..].find('{') {
            Some(i) => search + i,
            None => break,
        };
        search = open + 1;
        let (block_content, _close) = match extract_balanced_brace_block(&content[search..]) {
            Some((s, _)) => (s, search),
            None => continue,
        };
        let line_1based = content[..open].lines().count();
        blocks.push((line_1based, block_content.to_string(), macro_kind));
        search = open + 1 + block_content.len() + 1; // past closing }
    }
    blocks
}

/// Given content starting after an opening `{`, find the matching `}` and return the content between.
/// Skips braces inside strings ("...", '...', r"...") and comments (//, /* */).
fn extract_balanced_brace_block(content: &str) -> Option<(String, usize)> {
    let mut depth = 1u32;
    let mut i = 0;
    let mut out = String::new();
    let bytes = content.as_bytes();
    while i < bytes.len() && depth > 0 {
        let c = bytes[i] as char;
        if let Some(next) = skip_rust_raw_string(bytes, i) {
            out.push_str(&content[i..next]);
            i = next;
            continue;
        }
        if c == '"' || c == '\'' {
            let quote = c;
            out.push(c);
            i += 1;
            while i < bytes.len() {
                let b = bytes[i];
                if b == b'\\' && i + 1 < bytes.len() {
                    out.push(bytes[i] as char);
                    out.push(bytes[i + 1] as char);
                    i += 2;
                    continue;
                }
                if b == quote as u8 {
                    out.push(b as char);
                    i += 1;
                    break;
                }
                out.push(b as char);
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out.push(bytes[i] as char);
                    i += 1;
                }
                continue;
            }
            if bytes[i + 1] == b'*' {
                out.push('/');
                out.push('*');
                i += 2;
                while i + 1 < bytes.len() {
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        out.push('*');
                        out.push('/');
                        i += 2;
                        break;
                    }
                    out.push(bytes[i] as char);
                    i += 1;
                }
                continue;
            }
        }
        if c == '{' {
            depth += 1;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '}' {
            depth -= 1;
            if depth == 0 {
                return Some((out, i));
            }
            out.push(c);
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    if depth == 0 {
        Some((out, i))
    } else {
        None
    }
}

/// Collect all .rs files under dir (one level: dir/*.rs and dir/**/*.rs).
fn collect_rust_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().map_or(false, |e| e == "rs") {
                out.push(p);
            } else if p.is_dir() {
                out.extend(collect_rust_files(&p));
            }
        }
    }
    out.sort();
    out
}

/// Discover script blocks inside Rust files under src/. Returns
/// (path, line_offset_0based, code, macro kind) for each block.
fn discover_script_in_rust(cwd: &std::path::Path) -> Vec<(std::path::PathBuf, u32, String, ScriptMacroKind)> {
    let src = cwd.join("src");
    if !src.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for path in collect_rust_files(&src) {
        if let Ok(content) = std::fs::read_to_string(&path) {
            for (line_1based, code, macro_kind) in extract_script_blocks_from_rust(&content) {
                out.push((path.clone(), (line_1based as u32).saturating_sub(1), code, macro_kind));
            }
        }
    }
    out
}

/// Source to check: a script block extracted from .rs.
enum ScriptSource {
    RustBlock { path: std::path::PathBuf, line_offset: u32, code: String, macro_kind: ScriptMacroKind },
}

fn add_fallback_warning(issues: &mut Vec<ScriptCheckIssue>, message: String) {
    issues.push(ScriptCheckIssue {
        kind: ScriptCheckIssueKind::FallbackWarning,
        file: "<script-check>".to_string(),
        line: 1,
        column: 1,
        message,
    });
}

fn run_parse_checks_for_sources(sources: &[ScriptSource]) -> Vec<ScriptCheckIssue> {
    let mut all_errors = Vec::new();
    for src in sources {
        let (code, file_name, line_offset) = match src {
            ScriptSource::RustBlock { path, line_offset, code, .. } => {
                let name = path.display().to_string();
                (code.clone(), name, *line_offset)
            }
        };
        let mut errs = run_script_parse(&code, &file_name, &file_name);
        for e in &mut errs {
            e.line += line_offset;
        }
        all_errors.extend(errs);
    }
    all_errors
}

fn runtime_covered_files(modules: &[RuntimeModule]) -> HashSet<PathBuf> {
    modules
        .iter()
        .map(|m| canonicalize_best(&m.file_path))
        .collect::<HashSet<_>>()
}

fn script_mod_block_counts_by_file(sources: &[ScriptSource]) -> HashMap<PathBuf, usize> {
    let mut counts = HashMap::new();
    for source in sources {
        if let ScriptSource::RustBlock {
            path,
            macro_kind: ScriptMacroKind::ScriptMod,
            ..
        } = source
        {
            let canon = canonicalize_best(path);
            *counts.entry(canon).or_insert(0) += 1;
        }
    }
    counts
}

/// Check script sources in Rust files, report runtime and parse errors. Exit non-zero if any errors.
pub fn handle_check_script(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("Usage: cargo makepad check script".to_string());
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;

    let mut sources = Vec::new();
    let rust_blocks = discover_script_in_rust(&cwd);
    for (path, line_offset, code, macro_kind) in rust_blocks {
        sources.push(ScriptSource::RustBlock {
            path,
            line_offset,
            code,
            macro_kind,
        });
    }

    if sources.is_empty() {
        return Err(
            "No script found. Looked for script_mod!/script! in src/**/*.rs. Usage: cargo makepad check script"
                .to_string(),
        );
    }

    let mut issues = Vec::<ScriptCheckIssue>::new();
    let mut runtime_checked_files = HashSet::<PathBuf>::new();
    let script_mod_counts = script_mod_block_counts_by_file(&sources);
    let needs_widgets_prelude = sources.iter().any(|source| match source {
        ScriptSource::RustBlock { code, .. } => code.contains("mod.prelude.widgets"),
    });

    let has_script_mod_sources = sources.iter().any(|s| {
        matches!(
            s,
            ScriptSource::RustBlock {
                macro_kind: ScriptMacroKind::ScriptMod,
                ..
            }
        )
    });

    if has_script_mod_sources {
        match build_runtime_harness_plan(&cwd, &HashSet::new(), false, needs_widgets_prelude) {
            Ok(plan) => {
                if plan.modules.is_empty() {
                    add_fallback_warning(
                        &mut issues,
                        "Runtime script validation not applied: no resolvable script_mod modules found in package module tree; using parser fallback.".to_string(),
                    );
                } else {
                    runtime_checked_files = runtime_covered_files(&plan.modules);
                    match run_runtime_harness(&plan) {
                        Ok(mut runtime_issues) => issues.append(&mut runtime_issues),
                        Err(e) => {
                            runtime_checked_files.clear();
                            add_fallback_warning(
                                &mut issues,
                                format!("Runtime script validation unavailable: {}. Using parser fallback.", e),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                add_fallback_warning(
                    &mut issues,
                    format!("Runtime script validation unavailable: {}. Using parser fallback.", e),
                );
            }
        }
    }

    let parse_sources: Vec<ScriptSource> = sources
        .into_iter()
        .filter(|src| match src {
            ScriptSource::RustBlock {
                macro_kind: ScriptMacroKind::Script,
                ..
            } => true,
            ScriptSource::RustBlock {
                path,
                macro_kind: ScriptMacroKind::ScriptMod,
                ..
            } => {
                let canon = canonicalize_best(path);
                if !runtime_checked_files.contains(&canon) {
                    true
                } else {
                    script_mod_counts.get(&canon).copied().unwrap_or(0) > 1
                }
            }
        })
        .collect();

    let mut parse_issues = run_parse_checks_for_sources(&parse_sources);
    issues.append(&mut parse_issues);

    for issue in &issues {
        match issue.kind {
            ScriptCheckIssueKind::FallbackWarning => eprintln!("warning: {}", issue.message),
            ScriptCheckIssueKind::RuntimeError | ScriptCheckIssueKind::ParseError => {
                eprintln!("{}:{}:{}: {}", issue.file, issue.line, issue.column, issue.message)
            }
        }
    }

    let error_count = issues
        .iter()
        .filter(|e| matches!(e.kind, ScriptCheckIssueKind::RuntimeError | ScriptCheckIssueKind::ParseError))
        .count();
    let warning_count = issues
        .iter()
        .filter(|e| matches!(e.kind, ScriptCheckIssueKind::FallbackWarning))
        .count();

    if error_count == 0 {
        if warning_count > 0 {
            println!("Script checks completed with {} warning(s)", warning_count);
        } else {
            println!("All script checks completed successfully");
        }
        Ok(())
    } else {
        Err(format!("{} script check error(s)", error_count))
    }
}

#[derive(Copy, Clone, Debug)]
enum BuildTy {
    Binary,
    BinaryBuildStd,
    Lib,
    LinuxDirect,
}

impl BuildTy {
    fn nightly_only(&self) -> bool {
        match self {
            Self::Binary => false,
            Self::BinaryBuildStd => true,
            Self::Lib => false,
            Self::LinuxDirect => false,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Platform {
    Web,
    Mobile,
    Desktop,
    Embedded,
}

const TOOLCHAINS: [(&'static str, BuildTy, Platform); 16] = [
    ("aarch64-apple-darwin", BuildTy::Binary, Platform::Desktop),
    ("x86_64-pc-windows-msvc", BuildTy::Binary, Platform::Desktop),
    (
        "x86_64-unknown-linux-gnu",
        BuildTy::Binary,
        Platform::Desktop,
    ),
    (
        "x86_64-unknown-linux-gnu",
        BuildTy::LinuxDirect,
        Platform::Embedded,
    ),
    ("wasm32-unknown-unknown", BuildTy::Lib, Platform::Web),
    ("aarch64-linux-android", BuildTy::Lib, Platform::Mobile),
    ("aarch64-apple-ios", BuildTy::Binary, Platform::Mobile),
    ("x86_64-linux-android", BuildTy::Lib, Platform::Mobile),
    (
        "aarch64-apple-tvos",
        BuildTy::BinaryBuildStd,
        Platform::Mobile,
    ),
    (
        "aarch64-apple-tvos-sim",
        BuildTy::BinaryBuildStd,
        Platform::Mobile,
    ),
    //("arm-linux-androideabi",1),
    ("i686-linux-android", BuildTy::Lib, Platform::Mobile),
    ("aarch64-apple-ios-sim", BuildTy::Binary, Platform::Mobile),
    ("x86_64-apple-ios", BuildTy::Binary, Platform::Mobile),
    (
        "x86_64-apple-tvos",
        BuildTy::BinaryBuildStd,
        Platform::Mobile,
    ),
    ("x86_64-apple-darwin", BuildTy::Binary, Platform::Desktop),
    ("x86_64-pc-windows-gnu", BuildTy::Binary, Platform::Desktop),
];

pub fn check_crate(build_crate: &str, args: &[String]) -> Result<(), String> {
    let crate_dir = get_crate_dir(build_crate).expect("Cant find crate dir");

    // lets parse the toml
    let cargo_str =
        std::fs::read_to_string(&crate_dir.join("Cargo.toml")).expect("Cant find cargo.toml");
    let toml = makepad_toml_parser::parse_toml(&cargo_str).expect("Cant parse Cargo.toml");
    let platforms =
        if let Some(Toml::Str(ver, _)) = toml.get("package.metadata.makepad-check-platform") {
            ver.to_string()
        } else {
            "desktop,web,mobile".to_string()
        };
    let nightly_only =
        if let Some(Toml::Bool(ver, _)) = toml.get("package.metadata.makepad-check-nightly-only") {
            *ver
        } else {
            false
        };
    let mut platform_filter = Vec::new();
    for platform in platforms.split(",") {
        let platform = platform.trim();
        match platform {
            "desktop" => platform_filter.push(Platform::Desktop),
            "mobile" => platform_filter.push(Platform::Mobile),
            "web" => platform_filter.push(Platform::Web),
            "embedded" => platform_filter.push(Platform::Embedded),
            e => return Err(format!("Unexpected platform in makepad-check-platform {e}")),
        }
    }
    let mut count = 0;
    for (_toolchain, ty, platform) in TOOLCHAINS {
        if !platform_filter.contains(&platform) {
            continue;
        }
        if nightly_only || ty.nightly_only() {
            count += 1;
        } else {
            count += 2;
        }
    }

    println!("Check all for {} on {} builds", build_crate, count);
    let mut handles = Vec::new();
    let (sender, reciever) = std::sync::mpsc::channel();
    for (index, (toolchain, ty, platform)) in TOOLCHAINS.into_iter().enumerate() {
        if !platform_filter.contains(&platform) {
            continue;
        }
        let toolchain = toolchain.to_string();
        let args = args.to_vec();
        let sender = sender.clone();
        let thread = std::thread::spawn(move || {
            if !nightly_only && !ty.nightly_only() {
                let result = check(&toolchain, "stable", ty, &args, index);
                let _ = sender.send(("stable", toolchain.clone(), ty, result));
            }
            let result = check(&toolchain, "nightly", ty, &args, index);
            let _ = sender.send(("nightly", toolchain.clone(), ty, result));
        });
        handles.push(thread);
    }
    for handle in handles {
        let _ = handle.join();
    }
    let mut has_errors = false;
    while let Ok((branch, toolchain, ty, (stdout, stderr, success))) = reciever.try_recv() {
        if !success {
            has_errors = true;
            eprintln!("Errors found in build {} {} {:?}", toolchain, branch, ty)
        }
        if stdout.len() > 0 {
            if stdout.contains("warning") {
                print!("{}", stdout);
            }
        }
        if !success && stderr.len() > 0 {
            eprint!("{}", stderr)
        }
    }
    if has_errors {
        println!("Errors found whilst checking");
        Err("Errors found whilst checking".to_string())
    } else {
        println!("All checks completed successfully");
        Ok(())
    }
}

pub fn handle_check(args: &[String]) -> Result<(), String> {
    match args[0].as_ref() {
        "toolchain-install" | "install-toolchain" => {
            // lets install all toolchains we support
            rustup_toolchain_install()
        }
        "script" => {
            handle_check_script(&args[1..])
        }
        "all" => {
            let cwd = std::env::current_dir().unwrap();
            if let Ok(build_crate) = get_build_crate_from_args(&args[1..]) {
                return check_crate(&build_crate, &args[1..]);
            } else if let Err(e) = shell_env_cap(&[], &cwd, "cargo", &["run", "--bin"]) {
                let mut after_av = false;
                for line in e.split("\n") {
                    if after_av {
                        let binary = line.trim().to_string();
                        if binary.len() > 0 {
                            let mut args = args[1..].to_vec();
                            args.insert(0, binary.to_string());
                            args.insert(0, "-p".to_string());
                            check_crate(&binary, &args)?;
                        }
                    }
                    if line.contains("Available binaries:") {
                        after_av = true;
                    }
                }
                return Ok(());
            } else {
                return Err("No crate to check".to_string());
            }
        }
        _ => return Err("Unknown command".to_string()),
    }
}

fn check(
    toolchain: &str,
    branch: &str,
    ty: BuildTy,
    args: &[String],
    par: usize,
) -> (String, String, bool) {
    let toolchain = format!("--target={}", toolchain);

    let base_args = &["run", branch, "cargo", "check", &toolchain];
    let cwd = std::env::current_dir().unwrap();

    let mut args_out = Vec::new();
    args_out.extend_from_slice(base_args);
    for arg in args {
        args_out.push(arg);
    }
    let target_dir = format!("--target-dir=target/check_all/check{}", par);
    args_out.push(&target_dir);
    match ty {
        BuildTy::Binary => {
            if branch == "stable" {
                return shell_env_cap_split(&[("MAKEPAD", " ")], &cwd, "rustup", &args_out);
            } else {
                return shell_env_cap_split(&[("MAKEPAD", "lines")], &cwd, "rustup", &args_out);
            }
        }
        BuildTy::BinaryBuildStd => {
            args_out.push("-Z");
            args_out.push("build-std=std");
            if branch == "stable" {
                return shell_env_cap_split(&[("MAKEPAD", " ")], &cwd, "rustup", &args_out);
            } else {
                return shell_env_cap_split(&[("MAKEPAD", "lines")], &cwd, "rustup", &args_out);
            }
        }
        BuildTy::Lib => {
            args_out.push("--lib");
            if branch == "stable" {
                return shell_env_cap_split(&[("MAKEPAD", " ")], &cwd, "rustup", &args_out);
            } else {
                return shell_env_cap_split(&[("MAKEPAD", "lines")], &cwd, "rustup", &args_out);
            }
        }
        BuildTy::LinuxDirect => {
            if branch == "stable" {
                return shell_env_cap_split(
                    &[("MAKEPAD", "linux_direct")],
                    &cwd,
                    "rustup",
                    &args_out,
                );
            } else {
                return shell_env_cap_split(
                    &[("MAKEPAD", "lines,linux_direct")],
                    &cwd,
                    "rustup",
                    &args_out,
                );
            }
        }
    }
}

fn rustup_toolchain_install() -> Result<(), String> {
    println!("Installing Rust toolchains for wasm");
    shell_env(
        &[],
        &std::env::current_dir().unwrap(),
        "rustup",
        &["update"],
    )?;
    shell_env(
        &[],
        &std::env::current_dir().unwrap(),
        "rustup",
        &["install", "nightly"],
    )?;
    for (toolchain, ty, _platform) in TOOLCHAINS {
        if let BuildTy::BinaryBuildStd = ty {
            //TODO fix this better
            let _ = shell_env_cap(
                &[],
                &std::env::current_dir().unwrap(),
                "rustup",
                &["target", "add", toolchain, "--toolchain", "nightly"],
            );
            let _ = shell_env_cap(
                &[],
                &std::env::current_dir().unwrap(),
                "rustup",
                &["target", "add", toolchain, "--toolchain", "stable"],
            );
        } else {
            shell_env(
                &[],
                &std::env::current_dir().unwrap(),
                "rustup",
                &["target", "add", toolchain, "--toolchain", "nightly"],
            )?;
            shell_env(
                &[],
                &std::env::current_dir().unwrap(),
                "rustup",
                &["target", "add", toolchain, "--toolchain", "stable"],
            )?;
        }
    }
    Ok(())
}
