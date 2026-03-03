use crate::types::{HttpRequest, WebSocketMessage};
use makepad_live_id::LiveId;
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
const WINHTTP_OPTION_SECURITY_FLAGS: u32 = 31;
const WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET: u32 = 114;

const SECURITY_FLAG_IGNORE_UNKNOWN_CA: u32 = 0x0000_0100;
const SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE: u32 = 0x0000_0200;
const SECURITY_FLAG_IGNORE_CERT_CN_INVALID: u32 = 0x0000_1000;
const SECURITY_FLAG_IGNORE_CERT_DATE_INVALID: u32 = 0x0000_2000;

const WINHTTP_WEB_SOCKET_BINARY_MESSAGE_BUFFER_TYPE: u32 = 0;
const WINHTTP_WEB_SOCKET_BINARY_FRAGMENT_BUFFER_TYPE: u32 = 1;
const WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE: u32 = 2;
const WINHTTP_WEB_SOCKET_UTF8_FRAGMENT_BUFFER_TYPE: u32 = 3;
const WINHTTP_WEB_SOCKET_CLOSE_BUFFER_TYPE: u32 = 4;

const NO_ERROR: u32 = 0;
const WINHTTP_NORMAL_CLOSE_STATUS: u16 = 1000;

#[link(name = "winhttp")]
unsafe extern "system" {
    fn WinHttpOpen(
        user_agent: *const u16,
        access_type: u32,
        proxy_name: *const u16,
        proxy_bypass: *const u16,
        flags: u32,
    ) -> *mut c_void;

    fn WinHttpConnect(
        session: *mut c_void,
        server_name: *const u16,
        server_port: u16,
        reserved: u32,
    ) -> *mut c_void;

    fn WinHttpOpenRequest(
        connect: *mut c_void,
        verb: *const u16,
        object_name: *const u16,
        version: *const u16,
        referrer: *const u16,
        accept_types: *const *const u16,
        flags: u32,
    ) -> *mut c_void;

    fn WinHttpSetOption(
        handle: *mut c_void,
        option: u32,
        buffer: *mut c_void,
        buffer_len: u32,
    ) -> i32;

    fn WinHttpSendRequest(
        request: *mut c_void,
        headers: *const u16,
        headers_len: u32,
        optional: *mut c_void,
        optional_len: u32,
        total_len: u32,
        context: usize,
    ) -> i32;

    fn WinHttpReceiveResponse(request: *mut c_void, reserved: *mut c_void) -> i32;

    fn WinHttpWebSocketCompleteUpgrade(request: *mut c_void, context: usize) -> *mut c_void;

    fn WinHttpWebSocketSend(
        websocket: *mut c_void,
        buffer_type: u32,
        buffer: *mut c_void,
        buffer_len: u32,
    ) -> u32;

    fn WinHttpWebSocketReceive(
        websocket: *mut c_void,
        buffer: *mut c_void,
        buffer_len: u32,
        bytes_read: *mut u32,
        buffer_type: *mut u32,
    ) -> u32;

    fn WinHttpWebSocketClose(
        websocket: *mut c_void,
        close_status: u16,
        reason: *mut c_void,
        reason_len: u32,
    ) -> u32;

    fn WinHttpCloseHandle(handle: *mut c_void) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}

fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error_code() -> u32 {
    unsafe { GetLastError() }
}

fn os_error_string(code: u32) -> String {
    let err = std::io::Error::from_raw_os_error(code as i32);
    format!("{err} (code {code})")
}

struct WinHttpWebSocket {
    session: *mut c_void,
    connect: *mut c_void,
    websocket: *mut c_void,
    closed: AtomicBool,
}

unsafe impl Send for WinHttpWebSocket {}
unsafe impl Sync for WinHttpWebSocket {}

impl WinHttpWebSocket {
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    fn send_message(&self, message: WebSocketMessage) -> Result<(), u32> {
        if self.is_closed() {
            return Err(1);
        }
        let (buffer_type, bytes): (u32, Vec<u8>) = match message {
            WebSocketMessage::Binary(data) => (WINHTTP_WEB_SOCKET_BINARY_MESSAGE_BUFFER_TYPE, data),
            WebSocketMessage::String(data) => (
                WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE,
                data.into_bytes(),
            ),
            WebSocketMessage::Closed => {
                self.request_close();
                return Ok(());
            }
            WebSocketMessage::Opened | WebSocketMessage::Error(_) => return Ok(()),
        };

        let result = unsafe {
            WinHttpWebSocketSend(
                self.websocket,
                buffer_type,
                bytes.as_ptr() as *mut c_void,
                bytes.len() as u32,
            )
        };
        if result == NO_ERROR {
            Ok(())
        } else {
            Err(result)
        }
    }

    fn receive(&self, buffer: &mut [u8]) -> Result<(usize, u32), u32> {
        let mut bytes_read = 0u32;
        let mut buffer_type = 0u32;
        let result = unsafe {
            WinHttpWebSocketReceive(
                self.websocket,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                &mut bytes_read,
                &mut buffer_type,
            )
        };
        if result == NO_ERROR {
            Ok((bytes_read as usize, buffer_type))
        } else {
            Err(result)
        }
    }

    fn request_close(&self) {
        if self.is_closed() {
            return;
        }
        unsafe {
            let _ = WinHttpWebSocketClose(
                self.websocket,
                WINHTTP_NORMAL_CLOSE_STATUS,
                std::ptr::null_mut(),
                0,
            );
        }
    }

    fn shutdown(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        unsafe {
            let _ = WinHttpWebSocketClose(
                self.websocket,
                WINHTTP_NORMAL_CLOSE_STATUS,
                std::ptr::null_mut(),
                0,
            );
            let _ = WinHttpCloseHandle(self.websocket);
            let _ = WinHttpCloseHandle(self.connect);
            let _ = WinHttpCloseHandle(self.session);
        }
    }
}

impl Drop for WinHttpWebSocket {
    fn drop(&mut self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            unsafe {
                let _ = WinHttpWebSocketClose(
                    self.websocket,
                    WINHTTP_NORMAL_CLOSE_STATUS,
                    std::ptr::null_mut(),
                    0,
                );
                let _ = WinHttpCloseHandle(self.websocket);
                let _ = WinHttpCloseHandle(self.connect);
                let _ = WinHttpCloseHandle(self.session);
            }
        }
    }
}

fn open_winhttp_websocket(request: &HttpRequest) -> Result<Arc<WinHttpWebSocket>, String> {
    let split = request.split_url();
    let is_tls = match split.proto {
        "ws" | "http" => false,
        "wss" | "https" => true,
        other => {
            return Err(format!(
                "unsupported websocket scheme: {other} (expected ws/wss/http/https)"
            ));
        }
    };
    let port = split
        .port
        .parse::<u16>()
        .map_err(|_| format!("invalid websocket port: {}", split.port))?;

    let user_agent = wide_null("Makepad/1.0");
    let host = wide_null(split.host);
    let method = wide_null("GET");
    let path = if split.file.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", split.file)
    };
    let path = wide_null(&path);

    let session = unsafe {
        WinHttpOpen(
            user_agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    };
    if session.is_null() {
        return Err(format!(
            "WinHttpOpen failed: {}",
            os_error_string(last_error_code())
        ));
    }

    let connect = unsafe { WinHttpConnect(session, host.as_ptr(), port, 0) };
    if connect.is_null() {
        unsafe {
            let _ = WinHttpCloseHandle(session);
        }
        return Err(format!(
            "WinHttpConnect failed: {}",
            os_error_string(last_error_code())
        ));
    }

    let request_flags = if is_tls { WINHTTP_FLAG_SECURE } else { 0 };
    let request_handle = unsafe {
        WinHttpOpenRequest(
            connect,
            method.as_ptr(),
            path.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            request_flags,
        )
    };
    if request_handle.is_null() {
        unsafe {
            let _ = WinHttpCloseHandle(connect);
            let _ = WinHttpCloseHandle(session);
        }
        return Err(format!(
            "WinHttpOpenRequest failed: {}",
            os_error_string(last_error_code())
        ));
    }

    if is_tls && request.ignore_ssl_cert {
        let mut security_flags = SECURITY_FLAG_IGNORE_UNKNOWN_CA
            | SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE
            | SECURITY_FLAG_IGNORE_CERT_CN_INVALID
            | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID;
        let ok = unsafe {
            WinHttpSetOption(
                request_handle,
                WINHTTP_OPTION_SECURITY_FLAGS,
                (&mut security_flags as *mut u32).cast::<c_void>(),
                size_of::<u32>() as u32,
            )
        };
        if ok == 0 {
            unsafe {
                let _ = WinHttpCloseHandle(request_handle);
                let _ = WinHttpCloseHandle(connect);
                let _ = WinHttpCloseHandle(session);
            }
            return Err(format!(
                "WinHttpSetOption(SECURITY_FLAGS) failed: {}",
                os_error_string(last_error_code())
            ));
        }
    }

    let ok = unsafe {
        WinHttpSetOption(
            request_handle,
            WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok == 0 {
        unsafe {
            let _ = WinHttpCloseHandle(request_handle);
            let _ = WinHttpCloseHandle(connect);
            let _ = WinHttpCloseHandle(session);
        }
        return Err(format!(
            "WinHttpSetOption(UPGRADE_TO_WEB_SOCKET) failed: {}",
            os_error_string(last_error_code())
        ));
    }

    let additional_headers = request.get_headers_string();
    let additional_headers_wide = wide_null(&additional_headers);
    let (headers_ptr, headers_len) = if additional_headers.is_empty() {
        (std::ptr::null(), 0)
    } else {
        (additional_headers_wide.as_ptr(), u32::MAX)
    };

    let ok = unsafe {
        WinHttpSendRequest(
            request_handle,
            headers_ptr,
            headers_len,
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if ok == 0 {
        unsafe {
            let _ = WinHttpCloseHandle(request_handle);
            let _ = WinHttpCloseHandle(connect);
            let _ = WinHttpCloseHandle(session);
        }
        return Err(format!(
            "WinHttpSendRequest failed: {}",
            os_error_string(last_error_code())
        ));
    }

    let ok = unsafe { WinHttpReceiveResponse(request_handle, std::ptr::null_mut()) };
    if ok == 0 {
        unsafe {
            let _ = WinHttpCloseHandle(request_handle);
            let _ = WinHttpCloseHandle(connect);
            let _ = WinHttpCloseHandle(session);
        }
        return Err(format!(
            "WinHttpReceiveResponse failed: {}",
            os_error_string(last_error_code())
        ));
    }

    let websocket = unsafe { WinHttpWebSocketCompleteUpgrade(request_handle, 0) };
    unsafe {
        let _ = WinHttpCloseHandle(request_handle);
    }
    if websocket.is_null() {
        unsafe {
            let _ = WinHttpCloseHandle(connect);
            let _ = WinHttpCloseHandle(session);
        }
        return Err(format!(
            "WinHttpWebSocketCompleteUpgrade failed: {}",
            os_error_string(last_error_code())
        ));
    }

    Ok(Arc::new(WinHttpWebSocket {
        session,
        connect,
        websocket,
        closed: AtomicBool::new(false),
    }))
}

pub struct WindowsWebSocket {
    sender: Option<Sender<WebSocketMessage>>,
    socket: Option<Arc<WinHttpWebSocket>>,
}

impl Drop for WindowsWebSocket {
    fn drop(&mut self) {
        self.close();
    }
}

impl WindowsWebSocket {
    pub fn send_message(&mut self, message: WebSocketMessage) -> Result<(), ()> {
        if let Some(sender) = &self.sender {
            sender.send(message).map_err(|_| ())
        } else {
            Err(())
        }
    }

    pub fn close(&mut self) {
        self.sender.take();
        if let Some(socket) = self.socket.take() {
            socket.shutdown();
        }
    }

    pub fn open(
        _socket_id: LiveId,
        request: HttpRequest,
        rx_sender: Sender<WebSocketMessage>,
    ) -> WindowsWebSocket {
        let socket = match open_winhttp_websocket(&request) {
            Ok(socket) => socket,
            Err(error) => {
                let _ = rx_sender.send(WebSocketMessage::Error(error));
                return WindowsWebSocket {
                    sender: None,
                    socket: None,
                };
            }
        };

        let (sender, receiver) = channel::<WebSocketMessage>();

        {
            let writer_socket = Arc::clone(&socket);
            let writer_sender = rx_sender.clone();
            std::thread::spawn(move || {
                while let Ok(message) = receiver.recv() {
                    if let Err(code) = writer_socket.send_message(message) {
                        if !writer_socket.is_closed() {
                            let _ = writer_sender.send(WebSocketMessage::Error(format!(
                                "WinHTTP websocket send failed: {}",
                                os_error_string(code),
                            )));
                            let _ = writer_sender.send(WebSocketMessage::Closed);
                        }
                        writer_socket.shutdown();
                        break;
                    }
                }
            });
        }

        {
            let reader_socket = Arc::clone(&socket);
            std::thread::spawn(move || {
                let mut buf = [0u8; 64 * 1024];
                let mut text_fragments = Vec::new();
                let mut binary_fragments = Vec::new();

                loop {
                    match reader_socket.receive(&mut buf) {
                        Ok((size, buffer_type)) => {
                            let chunk = &buf[..size];
                            match buffer_type {
                                WINHTTP_WEB_SOCKET_BINARY_FRAGMENT_BUFFER_TYPE => {
                                    binary_fragments.extend_from_slice(chunk);
                                }
                                WINHTTP_WEB_SOCKET_BINARY_MESSAGE_BUFFER_TYPE => {
                                    if binary_fragments.is_empty() {
                                        if rx_sender
                                            .send(WebSocketMessage::Binary(chunk.to_vec()))
                                            .is_err()
                                        {
                                            reader_socket.shutdown();
                                            break;
                                        }
                                    } else {
                                        binary_fragments.extend_from_slice(chunk);
                                        if rx_sender
                                            .send(WebSocketMessage::Binary(std::mem::take(
                                                &mut binary_fragments,
                                            )))
                                            .is_err()
                                        {
                                            reader_socket.shutdown();
                                            break;
                                        }
                                    }
                                }
                                WINHTTP_WEB_SOCKET_UTF8_FRAGMENT_BUFFER_TYPE => {
                                    text_fragments.extend_from_slice(chunk);
                                }
                                WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE => {
                                    let message_bytes = if text_fragments.is_empty() {
                                        chunk.to_vec()
                                    } else {
                                        text_fragments.extend_from_slice(chunk);
                                        std::mem::take(&mut text_fragments)
                                    };
                                    let message =
                                        String::from_utf8_lossy(&message_bytes).into_owned();
                                    if rx_sender.send(WebSocketMessage::String(message)).is_err() {
                                        reader_socket.shutdown();
                                        break;
                                    }
                                }
                                WINHTTP_WEB_SOCKET_CLOSE_BUFFER_TYPE => {
                                    let _ = rx_sender.send(WebSocketMessage::Closed);
                                    reader_socket.shutdown();
                                    break;
                                }
                                _ => {}
                            }
                        }
                        Err(code) => {
                            if !reader_socket.is_closed() {
                                let _ = rx_sender.send(WebSocketMessage::Error(format!(
                                    "WinHTTP websocket receive failed: {}",
                                    os_error_string(code),
                                )));
                                let _ = rx_sender.send(WebSocketMessage::Closed);
                            }
                            reader_socket.shutdown();
                            break;
                        }
                    }
                }
            });
        }

        WindowsWebSocket {
            sender: Some(sender),
            socket: Some(socket),
        }
    }
}
