//! HTTP/1.1 over a TCP stream (plain, or TLS through the system OpenSSL on Linux): one request,
//! `Connection: close`, the response read to its end (Content-Length, chunked, or close).
use crate::{Request, Response, Sink, Url};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

const MAX_HEAD: usize = 64 * 1024;

pub(crate) fn connect(url: &Url, deadline: Instant) -> Result<TcpStream, String> {
    let host = url.host.trim_start_matches('[').trim_end_matches(']');
    let addrs = (host, url.port).to_socket_addrs().map_err(|e| format!("{}: {e}", url.host))?;
    let mut last = format!("{}: no address", url.host);
    for addr in addrs {
        let left = left(deadline)?;
        match TcpStream::connect_timeout(&addr, left.min(Duration::from_secs(30))) {
            Ok(s) => return Ok(s),
            Err(e) => last = format!("{}: {e}", url.host),
        }
    }
    Err(last)
}

pub(crate) fn left(deadline: Instant) -> Result<Duration, String> {
    let now = Instant::now();
    if now >= deadline {
        return Err("timed out".into());
    }
    Ok(deadline - now)
}

pub(crate) fn send(req: &Request, url: &Url, out: &mut dyn Write, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Response, String> {
    let deadline = Instant::now() + req.timeout;
    let tcp = connect(url, deadline)?;
    if url.https {
        #[cfg(target_os = "linux")]
        {
            let tls = crate::openssl::Tls::connect(tcp, url.host, deadline)?;
            return exchange(tls, req, url, deadline, out, progress);
        }
        #[cfg(not(target_os = "linux"))]
        return Err("https goes through the system client here".into());
    }
    exchange(Plain(tcp), req, url, deadline, out, progress)
}

/// A stream whose timeouts can follow the deadline.
pub(crate) trait Conn: Read + Write {
    fn set_deadline(&mut self, deadline: Instant) -> Result<(), String>;
}

struct Plain(TcpStream);

impl Read for Plain {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl Write for Plain {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
impl Conn for Plain {
    fn set_deadline(&mut self, deadline: Instant) -> Result<(), String> {
        let l = left(deadline)?;
        self.0.set_read_timeout(Some(l)).and_then(|_| self.0.set_write_timeout(Some(l))).map_err(|e| e.to_string())
    }
}

fn exchange(mut c: impl Conn, req: &Request, url: &Url, deadline: Instant, out: &mut dyn Write, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Response, String> {
    let mut head = format!("{} {} HTTP/1.1\r\nHost: {}", req.method, url.target, url.host);
    if url.port != if url.https { 443 } else { 80 } {
        head.push_str(&format!(":{}", url.port));
    }
    head.push_str("\r\nConnection: close\r\nAccept-Encoding: identity\r\n");
    if !req.headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("user-agent")) {
        head.push_str(&format!("User-Agent: {}\r\n", crate::USER_AGENT));
    }
    for (n, v) in req.headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    if !req.body.is_empty() || req.method == "POST" {
        head.push_str(&format!("Content-Length: {}\r\n", req.body.len()));
    }
    head.push_str("\r\n");
    c.set_deadline(deadline)?;
    c.write_all(head.as_bytes()).and_then(|_| c.write_all(req.body)).and_then(|_| c.flush()).map_err(|e| format!("send: {e}"))?;

    // the head
    let mut buf = Vec::with_capacity(8192);
    let mut chunk = [0u8; 16384];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err("response head too large".into());
        }
        c.set_deadline(deadline)?;
        let n = c.read(&mut chunk).map_err(|e| format!("receive: {e}"))?;
        if n == 0 {
            return Err("connection closed before the response".into());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let text = std::str::from_utf8(&buf[..end]).map_err(|_| "response head is not text")?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .strip_prefix("HTTP/1.")
        .and_then(|r| r.get(2..5))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line {status_line:?}"))?;
    let headers: Vec<(String, String)> =
        lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.trim().to_string(), v.trim().to_string())).collect();
    let resp = Response { status, headers };
    let rest = buf[end + 4..].to_vec();
    let no_body = req.method == "HEAD" || status == 204 || status == 304 || (100..200).contains(&status);
    let chunked = resp.header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked"));
    let length = if chunked {
        None
    } else {
        match resp.header("content-length") {
            Some(v) => Some(v.parse::<u64>().map_err(|_| "bad Content-Length")?),
            None => None,
        }
    };
    let mut sink = Sink { out, progress, received: 0, expected: length, max: req.max_body };
    if no_body {
        return Ok(resp);
    }
    let mut reader = Body { c: &mut c, pending: rest, deadline };
    if chunked {
        loop {
            let line = reader.line()?;
            let size = u64::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).map_err(|_| "bad chunk size")?;
            if size == 0 {
                // trailers up to the empty line
                while !reader.line()?.is_empty() {}
                break;
            }
            reader.copy(Some(size), &mut sink)?;
            if !reader.line()?.is_empty() {
                return Err("bad chunk end".into());
            }
        }
    } else {
        reader.copy(length, &mut sink)?;
    }
    Ok(resp)
}

struct Body<'a, C: Conn> {
    c: &'a mut C,
    pending: Vec<u8>,
    deadline: Instant,
}

impl<C: Conn> Body<'_, C> {
    fn fill(&mut self) -> Result<bool, String> {
        let mut chunk = [0u8; 65536];
        self.c.set_deadline(self.deadline)?;
        let n = self.c.read(&mut chunk).map_err(|e| format!("receive: {e}"))?;
        self.pending.extend_from_slice(&chunk[..n]);
        Ok(n > 0)
    }
    fn line(&mut self) -> Result<String, String> {
        loop {
            if let Some(i) = self.pending.windows(2).position(|w| w == b"\r\n") {
                let line = String::from_utf8_lossy(&self.pending[..i]).into_owned();
                self.pending.drain(..i + 2);
                return Ok(line);
            }
            if self.pending.len() > 4096 || !self.fill()? {
                return Err("bad chunked body".into());
            }
        }
    }
    /// `n` bytes, or everything up to the end of the stream.
    fn copy(&mut self, n: Option<u64>, sink: &mut Sink) -> Result<(), String> {
        let mut left = n;
        loop {
            if self.pending.is_empty() && left != Some(0) && !self.fill()? {
                return match left {
                    None => Ok(()),
                    Some(_) => Err("connection closed before the end of the body".into()),
                };
            }
            let take = match left {
                Some(l) => (l.min(self.pending.len() as u64)) as usize,
                None => self.pending.len(),
            };
            sink.put(&self.pending[..take])?;
            self.pending.drain(..take);
            if let Some(l) = &mut left {
                *l -= take as u64;
                if *l == 0 {
                    return Ok(());
                }
            }
        }
    }
}
