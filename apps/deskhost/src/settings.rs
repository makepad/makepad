//! `~/.makepad/deskhost.conf` (`MAKEPAD_DESKHOST_CONF` names another file), `key = value`
//! lines, every key optional:
//!
//! ```text
//! enabled = on            # off (the default): nothing listens, the host exits
//! host = 0.0.0.0          # loopback, a private LAN address of this machine, or 0.0.0.0
//! port = 8790
//! kbps = 0                # 0: by picture size
//! fps = 120               # the cap: the stream runs at the lowest of it, the screen's refresh
//!                         # and the watching viewers' displays
//! key_every = 2           # seconds between scheduled key frames while pictures flow (0: only
//!                         # on a new viewer, a request or a lost picture)
//! ```
//!
//! It serves only with this machine's fleet identity (`id.key`, `id.cred`, `root.pub` in
//! `~/.makepad/fleet`, `%USERPROFILE%\.makepad\fleet` on Windows; tools/fleet-identity.md),
//! and only to the admin (the Mac's device credential from its bound address).

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub kbps: u32,
    pub fps: u32,
    pub key_every: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: false, host: "0.0.0.0".into(), port: makepad_desk_link::DEFAULT_PORT, kbps: 0, fps: 120, key_every: 2 }
    }
}

/// `~/.makepad` (`MAKEPAD_HOME` names another).
pub fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("MAKEPAD_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join(".makepad")
}

pub fn host_name() -> String {
    #[cfg(windows)]
    let name = {
        use makepad_windows_sys::core::PWSTR;
        use makepad_windows_sys::session::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
        unsafe {
            let mut size = 0;
            let _ = GetComputerNameExW(ComputerNameDnsHostname, None, &mut size);
            if size == 0 {
                None
            } else {
                let mut buffer = vec![0u16; size as usize];
                GetComputerNameExW(ComputerNameDnsHostname, Some(PWSTR(buffer.as_mut_ptr())), &mut size)
                    .ok().map(|()| String::from_utf16_lossy(&buffer[..size as usize]))
            }
        }
    };
    #[cfg(not(windows))]
    let name = std::env::var("COMPUTERNAME").ok()
        .or_else(|| std::process::Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()));
    name.filter(|h| !h.is_empty()).unwrap_or_else(|| "desk host".into())
}

impl Settings {
    pub fn path() -> PathBuf {
        match std::env::var_os("MAKEPAD_DESKHOST_CONF").filter(|p| !p.is_empty()) {
            Some(p) => PathBuf::from(p),
            None => home().join("deskhost.conf"),
        }
    }

    pub fn load() -> Settings {
        std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            let ok = match key {
                "enabled" => match value {
                    "on" | "true" | "yes" | "1" => {
                        s.enabled = true;
                        true
                    }
                    "off" | "false" | "no" | "0" => {
                        s.enabled = false;
                        true
                    }
                    _ => false,
                },
                "host" => {
                    s.host = value.to_string();
                    !value.is_empty()
                }
                "port" => value.parse().map(|p| s.port = p).is_ok(),
                "kbps" => value.parse().map(|k| s.kbps = k).is_ok(),
                "fps" => value.parse::<u32>().ok().filter(|f| (1..=240).contains(f)).map(|f| s.fps = f).is_some(),
                "key_every" => value.parse().map(|k| s.key_every = k).is_ok(),
                _ => false,
            };
            if !ok {
                crate::log(&format!("settings: skipped {line:?}"));
            }
        }
        s
    }

    /// Writes `enabled = on|off` into the settings file (the tray's switch): that line is replaced,
    /// or added; every other line stays as it is.
    pub fn set_enabled(on: bool) -> Result<(), String> {
        let path = Self::path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let value = if on { "on" } else { "off" };
        let mut found = false;
        let mut lines: Vec<String> = text
            .lines()
            .map(|l| {
                let key = l.split('#').next().unwrap_or("").split('=').next().unwrap_or("").trim();
                if key == "enabled" && !found {
                    found = true;
                    format!("enabled = {value}")
                } else {
                    l.to_string()
                }
            })
            .collect();
        if !found {
            lines.insert(0, format!("enabled = {value}"));
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The settings as the host engine takes them.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn host(&self) -> makepad_desk_link::host::HostSettings {
        makepad_desk_link::host::HostSettings {
            enabled: self.enabled,
            listen: self.listen_addr(),
            fps: self.fps,
            kbps: self.kbps,
            key_every: self.key_every,
            host_name: host_name(),
            tls_name: format!("makepad desk host {}", host_name()),
            source: Self::path().display().to_string(),
        }
    }

    pub fn listen_addr(&self) -> Result<SocketAddr, String> {
        let ip: IpAddr = self.host.parse().map_err(|_| format!("host {:?} is not an IP address", self.host))?;
        if !makepad_desk_link::server::bindable(ip) {
            return Err(format!("host {ip} is not a LAN address"));
        }
        Ok(SocketAddr::new(ip, self.port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_unless_enabled_and_lan_only() {
        assert!(!Settings::parse("").enabled);
        let s = Settings::parse("enabled = on\nhost = 10.0.0.230\nfps = 0");
        assert!(s.enabled && s.fps == 120 && s.key_every == 2 && s.listen_addr().is_ok());
        assert_eq!(Settings::parse("key_every = 0").key_every, 0);
        assert!(Settings::parse("host = 1.1.1.1").listen_addr().is_err());
    }

    #[test]
    fn the_switch_rewrites_only_the_enabled_line() {
        let dir = std::env::temp_dir().join(format!("deskhost-conf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("deskhost.conf");
        std::fs::write(&path, "# mine\nenabled = on   # yes\nfps = 60\n").unwrap();
        std::env::set_var("MAKEPAD_DESKHOST_CONF", &path);
        Settings::set_enabled(false).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "# mine\nenabled = off\nfps = 60\n");
        assert!(!Settings::load().enabled);
        Settings::set_enabled(true).unwrap();
        assert!(Settings::load().enabled && Settings::load().fps == 60);
        std::env::remove_var("MAKEPAD_DESKHOST_CONF");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
