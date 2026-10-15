//! Smart HTTP, upload-pack side only (git's http-protocol.txt / pack-protocol.txt, protocol v0/v1):
//! the ref advertisement, the want/have request, and the pack out of the answer.
use super::{GitError, ObjectId};
use std::collections::{HashMap, HashSet};
use std::str;

const USER_AGENT: &str = "makepad-run/0.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status_code: u16,
    pub body: Vec<u8>,
}

/// The remote's commit for the asked ref (or its HEAD), and what it can do.
#[derive(Debug, Clone)]
pub struct RemoteHead {
    pub oid: ObjectId,
    pub ref_name: Option<String>,
    pub capabilities: Vec<String>,
}

fn header(n: &str, v: &str) -> (String, String) {
    (n.to_string(), v.to_string())
}

/// GET <url>/info/refs?service=git-upload-pack.
pub fn build_info_refs_request(remote_url: &str) -> Request {
    Request {
        method: Method::Get,
        url: format!("{}/info/refs?service=git-upload-pack", remote_url.trim_end_matches('/')),
        headers: vec![header("Accept", "application/x-git-upload-pack-advertisement"), header("User-Agent", USER_AGENT)],
        body: Vec::new(),
    }
}

/// The advertised commit of `branch` (a branch name or a full `refs/...` name); without one the
/// ref HEAD points at, else main, master, HEAD, the first ref.
pub fn parse_info_refs_response(response: &Response, branch: Option<&str>) -> Result<RemoteHead, GitError> {
    if response.status_code != 200 {
        return Err(GitError::InvalidRef(format!("info/refs request failed with HTTP {}", response.status_code)));
    }
    let mut refs = HashMap::<String, ObjectId>::new();
    let mut caps_seen = HashSet::<String>::new();
    let mut capabilities = Vec::<String>::new();
    let mut reader = PktLineReader::new(&response.body);
    let mut seen_service_separator = false;
    while let Some(pkt) = reader.next()? {
        let PktLine::Data(data) = pkt else {
            if !seen_service_separator {
                seen_service_separator = true;
                continue;
            }
            break;
        };
        let line = trim_line_end(data);
        if !seen_service_separator && line.starts_with(b"# service=git-upload-pack") {
            continue;
        }
        if line.starts_with(b"ERR ") {
            return Err(GitError::InvalidRef(format!("remote error: {}", String::from_utf8_lossy(line))));
        }
        let space = line.iter().position(|b| *b == b' ').ok_or_else(|| GitError::InvalidRef("invalid ref advertisement line".into()))?;
        let oid = ObjectId::from_hex(str::from_utf8(&line[..space]).map_err(|_| GitError::InvalidRef("invalid oid in info/refs".into()))?)?;
        let mut name = &line[space + 1..];
        if let Some(nul) = name.iter().position(|b| *b == 0) {
            for cap in name[nul + 1..].split(|b| *b == b' ').filter(|c| !c.is_empty()) {
                let cap = String::from_utf8_lossy(cap).to_string();
                if caps_seen.insert(cap.clone()) {
                    capabilities.push(cap);
                }
            }
            name = &name[..nul];
        }
        let name = str::from_utf8(name).map_err(|_| GitError::InvalidRef("invalid ref name in info/refs".into()))?;
        refs.insert(name.to_string(), oid);
    }
    if refs.is_empty() {
        return Err(GitError::InvalidRef("no refs found in info/refs response".into()));
    }
    let head_symref = capabilities.iter().find_map(|c| c.strip_prefix("symref=HEAD:")).map(str::to_string);
    let (oid, ref_name) = if let Some(branch) = branch {
        let want = if branch.starts_with("refs/") { branch.to_string() } else { format!("refs/heads/{branch}") };
        (*refs.get(&want).ok_or_else(|| GitError::RefNotFound(want.clone()))?, Some(want))
    } else if let Some(symref) = head_symref {
        (*refs.get(&symref).or_else(|| refs.get("HEAD")).ok_or_else(|| GitError::RefNotFound("HEAD".into()))?, Some(symref))
    } else if let Some(oid) = refs.get("refs/heads/main") {
        (*oid, Some("refs/heads/main".to_string()))
    } else if let Some(oid) = refs.get("refs/heads/master") {
        (*oid, Some("refs/heads/master".to_string()))
    } else if let Some(oid) = refs.get("HEAD") {
        (*oid, None)
    } else {
        let first = refs.keys().min().cloned().ok_or_else(|| GitError::RefNotFound("no advertised refs".into()))?;
        (refs[&first], Some(first))
    };
    Ok(RemoteHead { oid, ref_name, capabilities })
}

/// POST <url>/git-upload-pack: want `target` (with the capabilities we use that the remote has),
/// `deepen <depth>` when asked (needs `shallow`), a `have` per commit we hold, done.
pub fn build_upload_pack_request(remote_url: &str, target: ObjectId, capabilities: &[String], have: &[ObjectId], depth: Option<u32>) -> Result<Request, GitError> {
    let has_cap = |name: &str| capabilities.iter().any(|c| c == name || c.strip_prefix(name).is_some_and(|r| r.starts_with('=')));
    if depth.is_some() && !has_cap("shallow") {
        return Err(GitError::InvalidRef("remote does not advertise shallow capability".into()));
    }
    let mut want_caps = Vec::new();
    if has_cap("side-band-64k") {
        want_caps.push("side-band-64k");
    } else if has_cap("side-band") {
        want_caps.push("side-band");
    }
    for c in ["ofs-delta", "thin-pack", "no-progress"] {
        if has_cap(c) {
            want_caps.push(c);
        }
    }
    let mut body = Vec::new();
    let want = if want_caps.is_empty() { format!("want {}\n", target.to_hex()) } else { format!("want {} {}\n", target.to_hex(), want_caps.join(" ")) };
    write_pkt_line(&mut body, want.as_bytes())?;
    // part of the want list: before the first flush
    if let Some(depth) = depth {
        write_pkt_line(&mut body, format!("deepen {depth}\n").as_bytes())?;
    }
    body.extend_from_slice(b"0000");
    for oid in have {
        write_pkt_line(&mut body, format!("have {}\n", oid.to_hex()).as_bytes())?;
    }
    write_pkt_line(&mut body, b"done\n")?;
    Ok(Request {
        method: Method::Post,
        url: format!("{}/git-upload-pack", remote_url.trim_end_matches('/')),
        headers: vec![
            header("Content-Type", "application/x-git-upload-pack-request"),
            header("Accept", "application/x-git-upload-pack-result"),
            header("User-Agent", USER_AGENT),
        ],
        body,
    })
}

fn is_negotiation(line: &[u8]) -> bool {
    line.starts_with(b"NAK") || line.starts_with(b"ACK ") || line.starts_with(b"shallow ") || line.starts_with(b"unshallow ")
}

/// The pack in an upload-pack answer: raw, after the negotiation lines without side-band, or
/// collected from side-band channel 1. None when there is none.
pub fn extract_pack_from_response(response: &Response) -> Result<Option<Vec<u8>>, GitError> {
    if response.status_code != 200 {
        return Err(GitError::InvalidRef(format!("upload-pack request failed with HTTP {}", response.status_code)));
    }
    let body = &response.body;
    if body.is_empty() {
        return Ok(None);
    }
    if body.starts_with(b"PACK") {
        return Ok(Some(body.clone()));
    }
    // without side-band the pack follows the negotiation lines raw ("NAK" / "ACK ..." /
    // "shallow ..." pkt-lines, then the pack bytes unframed)
    let mut at = 0;
    while at + 4 <= body.len() {
        if body[at..].starts_with(b"PACK") {
            return Ok(Some(body[at..].to_vec()));
        }
        let Some(len) = str::from_utf8(&body[at..at + 4]).ok().and_then(|h| usize::from_str_radix(h, 16).ok()) else { break };
        if len == 0 {
            at += 4;
            continue;
        }
        if len < 4 || at + len > body.len() || !is_negotiation(&body[at + 4..at + len]) {
            break;
        }
        at += len;
    }
    let mut reader = PktLineReader::new(body);
    let mut pack = Vec::new();
    let mut saw_pkt_lines = false;
    while let Some(pkt) = reader.next()? {
        saw_pkt_lines = true;
        let PktLine::Data(data) = pkt else { continue };
        if data.starts_with(b"ERR ") {
            return Err(GitError::InvalidRef(format!("remote error: {}", String::from_utf8_lossy(data))));
        }
        if data.is_empty() || is_negotiation(data) {
            continue;
        }
        match data[0] {
            1 => pack.extend_from_slice(&data[1..]),
            2 => {}
            3 => return Err(GitError::InvalidRef(format!("remote fatal error: {}", String::from_utf8_lossy(&data[1..]).trim()))),
            _ if data.starts_with(b"PACK") => pack.extend_from_slice(data),
            _ => {}
        }
    }
    if !pack.is_empty() {
        return Ok(Some(pack));
    }
    if !saw_pkt_lines {
        if let Some(i) = body.windows(4).position(|w| w == b"PACK") {
            return Ok(Some(body[i..].to_vec()));
        }
    }
    Ok(None)
}

fn write_pkt_line(out: &mut Vec<u8>, payload: &[u8]) -> Result<(), GitError> {
    let len = payload.len() + 4;
    if len > 0xffff {
        return Err(GitError::InvalidRef("pkt-line payload too large".into()));
    }
    out.extend_from_slice(format!("{len:04x}").as_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

enum PktLine<'a> {
    Flush,
    Data(&'a [u8]),
}

struct PktLineReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> PktLineReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn next(&mut self) -> Result<Option<PktLine<'a>>, GitError> {
        if self.pos >= self.data.len() {
            return Ok(None);
        }
        let bad = |m: &str| GitError::InvalidRef(m.to_string());
        let hex = self.data.get(self.pos..self.pos + 4).ok_or_else(|| bad("truncated pkt-line length"))?;
        let len = str::from_utf8(hex).ok().and_then(|h| usize::from_str_radix(h, 16).ok()).ok_or_else(|| bad("invalid pkt-line length"))?;
        self.pos += 4;
        if len == 0 {
            return Ok(Some(PktLine::Flush));
        }
        if len < 4 {
            return Err(bad("invalid pkt-line length (<4)"));
        }
        let payload = self.data.get(self.pos..self.pos + len - 4).ok_or_else(|| bad("truncated pkt-line payload"))?;
        self.pos += len - 4;
        Ok(Some(PktLine::Data(payload)))
    }
}

fn trim_line_end(data: &[u8]) -> &[u8] {
    let data = data.strip_suffix(b"\n").unwrap_or(data);
    data.strip_suffix(b"\r").unwrap_or(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_after_nak_without_side_band() {
        let mut body = b"0008NAK\n".to_vec();
        body.extend_from_slice(b"PACK\0\0\0\x02rest");
        let r = Response { status_code: 200, body };
        assert_eq!(extract_pack_from_response(&r).unwrap().unwrap(), b"PACK\0\0\0\x02rest".to_vec());
    }

    #[test]
    fn pack_from_side_band() {
        let mut body = Vec::new();
        write_pkt_line(&mut body, b"NAK\n").unwrap();
        write_pkt_line(&mut body, b"\x01PACK\0\0").unwrap();
        write_pkt_line(&mut body, b"\x02progress").unwrap();
        write_pkt_line(&mut body, b"\x01\0\x02x").unwrap();
        body.extend_from_slice(b"0000");
        let r = Response { status_code: 200, body };
        assert_eq!(extract_pack_from_response(&r).unwrap().unwrap(), b"PACK\0\0\0\x02x".to_vec());
    }

    #[test]
    fn parse_info_refs_basic() {
        let mut body = Vec::new();
        write_pkt_line(&mut body, b"# service=git-upload-pack\n").unwrap();
        body.extend_from_slice(b"0000");
        write_pkt_line(&mut body, b"0123456789012345678901234567890123456789 refs/heads/main\0side-band-64k shallow symref=HEAD:refs/heads/main\n").unwrap();
        write_pkt_line(&mut body, b"1123456789012345678901234567890123456789 refs/heads/dev\n").unwrap();
        body.extend_from_slice(b"0000");
        let r = Response { status_code: 200, body };
        let head = parse_info_refs_response(&r, None).unwrap();
        assert_eq!(head.oid.to_hex(), "0123456789012345678901234567890123456789");
        assert_eq!(head.ref_name.as_deref(), Some("refs/heads/main"));
        assert_eq!(head.capabilities, ["side-band-64k", "shallow", "symref=HEAD:refs/heads/main"]);
        assert_eq!(parse_info_refs_response(&r, Some("dev")).unwrap().oid.to_hex(), "1123456789012345678901234567890123456789");
        assert!(parse_info_refs_response(&r, Some("nope")).is_err());
    }

    #[test]
    fn upload_pack_request_wants_deepens_then_haves() {
        let oid = ObjectId::from_hex("0123456789012345678901234567890123456789").unwrap();
        let caps = ["shallow".to_string(), "ofs-delta".to_string(), "side-band-64k".to_string()];
        let req = build_upload_pack_request("https://example.com/repo/", oid, &caps, &[oid], Some(1)).unwrap();
        assert_eq!(req.url, "https://example.com/repo/git-upload-pack");
        let want = format!("want {oid} side-band-64k ofs-delta\n");
        let mut rd = PktLineReader::new(&req.body);
        let mut lines = Vec::new();
        while let Some(p) = rd.next().unwrap() {
            lines.push(match p {
                PktLine::Flush => "<flush>".to_string(),
                PktLine::Data(d) => String::from_utf8(d.to_vec()).unwrap(),
            });
        }
        assert_eq!(lines, [want, "deepen 1\n".into(), "<flush>".into(), format!("have {oid}\n"), "done\n".into()]);
        assert!(build_upload_pack_request("https://example.com/repo", oid, &[], &[], Some(1)).is_err());
    }
}
