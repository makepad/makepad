//! Browser-like HTTP requests.
//!
//! Archive.org, TDM mirrors, and similar hosts treat unknown UAs as bots.
//! Every outbound request from Asset UI uses a stock Safari string.

use makepad_widgets::*;

pub const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.3 Safari/605.1.15";

pub fn request(url: impl Into<String>, method: HttpMethod) -> HttpRequest {
    let mut request = HttpRequest::new(url.into(), method);
    request.set_header("User-Agent".into(), BROWSER_UA.into());
    request.set_header("Accept".into(), "*/*".into());
    request
}

pub fn get(url: impl Into<String>) -> HttpRequest {
    request(url, HttpMethod::GET)
}

/// `end` is exclusive (zipsync byterange). HTTP Range is inclusive.
pub fn get_range(url: impl Into<String>, start: u64, end: u64) -> HttpRequest {
    let mut request = get(url);
    let last = end.saturating_sub(1);
    request.set_header("Range".into(), format!("bytes={start}-{last}"));
    request
}
