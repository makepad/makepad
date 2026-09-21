use makepad_network::blocking_http::{self, Limits, Request, Response};
use std::time::Duration;

const MAX_REDIRECTS: usize = 3;

pub(crate) fn get(
    url: &str,
    authorization: Option<&str>,
    max_body_bytes: usize,
) -> Result<Response, String> {
    let mut current = url.to_string();
    for redirect in 0..=MAX_REDIRECTS {
        let limits = Limits {
            max_body_bytes,
            total_timeout: Duration::from_secs(60),
            ..Limits::default()
        };
        let mut request = Request::get(&current).limits(limits);
        if let Some(value) = authorization {
            request = request
                .header("Authorization", value)
                .map_err(|error| format!("invalid authorization header: {error}"))?;
        }
        let response = blocking_http::request_no_redirect(request)
            .map_err(|error| format!("HTTP request failed: {error}"))?;
        if !(300..400).contains(&response.status) {
            return Ok(response);
        }
        if authorization.is_some() {
            return Err("authenticated HTTP redirect refused".into());
        }
        if redirect == MAX_REDIRECTS {
            return Err("HTTP redirect limit exceeded".into());
        }
        let location = response
            .header("location")
            .filter(|location| location.starts_with("https://"))
            .ok_or("HTTP redirect had no absolute HTTPS location")?;
        current = location.to_string();
    }
    unreachable!()
}
