//! The hosts registry, `~/.config/makepad/remote/hosts.toml` (0600):
//!
//! ```toml
//! [host.149]
//! via = "ssh"
//! ssh = "arch@10.0.0.149"
//! relax = "/home/arch/bin/makepad"   # optional: the makepad binary on that host
//! env = { DISPLAY = ":0" }          # optional: environment of the sessions launched there
//!
//! [host.phone]
//! via = "dial"                      # the host dials in to the hub's TCP listener
//! key = "<64 hex digits>"           # the pairing key
//! fingerprint = "<64 hex digits>"   # the hub identity it pinned when paired
//! ```

use makepad_toml_parser::{parse_toml, Toml};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    /// reached with `ssh <ssh> <relax> remote-relay [--env K=V ...]`
    Ssh { ssh: String, relax: String, env: Vec<(String, String)> },
    /// dials in; `key` and `fingerprint` are hex
    Dial { key: String, fingerprint: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub name: String,
    pub via: Via,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hosts {
    pub hosts: Vec<Host>,
}

/// `~/.config/makepad/remote`
pub fn config_dir() -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("makepad").join("remote")
}

pub fn registry_path() -> PathBuf {
    config_dir().join("hosts.toml")
}

/// The folder of the hub's TLS identity.
pub fn identity_dir() -> PathBuf {
    config_dir().join("identity")
}

/// Environment variable names: what a shell accepts and a TOML bare key holds.
pub fn valid_env_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Host names are TOML bare keys without dots, and one word on a command line.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn parse_env(host: &str, v: Option<&Toml>) -> Result<Vec<(String, String)>, String> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let Toml::Table(t) = v else { return Err(format!("host `{host}`: env must be a table like env = {{ DISPLAY = \":0\" }}")) };
    let mut out = Vec::new();
    for (k, v) in t {
        if !valid_env_name(k) {
            return Err(format!("host `{host}`: env name `{k}`: use letters, digits and `_`"));
        }
        let v = v.as_str().ok_or_else(|| format!("host `{host}`: env {k} must be a string"))?;
        out.push((k.clone(), v.to_string()));
    }
    Ok(out)
}

impl Hosts {
    pub fn get(&self, name: &str) -> Option<&Host> {
        self.hosts.iter().find(|h| h.name == name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.hosts.iter().map(|h| h.name.as_str()).collect()
    }

    /// Replaces the host of the same name or adds it.
    pub fn set(&mut self, host: Host) {
        match self.hosts.iter_mut().find(|h| h.name == host.name) {
            Some(h) => *h = host,
            None => self.hosts.push(host),
        }
    }

    pub fn parse(text: &str) -> Result<Hosts, String> {
        let doc = parse_toml(text).map_err(|e| e.to_string())?;
        let mut out = Hosts::default();
        let Some(table) = doc.root.get("host") else { return Ok(out) };
        let Toml::Table(table) = table else { return Err("`host` must be a table of [host.<name>] sections".into()) };
        for (name, t) in table {
            let Toml::Table(t) = t else { return Err(format!("host `{name}` must be a [host.{name}] section")) };
            if !valid_name(name) {
                return Err(format!("host name `{name}`: use letters, digits, `-` and `_`"));
            }
            let text = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
            let via = match text("via").as_deref() {
                Some("ssh") => Via::Ssh {
                    ssh: text("ssh").ok_or_else(|| format!("host `{name}`: via = \"ssh\" needs ssh = \"user@host\""))?,
                    relax: text("makepad").unwrap_or_else(|| "makepad".into()),
                    env: parse_env(name, t.get("env"))?,
                },
                Some("dial") if t.contains_key("env") => return Err(format!("host `{name}`: env applies to via = \"ssh\" hosts; a dialling relay takes `--env K=V` on its command line")),
                Some("dial") => Via::Dial {
                    key: text("key").ok_or_else(|| format!("host `{name}`: via = \"dial\" needs key = \"<hex>\""))?,
                    fingerprint: text("fingerprint").unwrap_or_default(),
                },
                Some(other) => return Err(format!("host `{name}`: via must be \"ssh\" or \"dial\", not `{other}`")),
                None => return Err(format!("host `{name}`: missing via = \"ssh\" | \"dial\"")),
            };
            out.hosts.push(Host { name: name.clone(), via });
        }
        Ok(out)
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("# makepad-remote hosts (makepad hub). Keys in here are secrets: keep this file private.\n");
        for h in &self.hosts {
            s.push_str(&format!("\n[host.{}]\n", h.name));
            match &h.via {
                Via::Ssh { ssh, relax, env } => {
                    s.push_str(&format!("via = \"ssh\"\nssh = {}\n", quote(ssh)));
                    if relax != "makepad" {
                        s.push_str(&format!("makepad = {}\n", quote(relax)));
                    }
                    if !env.is_empty() {
                        let pairs: Vec<String> = env.iter().map(|(k, v)| format!("{k} = {}", quote(v))).collect();
                        s.push_str(&format!("env = {{ {} }}\n", pairs.join(", ")));
                    }
                }
                Via::Dial { key, fingerprint } => {
                    s.push_str(&format!("via = \"dial\"\nkey = {}\n", quote(key)));
                    if !fingerprint.is_empty() {
                        s.push_str(&format!("fingerprint = {}\n", quote(fingerprint)));
                    }
                }
            }
        }
        s
    }

    /// Loads the registry; a missing file is an empty registry.
    pub fn load(path: &Path) -> io::Result<Hosts> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                makepad_network::tls::check_private_file(path)?;
                Hosts::parse(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Hosts::default()),
            Err(e) => Err(e),
        }
    }

    /// Writes the registry, readable by this user only.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        makepad_network::tls::write_private(path, self.to_text().as_bytes())
    }

    /// The pairing keys of the dial hosts (what the hub's listener accepts).
    pub fn dial_keys(&self) -> Vec<makepad_network::tunnel::Psk> {
        self.hosts
            .iter()
            .filter_map(|h| match &h.via {
                Via::Dial { key, .. } => makepad_network::tunnel::Psk::from_hex(key),
                Via::Ssh { .. } => None,
            })
            .collect()
    }
}
