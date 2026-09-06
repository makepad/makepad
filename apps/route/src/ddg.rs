//! DuckDuckGo image search (route.md §2.8) — the examples/ddgo protocol
//! ported onto `cx.http_request`: fetch the images page for a `vqd` token,
//! call `i.js` for JSON results, then download thumbnails for the card
//! strip. Unofficial endpoint: rate-limited and failure-tolerant by design
//! (the feature degrades, the app doesn't).

use makepad_widgets::*;

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";
/// Politeness: minimum seconds between searches.
const MIN_SEARCH_GAP_S: f64 = 5.0;
pub const MAX_CARDS: usize = 4;

pub struct DdgImage {
    pub title: String,
    /// Kept for future card taps (open source page) — not read yet.
    pub _thumbnail_url: String,
    pub thumb_request: Option<LiveId>,
    pub loaded: bool,
}

enum Stage {
    FetchingVqd { request: LiveId },
    FetchingResults { request: LiveId },
    Thumbnails,
}

pub struct DdgSearch {
    /// Pending agent tool call to answer (None = fired from the console).
    pub tool_use_id: Option<String>,
    pub query: String,
    stage: Stage,
    pub images: Vec<DdgImage>,
}

#[derive(Default)]
pub struct DdgState {
    pub active: Option<DdgSearch>,
    last_search: Option<f64>,
}

pub enum DdgEvent {
    /// Tool result is ready: (tool_use_id, digest, is_error).
    Done(Option<String>, String, bool),
    /// A thumbnail arrived for card slot `index`.
    Thumb(usize, Vec<u8>),
}

fn get(cx: &mut Cx, url: &str, referer: Option<&str>) -> LiveId {
    let request_id = LiveId::unique();
    let mut request = HttpRequest::new(url.to_string(), HttpMethod::GET);
    request.set_header("User-Agent".to_string(), USER_AGENT.to_string());
    if let Some(referer) = referer {
        request.set_header("Referer".to_string(), referer.to_string());
    }
    cx.http_request(request_id, request);
    request_id
}

fn url_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

impl DdgState {
    /// Kick off a search; Err when rate-limited or one is in flight.
    pub fn start(
        &mut self,
        cx: &mut Cx,
        query: &str,
        tool_use_id: Option<String>,
    ) -> Result<(), String> {
        if self.active.is_some() {
            return Err("an image search is already running".into());
        }
        if let Some(last) = self.last_search {
            let since = Cx::monotonic_now() - last;
            if since < MIN_SEARCH_GAP_S {
                return Err(format!(
                    "image search rate limit — retry in {:.0}s",
                    MIN_SEARCH_GAP_S - since
                ));
            }
        }
        self.last_search = Some(Cx::monotonic_now());
        let encoded = url_encode(query.trim());
        let request = get(
            cx,
            &format!("https://duckduckgo.com/?q={encoded}&iax=images&ia=images"),
            None,
        );
        self.active = Some(DdgSearch {
            tool_use_id,
            query: query.trim().to_string(),
            stage: Stage::FetchingVqd { request },
            images: Vec::new(),
        });
        Ok(())
    }

    /// Route one NetworkResponse through the state machine. May yield a
    /// thumbnail AND the completion in one call (the last thumb).
    pub fn handle_response(&mut self, cx: &mut Cx, item: &NetworkResponse) -> Vec<DdgEvent> {
        let mut out = Vec::new();
        let Some(search) = &mut self.active else {
            return out;
        };
        let (request_id, response, failed) = match item {
            NetworkResponse::HttpResponse { request_id, response } => {
                (*request_id, Some(response), false)
            }
            NetworkResponse::HttpError { request_id, .. } => (*request_id, None, true),
            _ => return out,
        };

        match &search.stage {
            Stage::FetchingVqd { request } if *request == request_id => {
                let body = response.and_then(|r| r.get_string_body());
                let (Some(body), false) = (body, failed) else {
                    let id = search.tool_use_id.take();
                    self.active = None;
                    out.push(DdgEvent::Done(id, "image search failed (network)".into(), true));
                    return out;
                };
                let vqd = body
                    .split("vqd=\"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .unwrap_or("")
                    .to_string();
                if vqd.is_empty() {
                    let id = search.tool_use_id.take();
                    self.active = None;
                    out.push(DdgEvent::Done(
                        id,
                        "image search failed (no token — endpoint may have changed)".into(),
                        true,
                    ));
                    return out;
                }
                let encoded = url_encode(&search.query);
                let request = get(
                    cx,
                    &format!(
                        "https://duckduckgo.com/i.js?l=us-en&o=json&q={encoded}&vqd={vqd}&f=,,,,,&p=1"
                    ),
                    Some("https://duckduckgo.com/"),
                );
                search.stage = Stage::FetchingResults { request };
            }
            Stage::FetchingResults { request } if *request == request_id => {
                let body = response.and_then(|r| r.get_string_body());
                let (Some(body), false) = (body, failed) else {
                    let id = search.tool_use_id.take();
                    self.active = None;
                    out.push(DdgEvent::Done(id, "image search failed (results)".into(), true));
                    return out;
                };
                let parsed: Option<serde_json::Value> = serde_json::from_str(&body).ok();
                let results = parsed
                    .as_ref()
                    .and_then(|v| v.get("results"))
                    .and_then(|r| r.as_array())
                    .cloned()
                    .unwrap_or_default();
                for item in results.iter().take(MAX_CARDS) {
                    let title = item
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    let thumbnail_url = item
                        .get("thumbnail")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    if thumbnail_url.is_empty() {
                        continue;
                    }
                    let thumb_request = Some(get(cx, &thumbnail_url, None));
                    search.images.push(DdgImage {
                        title,
                        _thumbnail_url: thumbnail_url,
                        thumb_request,
                        loaded: false,
                    });
                }
                if search.images.is_empty() {
                    let id = search.tool_use_id.take();
                    let query = search.query.clone();
                    self.active = None;
                    out.push(DdgEvent::Done(id, format!("no images found for '{query}'"), false));
                    return out;
                }
                search.stage = Stage::Thumbnails;
            }
            Stage::Thumbnails => {
                let slot = search
                    .images
                    .iter()
                    .position(|img| img.thumb_request == Some(request_id));
                let Some(slot) = slot else {
                    return out;
                };
                search.images[slot].thumb_request = None;
                let data = response.and_then(|r| r.body()).map(|b| b.to_vec());
                if let (Some(data), false) = (data, failed) {
                    search.images[slot].loaded = true;
                    out.push(DdgEvent::Thumb(slot, data));
                } else {
                    // failed thumb: drop the card
                    search.images.remove(slot);
                }
                let all_in = self
                    .active
                    .as_ref()
                    .map(|s| s.images.iter().all(|i| i.thumb_request.is_none()))
                    .unwrap_or(false);
                if all_in {
                    let digest = self.finish_digest();
                    let id = self.active.as_mut().and_then(|s| s.tool_use_id.take());
                    out.push(DdgEvent::Done(id, digest, false));
                }
            }
            _ => {}
        }
        out
    }

    fn finish_digest(&self) -> String {
        let Some(search) = &self.active else {
            return String::new();
        };
        let mut out = format!("images for '{}':\n", search.query);
        for (i, img) in search.images.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, img.title));
        }
        out.push_str("(thumbnails shown as cards in the panel)");
        out
    }
}
