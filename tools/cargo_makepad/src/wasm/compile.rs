use crate::makepad_network::http_server::*;
use crate::makepad_network::{digest::Sha1, NetworkConfig, NetworkRuntime};
use crate::makepad_shell::*;
use crate::makepad_wasm_strip::*;
use crate::server_manager::WasmServerOwnershipGuard;
use crate::utils::*;
use makepad_filesystem_watcher::{FileSystemWatcher, WatchRoot};
use makepad_micro_serde::{DeJson, DeJsonErr, DeJsonState, SerJson, SerJsonState};
use makepad_toml_parser::{parse_toml, Toml};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    fs::File,
    io::prelude::*,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command,
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

pub struct WasmBuildResult {
    app_dir: PathBuf,
    serve_mode: WasmServeMode,
    asset_manifest: Option<AssetManifest>,
}

#[derive(Clone, Copy)]
pub struct WasmConfig {
    pub strip: bool,
    pub lan: bool,
    pub port: Option<u16>,
    pub small_fonts: bool,
    pub brotli: bool,
    pub bindgen: bool,
    pub threads: bool,
    pub optimize_size: bool,
    pub wasm_opt: bool,
    pub split: bool,
    pub split_auto: bool,
    pub split_functions: bool,
    pub split_functions_threshold: usize,
    pub hot_reload: bool,
    pub serve: bool,
    pub shipping_build: bool,
    pub full_fonts: bool,
    pub brotli_explicit: bool,
    pub threads_explicit: bool,
    pub small_fonts_explicit: bool,
    pub split_explicit: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WasmServeMode {
    Dev,
    Shipping,
}

#[derive(Clone, Debug, Default)]
struct WasmWebMetadata {
    preserve: Vec<String>,
    full_i18n: bool,
}

#[derive(Clone, Debug)]
struct SelectedResource {
    logical_path: String,
    source_path: PathBuf,
    crate_name: String,
    direct_reference: bool,
    reason: String,
}

#[derive(Clone, Debug)]
struct PendingAsset {
    logical_path: String,
    emitted_path: String,
    kind: String,
    content_type: String,
    startup_blocking: bool,
    direct_reference: bool,
    crate_name: Option<String>,
    reason: String,
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct AssetManifest {
    version: u32,
    build_crate: String,
    profile: String,
    shipping_build: bool,
    threaded: bool,
    startup_assets: Vec<String>,
    assets: Vec<AssetManifestEntry>,
}

impl AssetManifest {
    fn emitted_path_for(&self, logical_path: &str) -> Option<&str> {
        self.assets
            .iter()
            .find(|asset| asset.logical_path == logical_path)
            .map(|asset| asset.emitted_path.as_str())
    }

    fn asset_for_request_path(&self, request_path: &str) -> Option<&AssetManifestEntry> {
        let request_path = request_path.trim_start_matches('/');
        self.assets
            .iter()
            .find(|asset| asset.emitted_path == request_path)
    }
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct AssetManifestEntry {
    logical_path: String,
    emitted_path: String,
    kind: String,
    content_type: String,
    cache_control: String,
    startup_blocking: bool,
    hashed: bool,
    direct_reference: bool,
    crate_name: Option<String>,
    reason: String,
    raw_bytes: u64,
    transfer_bytes: u64,
    brotli_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
struct WebPerfBudget {
    name: String,
    limit_bytes: u64,
    actual_bytes: u64,
    passed: bool,
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
struct WebPerfReport {
    version: u32,
    build_crate: String,
    profile: String,
    shipping_build: bool,
    threaded: bool,
    total_raw_bytes: u64,
    total_transfer_bytes: u64,
    startup_blocking_raw_bytes: u64,
    startup_blocking_transfer_bytes: u64,
    budgets: Vec<WebPerfBudget>,
    assets: Vec<AssetManifestEntry>,
}

#[derive(Clone, Debug)]
struct HtmlAssetPaths {
    wasm_path: String,
    split_data_path: Option<String>,
    split_data_active_only: bool,
    secondary_wasm_path: Option<String>,
    defer_secondary_wasm: bool,
    web_gl_js_path: String,
    full_canvas_css_path: String,
    bindgen_js_path: Option<String>,
    wasm_bridge_js_path: Option<String>,
}

#[derive(SerJson, Clone)]
struct WasmHotReloadEvent {
    kind: String,
    file_name: String,
    content: String,
}

enum WasmHotReloadCommand {
    LiveChange { file_name: String, content: String },
    Rebuild,
}

struct WasmHotReloadPlan {
    roots: Vec<WatchRoot>,
    files_by_root: HashMap<String, Vec<String>>,
    initial_contents: HashMap<String, String>,
    initial_script_mod_bodies: HashMap<String, Vec<String>>,
}

struct WasmHotReloadWatcher {
    _watcher: FileSystemWatcher,
}

#[derive(Clone)]
struct WasmRebuildPlan {
    config: WasmConfig,
    args: Vec<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AutoSplitOutcome {
    NotAttempted,
    Deferred,
    StartupPathFallback,
}

fn format_section_counts(summary: &WasmSectionSummary) -> String {
    if summary.counts.is_empty() {
        return "none".to_string();
    }

    summary
        .counts
        .iter()
        .map(|(name, count)| {
            if *count == 1 {
                name.clone()
            } else {
                format!("{name} x{count}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_wasm_size_report(report: &WasmSizeReport) {
    println!("Wasm size report:");
    println!("  original:  {} bytes", report.original_bytes);
    println!("  stripped:  {} bytes", report.stripped_bytes);
    println!("  optimized: {} bytes", report.optimized_bytes);
    println!(
        "  debug sections removed:  {} bytes ({})",
        report.debug_sections.total_bytes,
        format_section_counts(&report.debug_sections)
    );
    println!(
        "  custom sections removed: {} bytes ({})",
        report.custom_sections.total_bytes,
        format_section_counts(&report.custom_sections)
    );
}

fn print_wasm_split_report(primary_bytes: usize, split_bytes: usize, segments: usize) {
    println!("Wasm split report:");
    println!("  primary wasm:    {} bytes", primary_bytes);
    println!("  split data blob: {} bytes", split_bytes);
    println!("  segment count:   {}", segments);
    println!("  split total:     {} bytes", primary_bytes + split_bytes);
}

/// Run Binaryen wasm-opt -Os on the given wasm bytes if the tool is installed.
/// Returns the optimized bytes on success, or the original bytes on failure (with a note).
fn try_wasm_opt(data: &[u8], cwd: &Path) -> Vec<u8> {
    let build_dir = cwd.join("target/makepad-wasm-opt-tmp");
    if fs::create_dir_all(&build_dir).is_err() {
        println!("wasm-opt: skipped (cannot create temp dir)");
        return data.to_vec();
    }
    let in_path = build_dir.join("in.wasm");
    let out_path = build_dir.join("out.wasm");
    if fs::write(&in_path, data).is_err() {
        println!("wasm-opt: skipped (cannot write temp file)");
        return data.to_vec();
    }
    let args = vec![
        "--all-features".into(),
        "-Os".into(),
        "-o".into(),
        out_path.to_string_lossy().into_owned(),
        in_path.to_string_lossy().into_owned(),
    ];
    let status = Command::new("wasm-opt")
        .args(&args)
        .current_dir(cwd)
        .output();
    match status {
        Ok(ref output) if output.status.success() => match fs::read(&out_path) {
            Ok(optimized) => {
                let _ = fs::remove_file(&in_path);
                let _ = fs::remove_file(&out_path);
                println!("wasm-opt: {} -> {} bytes", data.len(), optimized.len());
                return optimized;
            }
            Err(_) => {
                println!("wasm-opt: skipped (cannot read output)");
            }
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.trim().is_empty() {
                println!("wasm-opt: skipped (Binaryen wasm-opt failed; install from https://github.com/WebAssembly/binaryen)");
            } else {
                println!(
                    "wasm-opt: skipped ({})",
                    stderr.lines().next().unwrap_or(stderr.trim())
                );
            }
        }
        Err(e) => {
            println!("wasm-opt: skipped ({e})");
        }
    }
    let _ = fs::remove_file(&in_path);
    let _ = fs::remove_file(&out_path);
    data.to_vec()
}

fn print_brotli_size_report(
    wasm_bytes: usize,
    wasm_brotli_bytes: usize,
    split_bytes: Option<usize>,
    split_brotli_bytes: Option<usize>,
) {
    println!("Brotli size report:");
    println!(
        "  wasm:            {} -> {} bytes",
        wasm_bytes, wasm_brotli_bytes
    );
    if let (Some(split_bytes), Some(split_brotli_bytes)) = (split_bytes, split_brotli_bytes) {
        println!(
            "  split data blob: {} -> {} bytes",
            split_bytes, split_brotli_bytes
        );
        println!(
            "  compressed total: {} bytes",
            wasm_brotli_bytes + split_brotli_bytes
        );
    }
}

fn app_relative_url(path: &str) -> String {
    format!("./{}", path.trim_start_matches('/'))
}

fn generate_html(title: &str, assets: &HtmlAssetPaths, config: &WasmConfig) -> String {
    let wasm_url = app_relative_url(&assets.wasm_path);
    let web_gl_js_url = app_relative_url(&assets.web_gl_js_path);
    let full_canvas_css_url = app_relative_url(&assets.full_canvas_css_path);
    let split_preloads = {
        let mut preloads = String::new();
        if let Some(split_data_path) = assets.split_data_path.as_deref() {
            preloads.push_str(&format!(
                "\n        <link rel='preload' href='{}' as='fetch' type='application/octet-stream' crossorigin>",
                app_relative_url(split_data_path)
            ));
        }
        if !assets.defer_secondary_wasm {
            if let Some(secondary_wasm_path) = assets.secondary_wasm_path.as_deref() {
                preloads.push_str(&format!(
                    "\n        <link rel='preload' href='{}' as='fetch' type='application/wasm' crossorigin>",
                    app_relative_url(secondary_wasm_path)
                ));
            }
        }
        preloads
    };

    let init = if config.bindgen {
        let wasm_bridge_js_url = app_relative_url(
            assets
                .wasm_bridge_js_path
                .as_deref()
                .unwrap_or("makepad_wasm_bridge/wasm_bridge.js"),
        );
        let bindgen_js_url =
            app_relative_url(assets.bindgen_js_path.as_deref().unwrap_or("bindgen.js"));
        format!(
            "
            const {{init_env}} = await import('{wasm_bridge_js_url}');
            const init = (await import('{bindgen_js_url}')).default;

            let env = {{}};
            let set_wasm = init_env(env);
            let module = await WebAssembly.compileStreaming(fetch('{wasm_url}'))
            let wasm = await init({{module_or_path: module}}, env);
            set_wasm(wasm);

            wasm._has_thread_support = wasm.exports.memory.buffer instanceof SharedArrayBuffer;
            wasm._memory = wasm.exports.memory;
            wasm._module = module;
            const {{WasmWebGL}} = await import('{web_gl_js_url}');
            "
        )
    } else {
        let defer_secondary = if assets.defer_secondary_wasm {
            ", defer_secondary_wasm: true"
        } else {
            ""
        };
        let split_data_active_only = if assets.split_data_active_only {
            ", split_data_active_only: true"
        } else {
            ""
        };
        let split_options = match (
            assets.split_data_path.as_deref(),
            assets.secondary_wasm_path.as_deref(),
        ) {
            (Some(data), Some(funcs)) => format!(
                ", undefined, {{ split_data_url: '{}'{split_data_active_only}, secondary_wasm_url: '{}'{defer_secondary} }}",
                app_relative_url(data),
                app_relative_url(funcs)
            ),
            (Some(data), None) => {
                format!(
                    ", undefined, {{ split_data_url: '{}'{split_data_active_only} }}",
                    app_relative_url(data)
                )
            }
            (None, Some(funcs)) => format!(
                ", undefined, {{ secondary_wasm_url: '{}'{defer_secondary} }}",
                app_relative_url(funcs)
            ),
            (None, None) => String::new(),
        };
        format!(
            "
            const {{WasmWebGL}} = await import('{web_gl_js_url}');
            const wasm = await WasmWebGL.fetch_and_instantiate_wasm(
                '{wasm_url}'{split_options}
            );
            "
        )
    };
    let auto_reload_script = if config.hot_reload {
        "\n        <script type='module' src='./makepad_platform/auto_reload.js'></script>"
    } else {
        ""
    };

    let preloads = if config.bindgen {
        let wasm_bridge_js_url = app_relative_url(
            assets
                .wasm_bridge_js_path
                .as_deref()
                .unwrap_or("makepad_wasm_bridge/wasm_bridge.js"),
        );
        let bindgen_js_url =
            app_relative_url(assets.bindgen_js_path.as_deref().unwrap_or("bindgen.js"));
        format!(
            "
        <link rel='modulepreload' href='{wasm_bridge_js_url}'>
        <link rel='modulepreload' href='{bindgen_js_url}'>
        <link rel='modulepreload' href='{web_gl_js_url}'>
        <link rel='preload' href='{wasm_url}' as='fetch' type='application/wasm' crossorigin>
        <link rel='preload' href='{full_canvas_css_url}' as='style'>
        {split_preloads}
        "
        )
    } else {
        format!(
            "
        <link rel='modulepreload' href='{web_gl_js_url}'>
        <link rel='preload' href='{wasm_url}' as='fetch' type='application/wasm' crossorigin>
        <link rel='preload' href='{full_canvas_css_url}' as='style'>
        {split_preloads}
        "
        )
    };

    format!(
        "
    <!DOCTYPE html>
    <html>
    <head>
        <meta charset='utf-8'>
        <meta name='viewport' content='width=device-width, initial-scale=1.0, user-scalable=no'>
        <title>{title}</title>
        {preloads}
        <script type='module'>
            const reportBrowserIssue = async (kind, data) => {{
                try {{
                    const payload = JSON.stringify({{
                        kind,
                        href: location.href,
                        user_agent: navigator.userAgent,
                        data
                    }});
                    const encoded = encodeURIComponent(payload.slice(0, 8192));
                    await fetch('/$report_error?data=' + encoded, {{cache: 'no-store'}});
                }} catch (_error) {{
                }}
            }};
            window.makepad_report_browser_issue = reportBrowserIssue;

            window.addEventListener('error', (event) => {{
                let stack = '';
                if (event.error && event.error.stack) {{
                    stack = '' + event.error.stack;
                }}
                reportBrowserIssue('window.error', {{
                    message: event.message || '',
                    filename: event.filename || '',
                    lineno: event.lineno || 0,
                    colno: event.colno || 0,
                    stack
                }});
            }});

            window.addEventListener('unhandledrejection', (event) => {{
                let reason_message = '';
                let reason_stack = '';
                if (typeof event.reason === 'string') {{
                    reason_message = event.reason;
                }} else if (event.reason) {{
                    reason_message = event.reason.message ? '' + event.reason.message : '' + event.reason;
                    reason_stack = event.reason.stack ? '' + event.reason.stack : '';
                }}
                reportBrowserIssue('window.unhandledrejection', {{
                    reason_message,
                    reason_stack
                }});
            }});

            try {{
                {init}
                class MyWasmApp {{
                    constructor(wasm) {{
                        let canvas = document.getElementsByClassName('full_canvas')[0];
                        this.webgl = new WasmWebGL (wasm, this, canvas);
                    }}
                }}
                let app = new MyWasmApp(wasm);
            }} catch (error) {{
                reportBrowserIssue('startup.exception', {{
                    message: error && error.message ? '' + error.message : '' + error,
                    stack: error && error.stack ? '' + error.stack : ''
                }});
                throw error;
            }}
        </script>
        {auto_reload_script}
        <link rel='stylesheet' type='text/css' href='{full_canvas_css_url}'>
    </head> 
    <body>
        <canvas class='full_canvas'></canvas>
            <div class='canvas_loader' >
            <div style=''>
            Loading..
            </div>
        </div>
    </body>
    </html>
    "
    )
}

fn brotli_compress(dest_path: &PathBuf) -> usize {
    let source_file_name = dest_path.file_name().unwrap().to_string_lossy().to_string();
    let dest_path_br = dest_path
        .parent()
        .unwrap()
        .join(&format!("{}.br", source_file_name));
    println!("Compressing {:?}", dest_path);
    // lets read the dest_path
    // lets brotli compress dest_path
    let mut brotli_data = Vec::new();
    let data = fs::read(&dest_path).expect("Can't read file");
    {
        let mut writer =
            brotli::CompressorWriter::new(&mut brotli_data, 65536 /* buffer size */, 11, 24);
        writer.write_all(&data).expect("Can't write data");
    }
    let mut brotli_file = File::create(dest_path_br).unwrap();
    brotli_file.write_all(&brotli_data).unwrap();
    brotli_data.len()
}

fn remove_brotli_artifact(dest_path: &PathBuf) {
    let source_file_name = match dest_path.file_name() {
        Some(name) => name.to_string_lossy().to_string(),
        None => return,
    };
    let dest_path_br = match dest_path.parent() {
        Some(parent) => parent.join(format!("{}.br", source_file_name)),
        None => return,
    };
    let _ = fs::remove_file(dest_path_br);
}

fn minify_js(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut string_char = '\0';
    let mut in_regex = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next_c) = chars.next() {
                    out.push(next_c);
                }
            } else if c == string_char {
                in_string = false;
            }
        } else if in_regex {
            out.push(c);
            if c == '\\' {
                if let Some(next_c) = chars.next() {
                    out.push(next_c);
                }
            } else if c == '/' {
                in_regex = false;
            }
        } else {
            match c {
                '\'' | '"' | '`' => {
                    in_string = true;
                    string_char = c;
                    out.push(c);
                }
                '/' => {
                    match chars.peek() {
                        Some(&'/') => {
                            // Line comment
                            while let Some(&next_c) = chars.peek() {
                                if next_c == '\n' {
                                    break;
                                }
                                chars.next();
                            }
                        }
                        Some(&'*') => {
                            // Block comment
                            chars.next();
                            while let Some(next_c) = chars.next() {
                                if next_c == '*' {
                                    if let Some(&'/') = chars.peek() {
                                        chars.next();
                                        break;
                                    }
                                }
                            }
                        }
                        _ => {
                            out.push(c);
                            // Very basic regex literal detection:
                            // If we see a slash not preceded by a value-like character
                            // it's likely a regex. This is a heuristic.
                            if let Some(last_c) = out.trim_end().chars().last() {
                                if "(,=:[!&|?<>~;{+*-".contains(last_c) {
                                    in_regex = true;
                                }
                            }
                        }
                    }
                }
                ' ' | '\t' | '\r' => {
                    // Only push a single space, and only if we need it
                    if out.ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$') {
                        if let Some(&next_c) = chars.peek() {
                            if next_c.is_alphanumeric() || next_c == '_' || next_c == '$' {
                                out.push(' ');
                            }
                        }
                    }
                }
                '\n' => {
                    out.push('\n');
                    // skip following whitespace
                    while let Some(&next_c) = chars.peek() {
                        if next_c == ' ' || next_c == '\t' || next_c == '\r' {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
                _ => out.push(c),
            }
        }
    }

    // final compacting: remove empty lines
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn cp_brotli(
    source_path: &PathBuf,
    dest_path: &PathBuf,
    exec: bool,
    compress: bool,
) -> Result<(), String> {
    if source_path.extension().and_then(|s| s.to_str()) == Some("js") {
        if let Ok(content) = std::fs::read_to_string(source_path) {
            mkdir(
                dest_path
                    .parent()
                    .ok_or_else(|| format!("Missing parent dir for {:?}", dest_path))?,
            )?;
            let minified = minify_js(&content);
            if let Err(e) = std::fs::write(dest_path, minified) {
                println!(
                    "Warning: could not write minified JS to {:?}: {}. Falling back to unminified copy.",
                    dest_path, e
                );
                cp(source_path, dest_path, exec)?;
            }
        } else {
            cp(source_path, dest_path, exec)?;
        }
    } else {
        cp(source_path, dest_path, exec)?;
    }

    if compress {
        brotli_compress(dest_path);
    } else {
        remove_brotli_artifact(dest_path);
    }
    Ok(())
}

fn short_cache_control() -> String {
    "no-cache, max-age=0, must-revalidate".to_string()
}

fn immutable_cache_control() -> String {
    "public, max-age=31536000, immutable".to_string()
}

fn mime_type_for_path(path: &str) -> Option<&'static str> {
    if path.ends_with(".html") {
        Some("text/html")
    } else if path.ends_with(".wasm") {
        Some("application/wasm")
    } else if path.ends_with(".css") {
        Some("text/css")
    } else if path.ends_with(".js") {
        Some("text/javascript")
    } else if path.ends_with(".ttf") {
        Some("application/ttf")
    } else if path.ends_with(".otf") {
        Some("font/otf")
    } else if path.ends_with(".png") {
        Some("image/png")
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        Some("image/jpg")
    } else if path.ends_with(".svg") {
        Some("image/svg+xml")
    } else if path.ends_with(".glb") {
        Some("model/gltf-binary")
    } else if path.ends_with(".bin") {
        Some("application/octet-stream")
    } else if path.ends_with(".md") {
        Some("text/markdown")
    } else if path.ends_with(".woff") {
        Some("font/woff")
    } else if path.ends_with(".woff2") {
        Some("font/woff2")
    } else if path.ends_with(".json") {
        Some("application/json")
    } else {
        None
    }
}

fn compute_asset_hash_hex(data: &[u8]) -> String {
    let mut sha1 = Sha1::new();
    sha1.update(data);
    let digest = sha1.finalise();
    digest[..8]
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect::<String>()
}

fn fingerprinted_asset_path(logical_path: &str, data: &[u8]) -> String {
    let hash = compute_asset_hash_hex(data);
    let path = Path::new(logical_path);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("asset");
    let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let file_name = if ext.is_empty() {
        format!("{stem}.{hash}")
    } else {
        format!("{stem}.{hash}.{ext}")
    };
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            parent.join(file_name).to_string_lossy().replace('\\', "/")
        }
        _ => file_name,
    }
}

fn rename_brotli_artifact_if_present(
    app_dir: &Path,
    old_rel_path: &str,
    new_rel_path: &str,
) -> Result<(), String> {
    let old_path = app_dir.join(old_rel_path);
    let new_path = app_dir.join(new_rel_path);
    let old_name = old_path
        .file_name()
        .ok_or_else(|| format!("Unable to get brotli source filename for {:?}", old_path))?
        .to_string_lossy()
        .to_string();
    let new_name = new_path
        .file_name()
        .ok_or_else(|| format!("Unable to get brotli dest filename for {:?}", new_path))?
        .to_string_lossy()
        .to_string();
    let Some(old_parent) = old_path.parent() else {
        return Ok(());
    };
    let Some(new_parent) = new_path.parent() else {
        return Ok(());
    };
    let old_br = old_parent.join(format!("{}.br", old_name));
    if old_br.exists() {
        mkdir(new_parent)?;
        fs::rename(&old_br, new_parent.join(format!("{}.br", new_name)))
            .map_err(|e| format!("Can't rename {:?} -> {:?}: {}", old_br, new_rel_path, e))?;
    }
    Ok(())
}

fn finalize_pending_assets(
    app_dir: &Path,
    build_crate: &str,
    profile: &str,
    threaded: bool,
    shipping_build: bool,
    pending_assets: &mut [PendingAsset],
) -> Result<AssetManifest, String> {
    let mut startup_assets = Vec::new();
    let mut assets = Vec::new();
    let mut seen_logical_paths = HashSet::new();

    for asset in pending_assets.iter_mut() {
        if !seen_logical_paths.insert(asset.logical_path.clone()) {
            continue;
        }
        let logical_path = asset.logical_path.clone();
        let original_path = app_dir.join(&logical_path);
        let data = fs::read(&original_path)
            .map_err(|e| format!("Can't read emitted asset {:?}: {}", original_path, e))?;
        let hashed = shipping_build
            && asset.startup_blocking
            && matches!(
                Path::new(&logical_path)
                    .extension()
                    .and_then(|ext| ext.to_str()),
                Some("wasm" | "js" | "css")
            );
        if hashed {
            let fingerprinted = fingerprinted_asset_path(&logical_path, &data);
            let fingerprinted_path = app_dir.join(&fingerprinted);
            mkdir(fingerprinted_path.parent().ok_or_else(|| {
                format!("Missing fingerprinted parent for {:?}", fingerprinted_path)
            })?)?;
            fs::rename(&original_path, &fingerprinted_path).map_err(|e| {
                format!(
                    "Can't fingerprint asset {:?} -> {:?}: {}",
                    original_path, fingerprinted_path, e
                )
            })?;
            rename_brotli_artifact_if_present(app_dir, &logical_path, &fingerprinted)?;
            asset.emitted_path = fingerprinted;
        } else {
            asset.emitted_path = logical_path.clone();
        }

        let emitted_path = app_dir.join(&asset.emitted_path);
        let raw_bytes = fs::metadata(&emitted_path)
            .map_err(|e| format!("Can't stat emitted asset {:?}: {}", emitted_path, e))?
            .len();
        let brotli_path = emitted_path.parent().and_then(|parent| {
            emitted_path
                .file_name()
                .map(|name| parent.join(format!("{}.br", name.to_string_lossy())))
        });
        let brotli_bytes = brotli_path
            .as_ref()
            .and_then(|path| fs::metadata(path).ok().map(|meta| meta.len()));
        let transfer_bytes = brotli_bytes.unwrap_or(raw_bytes);
        let cache_control = if hashed {
            immutable_cache_control()
        } else {
            short_cache_control()
        };
        if asset.startup_blocking {
            startup_assets.push(asset.emitted_path.clone());
        }
        assets.push(AssetManifestEntry {
            logical_path,
            emitted_path: asset.emitted_path.clone(),
            kind: asset.kind.clone(),
            content_type: asset.content_type.clone(),
            cache_control,
            startup_blocking: asset.startup_blocking,
            hashed,
            direct_reference: asset.direct_reference,
            crate_name: asset.crate_name.clone(),
            reason: asset.reason.clone(),
            raw_bytes,
            transfer_bytes,
            brotli_bytes,
        });
    }

    Ok(AssetManifest {
        version: 1,
        build_crate: build_crate.to_string(),
        profile: profile.to_string(),
        shipping_build,
        threaded,
        startup_assets,
        assets,
    })
}

fn write_json_file<T: SerJson>(path: &Path, value: &T, compress: bool) -> Result<(), String> {
    fs::write(path, value.serialize_json())
        .map_err(|e| format!("Can't write JSON file {:?}: {}", path, e))?;
    if compress {
        brotli_compress(&path.to_path_buf());
    } else {
        remove_brotli_artifact(&path.to_path_buf());
    }
    Ok(())
}

fn build_web_perf_report(manifest: &AssetManifest) -> WebPerfReport {
    let total_raw_bytes = manifest
        .assets
        .iter()
        .map(|asset| asset.raw_bytes)
        .sum::<u64>();
    let total_transfer_bytes = manifest
        .assets
        .iter()
        .map(|asset| asset.transfer_bytes)
        .sum::<u64>();
    let startup_assets: Vec<&AssetManifestEntry> = manifest
        .assets
        .iter()
        .filter(|asset| asset.startup_blocking)
        .collect();
    let startup_blocking_raw_bytes = startup_assets.iter().map(|asset| asset.raw_bytes).sum();
    let startup_blocking_transfer_bytes = startup_assets
        .iter()
        .map(|asset| asset.transfer_bytes)
        .sum();
    let oversized_optional_assets = manifest
        .assets
        .iter()
        .filter(|asset| {
            asset.kind == "resource"
                && !asset.direct_reference
                && !asset.startup_blocking
                && asset.raw_bytes > 500 * 1024
        })
        .count() as u64;

    WebPerfReport {
        version: 1,
        build_crate: manifest.build_crate.clone(),
        profile: manifest.profile.clone(),
        shipping_build: manifest.shipping_build,
        threaded: manifest.threaded,
        total_raw_bytes,
        total_transfer_bytes,
        startup_blocking_raw_bytes,
        startup_blocking_transfer_bytes,
        budgets: vec![
            WebPerfBudget {
                name: "raw_package_size".to_string(),
                limit_bytes: 10 * 1024 * 1024,
                actual_bytes: total_raw_bytes,
                passed: total_raw_bytes <= 10 * 1024 * 1024,
            },
            WebPerfBudget {
                name: "startup_blocking_transfer".to_string(),
                limit_bytes: 1_200_000,
                actual_bytes: startup_blocking_transfer_bytes,
                passed: startup_blocking_transfer_bytes <= 1_200_000,
            },
            WebPerfBudget {
                name: "oversized_optional_assets".to_string(),
                limit_bytes: 0,
                actual_bytes: oversized_optional_assets,
                passed: oversized_optional_assets == 0,
            },
        ],
        assets: manifest.assets.clone(),
    }
}

fn refresh_manifest_entry_sizes(
    app_dir: &Path,
    entry: &mut AssetManifestEntry,
) -> Result<(), String> {
    let asset_path = app_dir.join(&entry.emitted_path);
    entry.raw_bytes = fs::metadata(&asset_path)
        .map_err(|e| format!("Can't stat {:?}: {}", asset_path, e))?
        .len();
    let brotli_path = asset_path.parent().and_then(|parent| {
        asset_path
            .file_name()
            .map(|name| parent.join(format!("{}.br", name.to_string_lossy())))
    });
    entry.brotli_bytes = brotli_path
        .as_ref()
        .and_then(|path| fs::metadata(path).ok().map(|meta| meta.len()));
    entry.transfer_bytes = entry.brotli_bytes.unwrap_or(entry.raw_bytes);
    Ok(())
}

fn patch_bindgen_worker_import(
    app_dir: &Path,
    manifest: &mut AssetManifest,
    compress: bool,
) -> Result<(), String> {
    let Some(bindgen_emitted_path) = manifest.emitted_path_for("bindgen.js").map(str::to_string)
    else {
        return Ok(());
    };
    if bindgen_emitted_path == "bindgen.js" {
        return Ok(());
    }

    let worker_path = app_dir.join("makepad_platform/web_worker.js");
    if !worker_path.is_file() {
        return Ok(());
    }
    let worker_source = fs::read_to_string(&worker_path)
        .map_err(|e| format!("Can't read {:?}: {}", worker_path, e))?;
    if !worker_source.contains("../bindgen.js") {
        return Ok(());
    }
    let patched = worker_source.replace("../bindgen.js", &format!("../{}", bindgen_emitted_path));
    fs::write(&worker_path, patched)
        .map_err(|e| format!("Can't write {:?}: {}", worker_path, e))?;
    if compress {
        brotli_compress(&worker_path);
    } else {
        remove_brotli_artifact(&worker_path);
    }
    if let Some(entry) = manifest
        .assets
        .iter_mut()
        .find(|entry| entry.logical_path == "makepad_platform/web_worker.js")
    {
        refresh_manifest_entry_sizes(app_dir, entry)?;
    }
    Ok(())
}

fn parse_crate_resource_path(path: &str) -> Option<(&str, &str)> {
    let mut split = path.splitn(2, ':');
    let crate_part = split.next()?;
    let file_path = split.next()?;
    Some((crate_part, file_path.trim_start_matches('/')))
}

fn normalize_dependency_file_path(path: &str) -> Option<String> {
    let mut stack: Vec<&str> = Vec::new();
    let normalized = path.replace('\\', "/");
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            other => stack.push(other),
        }
    }
    Some(stack.join("/"))
}

fn normalize_resource_path(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            std::path::Component::RootDir => out.push(comp.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            std::path::Component::Normal(part) => out.push(part),
        }
    }
    Some(out)
}

fn resolve_dependency_path_from_manifests(
    abs_path: &Path,
    default_crate_name: Option<&str>,
    default_manifest_path: Option<&Path>,
    manifests: &HashMap<String, PathBuf>,
) -> Option<(String, String)> {
    let abs_norm = normalize_resource_path(abs_path)?;
    let mut best: Option<(usize, String, String)> = None;

    let mut candidates = Vec::<(String, PathBuf)>::new();
    if let (Some(crate_name), Some(manifest_path)) = (default_crate_name, default_manifest_path) {
        candidates.push((crate_name.to_string(), manifest_path.to_path_buf()));
    }
    for (crate_name, manifest_path) in manifests {
        candidates.push((crate_name.clone(), manifest_path.clone()));
    }

    for (crate_name, manifest_path) in candidates {
        let Some(manifest_norm) = normalize_resource_path(&manifest_path) else {
            continue;
        };
        let Ok(rel) = abs_norm.strip_prefix(&manifest_norm) else {
            continue;
        };
        let Some(rel_norm) = normalize_dependency_file_path(&rel.to_string_lossy()) else {
            continue;
        };
        let dep_path = format!("{}/{}", crate_name, rel_norm);
        let manifest_len = manifest_norm.to_string_lossy().len();
        match &best {
            Some((best_len, _, _)) if *best_len >= manifest_len => {}
            _ => best = Some((manifest_len, dep_path, crate_name)),
        }
    }

    best.map(|(_, dep_path, crate_name)| (dep_path, crate_name))
}

fn build_crate_manifests(build_crate: &str, build_dir: &Path) -> HashMap<String, PathBuf> {
    let mut manifests = HashMap::new();
    if let Ok(build_crate_dir) = get_crate_dir(build_crate) {
        manifests.insert(build_crate.replace('-', "_"), build_crate_dir);
    }
    for (name, dep_dir) in get_crate_dep_dirs(build_crate, build_dir, WASM_TARGET_TRIPLE) {
        manifests.insert(name.replace('-', "_"), dep_dir);
    }
    manifests
}

fn read_wasm_web_metadata(build_crate: &str) -> Result<WasmWebMetadata, String> {
    let crate_dir = get_crate_dir(build_crate)?;
    let cargo_toml_path = crate_dir.join("Cargo.toml");
    let cargo_toml = fs::read_to_string(&cargo_toml_path)
        .map_err(|e| format!("Can't read {:?}: {}", cargo_toml_path, e))?;
    Ok(parse_wasm_web_metadata_toml(&cargo_toml))
}

fn parse_wasm_web_metadata_toml(cargo_toml: &str) -> WasmWebMetadata {
    let Ok(toml) = parse_toml(cargo_toml) else {
        return WasmWebMetadata::default();
    };

    let preserve = match toml.get("package.metadata.makepad.web.preserve") {
        Some(Toml::Array(values)) => values
            .iter()
            .filter_map(|value| match value {
                Toml::Str(value, _) => Some(value.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let full_i18n = matches!(
        toml.get("package.metadata.makepad.web.full_i18n"),
        Some(Toml::Bool(true, _))
    );

    WasmWebMetadata {
        preserve,
        full_i18n,
    }
}

fn remapped_small_font_source(source_path: &Path) -> Option<PathBuf> {
    let file_name = source_path.file_name()?.to_string_lossy();
    let remap_target = match file_name.as_ref() {
        "GoNotoKurrent-Bold.ttf" => "IBMPlexSans-SemiBold.ttf",
        "GoNotoKurrent-Regular.ttf" => "IBMPlexSans-Text.ttf",
        "LXGWWenKaiBold.ttf" => "IBMPlexSans-Text.ttf",
        "LXGWWenKaiRegular.ttf" => "IBMPlexSans-Text.ttf",
        "NotoColorEmoji.ttf" => "IBMPlexSans-Text.ttf",
        _ => return None,
    };
    Some(source_path.parent()?.join(remap_target))
}

fn add_selected_resource(
    selected: &mut HashMap<String, SelectedResource>,
    entry: SelectedResource,
) {
    match selected.get(&entry.logical_path) {
        Some(existing) if existing.direct_reference && !entry.direct_reference => {}
        Some(existing) if existing.reason == entry.reason => {}
        _ => {
            selected.insert(entry.logical_path.clone(), entry);
        }
    }
}

fn collect_rust_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "target" || name == ".git" {
                continue;
            }
            collect_rust_files_recursive(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn collect_rust_files(crate_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rust_files_recursive(crate_dir, &mut files);
    files.sort();
    files
}

fn decode_basic_rust_string_literal(literal: &str) -> Option<String> {
    if let Some((_, hashes)) = raw_string_prefix(literal.as_bytes(), 0) {
        let first_quote = literal.find('"')?;
        let closing_len = 1 + hashes;
        if literal.len() < first_quote + 1 + closing_len {
            return None;
        }
        return Some(literal[first_quote + 1..literal.len() - closing_len].to_string());
    }
    let bytes = literal.as_bytes();
    let mut start = 0usize;
    if bytes.first() == Some(&b'b') {
        start = 1;
    }
    if bytes.get(start) != Some(&b'"') || *bytes.last()? != b'"' {
        return None;
    }
    let inner = &literal[start + 1..literal.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let escaped = chars.next()?;
        match escaped {
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '0' => out.push('\0'),
            other => out.push(other),
        }
    }
    Some(out)
}

fn extract_crate_resource_literals_from_rust_file(source: &str) -> Result<Vec<String>, String> {
    let bytes = source.as_bytes();
    let mut i = 0usize;
    let mut literals = Vec::new();

    while i < bytes.len() {
        if let Some(end) = skip_non_code_segment(bytes, i)? {
            i = end;
            continue;
        }
        if !is_ident_start(bytes[i]) {
            i += utf8_char_len(bytes[i]);
            continue;
        }

        let ident_start = i;
        i += 1;
        while i < bytes.len() && is_ident_continue(bytes[i]) {
            i += 1;
        }
        if &source[ident_start..i] != "crate_resource" {
            continue;
        }

        let mut j = skip_ws_and_comments(bytes, i)?;
        if bytes.get(j) != Some(&b'(') {
            continue;
        }
        j += 1;
        j = skip_ws_and_comments(bytes, j)?;

        let literal_end = if let Some((_, _)) = raw_string_prefix(bytes, j) {
            skip_non_code_segment(bytes, j)?.unwrap_or(j)
        } else if bytes.get(j) == Some(&b'"')
            || (bytes.get(j) == Some(&b'b') && bytes.get(j + 1) == Some(&b'"'))
        {
            skip_non_code_segment(bytes, j)?.unwrap_or(j)
        } else {
            continue;
        };
        let literal = &source[j..literal_end];
        if let Some(decoded) = decode_basic_rust_string_literal(literal) {
            literals.push(decoded);
        }
        i = literal_end;
    }

    Ok(literals)
}

fn resolve_resource_spec(
    resource_spec: &str,
    build_crate_dir: &Path,
    build_crate_name: &str,
    manifests: &HashMap<String, PathBuf>,
) -> Option<(String, PathBuf, String)> {
    if let Some((crate_part, file_path)) = parse_crate_resource_path(resource_spec) {
        let (abs_path, default_crate_name, default_manifest_path) = if crate_part == "self" {
            let abs_path = normalize_resource_path(&build_crate_dir.join(file_path))?;
            (
                abs_path,
                Some(build_crate_name.to_string()),
                Some(build_crate_dir.to_path_buf()),
            )
        } else {
            let crate_name = crate_part.replace('-', "_");
            let manifest_path = manifests.get(&crate_name)?.clone();
            let abs_path = normalize_resource_path(&manifest_path.join(file_path))?;
            (abs_path, Some(crate_name), Some(manifest_path))
        };
        let (logical_path, crate_name) = resolve_dependency_path_from_manifests(
            &abs_path,
            default_crate_name.as_deref(),
            default_manifest_path.as_deref(),
            manifests,
        )?;
        return Some((logical_path, abs_path, crate_name));
    }

    let logical_path = normalize_dependency_file_path(resource_spec.trim_start_matches('/'))?;
    let mut parts = logical_path.splitn(2, '/');
    let crate_name = parts.next()?.to_string();
    let rel_path = parts.next()?;
    let manifest_path = manifests.get(&crate_name)?;
    let abs_path = normalize_resource_path(&manifest_path.join(rel_path))?;
    Some((logical_path, abs_path, crate_name))
}

fn add_curated_widget_web_resources(
    selected: &mut HashMap<String, SelectedResource>,
    manifests: &HashMap<String, PathBuf>,
    small_fonts: bool,
) {
    let Some(widgets_dir) = manifests.get("makepad_widgets") else {
        return;
    };
    let resources_dir = widgets_dir.join("resources");
    let font_names = [
        "IBMPlexSans-Text.ttf",
        "IBMPlexSans-SemiBold.ttf",
        "IBMPlexSans-Italic.ttf",
        "IBMPlexSans-BoldItalic.ttf",
        "LiberationMono-Regular.ttf",
        "fa-solid-900.ttf",
        "LXGWWenKaiRegular.ttf",
        "LXGWWenKaiBold.ttf",
        "NotoColorEmoji.ttf",
    ];

    for font_name in font_names {
        let logical_path = format!("makepad_widgets/resources/{font_name}");
        let source_path = resources_dir.join(font_name);
        let source_path = if small_fonts {
            remapped_small_font_source(&source_path).unwrap_or(source_path)
        } else {
            source_path
        };
        add_selected_resource(
            selected,
            SelectedResource {
                logical_path,
                source_path,
                crate_name: "makepad_widgets".to_string(),
                direct_reference: false,
                reason: "widget_web_default".to_string(),
            },
        );
    }

    let icons_dir = resources_dir.join("icons");
    if icons_dir.is_dir() {
        let mut icon_files = Vec::new();
        collect_resource_files_recursive(&icons_dir, &mut icon_files);
        for icon_path in icon_files {
            let Ok(rel) = icon_path.strip_prefix(&resources_dir) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            add_selected_resource(
                selected,
                SelectedResource {
                    logical_path: format!("makepad_widgets/resources/{rel}"),
                    source_path: icon_path,
                    crate_name: "makepad_widgets".to_string(),
                    direct_reference: false,
                    reason: "widget_web_default".to_string(),
                },
            );
        }
    }
}

fn collect_resource_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_resource_files_recursive(&path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
}

fn collect_shipping_resources(
    build_crate: &str,
    build_dir: &Path,
    config: &WasmConfig,
) -> Result<(Vec<SelectedResource>, WasmWebMetadata), String> {
    let build_crate_dir = get_crate_dir(build_crate)?;
    let build_crate_name = build_crate.replace('-', "_");
    let manifests = build_crate_manifests(build_crate, build_dir);
    let metadata = read_wasm_web_metadata(build_crate)?;
    let full_i18n = config.full_fonts || metadata.full_i18n;
    let mut selected = HashMap::<String, SelectedResource>::new();
    let build_rust_files = collect_rust_files(&build_crate_dir);

    for rust_file in build_rust_files {
        let Ok(source) = fs::read_to_string(&rust_file) else {
            continue;
        };
        for resource_spec in extract_crate_resource_literals_from_rust_file(&source)? {
            let Some((logical_path, abs_path, crate_name)) = resolve_resource_spec(
                &resource_spec,
                &build_crate_dir,
                &build_crate_name,
                &manifests,
            ) else {
                println!(
                    "Warning: unresolved crate_resource path while packaging web app: {}",
                    resource_spec
                );
                continue;
            };
            if !abs_path.is_file() {
                println!(
                    "Warning: referenced web resource does not exist and will be skipped: {} ({})",
                    resource_spec,
                    abs_path.display()
                );
                continue;
            }
            let source_path = if config.small_fonts && !full_i18n {
                remapped_small_font_source(&abs_path).unwrap_or(abs_path.clone())
            } else {
                abs_path.clone()
            };
            add_selected_resource(
                &mut selected,
                SelectedResource {
                    logical_path,
                    source_path,
                    crate_name,
                    direct_reference: true,
                    reason: "direct_resource".to_string(),
                },
            );
        }
    }

    for preserve in &metadata.preserve {
        let Some((logical_path, abs_path, crate_name)) =
            resolve_resource_spec(preserve, &build_crate_dir, &build_crate_name, &manifests)
        else {
            println!(
                "Warning: unable to resolve preserved web resource path: {}",
                preserve
            );
            continue;
        };
        if !abs_path.is_file() {
            println!(
                "Warning: preserved web resource does not exist and will be skipped: {} ({})",
                preserve,
                abs_path.display()
            );
            continue;
        }
        let source_path = if config.small_fonts && !full_i18n {
            remapped_small_font_source(&abs_path).unwrap_or(abs_path.clone())
        } else {
            abs_path.clone()
        };
        add_selected_resource(
            &mut selected,
            SelectedResource {
                logical_path,
                source_path,
                crate_name,
                direct_reference: false,
                reason: "preserve".to_string(),
            },
        );
    }

    add_curated_widget_web_resources(&mut selected, &manifests, config.small_fonts && !full_i18n);

    let mut ordered = selected.into_values().collect::<Vec<_>>();
    ordered.sort_by(|a, b| a.logical_path.cmp(&b.logical_path));
    Ok((ordered, metadata))
}

fn copy_resource_asset(
    app_dir: &Path,
    asset: &SelectedResource,
    compress: bool,
) -> Result<PendingAsset, String> {
    let dest_path = app_dir.join(&asset.logical_path);
    cp(&asset.source_path, &dest_path, false)?;
    if compress {
        brotli_compress(&dest_path);
    } else {
        remove_brotli_artifact(&dest_path);
    }
    Ok(PendingAsset {
        logical_path: asset.logical_path.clone(),
        emitted_path: asset.logical_path.clone(),
        kind: "resource".to_string(),
        content_type: mime_type_for_path(&asset.logical_path)
            .unwrap_or("application/octet-stream")
            .to_string(),
        startup_blocking: false,
        direct_reference: asset.direct_reference,
        crate_name: Some(asset.crate_name.clone()),
        reason: asset.reason.clone(),
    })
}

const WASM_TARGET_TRIPLE: &str = "wasm32-unknown-unknown";
const WASM_TARGET_SPEC_FEATURES: &str = "+atomics,+bulk-memory,+mutable-globals";
const WASM_RUSTFLAGS_THREADED: &str = "-C codegen-units=1 -C debuginfo=0 -C link-arg=--export=__stack_pointer -C link-arg=--compress-relocations -C link-arg=--strip-debug -C link-arg=--shared-memory -C link-arg=--max-memory=2147483648 -C link-arg=--import-memory -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size -C link-arg=--export=__tls_align -C link-arg=--export=__tls_base -C opt-level=z";
const WASM_RUSTFLAGS_SINGLE_THREADED: &str =
    "-C codegen-units=1 -C debuginfo=0 -C link-arg=--export=__stack_pointer -C link-arg=--compress-relocations -C link-arg=--strip-debug -C opt-level=z";

fn build_wasm_target_spec(cwd: &PathBuf, threaded: bool) -> Result<PathBuf, String> {
    let target_spec_dir = if threaded {
        cwd.join("target/makepad-wasm-target/threads")
    } else {
        cwd.join("target/makepad-wasm-target/single")
    };
    mkdir(&target_spec_dir)?;
    let target_spec_path = target_spec_dir.join(format!("{WASM_TARGET_TRIPLE}.json"));

    let mut target_spec = shell_env_cap(
        &[],
        cwd,
        "rustup",
        &[
            "run",
            "nightly",
            "rustc",
            "-Z",
            "unstable-options",
            "--print",
            "target-spec-json",
            "--target",
            WASM_TARGET_TRIPLE,
        ],
    )?;

    if target_spec.contains("\"features\"") {
        return Err(
            "Built-in wasm target spec unexpectedly contains \"features\"; update cargo_makepad wasm target generation."
                .to_string(),
        );
    }

    if threaded {
        let insert_at = target_spec
            .rfind('}')
            .ok_or_else(|| "Unable to parse wasm target spec JSON from rustc".to_string())?;
        target_spec.insert_str(
            insert_at,
            &format!(",\n  \"features\": \"{WASM_TARGET_SPEC_FEATURES}\"\n"),
        );
    }

    fs::write(&target_spec_path, target_spec).map_err(|e| {
        format!(
            "Can't write wasm target spec {:?}: {:?}",
            target_spec_path, e
        )
    })?;
    Ok(target_spec_path)
}

pub fn build(config: WasmConfig, args: &[String]) -> Result<WasmBuildResult, String> {
    let build_crate = get_build_crate_from_args(args)?;
    let cwd = std::env::current_dir().unwrap();
    let profile = get_profile_from_args(&args);
    let app_dir = cwd.join(format!("target/makepad-wasm-app/{profile}/{}", build_crate));
    let build_dir = cwd.join(format!("target/{WASM_TARGET_TRIPLE}/{profile}"));
    let wasm_target_spec = build_wasm_target_spec(&cwd, config.threads)?;
    let target_arg = format!("--target={}", wasm_target_spec.display());

    let base_args = vec![
        "run".to_string(),
        "nightly".to_string(),
        "cargo".to_string(),
        "build".to_string(),
        target_arg,
        "-Z".to_string(),
        "json-target-spec".to_string(),
        "-Z".to_string(),
        "build-std=panic_abort,std".to_string(),
    ];

    let mut args_out = base_args;
    for arg in args {
        args_out.push(arg.clone());
    }
    let args_out_refs: Vec<&str> = args_out.iter().map(|arg| arg.as_str()).collect();

    let rustflags = if config.threads {
        WASM_RUSTFLAGS_THREADED
    } else {
        WASM_RUSTFLAGS_SINGLE_THREADED
    };
    let mut env = vec![("RUSTFLAGS", rustflags), ("MAKEPAD", "lines")];
    // `profile.small` with LTO enabled miscompiles single-threaded wasm in the script VM.
    if profile == "small" && !config.threads {
        env.push(("CARGO_PROFILE_SMALL_LTO", "off"));
    }

    shell_env(&env, &cwd, "rustup", &args_out_refs)?;

    if app_dir.exists() {
        fs::remove_dir_all(&app_dir)
            .map_err(|e| format!("Can't clean wasm app dir {:?}: {}", app_dir, e))?;
    }
    mkdir(&app_dir)?;

    let mut pending_assets = Vec::<PendingAsset>::new();
    let resources = get_crate_dep_dirs(build_crate, &build_dir, WASM_TARGET_TRIPLE);
    for (name, dep_dir) in resources.iter() {
        if name == "makepad-wasm-bridge" && config.bindgen {
            cp_brotli(
                &dep_dir.join("src/wasm_bridge.js"),
                &app_dir.join("makepad_wasm_bridge/wasm_bridge.js"),
                false,
                config.brotli,
            )?;
            if config.shipping_build {
                pending_assets.push(PendingAsset {
                    logical_path: "makepad_wasm_bridge/wasm_bridge.js".to_string(),
                    emitted_path: "makepad_wasm_bridge/wasm_bridge.js".to_string(),
                    kind: "javascript".to_string(),
                    content_type: "text/javascript".to_string(),
                    startup_blocking: true,
                    direct_reference: false,
                    crate_name: Some("makepad_wasm_bridge".to_string()),
                    reason: "runtime_support".to_string(),
                });
            }
        }
        if name == "makepad-platform" {
            cp_brotli(
                &dep_dir.join("src/os/web/audio_worklet.js"),
                &app_dir.join("makepad_platform/audio_worklet.js"),
                false,
                config.brotli,
            )?;
            if config.shipping_build {
                pending_assets.push(PendingAsset {
                    logical_path: "makepad_platform/audio_worklet.js".to_string(),
                    emitted_path: "makepad_platform/audio_worklet.js".to_string(),
                    kind: "javascript".to_string(),
                    content_type: "text/javascript".to_string(),
                    startup_blocking: false,
                    direct_reference: false,
                    crate_name: Some("makepad_platform".to_string()),
                    reason: "runtime_support".to_string(),
                });
            }

            cp_brotli(
                &dep_dir.join("src/os/web/web_gl.js"),
                &app_dir.join("makepad_platform/web_gl.js"),
                false,
                config.brotli,
            )?;
            if config.shipping_build {
                pending_assets.push(PendingAsset {
                    logical_path: "makepad_platform/web_gl.js".to_string(),
                    emitted_path: "makepad_platform/web_gl.js".to_string(),
                    kind: "javascript".to_string(),
                    content_type: "text/javascript".to_string(),
                    startup_blocking: true,
                    direct_reference: false,
                    crate_name: Some("makepad_platform".to_string()),
                    reason: "runtime_support".to_string(),
                });
            }

            if config.bindgen {
                let jsfile = dep_dir.join("src/os/web/web_worker.js");
                let js = std::fs::read_to_string(&jsfile)
                    .map_err(|e| format!("Unable to find web.js {e:?}"))?;
                let tmp = build_dir.join("web_worker.js");
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .create(true)
                    .open(&tmp)
                    .unwrap();
                file.write(format!("import init from '../bindgen.js';\n{js}").as_bytes())
                    .unwrap();
                cp_brotli(
                    &tmp,
                    &app_dir.join("makepad_platform/web_worker.js"),
                    false,
                    config.brotli,
                )?;
            } else {
                cp_brotli(
                    &dep_dir.join("src/os/web/web_worker.js"),
                    &app_dir.join("makepad_platform/web_worker.js"),
                    false,
                    config.brotli,
                )?;
            }
            if config.shipping_build {
                pending_assets.push(PendingAsset {
                    logical_path: "makepad_platform/web_worker.js".to_string(),
                    emitted_path: "makepad_platform/web_worker.js".to_string(),
                    kind: "javascript".to_string(),
                    content_type: "text/javascript".to_string(),
                    startup_blocking: false,
                    direct_reference: false,
                    crate_name: Some("makepad_platform".to_string()),
                    reason: "runtime_support".to_string(),
                });
            }

            cp_brotli(
                &dep_dir.join("src/os/web/web.js"),
                &app_dir.join("makepad_platform/web.js"),
                false,
                config.brotli,
            )?;
            if config.shipping_build {
                pending_assets.push(PendingAsset {
                    logical_path: "makepad_platform/web.js".to_string(),
                    emitted_path: "makepad_platform/web.js".to_string(),
                    kind: "javascript".to_string(),
                    content_type: "text/javascript".to_string(),
                    startup_blocking: false,
                    direct_reference: false,
                    crate_name: Some("makepad_platform".to_string()),
                    reason: "runtime_support".to_string(),
                });
            }

            if config.hot_reload {
                cp_brotli(
                    &dep_dir.join("src/os/web/auto_reload.js"),
                    &app_dir.join("makepad_platform/auto_reload.js"),
                    false,
                    config.brotli,
                )?;
            }

            cp_brotli(
                &dep_dir.join("src/os/web/full_canvas.css"),
                &app_dir.join("makepad_platform/full_canvas.css"),
                false,
                config.brotli,
            )?;
            if config.shipping_build {
                pending_assets.push(PendingAsset {
                    logical_path: "makepad_platform/full_canvas.css".to_string(),
                    emitted_path: "makepad_platform/full_canvas.css".to_string(),
                    kind: "stylesheet".to_string(),
                    content_type: "text/css".to_string(),
                    startup_blocking: true,
                    direct_reference: false,
                    crate_name: Some("makepad_platform".to_string()),
                    reason: "runtime_support".to_string(),
                });
            }
        }
    }

    if config.shipping_build {
        let (selected_resources, metadata) =
            collect_shipping_resources(build_crate, &build_dir, &config)?;
        if metadata.full_i18n || config.full_fonts {
            println!("Shipping web build: full i18n font payload enabled");
        }
        for asset in &selected_resources {
            pending_assets.push(copy_resource_asset(&app_dir, asset, config.brotli)?);
        }
    } else {
        let build_crate_dir = get_crate_dir(build_crate)?;
        let local_resources_path = build_crate_dir.join("resources");
        if local_resources_path.is_dir() {
            let underscore_build_crate = build_crate.replace('-', "_");
            let dst_dir = app_dir.join(underscore_build_crate).join("resources");
            mkdir(&dst_dir)?;
            walk_all(
                &local_resources_path,
                &dst_dir,
                &mut |source_path, dest_dir| {
                    let source_file_name = source_path
                        .file_name()
                        .ok_or_else(|| format!("Unable to get filename for {:?}", source_path))?
                        .to_string_lossy()
                        .to_string();
                    let dest_path = dest_dir.join(&source_file_name);
                    cp(&source_path, &dest_path, false)?;
                    if config.brotli {
                        brotli_compress(&dest_path);
                    } else {
                        remove_brotli_artifact(&dest_path);
                    }
                    Ok(())
                },
            )?;
        }

        for (name, dep_dir) in resources.iter() {
            let name = name.replace("-", "_");
            let resources_path = dep_dir.join("resources");
            let mut rename: HashMap<String, String> = HashMap::new();

            if config.small_fonts {
                rename.insert(
                    "GoNotoKurrent-Bold.ttf".into(),
                    "IBMPlexSans-SemiBold.ttf".into(),
                );
                rename.insert(
                    "GoNotoKurrent-Regular.ttf".into(),
                    "IBMPlexSans-Text.ttf".into(),
                );
                rename.insert("LXGWWenKaiBold.ttf".into(), "IBMPlexSans-Text.ttf".into());
                rename.insert(
                    "LXGWWenKaiRegular.ttf".into(),
                    "IBMPlexSans-Text.ttf".into(),
                );
                rename.insert("NotoColorEmoji.ttf".into(), "IBMPlexSans-Text.ttf".into());
            }

            if resources_path.is_dir() {
                let dst_dir = app_dir.join(&name).join("resources");
                mkdir(&dst_dir)?;
                walk_all(&resources_path, &dst_dir, &mut |source_path, dest_dir| {
                    let source_file_name = source_path
                        .file_name()
                        .ok_or_else(|| format!("Unable to get filename for {:?}", source_path))?
                        .to_string_lossy()
                        .to_string();
                    let source_path2 = if let Some(tgt) = rename.get(&source_file_name) {
                        &source_path.parent().unwrap().join(tgt)
                    } else {
                        source_path
                    };
                    let dest_path = dest_dir.join(&source_file_name);
                    cp(&source_path2, &dest_path, false)?;
                    if config.brotli {
                        brotli_compress(&dest_path);
                    } else {
                        remove_brotli_artifact(&dest_path);
                    }
                    Ok(())
                })?;
            }
        }
    }

    let wasm_source = if config.bindgen {
        shell(
            build_dir.as_path(),
            "wasm-bindgen",
            &[
                &format!("{build_crate}.wasm"),
                "--out-dir=.",
                "--out-name=bindgen",
                "--target=web",
                "--no-typescript",
            ],
        )?;
        let jsfile = build_dir.join("bindgen.js");
        let patched = std::fs::read_to_string(&jsfile)
            .map_err(|e| format!("Unable to find wasm-bidngen generated file {e:?}"))?
            .replace("import * as __wbg_star0 from 'env';", "")
            .replace("imports['env'] = __wbg_star0;", "")
            .replace("return wasm;\n}", "return instance;\n}")
            .replace(
                "__wbg_init(module_or_path, memory) {",
                "__wbg_init(module_or_path, env) {let memory;",
            )
            .replace(
                "__wbg_init(module_or_path) {",
                "__wbg_init(module_or_path, env) {let memory;",
            )
            .replace(
                "imports = __wbg_get_imports();",
                "imports = __wbg_get_imports(); imports.env = env;",
            );
        std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&jsfile)
            .unwrap()
            .write(patched.as_bytes())
            .unwrap();
        cp_brotli(&jsfile, &app_dir.join("bindgen.js"), false, config.brotli)?;
        if config.shipping_build {
            pending_assets.push(PendingAsset {
                logical_path: "bindgen.js".to_string(),
                emitted_path: "bindgen.js".to_string(),
                kind: "javascript".to_string(),
                content_type: "text/javascript".to_string(),
                startup_blocking: true,
                direct_reference: false,
                crate_name: None,
                reason: "runtime_support".to_string(),
            });
        }

        build_dir.join("bindgen_bg.wasm")
    } else {
        build_dir.join(format!("{}.wasm", build_crate))
    };

    let wasm_dest = app_dir.join(format!("{}.wasm", build_crate));
    let mut output = if config.optimize_size || config.strip {
        let data = fs::read(&wasm_source)
            .map_err(|_| format!("Cannot read wasm file {:?}", wasm_source))?;

        if config.optimize_size {
            let report = wasm_size_report(&data)
                .map_err(|_| format!("Cannot parse wasm {:?}", wasm_source))?;
            print_wasm_size_report(&report);
            wasm_optimize_size(&data).map_err(|_| format!("Cannot parse wasm {:?}", wasm_source))?
        } else {
            wasm_strip_custom_sections(&data)
                .map_err(|_| format!("Cannot parse wasm {:?}", wasm_source))?
        }
    } else {
        fs::read(&wasm_source).map_err(|_| format!("Cannot read wasm file {:?}", wasm_source))?
    };

    if config.wasm_opt {
        output = try_wasm_opt(&output, &cwd);
    }

    // `--split` implies function splitting as part of the higher-level split pipeline.
    let split_functions_enabled = config.split || config.split_functions;

    // Function splitting: split large functions into primary (stubs) + secondary (real bodies)
    let secondary_wasm_dest = app_dir.join(format!("{}.secondary.wasm", build_crate));
    let mut defer_secondary_wasm = false;
    let mut auto_split_outcome = AutoSplitOutcome::NotAttempted;
    let secondary_wasm_path = if split_functions_enabled {
        if config.bindgen {
            return Err(if config.split {
                "--split is not supported together with --bindgen".to_string()
            } else {
                "--split-functions is not supported together with --bindgen".to_string()
            });
        }
        let result = if config.split_auto && config.split {
            let cold_target = output.len().saturating_sub(1);
            let cold_result =
                wasm_split_functions_to_target_primary_size_cold(&output, cold_target).map_err(
                    |e| {
                        format!(
                            "Cannot auto split wasm functions {:?}: {:?}",
                            wasm_source, e
                        )
                    },
                )?;
            if cold_result.split_count > 0 && cold_result.primary_wasm.len() < output.len() {
                defer_secondary_wasm = true;
                auto_split_outcome = AutoSplitOutcome::Deferred;
                cold_result
            } else {
                let fallback = wasm_split_functions(&output, config.split_functions_threshold)
                    .map_err(|e| {
                        format!("Cannot split wasm functions {:?}: {:?}", wasm_source, e)
                    })?;
                if fallback.split_count > 0 {
                    auto_split_outcome = AutoSplitOutcome::StartupPathFallback;
                }
                fallback
            }
        } else {
            wasm_split_functions(&output, config.split_functions_threshold)
                .map_err(|e| format!("Cannot split wasm functions {:?}: {:?}", wasm_source, e))?
        };
        if result.split_count == 0 {
            if config.split_auto && config.split {
                println!(
                    "Function split: no selectable functions found for automatic split, skipping"
                );
            } else {
                println!(
                    "Function split: no functions above threshold ({} bytes), skipping",
                    config.split_functions_threshold
                );
            }
            let _ = fs::remove_file(&secondary_wasm_dest);
            remove_brotli_artifact(&secondary_wasm_dest);
            None
        } else {
            if config.split_auto && config.split {
                println!(
                    "Function split: {} of {} functions split (automatic mode)",
                    result.split_count, result.total_functions
                );
                match auto_split_outcome {
                    AutoSplitOutcome::Deferred => {
                        println!("  mode: cold-first split, secondary deferred");
                    }
                    AutoSplitOutcome::StartupPathFallback => {
                        println!("  mode: automatic fallback split, secondary remains on the startup path");
                    }
                    AutoSplitOutcome::NotAttempted => {}
                }
            } else {
                println!(
                    "Function split: {} of {} functions split (threshold: {} bytes)",
                    result.split_count, result.total_functions, config.split_functions_threshold
                );
            }
            println!("  primary:   {} bytes", result.primary_wasm.len());
            println!("  secondary: {} bytes", result.secondary_wasm.len());
            output = result.primary_wasm;
            fs::write(&secondary_wasm_dest, &result.secondary_wasm)
                .map_err(|e| format!("Can't write file {:?} {:?}", secondary_wasm_dest, e))?;
            if config.brotli {
                brotli_compress(&secondary_wasm_dest);
            } else {
                remove_brotli_artifact(&secondary_wasm_dest);
            }
            let secondary_startup_blocking = !defer_secondary_wasm;
            if config.shipping_build {
                pending_assets.push(PendingAsset {
                    logical_path: format!("{}.secondary.wasm", build_crate),
                    emitted_path: format!("{}.secondary.wasm", build_crate),
                    kind: "wasm".to_string(),
                    content_type: "application/wasm".to_string(),
                    startup_blocking: secondary_startup_blocking,
                    direct_reference: false,
                    crate_name: None,
                    reason: "split_secondary".to_string(),
                });
            }
            Some(format!("{}.secondary.wasm", build_crate))
        }
    } else {
        let _ = fs::remove_file(&secondary_wasm_dest);
        remove_brotli_artifact(&secondary_wasm_dest);
        None
    };

    let split_data_dest = app_dir.join(format!("{}.data.bin", build_crate));
    let mut split_data_bytes = None;
    let mut split_brotli_bytes = None;
    let mut split_data_active_only = false;
    let split_data_path = if config.split {
        if config.bindgen {
            return Err("--split is not supported together with --bindgen".to_string());
        }
        let split = wasm_split_data_segments(&output)
            .map_err(|_| format!("Cannot split wasm data section {:?}", wasm_source))?;
        if split.segment_count == 0 {
            let _ = fs::remove_file(&split_data_dest);
            remove_brotli_artifact(&split_data_dest);
            None
        } else {
            print_wasm_split_report(
                split.primary_wasm.len(),
                split.split_data.len(),
                split.segment_count,
            );
            println!(
                "  active segments: {} | passive segments: {}",
                split.active_segment_count, split.passive_segment_count
            );
            split_data_active_only = split.passive_segment_count == 0;
            output = split.primary_wasm;
            split_data_bytes = Some(split.split_data.len());
            fs::write(&split_data_dest, &split.split_data)
                .map_err(|e| format!("Can't write file {:?} {:?} ", split_data_dest, e))?;
            if config.brotli {
                split_brotli_bytes = Some(brotli_compress(&split_data_dest));
            } else {
                remove_brotli_artifact(&split_data_dest);
            }
            if config.shipping_build {
                pending_assets.push(PendingAsset {
                    logical_path: format!("{}.data.bin", build_crate),
                    emitted_path: format!("{}.data.bin", build_crate),
                    kind: "binary".to_string(),
                    content_type: "application/octet-stream".to_string(),
                    startup_blocking: true,
                    direct_reference: false,
                    crate_name: None,
                    reason: "split_data".to_string(),
                });
            }
            Some(format!("{}.data.bin", build_crate))
        }
    } else {
        let _ = fs::remove_file(&split_data_dest);
        remove_brotli_artifact(&split_data_dest);
        None
    };

    fs::write(&wasm_dest, output)
        .map_err(|e| format!("Can't write file {:?} {:?} ", wasm_dest, e))?;
    let wasm_bytes = fs::metadata(&wasm_dest)
        .map_err(|e| format!("Can't stat file {:?} {:?} ", wasm_dest, e))?
        .len() as usize;
    let wasm_brotli_bytes = if config.brotli {
        Some(brotli_compress(&wasm_dest))
    } else {
        remove_brotli_artifact(&wasm_dest);
        None
    };
    if config.shipping_build {
        pending_assets.push(PendingAsset {
            logical_path: format!("{}.wasm", build_crate),
            emitted_path: format!("{}.wasm", build_crate),
            kind: "wasm".to_string(),
            content_type: "application/wasm".to_string(),
            startup_blocking: true,
            direct_reference: false,
            crate_name: None,
            reason: "startup_wasm".to_string(),
        });
    }

    let mut asset_manifest = if config.shipping_build {
        Some(finalize_pending_assets(
            &app_dir,
            build_crate,
            &profile,
            config.threads,
            config.shipping_build,
            &mut pending_assets,
        )?)
    } else {
        None
    };

    if let Some(manifest) = asset_manifest.as_mut() {
        patch_bindgen_worker_import(&app_dir, manifest, config.brotli)?;
    }

    let html_assets = if let Some(manifest) = asset_manifest.as_ref() {
        HtmlAssetPaths {
            wasm_path: manifest
                .emitted_path_for(&format!("{}.wasm", build_crate))
                .unwrap_or(&format!("{}.wasm", build_crate))
                .to_string(),
            split_data_path: split_data_path
                .as_deref()
                .and_then(|path| {
                    manifest
                        .emitted_path_for(path)
                        .map(|value| value.to_string())
                })
                .or(split_data_path.clone()),
            split_data_active_only,
            secondary_wasm_path: secondary_wasm_path
                .as_deref()
                .and_then(|path| {
                    manifest
                        .emitted_path_for(path)
                        .map(|value| value.to_string())
                })
                .or(secondary_wasm_path.clone()),
            defer_secondary_wasm,
            web_gl_js_path: manifest
                .emitted_path_for("makepad_platform/web_gl.js")
                .unwrap_or("makepad_platform/web_gl.js")
                .to_string(),
            full_canvas_css_path: manifest
                .emitted_path_for("makepad_platform/full_canvas.css")
                .unwrap_or("makepad_platform/full_canvas.css")
                .to_string(),
            bindgen_js_path: if config.bindgen {
                Some(
                    manifest
                        .emitted_path_for("bindgen.js")
                        .unwrap_or("bindgen.js")
                        .to_string(),
                )
            } else {
                None
            },
            wasm_bridge_js_path: if config.bindgen {
                Some(
                    manifest
                        .emitted_path_for("makepad_wasm_bridge/wasm_bridge.js")
                        .unwrap_or("makepad_wasm_bridge/wasm_bridge.js")
                        .to_string(),
                )
            } else {
                None
            },
        }
    } else {
        HtmlAssetPaths {
            wasm_path: format!("{}.wasm", build_crate),
            split_data_path,
            split_data_active_only,
            secondary_wasm_path,
            defer_secondary_wasm,
            web_gl_js_path: "makepad_platform/web_gl.js".to_string(),
            full_canvas_css_path: "makepad_platform/full_canvas.css".to_string(),
            bindgen_js_path: config.bindgen.then(|| "bindgen.js".to_string()),
            wasm_bridge_js_path: config
                .bindgen
                .then(|| "makepad_wasm_bridge/wasm_bridge.js".to_string()),
        }
    };

    let index_path = app_dir.join("index.html");
    let html = generate_html(build_crate, &html_assets, &config);
    fs::write(&index_path, &html).map_err(|e| format!("Can't write {:?} {:?} ", index_path, e))?;
    if config.brotli {
        brotli_compress(&index_path);
    } else {
        remove_brotli_artifact(&index_path);
    }

    if let Some(manifest) = asset_manifest.as_mut() {
        let html_raw_bytes = fs::metadata(&index_path)
            .map_err(|e| format!("Can't stat {:?}: {}", index_path, e))?
            .len();
        let html_brotli_bytes = if config.brotli {
            fs::metadata(app_dir.join("index.html.br"))
                .ok()
                .map(|meta| meta.len())
        } else {
            None
        };
        manifest.assets.push(AssetManifestEntry {
            logical_path: "index.html".to_string(),
            emitted_path: "index.html".to_string(),
            kind: "html".to_string(),
            content_type: "text/html".to_string(),
            cache_control: short_cache_control(),
            startup_blocking: false,
            hashed: false,
            direct_reference: false,
            crate_name: None,
            reason: "entrypoint".to_string(),
            raw_bytes: html_raw_bytes,
            transfer_bytes: html_brotli_bytes.unwrap_or(html_raw_bytes),
            brotli_bytes: html_brotli_bytes,
        });

        let manifest_path = app_dir.join("asset-manifest.json");
        write_json_file(&manifest_path, manifest, config.brotli)?;
        let perf_report = build_web_perf_report(manifest);
        let perf_report_path = app_dir.join("web-perf-report.json");
        write_json_file(&perf_report_path, &perf_report, config.brotli)?;
    }

    if let Some(wasm_brotli_bytes) = wasm_brotli_bytes {
        print_brotli_size_report(
            wasm_bytes,
            wasm_brotli_bytes,
            split_data_bytes,
            split_brotli_bytes,
        );
    }
    println!("Created wasm package: {:?}", app_dir);
    if config.threads {
        println!("Copy this directory to any webserver, and serve with atleast these headers:");
        println!("Cross-Origin-Embedder-Policy: require-corp");
        println!("Cross-Origin-Opener-Policy: same-origin");
    } else {
        println!("Copy this directory to any webserver.");
        println!("This single-threaded wasm build does not require COOP/COEP headers.");
    }
    println!("Files need to be served with these mime types: ");
    println!("*.html => text/html");
    println!("*.wasm => application/wasm");
    println!("*.css => text/css");
    println!("*.js => text/javascript");
    println!("*.ttf => application/ttf");
    println!("*.png => image/png");
    println!("*.glb => data/binary");
    println!("*.jpg => image/jpg");
    println!("*.svg => image/svg+xml");
    println!("*.md => text/markdown");
    println!("*.bin => application/octet-stream");
    Ok(WasmBuildResult {
        app_dir,
        serve_mode: if config.shipping_build {
            WasmServeMode::Shipping
        } else {
            WasmServeMode::Dev
        },
        asset_manifest,
    })
}

pub fn run(config: WasmConfig, args: &[String]) -> Result<(), String> {
    let build_crate = get_build_crate_from_args(args)?.to_string();
    let profile = get_profile_from_args(args);
    let workspace_root = std::env::current_dir()
        .map_err(|err| format!("failed to resolve workspace root: {}", err))?;
    let build_dir = workspace_root.join(format!("target/{WASM_TARGET_TRIPLE}/{profile}"));
    let port = config.port.unwrap_or(8010);
    let mut run_config = config;
    run_config.hot_reload = true;

    let result = build(run_config, args)?;
    let hot_reload_plan = collect_wasm_hot_reload_watch_plan(&build_crate, &build_dir);
    let mut ownership_guard = WasmServerOwnershipGuard::prepare(
        &workspace_root,
        &build_crate,
        &profile,
        port,
        config.lan,
    )?;
    let rebuild_plan = WasmRebuildPlan {
        config: run_config,
        args: args.to_vec(),
    };
    start_wasm_server(
        result.app_dir,
        config.lan,
        port,
        config.threads,
        result.serve_mode,
        result.asset_manifest,
        hot_reload_plan,
        Some(rebuild_plan),
        &mut ownership_guard,
    )?;
    Ok(())
}

pub fn ship(config: WasmConfig, args: &[String]) -> Result<(), String> {
    let build_crate = get_build_crate_from_args(args)?.to_string();
    let profile = get_profile_from_args(args);
    let workspace_root = std::env::current_dir()
        .map_err(|err| format!("failed to resolve workspace root: {}", err))?;
    let port = config.port.unwrap_or(8010);

    let result = build(config, args)?;
    if !config.serve {
        return Ok(());
    }

    let mut ownership_guard = WasmServerOwnershipGuard::prepare(
        &workspace_root,
        &build_crate,
        &profile,
        port,
        config.lan,
    )?;
    start_wasm_server(
        result.app_dir,
        config.lan,
        port,
        config.threads,
        result.serve_mode,
        result.asset_manifest,
        None,
        None,
        &mut ownership_guard,
    )?;
    Ok(())
}

fn from_hex_digit(v: u8) -> Option<u8> {
    match v {
        b'0'..=b'9' => Some(v - b'0'),
        b'a'..=b'f' => Some(v - b'a' + 10),
        b'A'..=b'F' => Some(v - b'A' + 10),
        _ => None,
    }
}

fn decode_query_component(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) =
                    (from_hex_digit(bytes[i + 1]), from_hex_digit(bytes[i + 2]))
                {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            value => {
                out.push(value);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn client_accepts_brotli(header: Option<&str>) -> bool {
    let Some(header) = header else {
        return false;
    };
    for encoding in header.split(',') {
        let mut parts = encoding.trim().split(';');
        let name = parts.next().unwrap_or("").trim();
        if !name.eq_ignore_ascii_case("br") {
            continue;
        }
        let mut allowed = true;
        for part in parts {
            let part = part.trim();
            if let Some(q) = part.strip_prefix("q=") {
                allowed = q.parse::<f32>().map(|value| value > 0.0).unwrap_or(true);
            }
        }
        if allowed {
            return true;
        }
    }
    false
}

fn cache_control_for_request(
    serve_mode: WasmServeMode,
    request_path: &str,
    asset_manifest: Option<&AssetManifest>,
) -> String {
    match serve_mode {
        WasmServeMode::Dev => "no-store, must-revalidate".to_string(),
        WasmServeMode::Shipping => {
            if matches!(
                request_path,
                "index.html" | "asset-manifest.json" | "web-perf-report.json"
            ) {
                short_cache_control()
            } else if let Some(asset) =
                asset_manifest.and_then(|manifest| manifest.asset_for_request_path(request_path))
            {
                asset.cache_control.clone()
            } else {
                short_cache_control()
            }
        }
    }
}

fn collect_wasm_hot_reload_watch_plan(
    build_crate: &str,
    build_dir: &Path,
) -> Option<WasmHotReloadPlan> {
    let mut crate_roots = BTreeMap::<String, PathBuf>::new();
    let build_crate_dir = get_crate_dir(build_crate).ok()?;
    crate_roots.insert(build_crate.to_string(), build_crate_dir);

    for (name, path) in get_crate_dep_dirs(build_crate, build_dir, WASM_TARGET_TRIPLE) {
        if should_watch_wasm_crate(build_crate, &name) {
            crate_roots.entry(name).or_insert(path);
        }
    }

    let mut roots = BTreeMap::<String, WatchRoot>::new();
    let mut files_by_root = HashMap::<String, Vec<String>>::new();
    let mut initial_contents = HashMap::<String, String>::new();
    let mut initial_script_mod_bodies = HashMap::<String, Vec<String>>::new();

    for (name, crate_dir) in crate_roots {
        if !should_watch_wasm_crate(build_crate, &name) {
            continue;
        }

        let files = collect_script_mod_files_in_crate(&crate_dir);
        if files.is_empty() {
            continue;
        }

        let mount = normalize_path_string(&crate_dir);
        roots.entry(mount.clone()).or_insert_with(|| WatchRoot {
            mount: mount.clone(),
            path: crate_dir.clone(),
        });

        for file_name in files {
            let Ok(content) = fs::read_to_string(&file_name) else {
                continue;
            };
            let script_mod_bodies = extract_script_mod_bodies_from_rust_file(&content)
                .unwrap_or_else(|_| vec![content.clone()]);
            initial_contents.entry(file_name.clone()).or_insert(content);
            initial_script_mod_bodies
                .entry(file_name.clone())
                .or_insert(script_mod_bodies);
            files_by_root
                .entry(mount.clone())
                .or_default()
                .push(file_name);
        }
    }

    if initial_contents.is_empty() {
        return None;
    }

    for files in files_by_root.values_mut() {
        files.sort();
        files.dedup();
    }

    Some(WasmHotReloadPlan {
        roots: roots.into_values().collect(),
        files_by_root,
        initial_contents,
        initial_script_mod_bodies,
    })
}

fn should_watch_wasm_crate(build_crate: &str, crate_name: &str) -> bool {
    crate_name == build_crate
        || !matches!(
            crate_name,
            "makepad-platform" | "makepad-script" | "makepad-draw"
        )
}

fn collect_script_mod_files_in_crate(crate_dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_script_mod_files_recursive(crate_dir, &mut files);
    files.sort();
    files.dedup();
    files
}

fn collect_script_mod_files_recursive(dir: &Path, files: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "target" || name == ".git" {
                continue;
            }
            collect_script_mod_files_recursive(&path, files);
            continue;
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }

        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        if source.contains("script_mod!") {
            files.push(normalize_path_string(&path));
        }
    }
}

fn start_wasm_hot_reload_watcher(
    plan: WasmHotReloadPlan,
    tx: mpsc::Sender<WasmHotReloadCommand>,
) -> Option<WasmHotReloadWatcher> {
    let watched_file_count = plan.initial_contents.len();
    let root_count = plan.roots.len();
    let file_map = Arc::new(plan.files_by_root);
    let file_cache = Arc::new(Mutex::new(plan.initial_contents));
    let script_mod_body_cache = Arc::new(Mutex::new(plan.initial_script_mod_bodies));

    match FileSystemWatcher::start(plan.roots, {
        let file_map = Arc::clone(&file_map);
        let file_cache = Arc::clone(&file_cache);
        let script_mod_body_cache = Arc::clone(&script_mod_body_cache);
        move |event| {
            forward_hot_reload_fs_event(
                event.mount,
                event.path,
                &file_map,
                &file_cache,
                &script_mod_body_cache,
                &tx,
            );
        }
    }) {
        Ok(watcher) => {
            println!(
                "Watching {} hotpatchable script_mod source files across {} crate roots",
                watched_file_count, root_count
            );
            Some(WasmHotReloadWatcher { _watcher: watcher })
        }
        Err(err) => {
            eprintln!("hot reload watcher unavailable: {}", err);
            None
        }
    }
}

fn forward_hot_reload_fs_event(
    mount: String,
    path: PathBuf,
    files_by_root: &HashMap<String, Vec<String>>,
    file_cache: &Mutex<HashMap<String, String>>,
    script_mod_body_cache: &Mutex<HashMap<String, Vec<String>>>,
    tx: &mpsc::Sender<WasmHotReloadCommand>,
) {
    let changed_path = normalize_path_string(&path);
    let is_hot_file = files_by_root
        .get(&mount)
        .is_some_and(|files| files.iter().any(|file| file == &changed_path));
    let candidates = if is_hot_file {
        vec![changed_path]
    } else {
        files_by_root.get(&mount).cloned().unwrap_or_default()
    };

    if candidates.is_empty() {
        return;
    }

    let Ok(mut cache) = file_cache.lock() else {
        return;
    };
    let Ok(mut body_cache) = script_mod_body_cache.lock() else {
        return;
    };

    for file_name in candidates {
        let Ok(content) = fs::read_to_string(&file_name) else {
            continue;
        };
        if cache
            .get(&file_name)
            .is_some_and(|previous| previous == &content)
        {
            continue;
        }
        cache.insert(file_name.clone(), content.clone());
        let next_bodies = extract_script_mod_bodies_from_rust_file(&content)
            .unwrap_or_else(|_| vec![content.clone()]);
        let previous_bodies = body_cache.get(&file_name).cloned().unwrap_or_default();
        body_cache.insert(file_name.clone(), next_bodies.clone());

        if next_bodies != previous_bodies {
            let _ = tx.send(WasmHotReloadCommand::LiveChange { file_name, content });
        } else {
            let _ = tx.send(WasmHotReloadCommand::Rebuild);
        }
        return;
    }

    if !is_hot_file && should_trigger_wasm_rebuild(&path) {
        let _ = tx.send(WasmHotReloadCommand::Rebuild);
    }
}

fn broadcast_hot_reload_event(
    event: WasmHotReloadEvent,
    watch_clients: &mut HashMap<u64, mpsc::Sender<Vec<u8>>>,
) {
    let payload = event.serialize_json().into_bytes();
    let stale_clients: Vec<u64> = watch_clients
        .iter()
        .filter_map(|(web_socket_id, sender)| {
            sender.send(payload.clone()).err().map(|_| *web_socket_id)
        })
        .collect();
    for web_socket_id in stale_clients {
        watch_clients.remove(&web_socket_id);
    }
}

fn make_hot_reload_event(kind: &str) -> WasmHotReloadEvent {
    WasmHotReloadEvent {
        kind: kind.to_string(),
        file_name: String::new(),
        content: String::new(),
    }
}

fn rebuild_wasm_app(
    plan: &WasmRebuildPlan,
    watch_clients: &mut HashMap<u64, mpsc::Sender<Vec<u8>>>,
) {
    broadcast_hot_reload_event(make_hot_reload_event("build_start"), watch_clients);
    println!("Wasm hot reload fallback: rebuilding app");
    match build(plan.config, &plan.args) {
        Ok(_) => {
            println!("Wasm hot reload fallback: rebuild complete");
            broadcast_hot_reload_event(make_hot_reload_event("reload"), watch_clients);
        }
        Err(err) => {
            eprintln!("Wasm hot reload fallback: rebuild failed: {}", err);
        }
    }
}

fn should_trigger_wasm_rebuild(path: &Path) -> bool {
    if path.components().any(|component| {
        let part = component.as_os_str().to_string_lossy();
        part == "target" || part == ".git"
    }) {
        return false;
    }

    let Some(file_name) = path.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };
    if file_name.starts_with('.')
        || file_name == "4913"
        || file_name.ends_with('~')
        || file_name.ends_with(".swp")
        || file_name.ends_with(".tmp")
        || file_name.ends_with(".orig")
    {
        return false;
    }

    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => matches!(
            ext,
            "rs" | "toml"
                | "js"
                | "css"
                | "html"
                | "md"
                | "svg"
                | "png"
                | "jpg"
                | "jpeg"
                | "webp"
                | "glb"
                | "ttf"
                | "otf"
                | "woff"
                | "woff2"
                | "bin"
                | "ron"
        ),
        None => false,
    }
}

fn normalize_path_string(path: &Path) -> String {
    let path = if path.exists() {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    normalize_path(&path).to_string_lossy().replace('\\', "/")
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

fn extract_script_mod_bodies_from_rust_file(source: &str) -> Result<Vec<String>, String> {
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
                        extracted.push(source[j + 1..end].to_string());
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
    Err("wasm hot reload hit an unclosed delimiter while scanning Rust source".to_string())
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
    Err("wasm hot reload hit an unterminated raw string".to_string())
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
    Err("wasm hot reload hit an unterminated string literal".to_string())
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
    Err("wasm hot reload hit an unterminated block comment".to_string())
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

fn start_wasm_server(
    root: PathBuf,
    lan: bool,
    port: u16,
    threaded: bool,
    serve_mode: WasmServeMode,
    asset_manifest: Option<AssetManifest>,
    hot_reload_plan: Option<WasmHotReloadPlan>,
    rebuild_plan: Option<WasmRebuildPlan>,
    ownership_guard: &mut WasmServerOwnershipGuard,
) -> Result<(), String> {
    let net = NetworkRuntime::new(NetworkConfig::default());
    let addr = if lan {
        SocketAddr::new("0.0.0.0".parse().unwrap(), port)
    } else {
        SocketAddr::new("127.0.0.1".parse().unwrap(), port)
    };
    println!("Starting webserver on http://{:?}", addr);
    let (tx_request, rx_request) = mpsc::channel::<HttpServerRequest>();
    let (tx_hot_reload, rx_hot_reload) = mpsc::channel::<WasmHotReloadCommand>();

    let _listen_thread = net.start_http_server(HttpServer {
        listen_address: addr,
        post_max_size: 1024 * 1024,
        request: tx_request,
    });
    if _listen_thread.is_none() {
        return Err(format!("failed to bind wasm webserver on {}", addr));
    }
    ownership_guard.activate()?;

    let hot_reload_watcher =
        hot_reload_plan.and_then(|plan| start_wasm_hot_reload_watcher(plan, tx_hot_reload));

    let loop_thread = std::thread::spawn(move || {
        let _hot_reload_watcher = hot_reload_watcher;
        let mut watch_clients = HashMap::<u64, mpsc::Sender<Vec<u8>>>::new();
        let rebuild_plan = rebuild_plan;
        let asset_manifest = asset_manifest;
        let mut rebuild_queued = false;

        loop {
            let mut pending_live_changes = Vec::<(String, String)>::new();
            while let Ok(command) = rx_hot_reload.try_recv() {
                match command {
                    WasmHotReloadCommand::LiveChange { file_name, content } => {
                        pending_live_changes.push((file_name, content));
                    }
                    WasmHotReloadCommand::Rebuild => {
                        rebuild_queued = true;
                    }
                }
            }

            if rebuild_queued {
                rebuild_queued = false;
                if let Some(plan) = rebuild_plan.as_ref() {
                    rebuild_wasm_app(plan, &mut watch_clients);
                }
            } else {
                for (file_name, content) in pending_live_changes.drain(..) {
                    broadcast_hot_reload_event(
                        WasmHotReloadEvent {
                            kind: "live_change".to_string(),
                            file_name,
                            content,
                        },
                        &mut watch_clients,
                    );
                }
            }

            let message = match rx_request.recv_timeout(Duration::from_millis(100)) {
                Ok(message) => message,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };

            match message {
                HttpServerRequest::ConnectWebSocket {
                    web_socket_id,
                    headers,
                    response_sender,
                } => {
                    if headers.path == "/$watch" {
                        watch_clients.insert(web_socket_id, response_sender);
                    }
                }
                HttpServerRequest::DisconnectWebSocket { web_socket_id } => {
                    watch_clients.remove(&web_socket_id);
                }
                HttpServerRequest::BinaryMessage { .. } => {}
                HttpServerRequest::TextMessage { .. } => {}
                HttpServerRequest::Get {
                    headers,
                    response_sender,
                } => {
                    let mut path = headers.path.as_str();
                    let query = headers.search.as_deref().unwrap_or("");
                    if path == "/" {
                        path = "/index.html";
                    }
                    let cache_extra = if serve_mode == WasmServeMode::Dev {
                        "Pragma: no-cache\r\n\
                        Expires: 0\r\n\
                        "
                    } else {
                        ""
                    };

                    if path == "/$watch" || path == "/favicon.ico" {
                        let header = "HTTP/1.1 200 OK\r\n\
                        Cache-Control: max-age:0\r\n\
                        Connection: close\r\n\r\n"
                            .to_string();
                        let _ = response_sender.send(HttpServerResponse {
                            header,
                            body: vec![],
                        });
                        continue;
                    }
                    if path == "/$report_error" {
                        let encoded = query.strip_prefix("data=").unwrap_or(query);
                        let decoded = decode_query_component(encoded);
                        println!("Browser error report: {}", decoded);
                        let header = "HTTP/1.1 200 OK\r\n\
                        Cache-Control: max-age:0\r\n\
                        Connection: close\r\n\r\n"
                            .to_string();
                        let _ = response_sender.send(HttpServerResponse {
                            header,
                            body: vec![],
                        });
                        continue;
                    }

                    if path.contains("..") || path.contains('\\') {
                        let body = b"Bad request".to_vec();
                        let header = format!(
                            "HTTP/1.1 400 Bad Request\r\n\
                            Content-Type: text/plain\r\n\
                            Content-Length: {}\r\n\
                            Connection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = response_sender.send(HttpServerResponse { header, body });
                        continue;
                    }

                    let is_spa_route = |request_path: &str| {
                        if request_path.starts_with("/$") || request_path == "/favicon.ico" {
                            return false;
                        }
                        let trimmed = request_path.trim_end_matches('/');
                        let last_segment = trimmed.rsplit('/').next().unwrap_or("");
                        !last_segment.is_empty() && !last_segment.contains('.')
                    };

                    let mut spa_fallback_to_index = false;
                    let mime_type = if let Some(mime_type) = mime_type_for_path(path) {
                        mime_type
                    } else if path.ends_with(".ttf.2") {
                        "application/ttf"
                    } else if path.ends_with(".otf.2") {
                        "font/otf"
                    } else if is_spa_route(path) {
                        spa_fallback_to_index = true;
                        path = "/index.html";
                        "text/html"
                    } else {
                        println!("Wasm webserver 404 (unknown mime/path): {}", headers.path);
                        let body = b"Not found".to_vec();
                        let header = format!(
                            "HTTP/1.1 404 Not Found\r\n\
                            Content-Type: text/plain\r\n\
                            Content-Length: {}\r\n\
                            Connection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = response_sender.send(HttpServerResponse { header, body });
                        continue;
                    };

                    if spa_fallback_to_index {
                        println!("Wasm webserver SPA fallback: {} -> {}", headers.path, path);
                    }
                    let path = path.strip_prefix("/").unwrap();
                    let cache_control =
                        cache_control_for_request(serve_mode, path, asset_manifest.as_ref());

                    let path = root.join(&path);
                    let compressed_path = path.parent().and_then(|parent| {
                        path.file_name()
                            .map(|name| parent.join(format!("{}.br", name.to_string_lossy())))
                    });
                    let wants_brotli = client_accepts_brotli(headers.accept_encoding.as_deref());

                    if wants_brotli {
                        if let Some(compressed_path) = compressed_path.as_ref() {
                            if let Ok(mut file_handle) = File::open(compressed_path) {
                                let mut body = Vec::<u8>::new();
                                if file_handle.read_to_end(&mut body).is_ok() {
                                    let coop_coep_headers = if threaded {
                                        "Cross-Origin-Embedder-Policy: require-corp\r\n\
                                        Cross-Origin-Opener-Policy: same-origin\r\n"
                                    } else {
                                        ""
                                    };
                                    let header = format!(
                                        "HTTP/1.1 200 OK\r\n\
                                        Content-Type: {}\r\n\
                                        {}\
                                        Vary: Accept-Encoding\r\n\
                                        Content-Encoding: br\r\n\
                                        Cache-Control: {}\r\n\
                                        {}\
                                        Content-Length: {}\r\n\
                                        Connection: close\r\n\r\n",
                                        mime_type,
                                        coop_coep_headers,
                                        cache_control,
                                        cache_extra,
                                        body.len()
                                    );
                                    let _ =
                                        response_sender.send(HttpServerResponse { header, body });
                                    continue;
                                }
                            }
                        }
                    }
                    if let Ok(mut file_handle) = File::open(&path) {
                        let mut body = Vec::<u8>::new();
                        if file_handle.read_to_end(&mut body).is_ok() {
                            let coop_coep_headers = if threaded {
                                "Cross-Origin-Embedder-Policy: require-corp\r\n\
                                Cross-Origin-Opener-Policy: same-origin\r\n"
                            } else {
                                ""
                            };
                            let vary_header =
                                if compressed_path.as_ref().is_some_and(|path| path.exists()) {
                                    "Vary: Accept-Encoding\r\n"
                                } else {
                                    ""
                                };
                            let header = format!(
                                "HTTP/1.1 200 OK\r\n\
                                Content-Type: {}\r\n\
                                {}\
                                {}\
                                Cache-Control: {}\r\n\
                                {}\
                                Content-Length: {}\r\n\
                                Connection: close\r\n\r\n",
                                mime_type,
                                coop_coep_headers,
                                vary_header,
                                cache_control,
                                cache_extra,
                                body.len()
                            );
                            let _ = response_sender.send(HttpServerResponse { header, body });
                        }
                    } else {
                        println!("Wasm webserver 404 (missing file): {}", headers.path);
                        let body = b"Not found".to_vec();
                        let header = format!(
                            "HTTP/1.1 404 Not Found\r\n\
                            Content-Type: text/plain\r\n\
                            Content-Length: {}\r\n\
                            Connection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = response_sender.send(HttpServerResponse { header, body });
                    }
                }
                HttpServerRequest::Post {
                    headers,
                    body,
                    response,
                } => {
                    let path = headers
                        .path
                        .split('?')
                        .next()
                        .unwrap_or(headers.path.as_str());
                    if path == "/$report_error" {
                        let message = String::from_utf8_lossy(&body);
                        println!("Browser error report: {}", message);
                        let header = "HTTP/1.1 200 OK\r\n\
                            Cache-Control: max-age:0\r\n\
                            Connection: close\r\n\r\n"
                            .to_string();
                        let _ = response.send(HttpServerResponse {
                            header,
                            body: vec![],
                        });
                    } else {
                        let body = b"Not found".to_vec();
                        let header = format!(
                            "HTTP/1.1 404 Not Found\r\n\
                            Content-Type: text/plain\r\n\
                            Content-Length: {}\r\n\
                            Connection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = response.send(HttpServerResponse { header, body });
                    }
                }
            }
        }
    });
    loop_thread
        .join()
        .map_err(|_| "wasm webserver event loop thread panicked".to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_wasm_config() -> WasmConfig {
        WasmConfig {
            strip: false,
            lan: false,
            port: None,
            small_fonts: false,
            brotli: false,
            bindgen: false,
            threads: false,
            optimize_size: false,
            wasm_opt: false,
            split: false,
            split_auto: false,
            split_functions: false,
            split_functions_threshold: 0,
            hot_reload: false,
            serve: false,
            shipping_build: true,
            full_fonts: false,
            brotli_explicit: false,
            threads_explicit: false,
            small_fonts_explicit: false,
            split_explicit: false,
        }
    }

    #[test]
    fn script_mod_extraction_ignores_non_code_segments() {
        let source = r###"
            // script_mod!{ ignored_comment }
            const STR: &str = "script_mod!{ ignored_string }";
            const RAW: &str = r#"script_mod!{ ignored_raw }"#;

            script_mod!{
                use mod.prelude.widgets.*
                ui: Root{}
            }
        "###;

        let bodies = extract_script_mod_bodies_from_rust_file(source).unwrap();
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].contains("ui: Root{}"));
        assert!(!bodies[0].contains("ignored_comment"));
        assert!(!bodies[0].contains("ignored_string"));
        assert!(!bodies[0].contains("ignored_raw"));
    }

    #[test]
    fn script_mod_extraction_stays_stable_for_outside_edits() {
        let before = r#"
            fn helper() -> usize { 1 }
            script_mod!{
                use mod.prelude.widgets.*
                ui: Root{}
            }
        "#;
        let after = r#"
            fn helper() -> usize { 2 }
            script_mod!{
                use mod.prelude.widgets.*
                ui: Root{}
            }
        "#;

        assert_eq!(
            extract_script_mod_bodies_from_rust_file(before).unwrap(),
            extract_script_mod_bodies_from_rust_file(after).unwrap()
        );
    }

    #[test]
    fn wasm_rebuild_filter_skips_temp_and_target_paths() {
        assert!(should_trigger_wasm_rebuild(Path::new(
            "/tmp/app/src/main.rs"
        )));
        assert!(should_trigger_wasm_rebuild(Path::new(
            "/tmp/app/resources/theme.ron"
        )));
        assert!(!should_trigger_wasm_rebuild(Path::new(
            "/tmp/app/target/debug/main.rs"
        )));
        assert!(!should_trigger_wasm_rebuild(Path::new(
            "/tmp/app/src/main.rs.swp"
        )));
        assert!(!should_trigger_wasm_rebuild(Path::new(
            "/tmp/app/.git/index"
        )));
    }

    #[test]
    fn crate_resource_extraction_ignores_comments_and_strings() {
        let source = r###"
            // crate_resource("self:resources/comment.svg")
            let _fake = "crate_resource(\"self:resources/string.svg\")";
            let _also_fake = r#"crate_resource("self:resources/raw.svg")"#;

            let _real = crate_resource("self:resources/real.svg");
            let _real_raw = crate_resource(r#"makepad_widgets:resources/icons/back.svg"#);
        "###;

        let literals = extract_crate_resource_literals_from_rust_file(source).unwrap();
        assert_eq!(
            literals,
            vec![
                "self:resources/real.svg".to_string(),
                "makepad_widgets:resources/icons/back.svg".to_string()
            ]
        );
    }

    #[test]
    fn parse_wasm_web_metadata_reads_preserve_and_full_i18n() {
        let metadata = parse_wasm_web_metadata_toml(
            r#"
            [package]
            name = "example"

            [package.metadata.makepad.web]
            full_i18n = true
            preserve = [
                "self:resources/hero.glb",
                "makepad_widgets/resources/NewCMMath-Regular.otf",
            ]
            "#,
        );
        assert!(metadata.full_i18n);
        assert_eq!(
            metadata.preserve,
            vec![
                "self:resources/hero.glb".to_string(),
                "makepad_widgets/resources/NewCMMath-Regular.otf".to_string()
            ]
        );
    }

    #[test]
    fn resolve_resource_spec_supports_self_and_dependency_paths() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("makepad-wasm-test-{unique}"));
        let app_dir = temp_root.join("app");
        let widgets_dir = temp_root.join("widgets");
        fs::create_dir_all(app_dir.join("resources")).unwrap();
        fs::create_dir_all(widgets_dir.join("resources/icons")).unwrap();

        let manifests = HashMap::from([
            ("makepad_example".to_string(), app_dir.clone()),
            ("makepad_widgets".to_string(), widgets_dir.clone()),
        ]);

        let (logical_path, abs_path, crate_name) = resolve_resource_spec(
            "self:resources/local.svg",
            &app_dir,
            "makepad_example",
            &manifests,
        )
        .unwrap();
        assert_eq!(logical_path, "makepad_example/resources/local.svg");
        assert_eq!(abs_path, app_dir.join("resources/local.svg"));
        assert_eq!(crate_name, "makepad_example");

        let (logical_path, abs_path, crate_name) = resolve_resource_spec(
            "makepad_widgets:resources/icons/back.svg",
            &app_dir,
            "makepad_example",
            &manifests,
        )
        .unwrap();
        assert_eq!(logical_path, "makepad_widgets/resources/icons/back.svg");
        assert_eq!(abs_path, widgets_dir.join("resources/icons/back.svg"));
        assert_eq!(crate_name, "makepad_widgets");

        let (logical_path, abs_path, crate_name) = resolve_resource_spec(
            "makepad_widgets/resources/icons/back.svg",
            &app_dir,
            "makepad_example",
            &manifests,
        )
        .unwrap();
        assert_eq!(logical_path, "makepad_widgets/resources/icons/back.svg");
        assert_eq!(abs_path, widgets_dir.join("resources/icons/back.svg"));
        assert_eq!(crate_name, "makepad_widgets");

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn brotli_accept_header_respects_quality_values() {
        assert!(client_accepts_brotli(Some("gzip, br")));
        assert!(client_accepts_brotli(Some("br;q=1.0, gzip;q=0.8")));
        assert!(!client_accepts_brotli(Some("gzip, br;q=0")));
        assert!(!client_accepts_brotli(Some("gzip, deflate")));
        assert!(!client_accepts_brotli(None));
    }

    #[test]
    fn shipping_cache_control_uses_manifest_entries() {
        let manifest = AssetManifest {
            version: 1,
            build_crate: "app".to_string(),
            profile: "small".to_string(),
            shipping_build: true,
            threaded: false,
            startup_assets: vec!["app.1234.wasm".to_string()],
            assets: vec![AssetManifestEntry {
                logical_path: "app.wasm".to_string(),
                emitted_path: "app.1234.wasm".to_string(),
                kind: "wasm".to_string(),
                content_type: "application/wasm".to_string(),
                cache_control: immutable_cache_control(),
                startup_blocking: true,
                hashed: true,
                direct_reference: false,
                crate_name: None,
                reason: "startup_wasm".to_string(),
                raw_bytes: 100,
                transfer_bytes: 50,
                brotli_bytes: Some(50),
            }],
        };

        assert_eq!(
            cache_control_for_request(WasmServeMode::Shipping, "app.1234.wasm", Some(&manifest)),
            immutable_cache_control()
        );
        assert_eq!(
            cache_control_for_request(
                WasmServeMode::Shipping,
                "asset-manifest.json",
                Some(&manifest)
            ),
            short_cache_control()
        );
        assert_eq!(
            cache_control_for_request(WasmServeMode::Dev, "app.1234.wasm", Some(&manifest)),
            "no-store, must-revalidate".to_string()
        );
    }

    #[test]
    fn generate_html_preloads_only_startup_split_assets() {
        let config = test_wasm_config();
        let eager_assets = HtmlAssetPaths {
            wasm_path: "app.wasm".to_string(),
            split_data_path: Some("app.data.bin".to_string()),
            split_data_active_only: false,
            secondary_wasm_path: Some("app.secondary.wasm".to_string()),
            defer_secondary_wasm: false,
            web_gl_js_path: "makepad_platform/web_gl.js".to_string(),
            full_canvas_css_path: "makepad_platform/full_canvas.css".to_string(),
            bindgen_js_path: None,
            wasm_bridge_js_path: None,
        };
        let eager_html = generate_html("app", &eager_assets, &config);
        assert!(eager_html.contains("./app.data.bin"));
        assert!(eager_html.contains("./app.secondary.wasm"));
        assert!(!eager_html.contains("defer_secondary_wasm: true"));

        let deferred_assets = HtmlAssetPaths {
            defer_secondary_wasm: true,
            ..eager_assets
        };
        let deferred_html = generate_html("app", &deferred_assets, &config);
        assert!(deferred_html.contains("./app.data.bin"));
        assert!(deferred_html.contains("defer_secondary_wasm: true"));
        assert!(!deferred_html.contains(
            "href='./app.secondary.wasm' as='fetch' type='application/wasm' crossorigin"
        ));

        let active_only_assets = HtmlAssetPaths {
            split_data_active_only: true,
            ..deferred_assets
        };
        let active_only_html = generate_html("app", &active_only_assets, &config);
        assert!(active_only_html.contains("split_data_active_only: true"));
    }

    #[test]
    fn finalize_pending_assets_skips_duplicate_logical_entries() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let app_dir = std::env::temp_dir().join(format!("makepad-wasm-assets-{unique}"));
        fs::create_dir_all(app_dir.join("makepad_platform")).unwrap();
        fs::write(
            app_dir.join("makepad_platform/web_gl.js"),
            "console.log('webgl');",
        )
        .unwrap();

        let asset = PendingAsset {
            logical_path: "makepad_platform/web_gl.js".to_string(),
            emitted_path: "makepad_platform/web_gl.js".to_string(),
            kind: "javascript".to_string(),
            content_type: "text/javascript".to_string(),
            startup_blocking: true,
            direct_reference: false,
            crate_name: Some("makepad_platform".to_string()),
            reason: "runtime_support".to_string(),
        };
        let mut pending_assets = vec![asset.clone(), asset];
        let manifest =
            finalize_pending_assets(&app_dir, "app", "small", false, true, &mut pending_assets)
                .unwrap();

        assert_eq!(manifest.assets.len(), 1);
        assert_eq!(manifest.startup_assets.len(), 1);
        assert_ne!(
            manifest.assets[0].emitted_path,
            "makepad_platform/web_gl.js".to_string()
        );
        assert!(app_dir.join(&manifest.assets[0].emitted_path).exists());

        let _ = fs::remove_dir_all(&app_dir);
    }

    #[test]
    fn web_perf_report_flags_oversized_optional_assets() {
        let manifest = AssetManifest {
            version: 1,
            build_crate: "app".to_string(),
            profile: "small".to_string(),
            shipping_build: true,
            threaded: false,
            startup_assets: vec!["app.1234.wasm".to_string()],
            assets: vec![
                AssetManifestEntry {
                    logical_path: "app.wasm".to_string(),
                    emitted_path: "app.1234.wasm".to_string(),
                    kind: "wasm".to_string(),
                    content_type: "application/wasm".to_string(),
                    cache_control: immutable_cache_control(),
                    startup_blocking: true,
                    hashed: true,
                    direct_reference: false,
                    crate_name: None,
                    reason: "startup_wasm".to_string(),
                    raw_bytes: 400_000,
                    transfer_bytes: 200_000,
                    brotli_bytes: Some(200_000),
                },
                AssetManifestEntry {
                    logical_path: "makepad_widgets/resources/LXGWWenKaiRegular.ttf".to_string(),
                    emitted_path: "makepad_widgets/resources/LXGWWenKaiRegular.ttf".to_string(),
                    kind: "resource".to_string(),
                    content_type: "application/ttf".to_string(),
                    cache_control: short_cache_control(),
                    startup_blocking: false,
                    hashed: false,
                    direct_reference: false,
                    crate_name: Some("makepad_widgets".to_string()),
                    reason: "widget_web_default".to_string(),
                    raw_bytes: 900_000,
                    transfer_bytes: 900_000,
                    brotli_bytes: None,
                },
            ],
        };

        let report = build_web_perf_report(&manifest);
        let oversized_budget = report
            .budgets
            .iter()
            .find(|budget| budget.name == "oversized_optional_assets")
            .unwrap();
        assert_eq!(oversized_budget.actual_bytes, 1);
        assert!(!oversized_budget.passed);
    }

    #[test]
    fn web_perf_report_counts_split_assets_when_startup_blocking() {
        let manifest = AssetManifest {
            version: 1,
            build_crate: "app".to_string(),
            profile: "small".to_string(),
            shipping_build: true,
            threaded: false,
            startup_assets: vec![
                "app.1234.wasm".to_string(),
                "app.secondary.5678.wasm".to_string(),
                "app.data.bin".to_string(),
            ],
            assets: vec![
                AssetManifestEntry {
                    logical_path: "app.wasm".to_string(),
                    emitted_path: "app.1234.wasm".to_string(),
                    kind: "wasm".to_string(),
                    content_type: "application/wasm".to_string(),
                    cache_control: immutable_cache_control(),
                    startup_blocking: true,
                    hashed: true,
                    direct_reference: false,
                    crate_name: None,
                    reason: "startup_wasm".to_string(),
                    raw_bytes: 500_000,
                    transfer_bytes: 220_000,
                    brotli_bytes: Some(220_000),
                },
                AssetManifestEntry {
                    logical_path: "app.secondary.wasm".to_string(),
                    emitted_path: "app.secondary.5678.wasm".to_string(),
                    kind: "wasm".to_string(),
                    content_type: "application/wasm".to_string(),
                    cache_control: immutable_cache_control(),
                    startup_blocking: true,
                    hashed: true,
                    direct_reference: false,
                    crate_name: None,
                    reason: "split_secondary".to_string(),
                    raw_bytes: 120_000,
                    transfer_bytes: 40_000,
                    brotli_bytes: Some(40_000),
                },
                AssetManifestEntry {
                    logical_path: "app.data.bin".to_string(),
                    emitted_path: "app.data.bin".to_string(),
                    kind: "binary".to_string(),
                    content_type: "application/octet-stream".to_string(),
                    cache_control: short_cache_control(),
                    startup_blocking: true,
                    hashed: false,
                    direct_reference: false,
                    crate_name: None,
                    reason: "split_data".to_string(),
                    raw_bytes: 90_000,
                    transfer_bytes: 30_000,
                    brotli_bytes: Some(30_000),
                },
            ],
        };

        let report = build_web_perf_report(&manifest);
        assert_eq!(report.startup_blocking_raw_bytes, 710_000);
        assert_eq!(report.startup_blocking_transfer_bytes, 290_000);
    }
}
