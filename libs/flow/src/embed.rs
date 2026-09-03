//! Native flow-ui hosting policy. Hosting and attaching both converge on the
//! same HTTP client path; this module only decides which process owns root.

use crate::client::{health_answers, read_root_files, Endpoints};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmbedPolicy {
    #[default]
    Auto,
    Never,
    Always,
}

impl EmbedPolicy {
    pub fn from_env() -> Self {
        match std::env::var("FLOW_UI_FLOW_EMBED")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "never" | "no" | "off" | "0" | "false" | "attach" | "client" => Self::Never,
            "always" | "host" => Self::Always,
            _ => Self::Auto,
        }
    }
}

pub fn embed_policy() -> EmbedPolicy {
    EmbedPolicy::from_env()
}

pub fn default_root() -> PathBuf {
    // Dev-only override: `FLOW_ROOT=<dir>` points a test instance at an
    // isolated root so it never touches the user's `~/.makepad/flow`.
    if let Some(root) = std::env::var_os("FLOW_ROOT").filter(|value| !value.is_empty()) {
        return PathBuf::from(root);
    }
    makepad_ai_hub::home::makepad_home().join("flow")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// A missing endpoint is intentional: attach-only sessions keep reading
    /// `<root>/listen` and retrying while another process starts.
    Attach(
        Option<Endpoints>,
        Option<String>,
        Option<[u8; 16]>,
    ),
    Host,
}

pub fn resolve(policy: EmbedPolicy, root: impl AsRef<Path>, hint: Option<Endpoints>) -> Resolved {
    let root = root.as_ref();
    let files = read_root_files(root);
    let endpoints = hint.or(files.endpoints);
    let attach = || Resolved::Attach(endpoints, files.token.clone(), files.server_id);
    match policy {
        EmbedPolicy::Never => attach(),
        EmbedPolicy::Always => Resolved::Host,
        EmbedPolicy::Auto => {
            if endpoints.is_some_and(health_answers) || root_lock_held(root) {
                attach()
            } else {
                Resolved::Host
            }
        }
    }
}

fn root_lock_held(root: &Path) -> bool {
    let Ok(file) = OpenOptions::new()
        .write(true)
        .open(root.join("server.lock"))
    else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_root_hosts_in_auto_mode() {
        let root = std::env::temp_dir().join(format!(
            "makepad-flow-embed-empty-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(resolve(EmbedPolicy::Auto, &root, None), Resolved::Host);
        std::fs::remove_dir_all(root).unwrap();
    }
}
