//! JavaScript test pages, run in our own Chromium (libs/cef): no Node.js.
//!
//! A test file is an ES module (`*.test.mjs`) that imports `test`, `assert` and `read_text`
//! from `"makepad-web-test"` (src/harness.js: node:test's `test` and node:assert/strict's
//! `assert` as the files use them). [`main`] is a whole `harness = false` test target: it
//! serves the package's folder on a loopback port, opens one page per test file in CEF (an
//! off-screen browser, no window), and each page loads its module, runs its tests in order and
//! posts every result back. The output reads like libtest's; any failure, a module that does
//! not load, or no CEF to run in fails the target.
//!
//! CEF runs on a profile of its own in the temp folder, gone at the end, and headless on
//! Linux. Chromium re-runs this executable as its subprocesses: [`main`] answers those first.
//! It drives CEF on Linux only (elsewhere CEF needs the HAL main loop of an app): on another
//! platform the target holds no tests, as a `#[cfg(target_os = "linux")]` test would.

//!
//! With `--app DIR --out DIR [--size WxH] --steps "step; step; ..."`, the same target
//! opens a built Makepad wasm app on a real Linux GPU in headless CEF. Its private server
//! injects a WebGL2 observer before the app starts and enables cross-origin isolation.
//! Input coordinates are viewport pixels (scale 1). The default size is 800x600.
//!
//! - `info` prints the unmasked renderer and vendor. Software rendering fails the run.
//! - `frames N` waits for animation frames. `settle` waits for 500 ms of unchanged GPU
//!   frame count and reports a still-drawing app after 10 s.
//! - `grab NAME` saves a top-down PNG. `stats NAME` saves GPU stats and inventory as JSON.
//! - `lose` and `restore` request and await the corresponding WebGL context events.
//! - `click X Y`, `wheel X Y DX DY`, and `key NAME` send browser input.
//! - `same A B [TOLERANCE]` compares two grabs in RGB (default tolerance 0).
//!   `differ A B` requires at least one changed RGB pixel.
//!
//! From the lane worktree, with the workspace's cargo-makepad on PATH:
//! ```sh
//! makepad devhub run "$lane" build --needs os=linux,gui=1 --cost medium \
//!   --native "wasm app and CEF web_js subprocesses" --keep "$keep" -- sh -ec '
//!   export CARGO_NET_OFFLINE=true RUSTUP_AUTO_INSTALL=0
//!   cargo makepad wasm build --release -p makepad-example-counter
//!   cargo test --release -p makepad-platform --test web_js -- \
//!     --app target/makepad-wasm-app/release/makepad-example-counter --out "$DEVHUB_KEEP" \
//!     --steps "info; settle; grab a; lose; frames 10; restore; settle; grab b; same a b"'
//! ```

mod app;

use makepad_micro_serde::{DeJson, JsonValue};
use makepad_network::{start_http_server, HttpServer, HttpServerRequest, HttpServerResponse};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const HARNESS: &str = include_str!("harness.js");
const HARNESS_PATH: &str = "/.web-test/harness.js";
const PAGE_PATH: &str = "/.web-test/page/";
const REPORT_PATH: &str = "/.web-test/report/";
/// What the test files import the harness as (the page's import map names it).
const SPECIFIER: &str = "makepad-web-test";

/// The test target's arguments, as cargo test passes libtest's: filters (substrings of
/// `<file>::<test>`, or the whole name with `--exact`), `--skip <filter>`, `--list`; the rest
/// of libtest's flags are taken and ignored.
#[derive(Default)]
struct Args {
    filters: Vec<String>,
    skip: Vec<String>,
    exact: bool,
    list: bool,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = String>) -> Args {
        let mut out = Args::default();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--list" => out.list = true,
                "--exact" => out.exact = true,
                "--skip" => out.skip.extend(args.next()),
                // libtest flags that take a value
                "--test-threads" | "--color" | "--format" | "-Z" | "--logfile" | "--shuffle-seed" => {
                    args.next();
                }
                flag if flag.starts_with('-') => {}
                filter => out.filters.push(filter.to_string()),
            }
        }
        out
    }
}

/// One test file's page.
struct Page {
    /// the module's path on the server (`/src/os/web/x.test.mjs`)
    module: String,
    /// its file, as the output names it
    file: String,
}

/// What a page tells the runner.
enum Report {
    Loaded { count: u64 },
    Filtered,
    Ignored { name: String },
    Result { name: String, ok: bool, error: String, diagnostics: Vec<String> },
    LoadError { error: String },
    Done,
}

fn report(body: &[u8]) -> Result<Report, String> {
    let text = std::str::from_utf8(body).map_err(|e| e.to_string())?;
    let value = JsonValue::deserialize_json(text).map_err(|e| format!("{e:?}"))?;
    let s = |key: &str| value.get(key).and_then(JsonValue::as_str).unwrap_or_default().to_string();
    Ok(match s("kind").as_str() {
        "loaded" => Report::Loaded { count: value.get("count").and_then(JsonValue::as_u64).unwrap_or(0) },
        "filtered" => Report::Filtered,
        "ignored" => Report::Ignored { name: s("name") },
        "result" => Report::Result {
            name: s("name"),
            ok: value.get("ok").and_then(JsonValue::as_bool).unwrap_or(false),
            error: s("error"),
            diagnostics: value.get("diagnostics").and_then(JsonValue::as_array).map(|a| a.iter().filter_map(|d| d.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        },
        "load_error" => Report::LoadError { error: s("error") },
        "done" => Report::Done,
        kind => return Err(format!("a report of unknown kind {kind:?}")),
    })
}

/// `text` as a JSON string, safe inside a `<script>` element.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_strings(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| json_string(s)).collect::<Vec<_>>().join(","))
}

fn page_html(index: usize, page: &Page, args: &Args) -> String {
    format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<title>{file}</title>\n\
         <script type=\"importmap\">{{\"imports\":{{{specifier}:{harness}}}}}</script>\n\
         <script type=\"module\">\nimport {{ run }} from {specifier};\n\
         run({{module:{module},report:{report},filters:{filters},skip:{skip},exact:{exact}}});\n</script>\n",
        file = page.file.replace('<', "&lt;"),
        specifier = json_string(SPECIFIER),
        harness = json_string(HARNESS_PATH),
        module = json_string(&page.module),
        report = json_string(&format!("{REPORT_PATH}{index}")),
        filters = json_strings(&args.filters),
        skip = json_strings(&args.skip),
        exact = args.exact,
    )
}

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "txt" | "md" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// The file a request names inside `root`, or None for anything that could leave it.
fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let rel = path.split(['?', '#']).next().unwrap_or("").trim_start_matches('/');
    if rel.is_empty() || rel.split('/').any(|seg| seg.is_empty() || seg == ".." || seg.starts_with('.')) || rel.contains('\\') || rel.contains(':') {
        return None;
    }
    let file = root.join(rel).canonicalize().ok()?;
    (file.starts_with(root) && file.is_file()).then_some(file)
}

fn response(status: &str, content_type: &str, body: Vec<u8>, extra_headers: &str) -> HttpServerResponse {
    HttpServerResponse::new(
        format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{extra_headers}Connection: close\r\n\r\n", body.len()),
        body,
    )
}

enum Site {
    Pages(Vec<String>),
    App { index: String },
}

/// Serves `root`, the harness and the pages on a loopback port; reports go to `reports` as
/// (page, body). Returns the port.
fn serve(root: PathBuf, site: Site, reports: mpsc::Sender<(usize, Vec<u8>)>) -> Result<u16, String> {
    // A free port first: the server binds by address.
    let probe = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("no loopback port: {e}"))?;
    let addr = probe.local_addr().map_err(|e| e.to_string())?;
    drop(probe);
    let (tx, rx) = mpsc::channel::<HttpServerRequest>();
    start_http_server(HttpServer {
        listen_address: addr,
        request: tx,
        // what a page posts is one test's result; this is no budget it is meant to reach
        post_max_size: 1 << 30,
        post_max_size_overrides: Vec::new(),
        pre_admit_posts: false,
        client_ip_resolver: None,
        trusted_proxy: None,
        allowed_methods: Some(|_| Some("GET, POST")),
    })
    .ok_or_else(|| format!("cannot listen on {addr}"))?;
    std::thread::Builder::new()
        .name("web-test-server".into())
        .spawn(move || {
            let extra_headers = match site {
                Site::Pages(_) => "",
                Site::App { .. } => "Cross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\n",
            };
            let response = |status: &str, mime: &str, body| response(status, mime, body, extra_headers);
            while let Ok(request) = rx.recv() {
                match request {
                    HttpServerRequest::Get { headers, response_sender } => {
                        let path = headers.path.split(['?', '#']).next().unwrap_or("");
                        let content = match &site {
                            Site::Pages(_) if path == HARNESS_PATH => Some((mime(path), HARNESS.as_bytes().to_vec())),
                            Site::Pages(pages) if path.starts_with(PAGE_PATH) => path[PAGE_PATH.len()..].parse::<usize>().ok()
                                .and_then(|n| pages.get(n)).map(|page| (mime("index.html"), page.as_bytes().to_vec())),
                            Site::App { index } if path == "/" || path == "/index.html" => Some((mime("index.html"), index.as_bytes().to_vec())),
                            Site::App { .. } if path == app::RIG_PATH => Some((mime(path), app::RIG.as_bytes().to_vec())),
                            _ => resolve(&root, path).and_then(|file| std::fs::read(file).ok()).map(|body| (mime(path), body)),
                        };
                        let answer = match content {
                            Some((mime, body)) => response("200 OK", mime, body),
                            // Chromium requests this on its own even if index.html names no icon.
                            None if matches!(site, Site::App { .. }) && path == "/favicon.ico" => response("204 No Content", "image/x-icon", Vec::new()),
                            None => response("404 Not Found", "text/plain", b"Not Found".to_vec()),
                        };
                        let _ = response_sender.send(answer);
                    }
                    HttpServerRequest::Post { headers, body, response: sender } => {
                        let page = headers.path.strip_prefix(REPORT_PATH).and_then(|n| n.parse::<usize>().ok());
                        let answer = match page {
                            Some(page) if reports.send((page, body)).is_ok() => response("204 No Content", "text/plain", Vec::new()),
                            _ => response("404 Not Found", "text/plain", b"Not Found".to_vec()),
                        };
                        let _ = sender.send(answer);
                    }
                    _ => {}
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(addr.port())
}

/// Every `*.test.mjs` in `dirs` (relative to `root`), sorted.
fn discover(root: &Path, dirs: &[&str]) -> Result<Vec<Page>, String> {
    let mut pages = Vec::new();
    for dir in dirs {
        let read = std::fs::read_dir(root.join(dir)).map_err(|e| format!("{}: {e}", root.join(dir).display()))?;
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".test.mjs") && entry.path().is_file() {
                let rel = format!("{}/{name}", dir.trim_matches('/'));
                pages.push(Page { module: format!("/{rel}"), file: rel });
            }
        }
    }
    pages.sort_by(|a, b| a.file.cmp(&b.file));
    if pages.is_empty() {
        return Err(format!("no *.test.mjs in {}", dirs.join(", ")));
    }
    Ok(pages)
}

#[derive(Default)]
struct Totals {
    passed: usize,
    failed: usize,
    ignored: usize,
    filtered: usize,
    /// (name, what failed)
    failures: Vec<(String, String)>,
}

impl Totals {
    fn line(&self, seconds: f64) -> String {
        format!(
            "test result: {}. {} passed; {} failed; {} ignored; 0 measured; {} filtered out; finished in {seconds:.2}s",
            if self.failed == 0 { "ok" } else { "FAILED" },
            self.passed,
            self.failed,
            self.ignored,
            self.filtered
        )
    }

    fn add(&mut self, other: Totals) {
        self.passed += other.passed;
        self.failed += other.failed;
        self.ignored += other.ignored;
        self.filtered += other.filtered;
        self.failures.extend(other.failures);
    }
}

/// One page in a browser of its own, until it reports it is done.
fn run_page(url: &str, page: &Page, reports: &mpsc::Receiver<(usize, Vec<u8>)>, index: usize) -> Result<Totals, String> {
    let started = Instant::now();
    println!("     Running {} in CEF {}", page.file, makepad_cef::cef_version());
    let mut browser = makepad_cef::Browser::new(url, 256, 256, 1.0).map_err(|e| format!("{}: no browser: {e}", page.file))?;
    let mut totals = Totals::default();
    let mut console = Vec::new();
    let stem = page.file.rsplit('/').next().unwrap_or(&page.file).trim_end_matches(".test.mjs").to_string();
    'page: loop {
        makepad_cef::do_message_loop_work();
        if let Some(error) = browser.take_error() {
            return Err(format!("{}: its browser failed: {error}", page.file));
        }
        console.extend(browser.take_console_messages().into_iter().filter(|m| m.level >= 3).map(|m| format!("console: {} ({}:{})", m.message, m.source, m.line)));
        while let Ok((from, body)) = reports.try_recv() {
            if from != index {
                continue;
            }
            match report(&body).map_err(|e| format!("{}: {e}", page.file))? {
                Report::Loaded { count } => println!("running {count} test{}", if count == 1 { "" } else { "s" }),
                Report::Filtered => totals.filtered += 1,
                Report::Ignored { name } => {
                    println!("test {name} ... ignored");
                    totals.ignored += 1;
                }
                Report::Result { name, ok, error, diagnostics } => {
                    println!("test {name} ... {}", if ok { "ok" } else { "FAILED" });
                    for line in diagnostics {
                        println!("    {line}");
                    }
                    if ok {
                        totals.passed += 1;
                    } else {
                        totals.failed += 1;
                        totals.failures.push((name, error));
                    }
                }
                Report::LoadError { error } => {
                    // a module that does not load (or an error outside every test) fails as a test
                    let name = format!("{stem}::<load>");
                    println!("running 1 test\ntest {name} ... FAILED");
                    totals.failed += 1;
                    totals.failures.push((name, error));
                }
                Report::Done => break 'page,
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    if totals.failed > 0 && !console.is_empty() {
        let name = format!("{stem}::<console>");
        totals.failures.push((name, console.join("\n")));
    }
    drop(browser);
    println!("{}\n", totals.line(started.elapsed().as_secs_f64()));
    Ok(totals)
}

/// The page tests' run: the test target's `main` (`harness = false`). Runs every `*.test.mjs`
/// in `dirs`, relative to `root` (which the pages are served from: a test reads files beside
/// it, or anywhere under `root`, by URL).
pub fn main(root: &Path, dirs: &[&str]) -> ExitCode {
    // Chromium's subprocesses are this executable again: they go before anything else
    if let Some(code) = makepad_cef::run_helper_if_subprocess() {
        std::process::exit(code);
    }
    let argv: Vec<_> = std::env::args().skip(1).collect();
    if argv.iter().any(|arg| arg == "--app") {
        if !cfg!(target_os = "linux") {
            println!("web app test: skipped (real GPU app mode runs on Linux only)");
            return ExitCode::SUCCESS;
        }
        return match app::run(&argv) {
            Ok(()) => { println!("web app test: ok"); ExitCode::SUCCESS }
            Err(error) => { eprintln!("web app test: FAILED: {error}"); ExitCode::FAILURE }
        };
    }
    let args = Args::parse(argv.into_iter());
    if args.list {
        // a harness of its own: listed as no tests, so a run takes the target whole
        return ExitCode::SUCCESS;
    }
    if !cfg!(target_os = "linux") {
        println!("running 0 tests");
        println!("web tests: the pages run in CEF on Linux only (elsewhere CEF runs inside an app's HAL main loop)");
        println!("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s");
        return ExitCode::SUCCESS;
    }
    match run(root, dirs, args) {
        Ok(totals) if totals.failed == 0 => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(101),
        Err(error) => {
            eprintln!("web tests: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(root: &Path, dirs: &[&str], args: Args) -> Result<Totals, String> {
    let started = Instant::now();
    let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    let pages = discover(&root, dirs)?;
    let totals = with_cef_profile(|| run_pages(&root, &pages, &args))?;
    if !totals.failures.is_empty() {
        println!("failures:\n");
        for (name, error) in &totals.failures {
            println!("---- {name} ----\n{error}\n");
        }
        println!("failures:");
        for (name, _) in totals.failures.iter().filter(|(name, _)| !name.ends_with("::<console>")) {
            println!("    {name}");
        }
        println!();
    }
    println!("{} pages: {}", pages.len(), totals.line(started.elapsed().as_secs_f64()));
    Ok(totals)
}

fn with_cef_profile<T>(run: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    // A profile of its own, never a person's: made here, removed at the end.
    let profile = std::env::temp_dir().join(format!("makepad-web-test-{}", std::process::id()));
    std::fs::create_dir_all(&profile).map_err(|e| format!("{}: {e}", profile.display()))?;
    std::env::set_var("MAKEPAD_CEF_PROFILE_DIR", &profile);
    // off-screen pages need no display server
    if std::env::var_os("MAKEPAD_CEF_OZONE").is_none() {
        std::env::set_var("MAKEPAD_CEF_OZONE", "headless");
    }
    let result = run();
    makepad_cef::shutdown();
    let _ = std::fs::remove_dir_all(&profile);
    result
}

fn run_pages(root: &Path, pages: &[Page], args: &Args) -> Result<Totals, String> {
    let html: Vec<String> = pages.iter().enumerate().map(|(i, page)| page_html(i, page, args)).collect();
    let (tx, reports) = mpsc::channel();
    let port = serve(root.to_path_buf(), Site::Pages(html), tx)?;
    start_cef()?;
    let mut totals = Totals::default();
    for (index, page) in pages.iter().enumerate() {
        let url = format!("http://127.0.0.1:{port}{PAGE_PATH}{index}");
        totals.add(run_page(&url, page, &reports, index)?);
    }
    Ok(totals)
}

fn start_cef() -> Result<(), String> {
    match makepad_cef::bootstrap().map_err(|e| format!("CEF: {e}"))? {
        makepad_cef::BootstrapResult::Continue => {}
        makepad_cef::BootstrapResult::Exit(code) => std::process::exit(code),
    }
    makepad_cef::prepare().map_err(|e| format!("CEF: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_take_filters_and_ignore_libtest_flags() {
        let args = Args::parse(["--exact", "web_x::a b", "-Z", "unstable-options", "--report-time", "--skip", "slow", "--test-threads", "4", "other"].map(String::from).into_iter());
        assert_eq!(args.filters, ["web_x::a b", "other"], "filters");
        assert_eq!(args.skip, ["slow"], "--skip");
        assert!(args.exact && !args.list, "flags");
    }

    #[test]
    fn pages_stay_inside_the_root_and_quote_into_scripts() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(resolve(root, "/src/harness.js?x=1").is_some(), "a file under the root");
        for bad in ["/../Cargo.toml", "/src/../Cargo.toml", "//etc/passwd", "/.git/HEAD", "/src\\lib.rs", "/missing.js"] {
            assert!(resolve(root, bad).is_none(), "{bad} is refused");
        }
        assert_eq!(json_string("a\"</script>\n"), "\"a\\\"\\u003c/script>\\u000a\"", "a string that cannot end its script");
    }
}
