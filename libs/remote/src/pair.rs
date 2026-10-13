//! Pairing a dial host: a 256-bit key stored in the registry, the hub's identity pinned, and the
//! one-line command (or the pushed key file) that makes the device dial the hub.

use crate::hosts::{self, Host, Hosts, Via};
use makepad_network::tls::{to_hex, TlsIdentity};
use makepad_network::tunnel::{key_file_text, Psk};
use std::io;
use std::process::{Command, Stdio};

/// How the key reaches the device.
pub enum Delivery {
    /// the printed command writes the key file itself
    Print,
    /// `adb push` the key file, and `adb reverse` the hub port so the device dials 127.0.0.1
    Adb,
    /// pipe the key into `~/.config/makepad/remote/<name>.key` over `ssh <target>`
    Ssh(String),
}

/// Pairs `name` (new key; an earlier pairing of the same name is replaced) and returns the
/// command for the device. `hub`: the address the device dials (`host:port`).
pub fn pair(name: &str, hub: &str, delivery: Delivery) -> io::Result<String> {
    if !hosts::valid_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("host name `{name}`: use letters, digits, `-` and `_`")));
    }
    let path = hosts::registry_path();
    let mut registry = Hosts::load(&path)?;
    let identity = TlsIdentity::load_or_create(&hosts::identity_dir(), "makepad-remote hub")?;
    let fp = to_hex(&identity.fingerprint);
    let key = Psk::generate()?;
    registry.set(Host { name: name.to_string(), via: Via::Dial { key: key.to_hex(), fingerprint: fp.clone() } });
    registry.save(&path)?;
    let key_text = key_file_text(&[key]);
    let dial = |key_file: &str| format!("makepad remote-relay --name {name} --dial {hub} --key-file {key_file} --fingerprint {fp}");
    match delivery {
        Delivery::Print => {
            let file = format!("~/.config/makepad/remote/{name}.key");
            let hex = key_text.lines().last().unwrap_or("");
            Ok(format!("mkdir -p ~/.config/makepad/remote && (umask 077; echo {hex} > {file}) && {}", dial(&file)))
        }
        Delivery::Adb => {
            let local = std::env::temp_dir().join(format!("makepad-remote-{name}.key"));
            makepad_network::tls::write_private(&local, key_text.as_bytes())?;
            let remote = format!("/data/local/tmp/makepad-remote-{name}.key");
            let push = run(Command::new("adb").arg("push").arg(&local).arg(&remote));
            let _ = std::fs::remove_file(&local);
            push?;
            let port = hub.rsplit(':').next().unwrap_or("7907");
            run(Command::new("adb").args(["reverse", &format!("tcp:{port}"), &format!("tcp:{port}")]))?;
            Ok(dial_with(name, &format!("127.0.0.1:{port}"), &remote, &fp))
        }
        Delivery::Ssh(target) => {
            let file = format!("~/.config/makepad/remote/{name}.key");
            let mut child = Command::new("ssh")
                .args(["-o", "BatchMode=yes", &target, &format!("mkdir -p ~/.config/makepad/remote && umask 077 && cat > {file}")])
                .stdin(Stdio::piped())
                .spawn()?;
            {
                use std::io::Write;
                child.stdin.take().ok_or_else(|| io::Error::other("no ssh stdin"))?.write_all(key_text.as_bytes())?;
            }
            if !child.wait()?.success() {
                return Err(io::Error::other(format!("ssh {target}: could not store the key")));
            }
            Ok(dial(&file))
        }
    }
}

fn dial_with(name: &str, hub: &str, key_file: &str, fp: &str) -> String {
    format!("makepad remote-relay --name {name} --dial {hub} --key-file {key_file} --fingerprint {fp}")
}

fn run(c: &mut Command) -> io::Result<()> {
    let out = c.output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{:?}: {}", c.get_program(), String::from_utf8_lossy(&out.stderr).trim())))
    }
}
