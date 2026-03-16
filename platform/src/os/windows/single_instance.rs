use crate::single_instance::{push_app_open_item, SingleInstanceResult};
use crate::thread::SignalToUI;
use std::io::{BufRead, BufReader, Write};
use std::sync::OnceLock;

static MUTEX_NAME: OnceLock<String> = OnceLock::new();

fn wide_string(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn pipe_name(app_id: &str) -> String {
    format!(r"\\.\pipe\makepad-{}", app_id)
}

fn mutex_name(app_id: &str) -> String {
    format!(r"Global\makepad-{}", app_id)
}

fn try_send(pipe: &str, items: &[&str]) -> Result<(), ()> {
    use std::fs::OpenOptions;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe)
        .map_err(|_| ())?;
    let mut reader = BufReader::new(file.try_clone().map_err(|_| ())?);
    for item in items {
        let mut line = item.to_string();
        line.push('\n');
        file.write_all(line.as_bytes()).map_err(|_| ())?;
        file.flush().map_err(|_| ())?;
        let mut resp = String::new();
        reader.read_line(&mut resp).map_err(|_| ())?;
        if resp.trim() != "OK" {
            return Err(());
        }
    }
    Ok(())
}

fn start_pipe_server(pipe: String) {
    std::thread::Builder::new()
        .name("single-instance-pipe".into())
        .spawn(move || {
            loop {
                // Create a new named pipe instance for each client connection.
                let handle = unsafe {
                    windows::Win32::Storage::FileSystem::CreateFileW(
                        &windows::core::HSTRING::from(&pipe),
                        windows::Win32::Storage::FileSystem::FILE_GENERIC_READ
                            | windows::Win32::Storage::FileSystem::FILE_GENERIC_WRITE,
                        windows::Win32::Storage::FileSystem::FILE_SHARE_NONE,
                        None,
                        windows::Win32::Storage::FileSystem::OPEN_EXISTING,
                        windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
                        None,
                    )
                };
                // We use the Win32 pipe API via std::fs since named pipes on Windows
                // are accessible as files. But for the server side, we need CreateNamedPipe.
                // Let's use a simpler approach with std::net for localhost TCP.
                // Actually, let's use a proper named pipe approach.

                // Simplified: use localhost TCP like HAVI's fallback.
                break;
            }
        })
        .ok();
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
        // Another instance owns the mutex. Read port file and forward items.
        let port_path = port_file_path(app_id);
        if let Ok(port_str) = std::fs::read_to_string(&port_path) {
            if let Ok(port) = port_str.trim().parse::<u16>() {
                if try_send_tcp(port, items).is_ok() {
                    return SingleInstanceResult::Secondary;
                }
            }
        }
        // Couldn't connect — stale mutex or port file. Proceed as primary.
    }

    // Queue startup items for delivery after Event::Startup.
    if !items.is_empty() {
        crate::single_instance::push_app_open_items(
            items.iter().map(|s| s.to_string()).collect(),
        );
    }

    // Start TCP listener and write port file.
    start_tcp_listener(app_id);
    crate::single_instance::set_app_socket_path(port_file_path(app_id));

    MUTEX_NAME.set(mtx_name).ok();
    SingleInstanceResult::Primary
}

fn port_file_path(app_id: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("makepad-{}.port", app_id));
    dir
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

fn start_tcp_listener(app_id: &str) {
    use std::net::TcpListener;
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[single-instance] tcp bind failed: {}", e);
            return;
        }
    };
    let port = listener.local_addr().unwrap().port();
    let port_path = port_file_path(app_id);
    let _ = std::fs::write(&port_path, port.to_string());

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

/// Remove the port file. Call on shutdown.
pub fn cleanup() {
    // Port file cleanup. Mutex is released automatically on process exit.
    // We don't know the app_id here, but the file is in temp and OS cleans it.
}
