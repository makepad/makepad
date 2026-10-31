//! The network the registry fetch path (base/src/registry.rs: an HTTPS GET following up to 5
//! redirects) and the git client (base/src/git/fetch.rs: smart HTTP exchanges, with a progress
//! line for large downloads) use: relax-light-http (the operating system's TLS stack).

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

/// One smart-HTTP exchange of the git client; downloads of 1 MB and more report progress on the
/// one progress channel (relax_compiler::progress: the runner's bar, else a terminal line).
fn git_http(req: &crate::git::Request, timeout: std::time::Duration) -> Result<crate::git::Response, String> {
    let method = match req.method {
        crate::git::Method::Get => "GET",
        crate::git::Method::Post => "POST",
    };
    let headers: Vec<(&str, &str)> = req.headers.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let r = relax_light_http::Request { method, url: &req.url, headers: &headers, body: &req.body, timeout, max_body: 4 << 30 };
    let mut body = Vec::new();
    let label = repo_label(&req.url);
    let resp = relax_light_http::send(&r, &mut body, &mut |n, total| {
        if n >= 1 << 20 {
            crate::progress::progress(&label, n, total);
        }
    })
    .map_err(|e| format!("{}: {}", crate::git::fetch::host(&req.url), e))?;
    Ok(crate::git::Response { status_code: resp.status, body })
}

/// "makepad from host" for a git URL `https://host/.../makepad.git/...`.
fn repo_label(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let name = path.split('/').find(|s| s.ends_with(".git")).map(|s| s.trim_end_matches(".git")).unwrap_or("repository");
    format!("{} from {}", name, crate::git::fetch::host(url))
}

/// Lets the resolver fetch registry packages that are neither vendored nor in $CARGO_HOME, and git
/// repositories (`run <git url>`, git dependencies).
pub fn install() {
    crate::registry::set_fetch(https_get);
    crate::git::fetch::set_http(git_http);
}
