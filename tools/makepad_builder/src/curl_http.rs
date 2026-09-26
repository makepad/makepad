//! HTTP through the system's `curl` (Windows 10 1803 and later ship
//! `curl.exe` in System32; macOS and Linux have it too): one request, no
//! redirects followed (http.rs follows them), the whole body in memory,
//! progress while it arrives.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_head_bytes: usize,
    pub max_header_count: usize,
    pub max_header_line_bytes: usize,
    pub max_trailer_count: usize,
    pub max_trailer_bytes: usize,
    pub max_body_bytes: u64,
    pub max_chunk_line_bytes: usize,
    pub total_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_head_bytes: 64 * 1024,
            max_header_count: 128,
            max_header_line_bytes: 16 * 1024,
            max_trailer_count: 32,
            max_trailer_bytes: 8 * 1024,
            max_body_bytes: 64 * 1024 * 1024,
            max_chunk_line_bytes: 4096,
            total_timeout: Duration::from_secs(300),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Timeout,
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Timeout => write!(f, "timed out"),
            Error::Other(e) => write!(f, "{e}"),
        }
    }
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

type Progress = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

pub struct Request {
    url: String,
    method: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    limits: Limits,
    progress: Option<Progress>,
}

impl Request {
    fn new(url: &str, method: &'static str) -> Self {
        Request { url: url.to_owned(), method, headers: Vec::new(), body: Vec::new(), limits: Limits::default(), progress: None }
    }
    pub fn get(url: &str) -> Self {
        Self::new(url, "GET")
    }
    pub fn post(url: &str) -> Self {
        Self::new(url, "POST")
    }
    pub fn head(url: &str) -> Self {
        Self::new(url, "HEAD")
    }
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }
    pub fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }
    pub fn header(mut self, name: &str, value: &str) -> Result<Self, String> {
        let bad = |s: &str| s.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0);
        if name.is_empty() || bad(name) || bad(value) || name.contains(':') {
            return Err("invalid header".into());
        }
        self.headers.push((name.to_owned(), value.to_owned()));
        Ok(self)
    }
    pub fn on_body_progress(mut self, f: impl Fn(u64, Option<u64>) + Send + Sync + 'static) -> Self {
        self.progress = Some(Arc::new(f));
        self
    }
}

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch(kind: &str) -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("makepad-builder-{}-{n}.{kind}", std::process::id()))
}

/// Remove the listed files when dropped.
struct Scratch(Vec<PathBuf>);
impl Drop for Scratch {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

/// One request, redirects returned as they are.
pub fn request_no_redirect(req: Request) -> Result<Response, Error> {
    if !(req.url.starts_with("https://") || req.url.starts_with("http://") || req.url.starts_with("file://")) {
        return Err(Error::Other(format!("unsupported URL {}", req.url)));
    }
    let head_file = scratch("head");
    let body_file = scratch("body");
    let config_file = scratch("config");
    let upload_file = scratch("upload");
    let _cleanup = Scratch(vec![head_file.clone(), body_file.clone(), config_file.clone(), upload_file.clone()]);
    // Headers and the URL go through a config file, never the command line
    // (which other processes can read): they may carry the email.
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let mut config = String::new();
    config.push_str(&format!("url = {}\n", quote(&req.url)));
    for (name, value) in &req.headers {
        config.push_str(&format!("header = {}\n", quote(&format!("{name}: {value}"))));
    }
    fs::write(&config_file, config).map_err(|e| Error::Other(e.to_string()))?;
    let curl = if cfg!(windows) {
        std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join("System32").join("curl.exe"))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("curl.exe"))
    } else {
        PathBuf::from("curl")
    };
    let mut command = Command::new(curl);
    command
        .args(["--silent", "--show-error", "--globoff", "--proto", "=https,http,file"])
        .args(["--connect-timeout", "30"])
        .arg("--max-time")
        .arg(req.limits.total_timeout.as_secs().max(1).to_string())
        .arg("--max-filesize")
        .arg(req.limits.max_body_bytes.to_string())
        .arg("--dump-header")
        .arg(&head_file)
        .arg("--output")
        .arg(&body_file)
        .arg("--config")
        .arg(&config_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    match req.method {
        "HEAD" => {
            command.arg("--head");
        }
        "POST" => {
            fs::File::create(&upload_file)
                .and_then(|mut f| f.write_all(&req.body))
                .map_err(|e| Error::Other(e.to_string()))?;
            command.arg("--request").arg("POST").arg("--data-binary").arg(format!("@{}", upload_file.display()));
        }
        _ => {}
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|e| Error::Other(format!("curl: {e}")))?;
    let start = Instant::now();
    let mut total = None;
    let mut reported = u64::MAX;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| Error::Other(e.to_string()))? {
            break status;
        }
        if let Some(progress) = &req.progress {
            if total.is_none() {
                total = fs::read(&head_file).ok().and_then(|h| parse_head(&h).ok()).and_then(|r| {
                    r.header("content-length").and_then(|v| v.trim().parse::<u64>().ok())
                });
            }
            let loaded = fs::metadata(&body_file).map(|m| m.len()).unwrap_or(0);
            if loaded != reported {
                reported = loaded;
                progress(loaded, total);
            }
        }
        if start.elapsed() > req.limits.total_timeout + Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Timeout);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        let mut detail = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut err, &mut detail);
        }
        // curl's exit 28: operation timed out.
        if status.code() == Some(28) {
            return Err(Error::Timeout);
        }
        let detail = detail.trim().trim_start_matches("curl: ").to_owned();
        return Err(Error::Other(if detail.is_empty() { format!("curl failed ({status})") } else { detail }));
    }
    let head = fs::read(&head_file).unwrap_or_default();
    let mut response = if req.url.starts_with("file://") && head.is_empty() {
        Response { status: 200, headers: Vec::new(), body: Vec::new() }
    } else {
        parse_head(&head).map_err(Error::Other)?
    };
    response.body = if req.method == "HEAD" { Vec::new() } else { fs::read(&body_file).unwrap_or_default() };
    if let Some(progress) = &req.progress {
        progress(response.body.len() as u64, Some(response.body.len() as u64));
    }
    Ok(response)
}

/// The last response head in curl's `--dump-header` output (a 100 Continue
/// can come first).
fn parse_head(head: &[u8]) -> Result<Response, String> {
    let text = String::from_utf8_lossy(head);
    let blocks: Vec<&str> = text.split("\r\n\r\n").filter(|b| !b.trim().is_empty()).collect();
    let block = blocks.last().ok_or("no response")?;
    let mut lines = block.lines();
    let status_line = lines.next().ok_or("no status line")?;
    let status = status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("bad status line")?;
    let headers = lines
        .filter_map(|line| line.split_once(':').map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned())))
        .collect();
    Ok(Response { status, headers, body: Vec::new() })
}
