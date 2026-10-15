//! The network the registry fetch path uses (base/src/registry.rs): an HTTPS GET on
//! relax-light-http (the operating system's TLS stack), following up to 5 redirects.

fn https_get(url: &str) -> Result<Vec<u8>, String> {
    let mut url = url.to_string();
    for _ in 0..6 {
        let (resp, body) = relax_light_http::fetch(&relax_light_http::Request::get(&url))?;
        match resp.status {
            200 => return Ok(body),
            301 | 302 | 303 | 307 | 308 => {
                let to = resp.header("location").ok_or_else(|| format!("HTTP {} without a Location", resp.status))?;
                url = if to.starts_with("https://") { to.to_string() } else { return Err(format!("HTTP {} to {}: only https is followed", resp.status, to)) };
            }
            s => return Err(format!("HTTP {}", s)),
        }
    }
    Err("too many redirects".to_string())
}

/// Lets the resolver fetch registry packages that are neither vendored nor in $CARGO_HOME.
pub fn install() {
    crate::registry::set_fetch(https_get);
}
