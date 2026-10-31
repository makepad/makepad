//! HTTP(S) on Windows through WinHTTP (flat C; Schannel, the system trust store and proxy
//! settings). Redirects are never followed.
use crate::{Request, Response, Sink, Url};
use std::ffi::c_void;
use std::time::Instant;

type H = *mut c_void;

#[link(name = "winhttp")]
extern "system" {
    fn WinHttpOpen(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> H;
    fn WinHttpConnect(session: H, server: *const u16, port: u16, reserved: u32) -> H;
    fn WinHttpOpenRequest(connect: H, verb: *const u16, object: *const u16, version: *const u16, referrer: *const u16, accept: *const *const u16, flags: u32) -> H;
    fn WinHttpSetTimeouts(h: H, resolve: i32, connect: i32, send: i32, receive: i32) -> i32;
    fn WinHttpSetOption(h: H, option: u32, buffer: *const c_void, len: u32) -> i32;
    fn WinHttpSendRequest(h: H, headers: *const u16, headers_len: u32, optional: *const c_void, optional_len: u32, total: u32, context: usize) -> i32;
    fn WinHttpReceiveResponse(h: H, reserved: *mut c_void) -> i32;
    fn WinHttpQueryHeaders(h: H, level: u32, name: *const u16, buffer: *mut c_void, len: *mut u32, index: *mut u32) -> i32;
    fn WinHttpQueryDataAvailable(h: H, available: *mut u32) -> i32;
    fn WinHttpReadData(h: H, buffer: *mut c_void, to_read: u32, read: *mut u32) -> i32;
    fn WinHttpCloseHandle(h: H) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> u32;
}

const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
const WINHTTP_OPTION_REDIRECT_POLICY: u32 = 88;
const WINHTTP_OPTION_REDIRECT_POLICY_NEVER: u32 = 0;
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
const WINHTTP_QUERY_RAW_HEADERS_CRLF: u32 = 22;
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
const ERROR_WINHTTP_TIMEOUT: u32 = 12002;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

struct Handle(H);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn fail(what: &str) -> String {
    let e = unsafe { GetLastError() };
    if e == ERROR_WINHTTP_TIMEOUT { "timed out".into() } else { format!("{what} failed (WinHTTP error {e})") }
}

pub(crate) fn send(req: &Request, url: &Url, out: &mut dyn std::io::Write, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Response, String> {
    let deadline = Instant::now() + req.timeout;
    let ms = req.timeout.as_millis().min(i32::MAX as u128) as i32;
    let agent = req.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("user-agent")).map(|(_, v)| *v).unwrap_or(crate::USER_AGENT);
    unsafe {
        let session = Handle(WinHttpOpen(wide(agent).as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0));
        if session.0.is_null() {
            return Err(fail("WinHttpOpen"));
        }
        let host = url.host.trim_start_matches('[').trim_end_matches(']');
        let connect = Handle(WinHttpConnect(session.0, wide(host).as_ptr(), url.port, 0));
        if connect.0.is_null() {
            return Err(fail("connect"));
        }
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            wide(req.method).as_ptr(),
            wide(url.target).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            if url.https { WINHTTP_FLAG_SECURE } else { 0 },
        ));
        if request.0.is_null() {
            return Err(fail("open request"));
        }
        WinHttpSetTimeouts(request.0, ms, ms, ms, ms);
        let never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        WinHttpSetOption(request.0, WINHTTP_OPTION_REDIRECT_POLICY, &never as *const u32 as *const c_void, 4);
        let mut head = String::from("Accept-Encoding: identity\r\n");
        for (n, v) in req.headers.iter().filter(|(n, _)| !n.eq_ignore_ascii_case("user-agent")) {
            head.push_str(&format!("{n}: {v}\r\n"));
        }
        let head = wide(&head);
        if WinHttpSendRequest(request.0, head.as_ptr(), u32::MAX, req.body.as_ptr() as *const c_void, req.body.len() as u32, req.body.len() as u32, 0) == 0 {
            return Err(fail("send"));
        }
        if WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0 {
            return Err(fail("receive"));
        }
        let mut status: u32 = 0;
        let mut len = 4u32;
        if WinHttpQueryHeaders(request.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), &mut status as *mut u32 as *mut c_void, &mut len, std::ptr::null_mut()) == 0 {
            return Err(fail("status"));
        }
        let mut len = 0u32;
        WinHttpQueryHeaders(request.0, WINHTTP_QUERY_RAW_HEADERS_CRLF, std::ptr::null(), std::ptr::null_mut(), &mut len, std::ptr::null_mut());
        let mut raw = vec![0u16; (len as usize / 2) + 1];
        let mut headers = Vec::new();
        if WinHttpQueryHeaders(request.0, WINHTTP_QUERY_RAW_HEADERS_CRLF, std::ptr::null(), raw.as_mut_ptr() as *mut c_void, &mut len, std::ptr::null_mut()) != 0 {
            let text = String::from_utf16_lossy(&raw[..len as usize / 2]);
            for line in text.split("\r\n").skip(1) {
                if let Some((n, v)) = line.split_once(':') {
                    headers.push((n.trim().to_string(), v.trim().to_string()));
                }
            }
        }
        let resp = Response { status: status as u16, headers };
        let expected = resp.header("content-length").and_then(|v| v.parse().ok());
        let mut sink = Sink { out, progress, received: 0, expected, max: req.max_body };
        let mut buf = vec![0u8; 1 << 16];
        loop {
            crate_left(deadline)?;
            let mut avail = 0u32;
            if WinHttpQueryDataAvailable(request.0, &mut avail) == 0 {
                return Err(fail("read"));
            }
            if avail == 0 {
                break;
            }
            let mut got = 0u32;
            if WinHttpReadData(request.0, buf.as_mut_ptr() as *mut c_void, avail.min(buf.len() as u32), &mut got) == 0 {
                return Err(fail("read"));
            }
            if got == 0 {
                break;
            }
            sink.put(&buf[..got as usize])?;
        }
        Ok(resp)
    }
}

fn crate_left(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline { Err("timed out".into()) } else { Ok(()) }
}
