//! Fetching an item in the jail, through the proxy (the jail's only network): the issue, its
//! comments and, for a PR, its commits and its diff in the API's diff media type. No git objects of
//! a PR are ever fetched. The text becomes [`Field`]s for the scan and the prefilter; the numbers
//! become the item record. A byte cap over the whole item parks it as oversize.

use super::item::{ItemKind, Repo};
use super::text::{Field, Source};
use makepad_strict_json::{self as json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// The whole item's text, all sources together.
pub const MAX_ITEM_BYTES: usize = 2 << 20;
const PAGE: u32 = 100;

/// One request through the proxy: (status, body); a body over `max` answers as 413.
pub fn call(proxy: &str, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&[u8]>, max: usize, timeout: Duration) -> Result<(u16, Vec<u8>), String> {
    let mut s = TcpStream::connect(proxy).map_err(|e| format!("proxy {proxy}: {e}"))?;
    s.set_read_timeout(Some(timeout)).map_err(|e| format!("proxy: {e}"))?;
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {proxy}\r\nConnection: close\r\n");
    for (k, v) in headers {
        if k.contains([':', '\r', '\n']) || v.contains(['\r', '\n']) {
            return Err("a header with a line break".into());
        }
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        head.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).and_then(|_| s.write_all(body.unwrap_or(&[]))).map_err(|e| format!("proxy write: {e}"))?;
    let mut buf = Vec::new();
    s.take((max + 64 * 1024) as u64).read_to_end(&mut buf).map_err(|e| format!("proxy read: {e}"))?;
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n").ok_or("proxy: no complete head")?;
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| "proxy: head is not utf-8".to_string())?;
    let status: u16 = head.split(' ').nth(1).and_then(|s| s.parse().ok()).ok_or("proxy: no status")?;
    if head.lines().any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:")) {
        return Err("proxy: a chunked answer where none is expected".into());
    }
    let body = buf[end + 4..].to_vec();
    if body.len() > max {
        return Ok((413, Vec::new()));
    }
    Ok((status, body))
}

/// One GET through the proxy.
pub fn get(proxy: &str, path: &str, max: usize) -> Result<(u16, Vec<u8>), String> {
    call(proxy, "GET", path, &[], None, max, Duration::from_secs(120))
}

fn get_json(proxy: &str, path: &str, budget: &mut usize) -> Result<Value, Fetch> {
    let (status, body) = get(proxy, path, *budget).map_err(Fetch::Failed)?;
    if status == 413 {
        return Err(Fetch::Oversize);
    }
    if status != 200 {
        return Err(Fetch::Failed(format!("{path}: status {status}")));
    }
    *budget = budget.saturating_sub(body.len());
    json::parse(&body).map_err(|e| Fetch::Failed(format!("{path}: {e}")))
}

#[derive(Debug, PartialEq)]
pub enum Fetch {
    Oversize,
    Failed(String),
}

/// One item as fetched: the record's numbers and the text, as fields.
#[derive(Debug, Default)]
pub struct Fetched {
    pub kind: Option<ItemKind>,
    pub author_id: u64,
    pub author_login: String,
    pub updated: String,
    pub head_sha: Option<String>,
    pub comments: u32,
    pub bytes: u64,
    pub fields: Vec<Field>,
}

fn text_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn user_id(v: &Value) -> u64 {
    v.get("user").and_then(|u| u.get("id")).and_then(Value::as_u64).unwrap_or(0)
}

/// An RFC 3339 time as GitHub writes it (`2026-11-14T13:59:27Z`): the only form kept.
fn valid_time(t: &str) -> bool {
    t.len() == 20 && t.bytes().enumerate().all(|(i, b)| match i {
        4 | 7 => b == b'-',
        10 => b == b'T',
        13 | 16 => b == b':',
        19 => b == b'Z',
        _ => b.is_ascii_digit(),
    })
}

/// The issue or PR, its comments, and for a PR its commits and diff.
pub fn fetch(proxy: &str, repo: Repo, number: u32) -> Result<Fetched, Fetch> {
    let r = repo.word();
    let mut budget = MAX_ITEM_BYTES;
    let issue = get_json(proxy, &format!("/gh/{r}/issues/{number}"), &mut budget)?;
    let is_pr = issue.get("pull_request").is_some_and(|p| !matches!(p, Value::Null));
    let mut f = Fetched {
        kind: Some(if is_pr { ItemKind::Pr } else { ItemKind::Issue }),
        author_id: user_id(&issue),
        author_login: issue.get("user").map(|u| text_of(u, "login")).unwrap_or_default(),
        updated: Some(text_of(&issue, "updated_at")).filter(|t| valid_time(t)).unwrap_or_default(),
        comments: issue.get("comments").and_then(Value::as_u64).unwrap_or(0).min(u32::MAX as u64) as u32,
        ..Fetched::default()
    };
    f.fields.push(Field { source: Source::Title, author: f.author_id, text: text_of(&issue, "title") });
    f.fields.push(Field { source: Source::Body, author: f.author_id, text: text_of(&issue, "body") });
    let mut n = 0u32;
    for page in 1.. {
        if (page - 1) * PAGE >= f.comments.max(1) {
            break;
        }
        let list = get_json(proxy, &format!("/gh/{r}/issues/{number}/comments?per_page={PAGE}&page={page}"), &mut budget)?;
        let items = list.as_arr().unwrap_or(&[]);
        for c in items {
            f.fields.push(Field { source: Source::Comment(n), author: user_id(c), text: text_of(c, "body") });
            n += 1;
        }
        if items.len() < PAGE as usize {
            break;
        }
    }
    if is_pr {
        let pr = get_json(proxy, &format!("/gh/{r}/pulls/{number}"), &mut budget)?;
        f.head_sha = pr.get("head").map(|h| text_of(h, "sha")).filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()));
        let commits = get_json(proxy, &format!("/gh/{r}/pulls/{number}/commits?per_page={PAGE}&page=1"), &mut budget)?;
        for (i, c) in commits.as_arr().unwrap_or(&[]).iter().enumerate() {
            let message = c.get("commit").map(|c| text_of(c, "message")).unwrap_or_default();
            let (subject, body) = message.split_once('\n').unwrap_or((&message, ""));
            let author = c.get("author").and_then(|a| a.get("id")).and_then(Value::as_u64).unwrap_or(0);
            f.fields.push(Field { source: Source::CommitSubject(i as u32), author, text: subject.to_string() });
            if !body.trim().is_empty() {
                f.fields.push(Field { source: Source::CommitBody(i as u32), author, text: body.trim().to_string() });
            }
        }
        let (status, diff) = get(proxy, &format!("/gh/{r}/pulls/{number}.diff"), budget).map_err(Fetch::Failed)?;
        match status {
            200 => f.fields.push(Field { source: Source::Diff, author: f.author_id, text: String::from_utf8_lossy(&diff).into_owned() }),
            413 => return Err(Fetch::Oversize),
            s => return Err(Fetch::Failed(format!("diff: status {s}"))),
        }
    }
    f.bytes = f.fields.iter().map(|x| x.text.len() as u64).sum();
    if f.bytes as usize > MAX_ITEM_BYTES {
        return Err(Fetch::Oversize);
    }
    Ok(f)
}

/// The items updated since `since` (`2026-11-14T00:00:00Z`), newest first: (number, updated).
pub fn updated(proxy: &str, repo: Repo, since: &str) -> Result<Vec<(u32, String)>, String> {
    if !valid_time(since) {
        return Err(format!("{since}: not a GitHub time"));
    }
    let mut budget = 4 << 20;
    let list = get_json(proxy, &format!("/gh/{}/issues?state=all&sort=updated&direction=desc&per_page=50&since={since}", repo.word()), &mut budget).map_err(|e| match e {
        Fetch::Oversize => "the item list is too large".to_string(),
        Fetch::Failed(e) => e,
    })?;
    Ok(list
        .as_arr()
        .unwrap_or(&[])
        .iter()
        .filter_map(|i| {
            let n = i.get("number").and_then(Value::as_u64).filter(|n| *n > 0 && *n < 1_000_000_000)? as u32;
            Some((n, Some(text_of(i, "updated_at")).filter(|t| valid_time(t))?))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_heads() {
        assert!(valid_time("2026-11-14T13:59:27Z"), "GitHub's form");
        assert!(!valid_time("2026-11-14T13:59:27+01:00") && !valid_time("2026-11-14 13:59:27Z") && !valid_time(""), "nothing else");
        // a proxy that is not listening is an error, not a hang
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = free.local_addr().unwrap().to_string();
        drop(free);
        assert!(get(&addr, "/health", 10).is_err(), "no proxy, an error");
    }
}
