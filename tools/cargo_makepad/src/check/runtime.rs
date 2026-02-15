/// Runtime harness generation and execution for script validation.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::cargo::{canonicalize_path, CargoPackage};
use super::error::{CheckError, CheckResult, IssueKind, ScriptIssue};
use super::parser::discover_declared_modules;

/// Prefix used for runtime issue output parsing.
pub const RUNTIME_ISSUE_PREFIX: &str = "__MAKEPAD_SCRIPT_CHECK__";

/// A module that contains script_mod! and needs runtime validation.
#[derive(Debug, Clone)]
pub struct RuntimeModule {
    /// Module path components (e.g., ["foo", "bar"] for crate::foo::bar).
    pub module_path: Vec<String>,
    /// File path to the module source.
    pub file_path: PathBuf,
}

/// Extra bootstrap call for dependencies (like openpad-widgets).
#[derive(Debug, Clone)]
pub struct BootstrapCall {
    pub module_name: String,
    pub call_expr: String,
}

/// Configuration for generating the runtime harness.
#[derive(Debug)]
pub struct RuntimeHarnessPlan {
    /// Package name being checked.
    pub package_name: String,
    /// Crate name (lib target name).
    pub crate_name: String,
    /// Package directory.
    pub package_dir: PathBuf,
    /// Modules containing script_mod! to check.
    pub modules: Vec<RuntimeModule>,
    /// Dependency entries for the harness Cargo.toml.
    pub runtime_dependency_entries: Vec<String>,
    /// Path to Cx type.
    pub runtime_cx_path: String,
    /// Path to error_log module.
    pub runtime_log_path: String,
    /// Optional widgets bootstrap expression.
    pub widgets_bootstrap_expr: Option<String>,
    /// Extra bootstrap calls (e.g., openpad_widgets).
    pub extra_bootstrap_calls: Vec<BootstrapCall>,
}

impl RuntimeHarnessPlan {
    /// Create a new plan builder.
    pub fn builder() -> RuntimeHarnessPlanBuilder {
        RuntimeHarnessPlanBuilder::default()
    }

    /// Compute a hash for caching the harness.
    fn cache_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.package_name.hash(&mut hasher);
        self.crate_name.hash(&mut hasher);
        self.runtime_cx_path.hash(&mut hasher);
        self.runtime_log_path.hash(&mut hasher);
        for dep in &self.runtime_dependency_entries {
            dep.hash(&mut hasher);
        }
        if let Some(expr) = &self.widgets_bootstrap_expr {
            expr.hash(&mut hasher);
        }
        for call in &self.extra_bootstrap_calls {
            call.module_name.hash(&mut hasher);
            call.call_expr.hash(&mut hasher);
        }
        for m in &self.modules {
            m.module_path.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Get the set of files covered by runtime checking.
    pub fn covered_files(&self) -> HashSet<PathBuf> {
        self.modules
            .iter()
            .map(|m| canonicalize_path(&m.file_path))
            .collect()
    }
}

/// Builder for RuntimeHarnessPlan.
#[derive(Default)]
pub struct RuntimeHarnessPlanBuilder {
    package_name: Option<String>,
    crate_name: Option<String>,
    package_dir: Option<PathBuf>,
    modules: Vec<RuntimeModule>,
    runtime_dependency_entries: Vec<String>,
    runtime_cx_path: Option<String>,
    runtime_log_path: Option<String>,
    widgets_bootstrap_expr: Option<String>,
    extra_bootstrap_calls: Vec<BootstrapCall>,
}

impl RuntimeHarnessPlanBuilder {
    pub fn package_name(mut self, name: String) -> Self {
        self.package_name = Some(name);
        self
    }

    pub fn crate_name(mut self, name: String) -> Self {
        self.crate_name = Some(name);
        self
    }

    pub fn package_dir(mut self, dir: PathBuf) -> Self {
        self.package_dir = Some(dir);
        self
    }

    pub fn modules(mut self, modules: Vec<RuntimeModule>) -> Self {
        self.modules = modules;
        self
    }

    pub fn add_dependency_entry(mut self, entry: String) -> Self {
        self.runtime_dependency_entries.push(entry);
        self
    }

    pub fn runtime_paths(mut self, cx_path: String, log_path: String) -> Self {
        self.runtime_cx_path = Some(cx_path);
        self.runtime_log_path = Some(log_path);
        self
    }

    pub fn widgets_bootstrap(mut self, expr: Option<String>) -> Self {
        self.widgets_bootstrap_expr = expr;
        self
    }

    pub fn add_bootstrap_call(mut self, module_name: String, call_expr: String) -> Self {
        self.extra_bootstrap_calls.push(BootstrapCall { module_name, call_expr });
        self
    }

    pub fn build(self) -> CheckResult<RuntimeHarnessPlan> {
        Ok(RuntimeHarnessPlan {
            package_name: self.package_name.ok_or_else(|| CheckError::PackageNotFound)?,
            crate_name: self.crate_name.ok_or_else(|| CheckError::NoLibTarget)?,
            package_dir: self.package_dir.ok_or_else(|| CheckError::PackageNotFound)?,
            modules: self.modules,
            runtime_dependency_entries: self.runtime_dependency_entries,
            runtime_cx_path: self.runtime_cx_path.unwrap_or_else(|| "Cx".to_string()),
            runtime_log_path: self.runtime_log_path.unwrap_or_else(|| "makepad_error_log".to_string()),
            widgets_bootstrap_expr: self.widgets_bootstrap_expr,
            extra_bootstrap_calls: self.extra_bootstrap_calls,
        })
    }
}

/// Discover module files recursively from a root file.
fn discover_module_files(
    file: &Path,
    module_path: &[String],
    out: &mut HashMap<PathBuf, Vec<String>>,
    visited: &mut HashSet<PathBuf>,
) {
    let canon_file = canonicalize_path(file);
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

/// Parse lib.rs for `crate::module::script_mod(vm)` call order.
fn parse_script_mod_call_order(lib_src_path: &Path) -> Vec<Vec<String>> {
    let content = match std::fs::read_to_string(lib_src_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut order = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if !line.contains("script_mod") {
            continue;
        }

        if let Some(start) = line.find("crate::") {
            let rest = &line[start + "crate::".len()..];
            if let Some(end) = rest.find("::script_mod") {
                let path = &rest[..end];
                let module_path: Vec<String> = path
                    .split("::")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !module_path.is_empty() {
                    order.push(module_path);
                }
            }
        }
    }

    order
}

/// Discover runtime modules from lib source path.
pub fn discover_runtime_modules(
    lib_src_path: &Path,
    explicit_rs_paths: &HashSet<PathBuf>,
    restrict_to_explicit: bool,
) -> Vec<RuntimeModule> {
    let mut module_files = HashMap::<PathBuf, Vec<String>>::new();
    let mut visited = HashSet::new();
    discover_module_files(lib_src_path, &[], &mut module_files, &mut visited);

    let mut modules = Vec::new();

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

        modules.push(RuntimeModule { module_path, file_path: file });
    }

    // Sort by call order from lib.rs
    let call_order = parse_script_mod_call_order(lib_src_path);
    if !call_order.is_empty() {
        modules.retain(|m| !m.module_path.is_empty());
    }

    modules.sort_by(|a, b| {
        let order_index = |path: &[String]| call_order.iter().position(|o| o.as_slice() == path);
        let ia = order_index(&a.module_path);
        let ib = order_index(&b.module_path);

        match (ia, ib) {
            (Some(i), Some(j)) => i.cmp(&j),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => {
                let pa = a.module_path.join("::");
                let pb = b.module_path.join("::");
                pa.cmp(&pb)
            }
        }
    });

    modules
}

/// Build a runtime harness plan from package information.
pub fn build_harness_plan(
    package: &CargoPackage,
    _cwd: &Path,
    explicit_rs_paths: &HashSet<PathBuf>,
    restrict_to_explicit: bool,
    needs_widgets_prelude: bool,
) -> CheckResult<RuntimeHarnessPlan> {
    let lib_target = package.lib_target()
        .ok_or(CheckError::NoLibTarget)?;

    let lib_src_path = PathBuf::from(&lib_target.src_path);
    let package_dir = Path::new(&package.manifest_path)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| CheckError::InvalidManifestPath(PathBuf::from(&package.manifest_path)))?;

    let modules = discover_runtime_modules(&lib_src_path, explicit_rs_paths, restrict_to_explicit);

    let mut builder = RuntimeHarnessPlan::builder()
        .package_name(package.name.clone())
        .crate_name(lib_target.name.clone())
        .package_dir(package_dir)
        .modules(modules);

    // Configure runtime paths based on available dependencies
    if package.name == "makepad-widgets" {
        builder = builder.runtime_paths(
            "checked_crate::Cx".to_string(),
            "checked_crate::makepad_platform::makepad_error_log".to_string(),
        );
    } else if let Some(widgets_dep) = package.find_dependency("makepad-widgets") {
        builder = builder
            .add_dependency_entry(widgets_dep.to_toml_entry("runtime_makepad_widgets"))
            .runtime_paths(
                "runtime_makepad_widgets::Cx".to_string(),
                "runtime_makepad_widgets::makepad_platform::makepad_error_log".to_string(),
            );

        if needs_widgets_prelude {
            builder = builder.widgets_bootstrap(Some("runtime_makepad_widgets::script_mod(vm);".to_string()));
        }

        // Check for openpad-widgets
        if let Some(ow_dep) = package.find_dependency("openpad-widgets") {
            builder = builder
                .add_dependency_entry(ow_dep.to_toml_entry("runtime_openpad_widgets"))
                .add_bootstrap_call(
                    "openpad_widgets".to_string(),
                    "runtime_openpad_widgets::script_mod(vm);".to_string(),
                );
        }
    } else if let Some(platform_dep) = package.find_dependency("makepad-platform") {
        builder = builder
            .add_dependency_entry(platform_dep.to_toml_entry("runtime_makepad_platform"))
            .runtime_paths(
                "runtime_makepad_platform::Cx".to_string(),
                "runtime_makepad_platform::makepad_error_log".to_string(),
            );
    } else {
        // Fallback to bundled platform
        let platform_path = canonicalize_path(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../platform"));
        builder = builder
            .add_dependency_entry(format!(
                "runtime_makepad_platform = {{ package = \"makepad-platform\", path = \"{}\" }}\n",
                platform_path.display()
            ))
            .runtime_paths(
                "runtime_makepad_platform::Cx".to_string(),
                "runtime_makepad_platform::makepad_error_log".to_string(),
            );
    }

    builder.build()
}

/// Generate the runtime harness source code.
fn generate_harness_source(plan: &RuntimeHarnessPlan) -> String {
    let mut bootstrap_calls = String::new();

    // Add widgets bootstrap if needed
    if let Some(bootstrap_expr) = &plan.widgets_bootstrap_expr {
        bootstrap_calls.push_str(&format!(
            r#"        run_module("makepad_widgets", "<makepad-widgets>", || {{
            {}
        }});
        vm.drain_errors();
"#,
            bootstrap_expr
        ));
    }

    // Add extra bootstrap calls
    for call in &plan.extra_bootstrap_calls {
        bootstrap_calls.push_str(&format!(
            r#"        run_module("{}", "<{}>", || {{
            {}
        }});
        vm.drain_errors();
"#,
            call.module_name, call.module_name, call.call_expr
        ));
    }

    // Generate module calls
    let mut module_calls = String::new();
    for m in &plan.modules {
        let module_name = if m.module_path.is_empty() {
            "crate_root".to_string()
        } else {
            m.module_path.join("::")
        };

        let call_expr = if m.module_path.is_empty() {
            "checked_crate::script_mod".to_string()
        } else {
            format!("checked_crate::{}::script_mod", m.module_path.join("::"))
        };

        let file_name = m.file_path.display().to_string();

        module_calls.push_str(&format!(
            r#"        run_module("{}", "{}", || {{
            {}(vm);
        }});
        vm.drain_errors();
"#,
            module_name, file_name, call_expr
        ));
    }

    format!(
        r#"use {cx_path};
use std::sync::{{Mutex, OnceLock}};

const PREFIX: &str = "{prefix}";

#[derive(Clone)]
struct Issue {{
    file: String,
    line: u32,
    column: u32,
    message: String,
}}

static ISSUES: OnceLock<Mutex<Vec<Issue>>> = OnceLock::new();

fn issues() -> &'static Mutex<Vec<Issue>> {{
    ISSUES.get_or_init(|| Mutex::new(Vec::new()))
}}

fn push_issue(file: String, line: u32, column: u32, message: String) {{
    let mut guard = issues().lock().unwrap();
    guard.push(Issue {{ file, line, column, message }});
}}

fn capture_logger(
    file_name: &str,
    line_start: u32,
    column_start: u32,
    _line_end: u32,
    _column_end: u32,
    message: String,
    level: {log_path}::LogLevel,
) {{
    if matches!(level, {log_path}::LogLevel::Error | {log_path}::LogLevel::Warning) {{
        push_issue(file_name.to_string(), line_start + 1, column_start + 1, message);
    }}
}}

fn panic_to_message(payload: Box<dyn std::any::Any + Send>) -> String {{
    if let Some(s) = payload.downcast_ref::<&str>() {{
        return (*s).to_string();
    }}
    if let Some(s) = payload.downcast_ref::<String>() {{
        return s.clone();
    }}
    "panic without string payload".to_string()
}}

fn run_module<F: FnOnce()>(module_name: &str, file_name: &str, f: F) {{
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    if let Err(panic_payload) = result {{
        push_issue(
            file_name.to_string(),
            1,
            1,
            format!("panic while running {{}}: {{}}", module_name, panic_to_message(panic_payload)),
        );
    }}
}}

fn sanitize_message(message: &str) -> String {{
    message.replace('\n', "\\n").replace('\r', "\\r").replace('\t', " ")
}}

fn main() {{
    let old_logger = {{
        let mut guard = {log_path}::LOG_WITH_LEVEL.write().expect("logger lock poisoned");
        let old = *guard;
        *guard = capture_logger;
        old
    }};

    let mut cx = Cx::new(Box::new(|_, _| {{}}));
    cx.with_vm(|vm| {{
{bootstrap_calls}{module_calls}
    }});

    {{
        let mut guard = {log_path}::LOG_WITH_LEVEL.write().expect("logger lock poisoned");
        *guard = old_logger;
    }}

    let issues = issues().lock().unwrap();
    for issue in issues.iter() {{
        println!("{{}}\truntime_error\t{{}}\t{{}}\t{{}}\t{{}}", PREFIX, issue.file, issue.line, issue.column, sanitize_message(&issue.message));
    }}

    if !issues.is_empty() {{
        std::process::exit(1);
    }}
}}
"#,
        cx_path = plan.runtime_cx_path,
        prefix = RUNTIME_ISSUE_PREFIX,
        log_path = plan.runtime_log_path,
        bootstrap_calls = bootstrap_calls,
        module_calls = module_calls,
    )
}

/// Write the runtime harness to a temporary directory.
pub fn write_harness(plan: &RuntimeHarnessPlan) -> CheckResult<PathBuf> {
    let hash = plan.cache_hash();
    let dir = std::env::temp_dir().join(format!("makepad-script-check-{}", hash));
    let src_dir = dir.join("src");

    std::fs::create_dir_all(&src_dir)
        .map_err(CheckError::HarnessDirectoryError)?;

    // Write Cargo.toml
    let package_dir = canonicalize_path(&plan.package_dir);
    let mut cargo_toml = String::new();
    cargo_toml.push_str("[package]\nname = \"makepad-script-check-runner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n");

    for dep in &plan.runtime_dependency_entries {
        cargo_toml.push_str(dep);
    }

    cargo_toml.push_str(&format!(
        "checked_crate = {{ package = \"{}\", path = \"{}\" }}\n",
        plan.package_name,
        package_dir.display()
    ));

    std::fs::write(dir.join("Cargo.toml"), cargo_toml)
        .map_err(CheckError::HarnessWriteError)?;

    // Write main.rs
    let source = generate_harness_source(plan);
    let mut f = std::fs::File::create(src_dir.join("main.rs"))
        .map_err(CheckError::HarnessWriteError)?;
    f.write_all(source.as_bytes())
        .map_err(CheckError::HarnessWriteError)?;

    Ok(dir.join("Cargo.toml"))
}

/// Parse runtime issue output lines.
fn parse_runtime_output(output: &str) -> Vec<ScriptIssue> {
    let mut issues = Vec::new();

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
            issues.push(ScriptIssue::new(
                IssueKind::RuntimeError,
                file,
                line_num,
                col_num,
                message,
            ));
        }
    }

    issues
}

/// Run the runtime harness and collect issues.
pub fn run_harness(plan: &RuntimeHarnessPlan) -> CheckResult<Vec<ScriptIssue>> {
    let harness_manifest = write_harness(plan)?;

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&harness_manifest)
        .output()
        .map_err(|e| CheckError::HarnessExecutionError(format!("Failed to execute harness: {}", e)))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let mut issues = parse_runtime_output(&stdout);
    issues.extend(parse_runtime_output(&stderr));

    if !output.status.success() && issues.is_empty() {
        let detail = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .or_else(|| stdout.lines().map(str::trim).find(|line| !line.is_empty()))
            .unwrap_or("unknown error");

        return Err(CheckError::HarnessExecutionError(format!(
            "Runtime harness failed before reporting script issues: {}",
            detail
        )));
    }

    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_runtime_output() {
        let output = format!(
            "{}\truntime_error\ttest.rs\t10\t5\tTest error message",
            RUNTIME_ISSUE_PREFIX
        );
        let issues = parse_runtime_output(&output);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].file, "test.rs");
        assert_eq!(issues[0].line, 10);
        assert_eq!(issues[0].column, 5);
        assert_eq!(issues[0].message, "Test error message");
    }
}
