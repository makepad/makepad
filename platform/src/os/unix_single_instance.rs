#![cfg(unix)]

use crate::single_instance::{push_app_open_item, set_app_socket_path, SingleInstanceResult};
use crate::thread::SignalToUI;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

extern "C" {
    fn getuid() -> u32;
    fn fchmod(fd: std::os::raw::c_int, mode: u32) -> std::os::raw::c_int;
}

fn app_name(app_id: &str) -> &str {
    app_id.rsplit('.').next().unwrap_or(app_id)
}

fn socket_dir(app_id: &str) -> PathBuf {
    let app_name = app_name(app_id);
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(app_name);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join(app_name);
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("state")
                .join(app_name);
        }
    }
    let uid = unsafe { getuid() };
    PathBuf::from(format!("/tmp/{}-{}", app_name, uid))
}

fn socket_path(app_id: &str) -> PathBuf {
    socket_dir(app_id).join("app.sock")
}

fn build_socket_path(app_id: &str, build_id: &str) -> PathBuf {
    socket_dir(app_id).join(format!("app-{}.sock", sanitize_component(build_id)))
}

fn pointer_path(app_id: &str) -> PathBuf {
    socket_path(app_id)
}

fn sanitize_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('0');
    }
    out
}

fn read_pointer(path: &Path) -> Option<PathBuf> {
    let value = std::fs::read_to_string(path).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

fn write_pointer(path: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, format!("{}\n", target.display())).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| e.to_string())
}

fn try_send(path: &Path, items: &[&str]) -> Result<(), ()> {
    let stream = UnixStream::connect(path).map_err(|_| ())?;
    let timeout = std::time::Duration::from_secs(2);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let mut writer = &stream;
    let mut reader = BufReader::new(&stream);
    for item in items {
        let mut line = item.to_string();
        line.push('\n');
        writer.write_all(line.as_bytes()).map_err(|_| ())?;
        writer.flush().map_err(|_| ())?;
        let mut resp = String::new();
        reader.read_line(&mut resp).map_err(|_| ())?;
        if resp.trim() != "OK" {
            return Err(());
        }
    }
    Ok(())
}

fn start_listener(path: &Path) {
    let path = path.to_owned();
    std::thread::Builder::new()
        .name("single-instance-listener".into())
        .spawn(move || {
            if let Some(parent) = path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    eprintln!("[single-instance] create_dir_all failed: {}", e);
                    return;
                }
            }
            let listener = match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[single-instance] bind failed: {}", e);
                    return;
                }
            };
            {
                use std::os::unix::io::AsRawFd;
                unsafe {
                    fchmod(listener.as_raw_fd(), 0o600);
                }
            }
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let reader = BufReader::new(&stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if line.is_empty() {
                        continue;
                    }
                    push_app_open_item(line);
                    SignalToUI::set_ui_signal();
                    let mut writer = &stream;
                    let _ = writer.write_all(b"OK\n");
                    let _ = writer.flush();
                }
            }
        })
        .expect("failed to spawn single-instance listener thread");
}

/// Try to be primary. If another instance exists, forward items and return
/// Secondary. Otherwise start listener and return Primary.
pub fn enable(app_id: &str, items: &[&str]) -> SingleInstanceResult {
    let path = socket_path(app_id);

    if try_send(&path, items).is_ok() {
        return SingleInstanceResult::Secondary;
    }

    let _ = std::fs::remove_file(&path);

    start_listener(&path);

    set_app_socket_path(path);
    SingleInstanceResult::Primary
}

pub fn enable_with_build(app_id: &str, build_id: &str, items: &[&str]) -> SingleInstanceResult {
    let pointer = pointer_path(app_id);
    let path = build_socket_path(app_id, build_id);

    if let Some(active) = read_pointer(&pointer) {
        if active == path {
            if try_send(&path, items).is_ok() {
                return SingleInstanceResult::Secondary;
            }
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(&pointer);
        } else if try_send(&active, &[]).is_ok() {
            return SingleInstanceResult::DifferentBuild;
        } else {
            let _ = std::fs::remove_file(&pointer);
        }
    }

    let _ = std::fs::remove_file(&path);

    start_listener(&path);
    let _ = write_pointer(&pointer, &path);

    set_app_socket_path(path);
    SingleInstanceResult::Primary
}

/// Remove the socket file. Call on shutdown.
pub fn cleanup() {
    if let Some(path) = crate::single_instance::app_socket_path() {
        let _ = std::fs::remove_file(&path);
    }
}
