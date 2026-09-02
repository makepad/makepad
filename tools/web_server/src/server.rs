use crate::{
    api::{start_production_load, ServiceRegistry},
    config::Config,
    static_files::StaticHandler,
};
use makepad_network::{
    http_server::{HttpServer, HttpServerRequest},
    NetworkConfig, NetworkRuntime,
};
use std::{path::PathBuf, sync::mpsc};

pub fn run(config: Config) -> Result<(), String> {
    let registry = ServiceRegistry::new(config.major_graph.is_some());
    run_with_registry(config, registry, true)
}

/// Runs the HTTP/static dispatcher with an injected service registry. This
/// keeps API plumbing independently testable without special production
/// flags or an alternate protocol.
pub fn run_with_registry(
    config: Config,
    registry: std::sync::Arc<ServiceRegistry>,
    load_production_data: bool,
) -> Result<(), String> {
    let static_root = config.root.canonicalize()
        .map_err(|error| format!("canonicalize static root {}: {error}", config.root.display()))?;
    let data_root = canonical_data_root(&config, &static_root)?;
    let static_handler = StaticHandler::new_with_data_dir(&static_root, data_root.as_deref())?;
    let runtime = NetworkRuntime::new(NetworkConfig::default());
    let (request_sender, request_receiver) = mpsc::channel::<HttpServerRequest>();
    let listen = config.listen;
    let Some(_listen_thread) = runtime.start_http_server(HttpServer {
        listen_address: listen,
        post_max_size: 2 * 1024 * 1024,
        post_max_size_overrides: vec![
            ("/$report_error".into(), crate::static_files::REPORT_BODY_LIMIT as u64),
            ("/api/crash".into(), crate::static_files::CRASH_BODY_LIMIT as u64),
        ],
        pre_admit_posts: true,
        client_ip_resolver: Some(crate::static_files::client_ip),
        trusted_proxy: Some(crate::static_files::cloudflare_peer),
        allowed_methods: Some(allowed_methods),
        request: request_sender,
    }) else {
        return Err(format!("failed to bind {listen}"));
    };
    println!("makepad-web-server listening on http://{listen}");

    if load_production_data {
        if let Some(data_root) = data_root {
            start_production_load(config, data_root, registry.clone());
        }
    }

    while let Ok(request) = request_receiver.recv() {
        match request {
            HttpServerRequest::Get { headers, response_sender } => {
                if headers.path == "/api/crash" {
                    static_handler.handle_get(&headers, &response_sender);
                } else if !registry.handle_get(&headers, &response_sender) {
                    static_handler.handle_get(&headers, &response_sender);
                }
            }
            HttpServerRequest::Post { headers, body, response } => {
                if headers.path == "/api/crash" {
                    static_handler.handle_post(&headers, &body, &response);
                } else if headers.path.starts_with("/api/") {
                    registry.handle_post(&headers, body, &response);
                } else if !static_handler.handle_post(&headers, &body, &response) {
                    static_handler.handle_get(&headers, &response);
                }
            }
            HttpServerRequest::PostPending { headers, body, response } => {
                if headers.path == "/api/crash" {
                    let _ = static_handler.handle_post_pending(&headers, body, &response);
                } else if headers.path.starts_with("/api/") {
                    registry.handle_post_pending(&headers, body, &response);
                } else if let Err(body) = static_handler.handle_post_pending(&headers, body, &response) {
                    body.reject(crate::http::response(
                        405,
                        Some("text/plain; charset=utf-8"),
                        "no-store",
                        "Allow: GET, HEAD, OPTIONS\r\n",
                        b"method not allowed".to_vec(),
                    ));
                }
            }
            HttpServerRequest::ConnectWebSocket { response_sender, .. } => {
                let _ = response_sender.send(Vec::new());
            }
            HttpServerRequest::DisconnectWebSocket { .. }
            | HttpServerRequest::BinaryMessage { .. }
            | HttpServerRequest::TextMessage { .. } => {}
        }
    }
    Err("HTTP request channel closed".into())
}

fn allowed_methods(path: &str) -> &'static str {
    match path {
        "/api/along" => "POST, OPTIONS",
        "/api/healthz" | "/api/search" | "/api/route" => "GET, HEAD, OPTIONS",
        "/$report_error" => "GET, HEAD, POST, OPTIONS",
        path if path.starts_with("/api/") => "GET, HEAD, POST, OPTIONS",
        _ => "GET, HEAD, OPTIONS",
    }
}

fn canonical_data_root(config: &Config, static_root: &std::path::Path) -> Result<Option<PathBuf>, String> {
    let Some(data_dir) = &config.data_dir else { return Ok(None) };
    let data_root = data_dir
        .canonicalize()
        .map_err(|error| format!("canonicalize data root {}: {error}", data_dir.display()))?;
    if !data_root.is_dir() {
        return Err(format!("data root {} is not a directory", data_root.display()));
    }
    if data_root.starts_with(static_root) || static_root.starts_with(&data_root) {
        return Err("--data-dir and --root must not overlap".into());
    }
    Ok(Some(data_root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};

    #[test]
    fn rejects_data_inside_public_root() {
        let base = std::env::temp_dir().join(format!(
            "makepad-web-roots-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let root = base.join("site");
        let data = root.join("private");
        fs::create_dir_all(&data).unwrap();
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&data, fs::Permissions::from_mode(0o555)).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
        }
        let mut config = Config::parse([root.to_string_lossy().as_ref()]).unwrap();
        config.data_dir = Some(data);
        let static_handler = StaticHandler::new(&root).unwrap();
        assert!(canonical_data_root(&config, static_handler.root()).is_err());
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn method_policy_matches_resources() {
        assert_eq!(allowed_methods("/api/along"), "POST, OPTIONS");
        assert_eq!(allowed_methods("/api/search"), "GET, HEAD, OPTIONS");
        assert_eq!(allowed_methods("/asset.wasm"), "GET, HEAD, OPTIONS");
    }
}
