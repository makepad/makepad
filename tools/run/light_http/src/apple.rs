//! HTTPS on macOS through NSURLSession (the current system API: the system trust store, proxy
//! settings and TLS 1.3), called with objc_msgSend directly. One download task per request: the
//! body arrives in a temporary file (read into the writer afterwards, so memory stays flat), the
//! progress is the task's byte counters, and a session delegate refuses redirects so a 3xx comes
//! back as it is, as on the other platforms.
use crate::{Request, Response, Sink};
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::Read;
use std::sync::OnceLock;
use relax_session::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

type Id = *mut c_void;
type Sel = *const c_void;
type Class = *mut c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Class;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_allocateClassPair(superclass: Class, name: *const c_char, extra: usize) -> Class;
    fn objc_registerClassPair(cls: Class);
    fn class_addMethod(cls: Class, name: Sel, imp: *const c_void, types: *const c_char) -> bool;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "Foundation", kind = "framework")]
extern "C" {}

#[link(name = "System")]
extern "C" {
    static _NSConcreteGlobalBlock: [*const c_void; 32];
}

fn class(name: &str) -> Class {
    let c = CString::new(name).unwrap();
    unsafe { objc_getClass(c.as_ptr()) }
}
fn sel(name: &str) -> Sel {
    let c = CString::new(name).unwrap();
    unsafe { sel_registerName(c.as_ptr()) }
}

/// objc_msgSend with the argument and result types of the call (`msg!(obj, "sel:", A, B => R; a, b)`).
macro_rules! msg {
    ($obj:expr, $sel:expr $(, $t:ty)* => $r:ty $(; $($a:expr),*)?) => {{
        let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $r = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f($obj as Id, sel($sel) $($(, $a)*)?)
    }};
}

unsafe fn nsstring(s: &str) -> Id {
    let c = CString::new(s).unwrap_or_default();
    msg!(class("NSString"), "stringWithUTF8String:", *const c_char => Id; c.as_ptr())
}
unsafe fn rust_string(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    let p: *const c_char = msg!(s, "UTF8String" => *const c_char);
    if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
}

/// The block ABI's literal layout (a global block: no copy/dispose helpers, copying returns it).
#[repr(C)]
struct Block<F> {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: F,
    descriptor: *const BlockDescriptor,
    done: *const Done,
}
#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}
static DESCRIPTOR: BlockDescriptor = BlockDescriptor { reserved: 0, size: std::mem::size_of::<Block<usize>>() };
const BLOCK_IS_GLOBAL: i32 = 1 << 28;

/// What the completion handler hands back.
struct Done {
    result: Mutex<Option<Result<(Response, Option<std::path::PathBuf>), String>>>,
    cv: Condvar,
    file: std::path::PathBuf,
}

/// downloadTaskWithRequest:completionHandler:'s handler: (NSURL *location, NSURLResponse *, NSError *).
unsafe extern "C" fn completed(block: *mut Block<usize>, location: Id, response: Id, error: Id) {
    let done = &*(*block).done;
    let result = if !error.is_null() {
        let text = rust_string(msg!(error, "localizedDescription" => Id));
        Err(if text.is_empty() { "request failed".to_string() } else { text })
    } else {
        let status: isize = msg!(response, "statusCode" => isize);
        let mut headers = Vec::new();
        let fields: Id = msg!(response, "allHeaderFields" => Id);
        let keys: Id = msg!(fields, "allKeys" => Id);
        let n: usize = msg!(keys, "count" => usize);
        for i in 0..n {
            let k: Id = msg!(keys, "objectAtIndex:", usize => Id; i);
            let v: Id = msg!(fields, "objectForKey:", Id => Id; k);
            headers.push((rust_string(k), rust_string(v)));
        }
        // the file is deleted when this handler returns: move it first
        let mut file = None;
        if !location.is_null() {
            let path = rust_string(msg!(location, "path" => Id));
            if std::fs::rename(&path, &done.file).is_ok() || std::fs::copy(&path, &done.file).is_ok() {
                file = Some(done.file.clone());
            }
        }
        Ok((Response { status: status as u16, headers }, file))
    };
    *done.result.lock().unwrap() = Some(result);
    done.cv.notify_all();
}

/// URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler: -> completionHandler(nil)
unsafe extern "C" fn no_redirect(_this: Id, _cmd: Sel, _session: Id, _task: Id, _response: Id, _request: Id, handler: *mut Block<unsafe extern "C" fn(*mut c_void, Id)>) {
    ((*handler).invoke)(handler as *mut c_void, std::ptr::null_mut());
}

/// One ephemeral session for the process (no cookies, no cache), its delegate refusing redirects.
fn session() -> Id {
    static SESSION: OnceLock<usize> = OnceLock::new();
    *SESSION.get_or_init(|| unsafe {
        let name = CString::new("RelaxLightHttpDelegate").unwrap();
        let mut cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
        if cls.is_null() {
            cls = objc_getClass(name.as_ptr());
        } else {
            let types = CString::new("v@:@@@@@?").unwrap();
            class_addMethod(cls, sel("URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler:"), no_redirect as *const c_void, types.as_ptr());
            objc_registerClassPair(cls);
        }
        let delegate: Id = msg!(msg!(cls, "alloc" => Id), "init" => Id);
        let config: Id = msg!(class("NSURLSessionConfiguration"), "ephemeralSessionConfiguration" => Id);
        let s: Id = msg!(class("NSURLSession"), "sessionWithConfiguration:delegate:delegateQueue:", Id, Id, Id => Id; config, delegate, std::ptr::null_mut::<c_void>());
        let s: Id = msg!(s, "retain" => Id);
        s as usize
    }) as Id
}

pub(crate) fn send(req: &Request, out: &mut dyn std::io::Write, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Response, String> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let file = std::env::temp_dir().join(format!(
        "relax-light-http-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let done = Done { result: Mutex::new(None), cv: Condvar::new(), file };
    let deadline = Instant::now() + req.timeout;
    let mut sink = Sink { out, progress, received: 0, expected: None, max: req.max_body };
    let result = unsafe {
        let pool = objc_autoreleasePoolPush();
        let r = run(req, &done, deadline, &mut sink);
        objc_autoreleasePoolPop(pool);
        r
    };
    let _ = std::fs::remove_file(&done.file);
    result
}

unsafe fn run(req: &Request, done: &Done, deadline: Instant, sink: &mut Sink) -> Result<Response, String> {
    let url: Id = msg!(class("NSURL"), "URLWithString:", Id => Id; nsstring(req.url));
    if url.is_null() {
        return Err(format!("bad URL {}", req.url));
    }
    // NSURLRequestReloadIgnoringLocalCacheData
    let r: Id = msg!(class("NSMutableURLRequest"), "requestWithURL:cachePolicy:timeoutInterval:", Id, usize, f64 => Id; url, 1usize, req.timeout.as_secs_f64());
    msg!(r, "setHTTPMethod:", Id => (); nsstring(req.method));
    if !req.headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("user-agent")) {
        msg!(r, "setValue:forHTTPHeaderField:", Id, Id => (); nsstring(crate::USER_AGENT), nsstring("User-Agent"));
    }
    msg!(r, "setValue:forHTTPHeaderField:", Id, Id => (); nsstring("identity"), nsstring("Accept-Encoding"));
    for (n, v) in req.headers {
        msg!(r, "setValue:forHTTPHeaderField:", Id, Id => (); nsstring(v), nsstring(n));
    }
    if !req.body.is_empty() {
        let data: Id = msg!(class("NSData"), "dataWithBytes:length:", *const c_void, usize => Id; req.body.as_ptr() as *const c_void, req.body.len());
        msg!(r, "setHTTPBody:", Id => (); data);
    }
    let block = Block {
        isa: &_NSConcreteGlobalBlock as *const _ as *const c_void,
        flags: BLOCK_IS_GLOBAL,
        reserved: 0,
        invoke: completed as unsafe extern "C" fn(*mut Block<usize>, Id, Id, Id),
        descriptor: &DESCRIPTOR,
        done,
    };
    let task: Id = msg!(session(), "downloadTaskWithRequest:completionHandler:", Id, *const c_void => Id; r, &block as *const _ as *const c_void);
    msg!(task, "resume" => ());
    // wait, following the byte counters; a timeout cancels and still waits for the handler
    // (the block lives on this stack)
    let mut cancelled = false;
    let result = loop {
        let guard = done.result.lock().unwrap();
        let (mut guard, _) = done.cv.wait_timeout(guard, Duration::from_millis(100)).unwrap();
        if let Some(r) = guard.take() {
            break r;
        }
        drop(guard);
        let got: i64 = msg!(task, "countOfBytesReceived" => i64);
        let total: i64 = msg!(task, "countOfBytesExpectedToReceive" => i64);
        (sink.progress)(got.max(0) as u64, (total > 0).then_some(total as u64));
        if !cancelled && (Instant::now() >= deadline || got.max(0) as u64 > sink.max) {
            msg!(task, "cancel" => ());
            cancelled = true;
        }
    };
    if cancelled {
        return Err(if Instant::now() >= deadline { "timed out".into() } else { format!("response body larger than {} bytes", sink.max) });
    }
    let (resp, file) = result?;
    if let Some(file) = file {
        let mut f = std::fs::File::open(&file).map_err(|e| e.to_string())?;
        sink.expected = f.metadata().ok().map(|m| m.len());
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = f.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            sink.put(&buf[..n])?;
        }
    }
    Ok(resp)
}
