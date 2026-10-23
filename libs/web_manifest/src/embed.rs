//! The files a web export embeds in the wasm (`relax export-web`): the
//! responses the traced runs fetched, by URL, and the page's default query.
//! A build with `--cfg makepad_embed` and `MAKEPAD_EMBED=<file>` carries
//! one; the platform's web network backend answers a GET or HEAD of an
//! embedded URL itself (200, or 206 for a `Range`), so the app reads its
//! files exactly as it would read fetched ones, and only what is not
//! embedded (a song beside the page) goes to the network.
//!
//! ```text
//! "MPE1" | u32 query length | query (UTF-8)
//!        | u32 count | count x { u32 url length | url | u32 body length | body }
//! ```
//! Little-endian. A URL is page-relative without a leading `/` or `./`
//! ([`key`]); a directory's listing is the URL with a trailing `/`.
//!
//! The query is merged under the page's own (`location.search`, [`merge_query`]):
//! a key the page gives replaces the embedded one, so `?deterministic&end=exit`
//! on an exported page adds to its embedded `doc=...`.

pub const MAGIC: &[u8; 4] = b"MPE1";

/// An embed: the default query and the bodies by URL key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Embed {
    pub query: String,
    pub files: Vec<(String, Vec<u8>)>,
}

/// A borrowed view of an embed (the wasm reads its own data in place).
pub struct EmbedRef<'a> {
    pub query: &'a str,
    files: Vec<(&'a str, &'a [u8])>,
}

/// The lookup key of a URL: no scheme-less leading `/` or `./`, no query or
/// fragment. An absolute URL (`https://...`) is never embedded: None.
pub fn key(url: &str) -> Option<&str> {
    if url.contains("://") {
        return None;
    }
    let url = url.split(['?', '#']).next().unwrap_or("");
    let mut url = url;
    loop {
        if let Some(rest) = url.strip_prefix("./") {
            url = rest;
        } else if let Some(rest) = url.strip_prefix('/') {
            url = rest;
        } else {
            return Some(url);
        }
    }
}

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

impl Embed {
    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.files.iter().map(|(u, b)| u.len() + b.len() + 8).sum::<usize>());
        out.extend_from_slice(MAGIC);
        put(&mut out, self.query.as_bytes());
        out.extend_from_slice(&(self.files.len() as u32).to_le_bytes());
        for (url, body) in &self.files {
            put(&mut out, url.as_bytes());
            put(&mut out, body);
        }
        out
    }
}

impl<'a> EmbedRef<'a> {
    pub fn read(bytes: &'a [u8]) -> Result<EmbedRef<'a>, String> {
        let mut at = 0usize;
        let mut take = |n: usize| -> Result<&'a [u8], String> {
            let end = at.checked_add(n).filter(|&end| end <= bytes.len()).ok_or("the embedded files are cut short")?;
            let piece = &bytes[at..end];
            at = end;
            Ok(piece)
        };
        if take(4)? != MAGIC {
            return Err("not an embed (no MPE1)".into());
        }
        let u32_of = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
        let text = |b: &'a [u8]| std::str::from_utf8(b).map_err(|_| "a URL or the query of the embed is not UTF-8".to_string());
        let n = u32_of(take(4)?);
        let query = text(take(n)?)?;
        let count = u32_of(take(4)?);
        let mut files = Vec::with_capacity(count);
        for _ in 0..count {
            let n = u32_of(take(4)?);
            let url = text(take(n)?)?;
            let n = u32_of(take(4)?);
            files.push((url, take(n)?));
        }
        files.sort_by(|a, b| a.0.cmp(b.0));
        Ok(EmbedRef { query, files })
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The body embedded for `url`, if any.
    pub fn get(&self, url: &str) -> Option<&'a [u8]> {
        let key = key(url)?;
        self.files.binary_search_by(|(u, _)| (*u).cmp(key)).ok().map(|i| self.files[i].1)
    }

    pub fn owned(&self) -> Embed {
        Embed { query: self.query.to_string(), files: self.files.iter().map(|(u, b)| (u.to_string(), b.to_vec())).collect() }
    }
}

/// `embedded` under `page` (both `a=1&b` with or without `?`): the embedded
/// pairs whose key the page does not give, then the page's pairs.
pub fn merge_query(page: &str, embedded: &str) -> String {
    let page = page.trim_start_matches('?');
    let embedded = embedded.trim_start_matches('?');
    let key_of = |pair: &str| pair.split('=').next().unwrap_or("").to_string();
    let page_keys: Vec<String> = page.split('&').filter(|p| !p.is_empty()).map(key_of).collect();
    let mut out: Vec<&str> = embedded.split('&').filter(|p| !p.is_empty() && !page_keys.contains(&key_of(p))).collect();
    out.extend(page.split('&').filter(|p| !p.is_empty()));
    if out.is_empty() {
        String::new()
    } else {
        format!("?{}", out.join("&"))
    }
}

/// The answer to a GET of `body` with an optional `Range: bytes=a-b` value:
/// (status, `Content-Range`), and the bytes it sends.
pub fn range<'b>(body: &'b [u8], range: Option<&str>) -> (u16, Option<String>, &'b [u8]) {
    let size = body.len() as u64;
    let Some(spec) = range.and_then(|r| r.trim().strip_prefix("bytes=")) else { return (200, None, body) };
    let (a, b) = spec.split_once('-').unwrap_or((spec, ""));
    let (from, to) = match (a.trim().parse::<u64>().ok(), b.trim().parse::<u64>().ok()) {
        (Some(from), Some(to)) => (from, to.min(size.saturating_sub(1))),
        (Some(from), None) => (from, size.saturating_sub(1)),
        (None, Some(last)) => (size.saturating_sub(last), size.saturating_sub(1)),
        (None, None) => return (200, None, body),
    };
    if size == 0 || from >= size || from > to {
        return (416, Some(format!("bytes */{size}")), &[]);
    }
    (206, Some(format!("bytes {from}-{to}/{size}")), &body[from as usize..=to as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_embed_reads_back_and_finds_by_url() {
        let e = Embed { query: "doc=a/b.splash".into(), files: vec![("pdoom/b.splash".into(), b"x".to_vec()), ("pdoom/".into(), b"f 1 b.splash\n".to_vec())] };
        let bytes = e.write();
        let r = EmbedRef::read(&bytes).unwrap();
        assert_eq!(r.query, "doc=a/b.splash");
        assert_eq!(r.get("/pdoom/b.splash"), Some(&b"x"[..]));
        assert_eq!(r.get("./pdoom/"), Some(&b"f 1 b.splash\n"[..]));
        assert_eq!(r.get("pdoom/b.splash?v=1"), Some(&b"x"[..]));
        assert_eq!(r.get("https://h/pdoom/b.splash"), None);
        assert_eq!(r.get("song.mp3"), None);
        assert!(EmbedRef::read(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn the_page_query_overrides_by_key() {
        assert_eq!(merge_query("?deterministic&end=exit", "doc=p/f.cut.splash&end=hold"), "?doc=p/f.cut.splash&deterministic&end=exit");
        assert_eq!(merge_query("", "doc=x"), "?doc=x");
        assert_eq!(merge_query("?doc=y", "doc=x"), "?doc=y");
        assert_eq!(merge_query("", ""), "");
    }

    #[test]
    fn ranges_answer_like_a_server() {
        let body = b"0123456789";
        assert_eq!(range(body, None), (200, None, &body[..]));
        assert_eq!(range(body, Some("bytes=2-4")), (206, Some("bytes 2-4/10".into()), &body[2..5]));
        assert_eq!(range(body, Some("bytes=8-100")), (206, Some("bytes 8-9/10".into()), &body[8..]));
        assert_eq!(range(body, Some("bytes=-3")), (206, Some("bytes 7-9/10".into()), &body[7..]));
        assert_eq!(range(body, Some("bytes=10-")).0, 416);
    }
}
