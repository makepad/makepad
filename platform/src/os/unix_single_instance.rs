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

    // Try connecting to existing instance.
    if try_send(&path, items).is_ok() {
        return SingleInstanceResult::Secondary;
    }

    // Remove stale socket file.
    let _ = std::fs::remove_file(&path);

    // Queue startup items for delivery after Event::Startup.
    if !items.is_empty() {
        crate::single_instance::push_app_open_items(
            items.iter().map(|s| s.to_string()).collect(),
        );
    }

    // Start listener. On EADDRINUSE, another instance won the race — retry as sender.
    start_listener(&path);

    set_app_socket_path(path);
    SingleInstanceResult::Primary
}

/// Remove the socket file. Call on shutdown.
pub fn cleanup() {
    if let Some(path) = crate::single_instance::app_socket_path() {
        let _ = std::fs::remove_file(path);
    }
}
