use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use makepad_studio_hub::MountConfig;

pub(super) fn parse_mounts_spec(spec: &str, item_sep: char, pair_sep: char) -> Vec<MountConfig> {
    spec.split(item_sep)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .filter_map(|token| {
            let (name, path_str) = token.split_once(pair_sep)?;
            let name = name.trim();
            let path_str = path_str.trim();
            if name.is_empty() || path_str.is_empty() {
                return None;
            }
            let path = std::path::PathBuf::from(path_str).canonicalize().ok()?;
            Some(MountConfig {
                name: name.to_string(),
                path,
            })
        })
        .collect()
}

pub(super) fn parse_cli_arg_value(name: &str) -> Option<String> {
    let mut value = None;
    let prefixed = format!("--{name}=");
    let plain = format!("--{name}");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(parsed) = arg.strip_prefix(&prefixed) {
            value = Some(parsed.to_string());
            continue;
        }
        if arg == plain {
            value = Some(args.next().unwrap_or_default());
        }
    }
    value
}

pub(super) fn parse_cli_mounts_spec() -> Option<String> {
    parse_cli_arg_value("mounts")
}

pub(super) fn parse_cli_bind_spec() -> Option<String> {
    let mut value = None;
    let prefixed = "--bind=";
    for arg in std::env::args().skip(1) {
        if let Some(parsed) = arg.strip_prefix(prefixed) {
            value = Some(parsed.to_string());
            continue;
        }
        if arg == "--bind" {
            value = Some("0.0.0.0".to_string());
        }
    }
    value
}

pub(super) fn parse_cli_bind_address(spec: Option<String>) -> Result<SocketAddr, String> {
    let Some(spec) = spec.map(|spec| spec.trim().to_string()) else {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8001));
    };
    if spec.is_empty() {
        return Err("invalid --bind value '', expected ip or ip:port".to_string());
    }
    if let Ok(addr) = spec.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Ok(ip) = spec.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, 8001));
    }
    Err(format!(
        "invalid --bind value '{}', expected ip or ip:port",
        spec
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_bind_address_defaults_to_localhost() {
        assert_eq!(
            parse_cli_bind_address(None).unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8001)
        );
    }

    #[test]
    fn parse_cli_bind_address_accepts_ip_without_port() {
        assert_eq!(
            parse_cli_bind_address(Some("0.0.0.0".to_string())).unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8001)
        );
    }

    #[test]
    fn parse_cli_bind_address_accepts_ip_with_port() {
        assert_eq!(
            parse_cli_bind_address(Some("127.0.0.1:9001".to_string())).unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)
        );
    }
}
