//! The jail's one-hop, allowlisted network exit. Client authority and credentials
//! are never forwarded; every upstream address is constructed here or read from Conf.

use super::{hub, Conf};
use makepad_network::blocking_http::{self as http, Limits, Request};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::time::Duration;

const HEAD_MAX: usize = 16 * 1024;
const GIT_BUFFER: usize = 64 * 1024;
const GIT_MAX: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Health,
    GhApi {
        repo: &'static str,
        path: String,
        diff: bool,
    },
    GitRefs {
        repo: &'static str,
    },
    GitPack {
        repo: &'static str,
    },
    Qwen {
        path: String,
    },
    Model,
}

fn number(s: &str) -> bool {
    (1..=9).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

fn query(q: &str, keys: &[&str]) -> bool {
    q.split('&').all(|part| {
        part.split_once('=').is_some_and(|(key, value)| {
            keys.contains(&key)
                && (1..=32).contains(&value.len())
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b":.-".contains(&b))
        })
    })
}

/// Match the literal request target, without URL decoding or normalization.
pub fn route(method: &str, target: &str) -> Option<Route> {
    if target.contains('%') || target.contains("..") {
        return None;
    }
    match (method, target) {
        ("GET", "/health") => return Some(Route::Health),
        ("POST", "/generate") => {
            return Some(Route::Qwen {
                path: target.into(),
            })
        }
        ("POST", "/v1/messages") => return Some(Route::Model),
        _ => {}
    }
    let (path, q) = target
        .split_once('?')
        .map_or((target, None), |(p, q)| (p, Some(q)));
    let parts: Vec<_> = path.split('/').collect();
    match parts.as_slice() {
        ["", "job", id] | ["", "job", id, "cancel"] if q.is_none() => {
            if (1..=64).contains(&id.len())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                && ((parts.len() == 3 && method == "GET") || (parts.len() == 4 && method == "POST"))
            {
                return Some(Route::Qwen {
                    path: target.into(),
                });
            }
            return None;
        }
        _ => {}
    }
    let repo = match parts.get(2).copied()? {
        "relax" => "relax",
        "makepad" => "makepad",
        _ => return None,
    };
    let page_query = || q.is_none_or(|q| query(q, &["per_page", "page"]));
    match (method, parts.as_slice()) {
        ("GET", ["", "gh", _, "issues"])
            if q.is_some_and(|q| {
                query(
                    q,
                    &["state", "sort", "direction", "since", "per_page", "page"],
                )
            }) => {}
        ("GET", ["", "gh", _, "issues" | "pulls", n]) if number(n) && q.is_none() => {}
        ("GET", ["", "gh", _, "issues", n, "comments"])
        | ("GET", ["", "gh", _, "pulls", n, "commits"])
            if number(n) && page_query() => {}
        ("GET", ["", "gh", _, "pulls", n]) if q.is_none() => {
            let n = n.strip_suffix(".diff").filter(|n| number(n))?;
            return Some(Route::GhApi {
                repo,
                path: format!("pulls/{n}"),
                diff: true,
            });
        }
        ("GET", ["", "git", _, "info", "refs"]) if q == Some("service=git-upload-pack") => {
            return Some(Route::GitRefs { repo });
        }
        ("POST", ["", "git", _, "git-upload-pack"]) if q.is_none() => {
            return Some(Route::GitPack { repo });
        }
        _ => return None,
    }
    Some(Route::GhApi {
        repo,
        path: target.strip_prefix(&format!("/gh/{repo}/"))?.into(),
        diff: false,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Failure(u16, &'static str);

const BAD: Failure = Failure(400, "bad request");
const LARGE: Failure = Failure(413, "body too large");
const UPSTREAM: Failure = Failure(502, "upstream request failed");

impl From<io::Error> for Failure {
    fn from(_: io::Error) -> Self {
        BAD
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Framing {
    Length(u64),
    Chunked,
}

#[derive(Debug)]
struct Head {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    framing: Framing,
    expect: bool,
}

fn token(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn header(line: &str) -> Result<(String, String), Failure> {
    let (name, value) = line.split_once(':').ok_or(BAD)?;
    if name.is_empty()
        || !name.bytes().all(token)
        || !value.bytes().all(|b| b == b'\t' || (32..=126).contains(&b))
    {
        return Err(BAD);
    }
    Ok((
        name.to_ascii_lowercase(),
        value.trim_matches([' ', '\t']).into(),
    ))
}

impl Head {
    fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

fn parse_head(bytes: &[u8]) -> Result<Head, Failure> {
    if bytes.len() > HEAD_MAX {
        return Err(Failure(431, "head too large"));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| BAD)?
        .strip_suffix("\r\n\r\n")
        .ok_or(BAD)?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next().ok_or(BAD)?.split(' ');
    let method = first.next().ok_or(BAD)?;
    let target = first.next().ok_or(BAD)?;
    let version = first.next().ok_or(BAD)?;
    if first.next().is_some()
        || method.is_empty()
        || !method.bytes().all(token)
        || target.is_empty()
        || !target.bytes().all(|b| (33..=126).contains(&b))
        || !matches!(version, "HTTP/1.0" | "HTTP/1.1")
    {
        return Err(BAD);
    }
    let mut headers = Vec::new();
    for line in lines {
        let pair = header(line)?;
        // Duplicated authority, framing or forwarded headers must never be
        // interpreted differently by this proxy and an upstream HTTP parser.
        if headers.iter().any(|(name, _)| name == &pair.0) {
            return Err(BAD);
        }
        headers.push(pair);
    }
    let mut head = Head {
        method: method.into(),
        target: target.into(),
        headers,
        framing: Framing::Length(0),
        expect: false,
    };
    head.framing = match (head.get("content-length"), head.get("transfer-encoding")) {
        (Some(_), Some(_)) => return Err(BAD),
        (Some(n), None) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
            Framing::Length(n.parse().map_err(|_| LARGE)?)
        }
        (None, Some(te)) if te.eq_ignore_ascii_case("chunked") => Framing::Chunked,
        (None, None) => Framing::Length(0),
        _ => return Err(BAD),
    };
    head.expect = match head.get("expect") {
        None => false,
        Some(s) if s.eq_ignore_ascii_case("100-continue") => true,
        _ => return Err(Failure(417, "unsupported expectation")),
    };
    Ok(head)
}

/// Read one CRLF line without ever allocating past the supplied head budget.
fn line(reader: &mut impl BufRead, budget: usize) -> Result<Vec<u8>, Failure> {
    let mut out = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Err(BAD);
        }
        let n = buf
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        if n > budget.saturating_sub(out.len()) {
            return Err(Failure(431, "head too large"));
        }
        out.extend_from_slice(&buf[..n]);
        reader.consume(n);
        if out.last() == Some(&b'\n') {
            return if out.ends_with(b"\r\n") {
                Ok(out)
            } else {
                Err(BAD)
            };
        }
    }
}

fn read_head(reader: &mut impl BufRead) -> Result<Vec<u8>, Failure> {
    let mut bytes = Vec::new();
    loop {
        let next = line(reader, HEAD_MAX - bytes.len())?;
        let done = next == b"\r\n";
        bytes.extend_from_slice(&next);
        if done {
            return Ok(bytes);
        }
    }
}

fn append_body(
    reader: &mut impl Read,
    out: &mut Vec<u8>,
    mut n: u64,
    max: u64,
) -> Result<(), Failure> {
    if n > max.saturating_sub(out.len() as u64) {
        return Err(LARGE);
    }
    let mut buf = [0; 16 * 1024];
    while n > 0 {
        let want = n.min(buf.len() as u64) as usize;
        let got = reader.read(&mut buf[..want])?;
        if got == 0 {
            return Err(BAD);
        }
        out.extend_from_slice(&buf[..got]);
        n -= got as u64;
    }
    Ok(())
}

fn read_body(reader: &mut impl BufRead, framing: &Framing, max: u64) -> Result<Vec<u8>, Failure> {
    let mut out = Vec::new();
    if let Framing::Length(n) = framing {
        append_body(reader, &mut out, *n, max)?;
        return Ok(out);
    }
    loop {
        let size = line(reader, HEAD_MAX)?;
        let size = std::str::from_utf8(&size[..size.len() - 2]).map_err(|_| BAD)?;
        let (hex, extension) = size
            .split_once(';')
            .map_or((size, None), |(h, e)| (h, Some(e)));
        if hex.is_empty()
            || !hex.bytes().all(|b| b.is_ascii_hexdigit())
            || extension.is_some_and(|e| !e.bytes().all(|b| (32..=126).contains(&b)))
        {
            return Err(BAD);
        }
        let n = u64::from_str_radix(hex, 16).map_err(|_| LARGE)?;
        if n == 0 {
            let mut budget = HEAD_MAX;
            loop {
                let trailer = line(reader, budget)?;
                budget -= trailer.len();
                if trailer == b"\r\n" {
                    return Ok(out);
                }
                let (name, _) =
                    header(std::str::from_utf8(&trailer[..trailer.len() - 2]).map_err(|_| BAD)?)?;
                if matches!(
                    name.as_str(),
                    "content-length" | "transfer-encoding" | "host" | "x-intake-item"
                ) {
                    return Err(BAD);
                }
            }
        }
        append_body(reader, &mut out, n, max)?;
        let mut end = [0; 2];
        reader.read_exact(&mut end)?;
        if end != *b"\r\n" {
            return Err(BAD);
        }
    }
}

fn item(value: &str) -> bool {
    matches!(value, "eval" | "canary")
        || value
            .split_once('#')
            .is_some_and(|(repo, n)| matches!(repo, "relax" | "makepad") && number(n))
}

struct Client<'a> {
    stream: &'a mut TcpStream,
    started: bool,
    status: u16,
    bytes: u64,
}

impl Client<'_> {
    fn head(&mut self, status: u16, content_type: &str, framing: &str) -> io::Result<()> {
        self.started = true;
        self.status = status;
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            403 => "Forbidden",
            413 => "Payload Too Large",
            417 => "Expectation Failed",
            431 => "Request Header Fields Too Large",
            502 => "Bad Gateway",
            _ => "Upstream",
        };
        write!(self.stream, "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nCache-Control: no-cache\r\nConnection: close\r\n{framing}\r\n\r\n")
    }

    fn body(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            match self.stream.write(bytes) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    self.bytes += n as u64;
                    bytes = &bytes[n..];
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn send(&mut self, status: u16, content_type: &str, bytes: &[u8]) -> io::Result<()> {
        self.head(
            status,
            content_type,
            &format!("Content-Length: {}", bytes.len()),
        )?;
        self.body(bytes)
    }

    fn error(&mut self, Failure(status, message): Failure) {
        if !self.started {
            let _ = self.send(
                status,
                "application/json",
                format!("{{\"error\":\"{message}\"}}").as_bytes(),
            );
        }
    }
}

struct GitWriter<'a, 'b> {
    client: &'a mut Client<'b>,
    buffer: Vec<u8>,
    content_type: &'static str,
}

impl GitWriter<'_, '_> {
    fn chunk(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        write!(self.client.stream, "{:x}\r\n", bytes.len())?;
        self.client.body(bytes)?;
        self.client.stream.write_all(b"\r\n")
    }
}

impl Write for GitWriter<'_, '_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let mut rest = bytes;
        if !self.client.started {
            let n = rest.len().min(GIT_BUFFER - self.buffer.len());
            self.buffer.extend_from_slice(&rest[..n]);
            rest = &rest[n..];
            if self.buffer.len() < GIT_BUFFER {
                return Ok(bytes.len());
            }
            self.client
                .head(200, self.content_type, "Transfer-Encoding: chunked")?;
            let buffer = std::mem::take(&mut self.buffer);
            self.chunk(&buffer)?;
        }
        self.chunk(rest)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.client.stream.flush()
    }
}

fn git_headers(head: &Head) -> Result<(), Failure> {
    if head.method == "POST"
        && head.get("content-type") != Some("application/x-git-upload-pack-request")
    {
        return Err(Failure(400, "invalid git content type"));
    }
    if head.get("content-encoding").is_some_and(|s| s != "gzip")
        || head.get("git-protocol").is_some_and(|s| {
            s.is_empty()
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"=:.-".contains(&b))
        })
    {
        return Err(Failure(400, "invalid git header"));
    }
    Ok(())
}

fn forward(
    conf: &Conf,
    head: &Head,
    route: &Route,
    body: Vec<u8>,
    client: &mut Client<'_>,
) -> Result<(), Failure> {
    match route {
        Route::Health => client.send(200, "application/json", b"{\"ok\":true}")?,
        Route::GhApi { repo, path, diff } => {
            let accept = if *diff {
                "application/vnd.github.diff"
            } else {
                "application/vnd.github+json"
            };
            let request = Request::get(format!(
                "https://api.github.com/repos/makepad/{repo}/{path}"
            ))
            .header("User-Agent", "makepad-intake")
            .map_err(|_| UPSTREAM)?
            .header("X-GitHub-Api-Version", "2022-11-28")
            .map_err(|_| UPSTREAM)?
            .header("Accept", accept)
            .map_err(|_| UPSTREAM)?
            .limits(Limits {
                max_body_bytes: 8 << 20,
                idle_timeout: Duration::from_secs(60),
                ..Limits::default()
            });
            let response = http::request_no_redirect(request).map_err(|e| {
                if e == http::Error::ResponseTooLarge {
                    LARGE
                } else {
                    UPSTREAM
                }
            })?;
            let bytes = if (300..400).contains(&response.status) {
                &[][..]
            } else {
                &response.body
            };
            client.send(
                response.status,
                if *diff {
                    "text/plain"
                } else {
                    "application/json"
                },
                bytes,
            )?;
        }
        Route::GitRefs { repo } | Route::GitPack { repo } => {
            let refs = matches!(route, Route::GitRefs { .. });
            let tail = if refs {
                "info/refs?service=git-upload-pack"
            } else {
                "git-upload-pack"
            };
            let url = format!("https://github.com/makepad/{repo}.git/{tail}");
            let mut request = if refs {
                Request::get(url)
            } else {
                Request::post(url).body(body)
            };
            request = request
                .header("User-Agent", "git/2-makepad-intake")
                .map_err(|_| UPSTREAM)?;
            for name in ["content-type", "content-encoding", "git-protocol", "accept"] {
                if let Some(value) = head.get(name) {
                    request = request.header(name, value).map_err(|_| BAD)?;
                }
            }
            request = request.limits(Limits {
                max_body_bytes: usize::try_from(GIT_MAX).map_err(|_| UPSTREAM)?,
                idle_timeout: Duration::from_secs(300),
                ..Limits::default()
            });
            let content_type = if refs {
                "application/x-git-upload-pack-advertisement"
            } else {
                "application/x-git-upload-pack-result"
            };
            let mut writer = GitWriter {
                client,
                buffer: Vec::with_capacity(GIT_BUFFER),
                content_type,
            };
            let response =
                http::request_to_writer_no_redirect(request, &mut writer).map_err(|_| UPSTREAM)?;
            if writer.client.started {
                // A 200 head was already emitted. Any later error/non-200 must
                // remain an incomplete chunked response, never a successful EOF.
                if response.status == 200 {
                    writer.client.stream.write_all(b"0\r\n\r\n")?;
                }
            } else {
                writer
                    .client
                    .send(response.status, content_type, &writer.buffer)?;
            }
        }
        Route::Qwen { .. } | Route::Model => {
            let (base, path, max, timeout) = match route {
                Route::Qwen { path } => (&conf.qwen_hub, path.as_str(), 4 << 20, 120),
                _ => (&conf.model_hub, "/v1/messages", 32 << 20, 900),
            };
            let value = head
                .get("x-intake-item")
                .filter(|s| item(s))
                .ok_or(Failure(400, "invalid intake item"))?;
            let (status, bytes) = hub::request(
                base,
                &head.method,
                path,
                &[("X-Intake-Item", value)],
                Some(&body),
                max,
                Duration::from_secs(timeout),
            )
            .map_err(|_| UPSTREAM)?;
            // hub::request permits some framing overhead in its receive buffer;
            // independently enforce the proxy's exact decoded answer cap.
            if bytes.len() > max {
                return Err(UPSTREAM);
            }
            client.send(status, "application/json", &bytes)?;
        }
    }
    Ok(())
}

fn log_word(s: &str) -> String {
    s.bytes()
        .take(80)
        .map(|b| {
            if (33..=126).contains(&b) {
                b as char
            } else {
                '?'
            }
        })
        .collect()
}

fn serve(stream: &mut TcpStream, conf: &Conf) {
    let mut method = String::from("-");
    let mut kind = String::from("invalid");
    let mut repo = "-";
    let mut intake_item = String::from("-");
    let mut client = Client {
        stream,
        started: false,
        status: 400,
        bytes: 0,
    };
    let result = (|| -> Result<(), Failure> {
        client
            .stream
            .set_read_timeout(Some(Duration::from_secs(60)))?;
        client
            .stream
            .set_write_timeout(Some(Duration::from_secs(60)))?;
        let mut reader = BufReader::new(&mut *client.stream);
        let bytes = read_head(&mut reader)?;
        // Even a malformed head gets bounded, control-free request-line logging.
        let first = bytes
            .split(|b| *b == b'\r' || *b == b'\n')
            .next()
            .unwrap_or(&[]);
        let mut words = first.split(|b| *b == b' ');
        method = log_word(&String::from_utf8_lossy(words.next().unwrap_or(&[])));
        kind = log_word(&String::from_utf8_lossy(words.next().unwrap_or(&[])));
        let head = parse_head(&bytes)?;
        let route = route(&head.method, &head.target).ok_or(Failure(403, "not an intake route"))?;
        let (label, max) = match &route {
            Route::Health => ("health", 0),
            Route::GhApi { repo: r, .. } => {
                repo = r;
                ("gh-api", 0)
            }
            Route::GitRefs { repo: r } => {
                repo = r;
                ("git-refs", 0)
            }
            Route::GitPack { repo: r } => {
                repo = r;
                ("git-pack", GIT_MAX)
            }
            Route::Qwen { .. } => ("qwen", 1 << 20),
            Route::Model => ("model", 8 << 20),
        };
        kind = label.into();
        if let Some(value) = head.get("x-intake-item").filter(|s| item(s)) {
            intake_item = value.into();
        }
        if matches!(route, Route::Qwen { .. } | Route::Model) && intake_item == "-" {
            return Err(Failure(400, "invalid intake item"));
        }
        if matches!(route, Route::GitRefs { .. } | Route::GitPack { .. }) {
            git_headers(&head)?;
        }
        if matches!(head.framing, Framing::Length(n) if n > max) {
            return Err(LARGE);
        }
        if head.expect {
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        }
        let body = read_body(&mut reader, &head.framing, max)?;
        drop(reader);
        forward(conf, &head, &route, body, &mut client)
    })();
    if let Err(error) = result {
        client.error(error);
    }
    eprintln!(
        "intake-proxy {method} {kind} {repo} {} {} {intake_item}",
        client.status, client.bytes
    );
}

/// `intake jail proxy --conf <path>`: sequential service, one request per connection.
pub fn cli(args: &[&str]) -> i32 {
    let ["--conf", path] = args else {
        eprintln!("usage: makepad devhub intake jail proxy --conf <path>");
        return 2;
    };
    let start = || -> Result<(Conf, TcpListener), String> {
        let conf = Conf::read(path)?;
        let address: SocketAddrV4 = conf
            .proxy
            .parse()
            .map_err(|_| "proxy must be 127.0.0.1:<port>".to_string())?;
        if address.ip() != &Ipv4Addr::LOCALHOST {
            return Err("proxy must be 127.0.0.1:<port>".into());
        }
        for base in [&conf.qwen_hub, &conf.model_hub] {
            let (host, port) = hub::host_port(base)?;
            makepad_network::fleet::id::mark_fleet_endpoint(&format!("{host}:{port}"));
        }
        let listener = TcpListener::bind(address).map_err(|e| format!("proxy bind: {e}"))?;
        Ok((conf, listener))
    };
    let (conf, listener) = match start() {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("intake-proxy: {e}");
            return 1;
        }
    };
    for connection in listener.incoming() {
        match connection {
            Ok(mut stream) => serve(&mut stream, &conf),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                eprintln!("intake-proxy accept: {e}");
                return 1;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_allow_only_the_jail_protocol() {
        assert_eq!(route("GET", "/health"), Some(Route::Health), "health route");
        assert_eq!(
            route("POST", "/v1/messages"),
            Some(Route::Model),
            "model route"
        );
        for repo in ["relax", "makepad"] {
            for path in ["issues?state=open&sort=updated&direction=desc&since=2026-11-01T00:00:00Z&per_page=100&page=1", "issues/1", "issues/123456789", "issues/1/comments", "issues/1/comments?per_page=10&page=2", "pulls/2", "pulls/2/commits", "pulls/2/commits?page=2", "pulls/2/commits?per_page=10"] {
                assert_eq!(route("GET", &format!("/gh/{repo}/{path}")), Some(Route::GhApi { repo, path: path.into(), diff: false }), "github {repo}/{path}");
            }
            assert_eq!(
                route("GET", &format!("/gh/{repo}/pulls/1.diff")),
                Some(Route::GhApi {
                    repo,
                    path: "pulls/1".into(),
                    diff: true
                }),
                "diff accept mapping"
            );
            assert_eq!(
                route(
                    "GET",
                    &format!("/git/{repo}/info/refs?service=git-upload-pack")
                ),
                Some(Route::GitRefs { repo }),
                "read-only refs"
            );
            assert_eq!(
                route("POST", &format!("/git/{repo}/git-upload-pack")),
                Some(Route::GitPack { repo }),
                "read-only pack"
            );
        }
        for (method, path) in [
            ("POST", "/generate"),
            ("GET", "/job/a-Z_09"),
            ("POST", "/job/a-Z_09/cancel"),
        ] {
            assert_eq!(
                route(method, path),
                Some(Route::Qwen { path: path.into() }),
                "qwen {method} {path}"
            );
        }
        for (method, path) in [
            ("POST", "/gh/relax/issues/1"),
            ("GET", "/gh/other/issues/1"),
            ("GET", "/gh/relax/issues/1/../2"),
            ("GET", "/gh/relax/issues/%31"),
            ("GET", "/gh/relax/issues/1/comments?foo=1"),
            ("GET", "/gh/relax/issues/1234567890"),
            ("POST", "/git/relax/git-receive-pack"),
            ("GET", "/git/relax/info/refs?service=git-receive-pack"),
            ("GET", "/gh/relax/pulls/1/files"),
            ("GET", "/v1/models"),
            ("GET", "/v1/messages"),
            ("GET", "/job/a/b"),
            ("GET", "/gh/relax/issues"),
            ("GET", "/gh/relax/issues?"),
            ("GET", "/gh/relax/issues?state="),
            ("GET", "/gh/relax/issues?state=a+b"),
            ("GET", "/gh/relax/issues/1?state=open"),
            ("GET", "/gh/relax/pulls/1.diff?page=1"),
            ("GET", "/git/relax/info/refs?service=git-upload-pack&page=1"),
            ("GET", "/health?x=1"),
            ("POST", "/generate/"),
            ("GET", "/job/"),
            ("POST", "/job/a"),
            ("GET", "/job/a/cancel"),
            ("GET", "/job/a?x=1"),
            ("CONNECT", "/health"),
        ] {
            assert_eq!(route(method, path), None, "refuse {method} {path}");
        }
        assert!(
            route("GET", &format!("/job/{}", "a".repeat(64))).is_some(),
            "64-byte job id accepted"
        );
        assert!(
            route("GET", &format!("/job/{}", "a".repeat(65))).is_none(),
            "long job id refused"
        );
        assert!(
            route("GET", &format!("/gh/relax/issues?state={}", "a".repeat(33))).is_none(),
            "query value capped"
        );
        for value in ["eval", "canary", "relax#1", "makepad#123456789"] {
            assert!(item(value), "valid item {value}");
        }
        for value in ["", "other#1", "relax#", "makepad#1234567890", "eval\r\n"] {
            assert!(!item(value), "invalid item {value:?}");
        }
    }

    #[test]
    fn heads_are_bounded_and_unambiguous() {
        let mut input = &b"POST /generate HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\nExpect: 100-continue\r\nX-Intake-Item: eval\r\n\r\n{}"[..];
        let head = parse_head(&read_head(&mut input).unwrap()).unwrap();
        assert_eq!(
            (&*head.method, &*head.target),
            ("POST", "/generate"),
            "request line preserved"
        );
        assert_eq!(head.framing, Framing::Length(2), "length parsed");
        assert!(head.expect, "continue recognized");
        assert_eq!(
            head.get("x-intake-item"),
            Some("eval"),
            "header name normalized"
        );
        assert_eq!(input, b"{}", "head reader leaves body intact");
        let chunked =
            parse_head(b"POST /generate HTTP/1.1\r\nTransfer-Encoding: Chunked\r\n\r\n").unwrap();
        assert_eq!(
            chunked.framing,
            Framing::Chunked,
            "chunked case insensitive"
        );
        for bytes in [
            &b"GET /health HTTP/1.1\r\nContent-Length: 1\r\ncontent-length: 1\r\n\r\n"[..],
            b"POST /generate HTTP/1.1\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n",
            b"POST /generate HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
            b"POST /generate HTTP/1.1\r\nContent-Length: +1\r\n\r\n",
            b"GET /health HTTP/1.1\r\nHost : localhost\r\n\r\n",
            b"GET /health HTTP/1.1\r\nX: a\nInjected: b\r\n\r\n",
            b"GET /health HTTP/1.1\r\nX: a\r\n folded\r\n\r\n",
            b"GET  /health HTTP/1.1\r\n\r\n",
            b"GET /health HTTP/1.1\r\nX: \0\r\n\r\n",
            b"GET /health HTTP/1.1\r\nExpect: other\r\n\r\n",
        ] {
            assert!(parse_head(bytes).is_err(), "invalid head {bytes:?}");
        }
        let big = vec![b'x'; HEAD_MAX + 1];
        assert_eq!(
            read_head(&mut big.as_slice()).unwrap_err().0,
            431,
            "head read bounded"
        );
        assert_eq!(parse_head(&big).unwrap_err().0, 431, "head parser bounded");
        let git = parse_head(b"POST /git/relax/git-upload-pack HTTP/1.1\r\nContent-Type: application/x-git-upload-pack-request\r\nContent-Encoding: gzip\r\nGit-Protocol: version=2\r\n\r\n").unwrap();
        assert!(git_headers(&git).is_ok(), "permitted git headers");
        assert!(
            git_headers(&head).is_err(),
            "git pack requires content type"
        );
    }

    #[test]
    fn bodies_decode_chunks_and_enforce_the_route_cap() {
        let mut input = &b"3;ignored=yes\r\nabc\r\n2\r\nde\r\n0\r\nX-Checksum: ok\r\n\r\nnext"[..];
        assert_eq!(
            read_body(&mut input, &Framing::Chunked, 5).unwrap(),
            b"abcde",
            "chunks combined exactly at cap"
        );
        assert_eq!(input, b"next", "decoder consumes only one body");
        assert_eq!(
            read_body(&mut &b"0\r\n\r\n"[..], &Framing::Chunked, 0).unwrap(),
            b"",
            "empty body"
        );
        assert_eq!(
            read_body(&mut &b"{}next"[..], &Framing::Length(2), 2).unwrap(),
            b"{}",
            "content length framing"
        );
        assert_eq!(
            read_body(&mut &b"6\r\n"[..], &Framing::Chunked, 5),
            Err(LARGE),
            "oversized chunk rejected before data read"
        );
        assert_eq!(
            read_body(&mut &b"3\r\nabc\r\n3\r\n"[..], &Framing::Chunked, 5),
            Err(LARGE),
            "cumulative cap"
        );
        assert_eq!(
            read_body(&mut &b""[..], &Framing::Length(6), 5),
            Err(LARGE),
            "oversized length rejected before allocation"
        );
        for bytes in [
            &b"z\r\n"[..],
            b"1\r\naXX0\r\n\r\n",
            b"2\r\na",
            b"0\r\n",
            b"1\na\r\n0\r\n\r\n",
            b"0\r\nContent-Length: 0\r\n\r\n",
            b"0\r\ninvalid\r\n\r\n",
        ] {
            assert!(
                read_body(&mut &bytes[..], &Framing::Chunked, 5).is_err(),
                "invalid chunked body {bytes:?}"
            );
        }
        assert!(
            read_body(&mut &b"a"[..], &Framing::Length(2), 2).is_err(),
            "truncated length body"
        );
    }
}
