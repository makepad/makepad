use crate::single_instance::{push_app_open_item, SingleInstanceResult};
use crate::thread::SignalToUI;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static MUTEX_NAME: OnceLock<String> = OnceLock::new();

fn wide_string(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn mutex_name(app_id: &str) -> String {
    format!(r"Global\makepad-{}", app_id)
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

fn port_file_path(app_id: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("makepad-{}.port", app_id));
    dir
}

fn build_port_file_path(app_id: &str, build_id: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "makepad-{}-{}.port",
        app_id,
        sanitize_component(build_id)
    ));
    dir
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

fn try_send_tcp(port: u16, items: &[&str]) -> Result<(), ()> {
    use std::net::TcpStream;
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|_| ())?;
    let timeout = std::time::Duration::from_secs(2);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let mut reader = BufReader::new(stream.try_clone().map_err(|_| ())?);
    for item in items {
        let mut line = item.to_string();
        line.push('\n');
        stream.write_all(line.as_bytes()).map_err(|_| ())?;
        stream.flush().map_err(|_| ())?;
        let mut resp = String::new();
        reader.read_line(&mut resp).map_err(|_| ())?;
        if resp.trim() != "OK" {
            return Err(());
        }
    }
    Ok(())
}

fn try_send_port_file(path: &Path, items: &[&str]) -> Result<(), ()> {
    let port_str = std::fs::read_to_string(path).map_err(|_| ())?;
    let port = port_str.trim().parse::<u16>().map_err(|_| ())?;
    try_send_tcp(port, items)
}

fn start_tcp_listener(port_path: &Path) {
    use std::net::TcpListener;
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[single-instance] tcp bind failed: {}", e);
            return;
        }
    };
    let port = listener.local_addr().unwrap().port();
    let _ = std::fs::write(port_path, port.to_string());

    std::thread::Builder::new()
        .name("single-instance-listener".into())
        .spawn(move || {
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

/// Enable single-instance mode on Windows using a named mutex + localhost TCP.
pub fn enable(app_id: &str, items: &[&str]) -> SingleInstanceResult {
    let mtx_name = mutex_name(app_id);
    let mtx_wide = wide_string(&mtx_name);

    let handle = unsafe {
        windows::Win32::System::Threading::CreateMutexW(
            None,
            false,
            windows::core::PCWSTR(mtx_wide.as_ptr()),
        )
    };

    let already_exists = match handle {
        Ok(_) => unsafe {
            windows::Win32::Foundation::GetLastError()
                == windows::Win32::Foundation::ERROR_ALREADY_EXISTS
        },
        Err(_) => false,
    };

    if already_exists {
        let port_path = port_file_path(app_id);
        if try_send_port_file(&port_path, items).is_ok() {
            return SingleInstanceResult::Secondary;
        }
    }

    let port_path = port_file_path(app_id);
    start_tcp_listener(&port_path);
    crate::single_instance::set_app_socket_path(port_path);

    MUTEX_NAME.set(mtx_name).ok();
    SingleInstanceResult::Primary
}

pub fn enable_with_build(app_id: &str, build_id: &str, items: &[&str]) -> SingleInstanceResult {
    let mtx_name = mutex_name(app_id);
    let mtx_wide = wide_string(&mtx_name);

    let handle = unsafe {
        windows::Win32::System::Threading::CreateMutexW(
            None,
            false,
            windows::core::PCWSTR(mtx_wide.as_ptr()),
        )
    };

    let already_exists = match handle {
        Ok(_) => unsafe {
            windows::Win32::Foundation::GetLastError()
                == windows::Win32::Foundation::ERROR_ALREADY_EXISTS
        },
        Err(_) => false,
    };

    let pointer = port_file_path(app_id);
    let path = build_port_file_path(app_id, build_id);

    if already_exists {
        if let Some(active) = read_pointer(&pointer) {
            if active == path {
                if try_send_port_file(&path, items).is_ok() {
                    return SingleInstanceResult::Secondary;
                }
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_file(&pointer);
            } else if try_send_port_file(&active, &[]).is_ok() {
                return SingleInstanceResult::DifferentBuild;
            } else {
                let _ = std::fs::remove_file(&pointer);
            }
        }
    }

    start_tcp_listener(&path);
    let _ = write_pointer(&pointer, &path);
    crate::single_instance::set_app_socket_path(path);

    MUTEX_NAME.set(mtx_name).ok();
    SingleInstanceResult::Primary
}

/// Remove the port file. Call on shutdown.
pub fn cleanup() {
    // Port file cleanup. Mutex is released automatically on process exit.
}
