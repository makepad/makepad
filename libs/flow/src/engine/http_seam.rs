use super::executors::http::{HttpReq, HttpResp, HttpSeam};
use makepad_ai_hub::http_client::{http_fetch_no_redirect, HttpClientRequest};

/// Real bounded HTTP seam: the hub client supplies plain HTTP and platform
/// TLS, while `http_fetch_no_redirect` preserves flow's redirect-refusal law.
pub struct HubHttp;

impl HttpSeam for HubHttp {
    fn request(&self, req: HttpReq) -> Result<HttpResp, String> {
        if std::time::Instant::now() >= req.deadline {
            return Err("HTTP deadline elapsed".to_string());
        }
        let body = (!req.body.is_empty()).then_some((req.content_type.as_str(), req.body.as_slice()));
        let request = HttpClientRequest {
            method: &req.method,
            url: &req.url,
            range_from: None,
            range_to: None,
            bearer: None,
            body,
            extra_headers: &req.headers,
        };
        let response = http_fetch_no_redirect(&request).map_err(|error| error.to_string())?;
        let status = response.status;
        let headers = response.headers.clone();
        let body = response
            .read_body_to_vec(32 * 1024 * 1024)
            .map_err(|error| error.to_string())?;
        if std::time::Instant::now() >= req.deadline {
            return Err("HTTP deadline elapsed".to_string());
        }
        Ok(HttpResp {
            status,
            headers,
            body,
        })
    }
}

impl crate::engine::NetPolicy {
    pub fn check(&self, url: &str) -> Result<(), String> {
        let parsed = makepad_ai_hub::http_client::parse_url(url)
            .map_err(|error| format!("refused by policy: {error}"))?;
        let host = parsed.host.to_ascii_lowercase();
        if !self.allow.iter().any(|pattern| host_matches(&host, pattern)) {
            return Err(format!("refused by policy: host `{host}` is not allowed"));
        }
        if self.deny_private && is_private(&host) {
            return Err(format!("refused by policy: private host `{host}`"));
        }
        if self.deny_private {
            use std::net::ToSocketAddrs;
            let addresses = (host.as_str(), parsed.port)
                .to_socket_addrs()
                .map_err(|error| format!("refused by policy: cannot resolve `{host}`: {error}"))?;
            for address in addresses {
                if is_private(&address.ip().to_string()) {
                    return Err(format!(
                        "refused by policy: `{host}` resolves to private address {}",
                        address.ip()
                    ));
                }
            }
        }
        Ok(())
    }
}

fn host_matches(host: &str, pattern: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        return host == suffix || host.ends_with(&format!(".{suffix}"));
    }
    host == pattern
}

fn is_private(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    let Ok(ip) = host.parse::<std::net::IpAddr>() else {
        return false;
    };
    match ip {
        std::net::IpAddr::V4(ip) => {
            ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified()
        }
        std::net::IpAddr::V6(ip) => {
            ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}
