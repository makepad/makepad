//! The public localhost remote protocol, with ownership checked on every reply.
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub enum Failure {
    Message(String),
    Interrupted(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(s) | Self::Interrupted(s) => f.write_str(s),
        }
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::Message(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Failure>;

pub struct Remote {
    pub port: u16,
    pub user_seq: Option<u64>,
}

impl Remote {
    pub fn request(&self, route: &str, mutate: bool) -> Result<String> {
        let route = if mutate {
            let seq = self
                .user_seq
                .ok_or_else(|| Failure::Message("refusing mutation before /activity".into()))?;
            format!(
                "{route}{}if_user_seq={seq}",
                if route.contains('?') { '&' } else { '?' }
            )
        } else {
            route.to_string()
        };
        let address = SocketAddr::from(([127, 0, 0, 1], self.port));
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(1))?;
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        write!(
            stream,
            "GET {route} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            self.port
        )?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut bytes = Vec::new();
        let mut chunk = [0; 8192];
        loop {
            if Instant::now() >= deadline {
                return Err(Failure::Message(format!("GET {route}: response timeout")));
            }
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue
                }
                Err(e) => return Err(e.into()),
            }
            if bytes.len() > 16 * 1024 * 1024 {
                return Err(Failure::Message("remote response exceeds 16 MiB".into()));
            }
        }
        let split = bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| Failure::Message("invalid HTTP response".into()))?;
        let header =
            std::str::from_utf8(&bytes[..split]).map_err(|e| Failure::Message(e.to_string()))?;
        let status = header
            .lines()
            .next()
            .and_then(|s| s.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or_else(|| Failure::Message("missing HTTP status".into()))?;
        let field = |name: &str| {
            header.lines().skip(1).find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name).then_some(value.trim())
            })
        };
        let current_seq = field("X-Makepad-User-Seq").and_then(|s| s.parse::<u64>().ok());
        if status == 409
            || self
                .user_seq
                .is_some_and(|seq| current_seq.is_some_and(|now| now != seq))
        {
            return Err(Failure::Interrupted(format!("GET {route}: human intervention (HTTP {status}, user_seq={current_seq:?}); leaving WM running")));
        }
        if self.user_seq.is_some() && current_seq.is_none() {
            return Err(Failure::Interrupted(
                "missing ownership header; leaving the instance running".into(),
            ));
        }
        let body = &bytes[split + 4..];
        if let Some(length) = field("Content-Length") {
            if length.parse::<usize>().ok() != Some(body.len()) {
                return Err(Failure::Message("truncated HTTP body".into()));
            }
        }
        let body = String::from_utf8(body.to_vec()).map_err(|e| Failure::Message(e.to_string()))?;
        if status != 200 {
            return Err(Failure::Message(format!(
                "GET {route}: HTTP {status}: {body}"
            )));
        }
        Ok(body)
    }
}
