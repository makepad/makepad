//! The virtual browser: the page side of a makepad web build (web.js `WasmWebBrowser`, web_gl.js
//! `WasmWebGL`, wasm_bridge.js `WasmBridge`, web_worker.js, audio_worklet.js), run against the
//! program's exports on a virtual clock.
//!
//! Order of a page's life, as the browser does it: `wasm_init_panic_hook`, `wasm_create_app`, the
//! message bridge (`wasm_get_js_message_bridge`: the page's message code, which a browser evaluates;
//! here the messages are read natively, so only its length is kept), the first message
//! (`wasm_return_first_msg`), then `load_deps`: ToWasmInit and a pump, ToWasmRedrawAll, the 16 ms
//! signal poll and a pump. After that everything is a task on the clock: animation frames at
//! 60 Hz while requested, timers, the signal poll, fetch completions, worker messages, the
//! script's input, the audio quanta.
//!
//! A pump (`do_wasm_pump`) sends the queued ToWasm messages in one `wasm_process_msg` and works
//! through the FromWasm messages it returns (`dispatch`), then frees the message.
//!
//! Workers (`js_spawn_thread`) run on threads of their own; before the clock moves the page
//! waits until every worker sleeps in `js_worker_wait` or has finished (`settle`), so what a
//! worker does lands in the same frame from run to run.

use crate::gl::{Gl, Reply};
use crate::imports::{self, mem, read_bytes, Queued};
use crate::trace::{Asset, Coverage};
use makepad_platform::web_protocol::from_wasm::*;
use makepad_platform::web_protocol::to_wasm::*;
use makepad_wasm_bridge::{take_data_u8_frees, FromWasm, FromWasmRead, ToWasm, ToWasmMsgRef, ToWasmWrite, ToWasmWriter, WasmDataU8};
use std::collections::HashMap;

type F0 = extern "C" fn() -> u32;
type F1 = extern "C" fn(u32) -> u32;
type F2 = extern "C" fn(u32, u32) -> u32;
type F3 = extern "C" fn(u32, u32, u32) -> u32;
type V1 = extern "C" fn(u32);
type V2 = extern "C" fn(u32, u32);
type V3 = extern "C" fn(u32, u32, u32);
type V4 = extern "C" fn(u32, u32, u32, u32);
type V6 = extern "C" fn(u32, u32, u32, u32, u32, u32);
type V9 = extern "C" fn(u32, u32, u32, u32, u32, u32, u32, u32, u32);
type AudioFn = extern "C" fn(u32, u32, u32, f64) -> u32;

/// The program's exports, by name (`relax_compiler::run::exports`), and how often the page called each.
pub struct Exports {
    addrs: HashMap<String, usize>,
    pub calls: std::sync::Mutex<HashMap<&'static str, u64>>,
}

macro_rules! export {
    ($self:ident, $name:literal, $ty:ty) => {{
        *$self.calls.lock().unwrap().entry($name).or_default() += 1;
        let a = *$self.addrs.get($name).unwrap_or_else(|| panic!("the program exports no {}", $name));
        unsafe { std::mem::transmute::<usize, $ty>(a) }
    }};
}

impl Exports {
    pub fn new(list: Vec<(String, u64)>) -> Self {
        Exports { addrs: list.into_iter().map(|(n, a)| (n, a as usize)).collect(), calls: Default::default() }
    }
    pub fn has(&self, name: &str) -> bool {
        self.addrs.contains_key(name)
    }
    /// The trace's own switch of the glyph records (draw's layouts, 3D type), where the program
    /// has them: not the page's calls, so not counted.
    pub fn record_glyphs(&self, on: bool) {
        for name in ["wasm_record_glyphs", "wasm_record_text3d_glyphs", "wasm_record_outline_glyphs"] {
            if let Some(&a) = self.addrs.get(name) {
                let f = unsafe { std::mem::transmute::<usize, V1>(a) };
                f(on as u32);
            }
        }
    }
    /// A trace-only switch `name(on)` (stress modes), when the program has it: whether it had.
    pub fn switch(&self, name: &str, on: bool) -> bool {
        let Some(&a) = self.addrs.get(name) else { return false };
        let f = unsafe { std::mem::transmute::<usize, V1>(a) };
        f(on as u32);
        true
    }
    /// The recorded shader pack (`wasm_take_shader_pack`).
    pub fn take_shader_pack(&self, app: u32) -> Option<Vec<u8>> {
        let &a = self.addrs.get("wasm_take_shader_pack")?;
        let f = unsafe { std::mem::transmute::<usize, F1>(a) };
        let p = mem(f(app)) as *const u64;
        let len = unsafe { p.read() } as usize;
        Some(unsafe { std::slice::from_raw_parts(p.add(1) as *const u8, len) }.to_vec())
    }
    /// The trace's Splash record switch (platform splash_trace.rs), before the app is made.
    pub fn record_splash(&self, on: bool) {
        if let Some(&a) = self.addrs.get("wasm_record_splash") {
            let f = unsafe { std::mem::transmute::<usize, V1>(a) };
            f(on as u32);
        }
    }
    /// The program's Splash record (JSON), when it has the export.
    pub fn take_splash_use(&self, app: u32) -> Option<String> {
        let &a = self.addrs.get("wasm_take_splash_use")?;
        let f = unsafe { std::mem::transmute::<usize, F1>(a) };
        let p = mem(f(app)) as *const u64;
        let len = unsafe { p.read() } as usize;
        let bytes = unsafe { std::slice::from_raw_parts(p.add(1) as *const u8, len) };
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
    /// The glyph records: (font file hash, glyph ids) from every recorder the program has.
    pub fn take_glyphs(&self) -> Vec<(u64, Vec<u64>)> {
        let mut out = Vec::new();
        for name in ["wasm_take_recorded_glyphs", "wasm_take_text3d_glyphs", "wasm_take_outline_glyphs"] {
            let Some(&a) = self.addrs.get(name) else { continue };
            let f = unsafe { std::mem::transmute::<usize, F0>(a) };
            let p = mem(f()) as *const u64;
            let len = unsafe { p.read() } as usize;
            let words = unsafe { std::slice::from_raw_parts(p, len.max(2)) };
            let mut at = 2;
            for _ in 0..words[1] {
                let (hash, n) = (words[at], words[at + 1] as usize);
                out.push((hash, words[at + 2..at + 2 + n].to_vec()));
                at += 2 + n;
            }
        }
        out
    }
    pub fn init_panic_hook(&self) { export!(self, "wasm_init_panic_hook", extern "C" fn())() }
    pub fn create_app(&self) -> u32 { export!(self, "wasm_create_app", F0)() }
    pub fn get_js_message_bridge(&self, app: u32) -> u32 { export!(self, "wasm_get_js_message_bridge", F1)(app) }
    pub fn return_first_msg(&self, app: u32) -> u32 { export!(self, "wasm_return_first_msg", F1)(app) }
    pub fn process_msg(&self, msg: u32, app: u32) -> u32 { export!(self, "wasm_process_msg", F2)(msg, app) }
    pub fn new_msg_with_u64_capacity(&self, cap: u32) -> u32 { export!(self, "wasm_new_msg_with_u64_capacity", F1)(cap) }
    pub fn msg_reserve_u64(&self, ptr: u32, cap: u32) -> u32 { export!(self, "wasm_msg_reserve_u64", F2)(ptr, cap) }
    pub fn msg_free(&self, ptr: u32) { export!(self, "wasm_msg_free", V1)(ptr) }
    pub fn new_data_u8(&self, cap: u32) -> u32 { export!(self, "wasm_new_data_u8", F1)(cap) }
    pub fn free_data_u8(&self, ptr: u32, len: u32, cap: u32) { export!(self, "wasm_free_data_u8", V3)(ptr, len, cap) }
    pub fn check_signal(&self) -> u32 { export!(self, "wasm_check_signal", F0)() }
    pub fn thread_alloc_tls_and_stack(&self, words: u32) -> u32 { export!(self, "wasm_thread_alloc_tls_and_stack", F1)(words) }
    pub fn thread_started(&self, id: u32) { export!(self, "wasm_thread_started", V1)(id) }
    pub fn thread_finished(&self, id: u32) { export!(self, "wasm_thread_finished", V1)(id) }
    pub fn thread_entrypoint(&self, id: u32, ctx: u32) { export!(self, "wasm_thread_entrypoint", V2)(id, ctx) }
    pub fn network_http_progress(&self, a: u32, b: u32, c: u32, d: u32) { export!(self, "wasm_network_http_progress", V4)(a, b, c, d) }
    #[allow(clippy::too_many_arguments)]
    pub fn network_http_response(&self, a: u32, b: u32, c: u32, d: u32, e: u32, f: u32, g: u32, h: u32, i: u32) {
        export!(self, "wasm_network_http_response", V9)(a, b, c, d, e, f, g, h, i)
    }
    pub fn network_http_error(&self, a: u32, b: u32, c: u32, d: u32, e: u32, f: u32) { export!(self, "wasm_network_http_error", V6)(a, b, c, d, e, f) }
    pub fn network_ws_error(&self, a: u32, b: u32, p: u32, l: u32) { export!(self, "wasm_network_ws_error", V4)(a, b, p, l) }
    pub fn network_ws_closed(&self, a: u32, b: u32) { export!(self, "wasm_network_ws_closed", V2)(a, b) }
    pub fn web_socket_closed(&self, id: u32) { export!(self, "wasm_web_socket_closed", V1)(id) }
    pub fn audio_output(&self, ctx: u32, frames: u32, channels: u32, rate: f64) -> u32 { export!(self, "wasm_audio_output_entrypoint", AudioFn)(ctx, frames, channels, rate) }

    /// `string_to_u8` / `array_to_u8`: a copy of `bytes` in the program's memory.
    pub fn data_u8(&self, bytes: &[u8]) -> u32 {
        let p = self.new_data_u8(bytes.len() as u32);
        if !bytes.is_empty() {
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), mem(p), bytes.len()) };
        }
        p
    }
}

/// The window the page has (CSS pixels) and its device pixel ratio.
#[derive(Clone, Copy)]
pub struct Window {
    pub width: f64,
    pub height: f64,
    pub dpi: f64,
}

struct Timer {
    id: usize,
    repeats: bool,
    interval: f64,
    next: f64,
}

/// One input or window event of the script, at a virtual time (seconds).
#[derive(Clone)]
pub enum Input {
    MouseDown { x: f64, y: f64, button: u32 },
    MouseMove { x: f64, y: f64 },
    MouseUp { x: f64, y: f64, button: u32 },
    Scroll { x: f64, y: f64, dx: f64, dy: f64 },
    KeyDown { key_code: u32, char_code: u32 },
    KeyUp { key_code: u32, char_code: u32 },
    Text(String),
    Resize(Window),
}

pub struct Audio {
    ctx: u32,
    /// the next quantum's virtual time
    next: f64,
    frames: u64,
}

const FRAME_S: f64 = 1.0 / 60.0;
const SIGNAL_POLL_S: f64 = 0.016;
const AUDIO_RATE: f64 = 48_000.0;
const AUDIO_QUANTUM: u32 = 128;

pub struct Page {
    pub x: &'static Exports,
    app: u32,
    window: Window,
    queued: Vec<(u64, &'static str, Box<dyn ToWasmWrite>)>,
    raf: bool,
    next_vsync: f64,
    timers: Vec<Timer>,
    next_poll: f64,
    pub gl: Gl,
    pub cov: Coverage,
    crate_dirs: HashMap<String, String>,
    /// `--serve /<prefix>=<dir>`: the page's own files, longest prefix first.
    serves: Vec<(String, String)>,
    /// `--query`: the page's `location.search`.
    query: String,
    workers: HashMap<u32, std::thread::JoinHandle<()>>,
    thread_stacks: Vec<(u32, u32)>,
    audio: Option<Audio>,
    from_names: HashMap<u64, &'static str>,
    unhandled: std::collections::BTreeSet<&'static str>,
    pub frames: u64,
    pack_record: bool,
    pack_install: Vec<Vec<u8>>,
    /// The FNV-1a 64 hash of every file served, to its URL (what a glyph record names a font by).
    pub file_hashes: HashMap<u64, String>,
    /// The linear memory's most committed and most live bytes, sampled each frame.
    pub peak_memory: (usize, usize),
    /// IndexedDB's `values` store (web.js storage_*): a fresh profile, empty at start
    storage: std::collections::BTreeMap<(String, String), Vec<u8>>,
    bridge_code_len: usize,
    title: String,
}

macro_rules! from_wasm_names {
    ($($t:ident),* $(,)?) => {
        fn from_wasm_names() -> HashMap<u64, &'static str> {
            let mut m = HashMap::new();
            $( m.insert(<$t as FromWasm>::live_id().0, stringify!($t)); )*
            m
        }
    };
}
from_wasm_names!(
    FromWasmStartTimer, FromWasmStopTimer, FromWasmStartLocationUpdates, FromWasmStopLocationUpdates, FromWasmFullScreen,
    FromWasmNormalScreen, FromWasmRequestAnimationFrame, FromWasmSetDocumentTitle, FromWasmSetMouseCursor,
    FromWasmTextCopyResponse, FromWasmGpuFrameTimer, FromWasmGpuInventory, FromWasmLinkModule, FromWasmUnlinkSlots,
    FromWasmStorageGet, FromWasmStorageSet, FromWasmStorageDelete, FromWasmStorageList, FromWasmStorageGetRange,
    FromWasmStorageStat, FromWasmStorageEstimate, FromWasmOpenUrl, FromWasmBrowserUpdateUrl, FromWasmBrowserHistoryGo,
    FromWasmShowTextIME, FromWasmHideTextIME, FromWasmSetVirtualFileLimits, FromWasmSelectFileDialog, FromWasmHTTPRequest,
    FromWasmCancelHTTPRequest, FromWasmCheckPermission, FromWasmRequestPermission, FromWasmSpreadFirstDraws,
    FromWasmCompileWebGLShader, FromWasmAllocArrayBuffer, FromWasmRetainedArrayBuffer, FromWasmRetainedArrayUpdate,
    FromWasmAllocIndexBuffer, FromWasmAllocVao, FromWasmFreeWebGLResources, FromWasmPollGpuCompletion,
    FromWasmAllocTextureImage2D_BGRAu8_32, FromWasmAllocTextureImage2D_Ru8, FromWasmAllocTextureImage2D_RGBAf32,
    FromWasmAllocTextureCube_BGRAu8_32, FromWasmBeginRenderTexture, FromWasmRequestRenderTextureCapture,
    FromWasmBeginRenderCanvas, FromWasmSetDefaultDepthAndBlendMode, FromWasmDrawCall, FromWasmXrStartPresenting,
    FromWasmXrStopPresenting, FromWasmQueryMidiPorts, FromWasmUseMidiInputs, FromWasmSendMidiOutput,
    FromWasmQueryAudioDevices, FromWasmStartAudioOutput, FromWasmStopAudioOutput, FromWasmPrepareVideoPlayback,
    FromWasmBeginVideoPlayback, FromWasmPauseVideoPlayback, FromWasmResumeVideoPlayback, FromWasmMuteVideoPlayback,
    FromWasmUnmuteVideoPlayback, FromWasmSeekVideoPlayback, FromWasmCleanupVideoPlaybackResources,
);

/// MAKEPAD_WEB_TRACE_LOG=1: every message both ways, with its virtual time.
fn log_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_WEB_TRACE_LOG").is_some())
}

/// The page's location (web.js reads `location`): resources resolve against it.
const ORIGIN: &str = "http://localhost";

impl Page {
    pub fn new(x: &'static Exports, window: Window, crate_dirs: Vec<(String, String)>, mut serves: Vec<(String, String)>, query: String) -> Self {
        serves.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        Page {
            x,
            app: 0,
            window,
            queued: Vec::new(),
            raf: false,
            next_vsync: 0.0,
            timers: Vec::new(),
            next_poll: f64::INFINITY,
            gl: Gl::new(),
            cov: Coverage::default(),
            crate_dirs: crate_dirs.into_iter().collect(),
            serves,
            query,
            workers: HashMap::new(),
            thread_stacks: Vec::new(),
            audio: None,
            from_names: from_wasm_names(),
            unhandled: Default::default(),
            frames: 0,
            pack_record: false,
            pack_install: Vec::new(),
            file_hashes: HashMap::new(),
            peak_memory: (0, 0),
            storage: Default::default(),
            bridge_code_len: 0,
            title: String::new(),
        }
    }

    /// `this.to_wasm.<T>(..)`: queued for the next pump.
    fn send<T: ToWasm + ToWasmWrite + 'static>(&mut self, m: T) {
        if log_on() {
            eprintln!("[page {:9.3}] -> {}", imports::now() * 1000.0, T::type_name());
        }
        *self.cov.to_wasm.entry(T::type_name().to_string()).or_default() += 1;
        self.queued.push((T::live_id().0, T::type_name(), Box::new(m)));
    }

    fn window_info(&self) -> WWindowInfo {
        WWindowInfo {
            is_fullscreen: false,
            can_fullscreen: true,
            xr_is_presenting: false,
            vr_supported: false,
            ar_supported: false,
            dpi_factor: self.window.dpi,
            inner_width: self.window.width,
            inner_height: self.window.height,
        }
    }

    // ---- start (WasmBridge / WasmWebBrowser constructors, load_deps)

    /// What runs as the app is made, before its first message: the shader pack recorded
    /// (`--record-pack`) or installed (`--pack`).
    pub fn with_pack(&mut self, record: bool, install: Vec<Vec<u8>>) {
        self.pack_record = record;
        self.pack_install = install;
    }

    /// The app the page made (`wasm_create_app`).
    pub fn app(&self) -> u32 {
        self.app
    }

    pub fn start(&mut self) {
        let x = self.x;
        x.init_panic_hook();
        self.app = x.create_app();
        if self.pack_record {
            let Some(&a) = x.addrs.get("wasm_record_shader_pack") else { panic!("--record-pack: the program has no wasm_record_shader_pack (a build without the compiler?)") };
            let f = unsafe { std::mem::transmute::<usize, V2>(a) };
            f(self.app, 1);
        }
        for pack in std::mem::take(&mut self.pack_install) {
            let Some(&a) = x.addrs.get("wasm_install_shader_pack") else { panic!("--pack: the program has no wasm_install_shader_pack") };
            let p = x.data_u8(&pack);
            let f = unsafe { std::mem::transmute::<usize, F3>(a) };
            let n = f(self.app, p, pack.len() as u32);
            eprintln!("makepad-web-trace: shader pack installed: {} entries", n);
        }
        // create_js_message_bridge: the page's message code (a string message), then freed
        let code = x.get_js_message_bridge(self.app);
        let words = self.msg_words(code);
        self.bridge_code_len = ToWasmMsgRef::from_words(words, 2).read_string().len();
        x.msg_free(code);
        // dispatch_first_msg
        let first = x.return_first_msg(self.app);
        self.dispatch(first);
        x.msg_free(first);
        self.drain();
        // load_deps
        let init = ToWasmInit {
            gpu_info: WGpuInfo { min_uniform_vectors: 1024, float_color_targets: true, vendor: "makepad-web-trace".into(), renderer: "makepad-web-trace (recorded WebGL 2)".into() },
            cpu_cores: 4,
            wasm_memory_max_pages: 65536,
            xr_capabilities: WXrCapabilities { vr_supported: false, ar_supported: false },
            browser_info: WBrowserInfo {
                protocol: "http:".into(),
                hostname: "localhost".into(),
                host: "localhost".into(),
                pathname: "/index.html".into(),
                search: self.query.clone(),
                hash: String::new(),
                has_thread_support: true,
                is_phone: false,
            },
            window_info: self.window_info(),
        };
        self.send(init);
        self.pump();
        self.send(ToWasmRedrawAll {});
        self.next_poll = imports::now() + SIGNAL_POLL_S;
        self.pump();
    }

    // ---- messages

    /// The u64 words of a message the program handed over (`len` in the header's high half).
    fn msg_words(&self, ptr: u32) -> &'static [u64] {
        let p = mem(ptr) as *const u64;
        let len = (unsafe { p.read() } >> 32) as usize;
        unsafe { std::slice::from_raw_parts(p, len.max(1)) }
    }

    /// `do_wasm_pump`: the queued messages in one `wasm_process_msg`, then the program's answer.
    pub fn pump(&mut self) {
        let x = self.x;
        let queued = std::mem::take(&mut self.queued);
        let mut data_u8 = |b: &[u8]| x.data_u8(b);
        let mut w = ToWasmWriter::new(&mut data_u8);
        for (id, _, m) in &queued {
            w.block(*id, |w| m.write_to_wasm(w));
        }
        let words = w.into_words();
        // new ToWasmMsg: a 1024-word message, grown by wasm_msg_reserve_u64 when short
        let mut ptr = x.new_msg_with_u64_capacity(1024);
        let cap = (unsafe { (mem(ptr) as *const u64).read() } & 0xffff_ffff) as usize;
        if cap < words.len() {
            ptr = x.msg_reserve_u64(ptr, words.len() as u32);
        }
        let p = mem(ptr) as *mut u64;
        unsafe {
            for (i, wd) in words.iter().enumerate().skip(1) {
                p.add(i).write(*wd);
            }
            let head = p.read() & 0xffff_ffff;
            p.write(head | (words.len() as u64) << 32);
        }
        let ret = x.process_msg(ptr, self.app);
        self.dispatch(ret);
        x.msg_free(ret);
    }

    /// `FromWasmMsg.dispatch_on_app`: each block's handler.
    fn dispatch(&mut self, ptr: u32) {
        let words = self.msg_words(ptr);
        let mut i = 1;
        let mut replies = Vec::new();
        while i < words.len() {
            let id = words[i];
            let len = (words.get(i + 1).copied().unwrap_or(0) & 0xffff_ffff) as usize;
            if len == 0 {
                break;
            }
            let mut r = ToWasmMsgRef::from_words(words, (i + 1) * 2 + 1);
            let name = self.from_names.get(&id).copied().unwrap_or("<unknown>");
            *self.cov.from_wasm.entry(name.to_string()).or_default() += 1;
            self.handle(name, &mut r, &mut replies);
            for (p, l, c) in take_data_u8_frees() {
                self.x.free_data_u8(p, l, c);
            }
            i += len;
        }
        for r in replies {
            match r {
                Reply::ShadersDone { count, waited } => self.send(ToWasmWebGLShadersDone { count, waited }),
                Reply::ShadersLinked { count } => self.send(ToWasmWebGLShadersLinked { count }),
                Reply::GpuCompletion { serial_lo, serial_hi, success } => {
                    self.send(ToWasmGpuCompletion { serial_lo, serial_hi, success });
                    self.pump();
                }
                Reply::Capture { texture_id, ticket_lo, ticket_hi, width, height } => {
                    self.send(ToWasmRenderTextureCapture {
                        texture_id, ticket_lo, ticket_hi, width, height, offset: 0, complete: true,
                        data: WasmDataU8::from_vec_u8(vec![0; width * height * 4]), error: String::new(),
                    });
                }
            }
        }
    }

    fn handle(&mut self, name: &'static str, r: &mut ToWasmMsgRef, replies: &mut Vec<Reply>) {
        if log_on() {
            eprintln!("[page {:9.3}] <- {}", imports::now() * 1000.0, name);
        }
        macro_rules! read {
            ($t:ident) => {
                <$t as FromWasmRead>::read_from_wasm(r)
            };
        }
        match name {
            "FromWasmStartTimer" => {
                let m = read!(FromWasmStartTimer);
                let id = m.timer_id as usize;
                if !m.interval.is_finite() || m.interval < 0.0 || self.timers.iter().any(|t| t.id == id) {
                    eprintln!("[page] timer {} refused (interval {}, or a collision)", id, m.interval);
                    return;
                }
                // setInterval clamps to 4 ms
                let interval = if m.repeats { m.interval.max(0.004) } else { m.interval };
                self.timers.push(Timer { id, repeats: m.repeats, interval, next: imports::now() + interval });
            }
            "FromWasmStopTimer" => {
                let m = read!(FromWasmStopTimer);
                self.timers.retain(|t| t.id != m.timer_id as usize);
            }
            "FromWasmRequestAnimationFrame" => {
                if !self.raf {
                    self.raf = true;
                    let now = imports::now();
                    self.next_vsync = ((now / FRAME_S + 1e-6).floor() + 1.0) * FRAME_S;
                }
            }
            "FromWasmSetDocumentTitle" => self.title = read!(FromWasmSetDocumentTitle).title,
            "FromWasmSpreadFirstDraws" => {
                if self.gl.spread_first_draws(read!(FromWasmSpreadFirstDraws), replies) {
                    self.send(ToWasmRedrawAll {});
                    self.request_frame();
                }
            }
            "FromWasmCompileWebGLShader" => self.gl.compile(read!(FromWasmCompileWebGLShader), replies),
            "FromWasmAllocArrayBuffer" => self.gl.array_buffer(read!(FromWasmAllocArrayBuffer)),
            "FromWasmRetainedArrayBuffer" => self.gl.retained_buffer(read!(FromWasmRetainedArrayBuffer)),
            "FromWasmRetainedArrayUpdate" => self.gl.retained_update(read!(FromWasmRetainedArrayUpdate)),
            "FromWasmAllocIndexBuffer" => self.gl.index_buffer(read!(FromWasmAllocIndexBuffer)),
            "FromWasmAllocVao" => self.gl.vao(read!(FromWasmAllocVao)),
            "FromWasmFreeWebGLResources" => self.gl.free(read!(FromWasmFreeWebGLResources)),
            "FromWasmAllocTextureImage2D_BGRAu8_32" => self.gl.texture_bgra(read!(FromWasmAllocTextureImage2D_BGRAu8_32)),
            "FromWasmAllocTextureImage2D_Ru8" => self.gl.texture_r8(read!(FromWasmAllocTextureImage2D_Ru8)),
            "FromWasmAllocTextureImage2D_RGBAf32" => self.gl.texture_rgbaf32(read!(FromWasmAllocTextureImage2D_RGBAf32)),
            "FromWasmAllocTextureCube_BGRAu8_32" => self.gl.texture_cube(read!(FromWasmAllocTextureCube_BGRAu8_32)),
            "FromWasmBeginRenderTexture" => self.gl.begin_texture(read!(FromWasmBeginRenderTexture)),
            "FromWasmBeginRenderCanvas" => self.gl.begin_canvas(read!(FromWasmBeginRenderCanvas)),
            "FromWasmSetDefaultDepthAndBlendMode" => {}
            "FromWasmDrawCall" => self.gl.draw(read!(FromWasmDrawCall), replies),
            "FromWasmPollGpuCompletion" => {
                let m = read!(FromWasmPollGpuCompletion);
                replies.push(Reply::GpuCompletion { serial_lo: m.serial_lo, serial_hi: m.serial_hi, success: true });
            }
            "FromWasmRequestRenderTextureCapture" => {
                let m = read!(FromWasmRequestRenderTextureCapture);
                if !m.register_only {
                    replies.push(Reply::Capture { texture_id: m.texture_id, ticket_lo: m.ticket_lo, ticket_hi: m.ticket_hi, width: m.width, height: m.height });
                }
            }
            "FromWasmQueryAudioDevices" => {
                // no inputs; the one honest output route (web.js publish_devices)
                self.send(ToWasmAudioDeviceList { devices: vec![WAudioDevice { web_device_id: "default".into(), label: "Browser audio".into(), is_output: true }] });
                self.pump();
            }
            "FromWasmStartAudioOutput" => {
                let m = read!(FromWasmStartAudioOutput);
                self.audio = Some(Audio { ctx: m.context_ptr, next: imports::now(), frames: 0 });
            }
            "FromWasmStopAudioOutput" => {
                if let Some(a) = self.audio.take() {
                    *self.cov.exports.entry("(audio frames)".into()).or_default() += a.frames;
                }
            }
            "FromWasmHTTPRequest" => {
                let m = read!(FromWasmHTTPRequest);
                let (status, headers, body) = self.serve(&m.url, &m.method, &m.headers);
                self.send(ToWasmHTTPResponse {
                    request_id_lo: m.request_id_lo, request_id_hi: m.request_id_hi,
                    metadata_id_hi: m.metadata_id_hi, metadata_id_lo: m.metadata_id_lo,
                    status, headers, body: WasmDataU8::from_vec_u8(body),
                });
            }
            "FromWasmCheckPermission" | "FromWasmRequestPermission" => {
                // the page has no camera, microphone or location: denied for good (state 3)
                let (permission, request_id) = if name == "FromWasmCheckPermission" {
                    let m = read!(FromWasmCheckPermission);
                    (m.permission, m.request_id)
                } else {
                    let m = read!(FromWasmRequestPermission);
                    (m.permission, m.request_id)
                };
                self.send(ToWasmPermissionResult { permission, request_id, status: 3 });
                self.pump();
            }
            "FromWasmStorageGet" => {
                let m = read!(FromWasmStorageGet);
                let v = self.storage.get(&(m.namespace, m.key)).cloned();
                self.storage_result(m.request_id_lo, m.request_id_hi, 0, v, Vec::new(), None, None);
            }
            "FromWasmStorageSet" => {
                let m = read!(FromWasmStorageSet);
                self.storage.insert((m.namespace, m.key), m.value.into_vec_u8());
                self.storage_result(m.request_id_lo, m.request_id_hi, 1, None, Vec::new(), None, None);
            }
            "FromWasmStorageDelete" => {
                let m = read!(FromWasmStorageDelete);
                self.storage.remove(&(m.namespace, m.key));
                self.storage_result(m.request_id_lo, m.request_id_hi, 2, None, Vec::new(), None, None);
            }
            "FromWasmStorageList" => {
                let m = read!(FromWasmStorageList);
                let mut keys: Vec<String> = self.storage.keys()
                    .filter(|(ns, k)| *ns == m.namespace && k.starts_with(&m.prefix) && (!m.has_after || *k > m.after))
                    .map(|(_, k)| k.clone()).take(m.limit as usize + 1).collect();
                let next = if keys.len() > m.limit as usize {
                    keys.truncate(m.limit as usize);
                    keys.last().cloned()
                } else { None };
                self.storage_result(m.request_id_lo, m.request_id_hi, 3, None, keys, next, None);
            }
            "FromWasmStorageGetRange" => {
                let m = read!(FromWasmStorageGetRange);
                let off = (m.offset_lo as u64 | (m.offset_hi as u64) << 32) as usize;
                let v = self.storage.get(&(m.namespace, m.key)).map(|v| v[off.min(v.len())..(off + m.len as usize).min(v.len())].to_vec());
                self.storage_result(m.request_id_lo, m.request_id_hi, 4, v, Vec::new(), None, None);
            }
            "FromWasmStorageStat" => {
                let m = read!(FromWasmStorageStat);
                let len = self.storage.get(&(m.namespace, m.key)).map(|v| v.len() as u64);
                self.storage_result(m.request_id_lo, m.request_id_hi, 5, len.map(|_| Vec::new()), Vec::new(), None, len);
            }
            "FromWasmStorageEstimate" => {
                let m = read!(FromWasmStorageEstimate);
                self.storage_result(m.request_id_lo, m.request_id_hi, 6, None, Vec::new(), None, None);
            }
            _ => {
                if self.unhandled.insert(name) {
                    eprintln!("[page] {} has no handler here (recorded, ignored)", name);
                }
            }
        }
    }

    /// `storage_send_result`; an estimate answers 0 used of 1 GiB.
    #[allow(clippy::too_many_arguments)]
    fn storage_result(&mut self, lo: u32, hi: u32, op: u32, value: Option<Vec<u8>>, keys: Vec<String>, next: Option<String>, length: Option<u64>) {
        let length = length.unwrap_or(0);
        let quota: u64 = if op == 6 { 1 << 30 } else { 0 };
        self.send(ToWasmStorageResult {
            request_id_lo: lo, request_id_hi: hi, op, found: value.is_some(),
            value: WasmDataU8::from_vec_u8(value.unwrap_or_default()), keys, has_next: next.is_some(), next: next.unwrap_or_default(),
            length_lo: length as u32, length_hi: (length >> 32) as u32, usage_lo: 0, usage_hi: 0,
            quota_lo: quota as u32, quota_hi: (quota >> 32) as u32, error_kind: 0, error: String::new(),
        });
        self.pump();
    }

    fn request_frame(&mut self) {
        if !self.raf {
            self.raf = true;
            self.next_vsync = ((imports::now() / FRAME_S + 1e-6).floor() + 1.0) * FRAME_S;
        }
    }

    // ---- resources: the page's files (`--serve`), `/<crate>/<path>` of the program's crates

    /// The file a page path names: a `--serve` prefix's, else a crate's.
    fn local_file(&self, path: &str) -> Option<String> {
        for (prefix, dir) in &self.serves {
            let prefix = prefix.trim_matches('/');
            if prefix.is_empty() {
                return Some(format!("{}/{}", dir, path));
            }
            if path == prefix {
                return Some(dir.clone());
            }
            if let Some(rest) = path.strip_prefix(prefix).and_then(|rest| rest.strip_prefix('/')) {
                return Some(format!("{}/{}", dir, rest));
            }
        }
        let (krate, rest) = path.split_once('/').unwrap_or((path, ""));
        let dir = self.crate_dirs.get(krate).or_else(|| self.crate_dirs.get(&krate.replace('-', "_")))?;
        Some(format!("{}/{}", dir, rest))
    }

    /// A fetch of the page: (status, `key: value\r\n` headers, body), recorded as an asset.
    /// A URL resolves against the page (`/index.html`), as `fetch` resolves it. A directory
    /// (a trailing `/`) answers with its listing: `f <size> <name>` / `d - <name>` lines.
    fn serve(&mut self, url: &str, method: &str, req_headers: &str) -> (u32, String, Vec<u8>) {
        let path = url.strip_prefix(ORIGIN).unwrap_or(url);
        if path.contains("://") {
            return (502, String::new(), format!("no network in the trace host: {}", url).into_bytes());
        }
        let path = path.split(['?', '#']).next().unwrap_or("");
        let mut parts: Vec<&str> = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => { parts.pop(); }
                part => parts.push(part),
            }
        }
        let is_dir = path.is_empty() || path.ends_with('/');
        let path = parts.join("/");
        let Some(file) = self.local_file(&path) else {
            return (404, String::new(), Vec::new());
        };
        let data = if is_dir {
            let Ok(entries) = std::fs::read_dir(&file) else {
                return (404, String::new(), Vec::new());
            };
            let mut lines: Vec<String> = entries.filter_map(|e| {
                let e = e.ok()?;
                let ty = e.file_type().ok()?;
                let name = e.file_name().into_string().ok()?;
                if ty.is_dir() { Some(format!("d - {}", name)) } else if ty.is_file() { Some(format!("f {} {}", e.metadata().ok()?.len(), name)) } else { None }
            }).collect();
            lines.sort();
            lines.into_iter().map(|l| l + "\n").collect::<String>().into_bytes()
        } else {
            match std::fs::read(&file) {
                Ok(data) if std::path::Path::new(&file).is_file() => data,
                _ => return (404, String::new(), Vec::new()),
            }
        };
        let path = if is_dir && !path.is_empty() { format!("{}/", path) } else { path };
        let size = data.len() as u64;
        // `Range: bytes=lo-hi` (streaming): the part, 206
        let range = req_headers.split("\r\n").find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if !k.trim().eq_ignore_ascii_case("range") {
                return None;
            }
            let v = v.trim().strip_prefix("bytes=")?;
            let (a, b) = v.split_once('-')?;
            let lo: u64 = a.trim().parse().ok()?;
            let hi: u64 = if b.trim().is_empty() { size.saturating_sub(1) } else { b.trim().parse().ok()? };
            Some((lo.min(size), (hi + 1).min(size)))
        });
        let key = format!("/{}", path);
        if !is_dir && !self.cov.assets.contains_key(&key) {
            let hash = data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
            self.file_hashes.insert(hash, key.clone());
        }
        let a = self.cov.assets.entry(key).or_insert_with(|| Asset { path: file.clone(), size, ..Default::default() });
        a.fetches += 1;
        if method == "HEAD" {
            return (200, format!("content-length: {}\r\n", size), Vec::new());
        }
        match range {
            Some((lo, hi)) => {
                a.add_range(lo, hi);
                (206, format!("content-length: {}\r\ncontent-range: bytes {}-{}/{}\r\n", hi - lo, lo, hi.saturating_sub(1), size), data[lo as usize..hi as usize].to_vec())
            }
            None => {
                a.add_range(0, size);
                (200, format!("content-length: {}\r\n", size), data)
            }
        }
    }

    // ---- what imports started (the queue), worked through after each call

    pub fn drain(&mut self) {
        loop {
            let q = std::mem::take(&mut *imports::QUEUE.lock().unwrap());
            if q.is_empty() {
                return;
            }
            for e in q {
                self.queued_event(e);
            }
        }
    }

    fn queued_event(&mut self, e: Queued) {
        let x = self.x;
        match e {
            Queued::WakeUi => self.deliver_signals(),
            Queued::SpawnThread { request_id, context_ptr, stack_size, name } => self.spawn_worker(request_id, context_ptr, stack_size, name),
            Queued::WorkerStarted(id) => x.thread_started(id),
            Queued::WorkerFinished(id) => {
                x.thread_finished(id);
                if let Some(h) = self.workers.remove(&id) {
                    let _ = h.join();
                }
            }
            Queued::Fetch { request: [a, b, c, d], url, method, headers, max_body } => {
                x.network_http_progress(a, b, 0, 0);
                let (status, rh, body) = self.serve(&url, &method, &headers);
                if method != "HEAD" && body.len() as u64 > max_body {
                    let m = x.data_u8(b"response body exceeds configured limit");
                    x.network_http_error(a, b, c, d, m, 38);
                    return;
                }
                if status >= 400 {
                    eprintln!("[page] [makepad][http][fail] {} {}", status, url);
                }
                let h = x.data_u8(rh.as_bytes());
                let bp = x.data_u8(&body);
                x.network_http_response(a, b, c, d, status, h, rh.len() as u32, bp, body.len() as u32);
            }
            Queued::CancelFetch => {}
            Queued::WsOpen { socket: [a, b], url } => {
                let m = format!("no network in the trace host: {}", url);
                let p = x.data_u8(m.as_bytes());
                x.network_ws_error(a, b, p, m.len() as u32);
                x.network_ws_closed(a, b);
            }
            Queued::LegacySocketOpen { id } => x.web_socket_closed(id),
        }
    }

    /// `deliver_signals`: other threads' signals go to the app now unless a frame is coming.
    fn deliver_signals(&mut self) {
        if self.raf {
            return;
        }
        let flags = self.x.check_signal();
        if flags != 0 {
            self.send(ToWasmSignal { flags });
            self.pump();
        }
    }

    /// create_thread: the worker's stack and TLS block in the program's memory
    /// (alloc_thread_stack), its thread, `started`, the entry point, `finished`.
    fn spawn_worker(&mut self, request_id: u32, context_ptr: u32, stack_size: u32, name: String) {
        let x = self.x;
        let stack = if stack_size == 0 { 2 << 20 } else { stack_size };
        let words = stack >> 3;
        let tls = match self.thread_stacks.iter().position(|b| b.1 == words) {
            Some(i) => self.thread_stacks.remove(i).0,
            None => x.thread_alloc_tls_and_stack(words),
        };
        let _ = tls;
        imports::WORKERS.lock().unwrap().sleeping.insert(request_id, None);
        let h = std::thread::Builder::new()
            .name(if name.is_empty() { format!("makepad-worker-{}", request_id) } else { name })
            .stack_size((stack as usize).max(16 << 20))
            .spawn(move || {
                imports::WORKER.with(|w| w.set(Some(request_id)));
                imports::WORKERS.lock().unwrap().native.insert(request_id, imports::native_thread());
                imports::QUEUE.lock().unwrap().push(Queued::WorkerStarted(request_id));
                x.thread_entrypoint(request_id, context_ptr);
                {
                    let mut g = imports::WORKERS.lock().unwrap();
                    g.sleeping.remove(&request_id);
                    g.native.remove(&request_id);
                }
                imports::WORKER_CLOCK.notify_all();
                imports::QUEUE.lock().unwrap().push(Queued::WorkerFinished(request_id));
            })
            .expect("worker thread");
        self.workers.insert(request_id, h);
    }

    /// Waits until every worker sleeps (js_worker_wait), waits on something (the scheduler says
    /// its thread is blocked: a lock, a condition, a channel, a file the page must fetch first) or
    /// has finished, so the clock moves only when nothing runs: what workers do lands in the same
    /// frame from run to run. Blocked means blocked in three looks 1 ms apart. After 250 ms of
    /// real time without that the page goes on (noted once).
    fn settle(&mut self) {
        let start = std::time::Instant::now();
        let mut g = imports::WORKERS.lock().unwrap();
        let mut still = 0;
        loop {
            let now = imports::now();
            let running: Vec<u64> = g.sleeping.iter().filter(|(_, s)| !matches!(s, Some(until) if *until > now)).map(|(id, _)| g.native.get(id).copied().unwrap_or(0)).collect();
            if running.is_empty() {
                return;
            }
            if running.iter().all(|&t| t != 0 && imports::blocked(t)) {
                still += 1;
                if still >= 3 {
                    return;
                }
                drop(g);
                std::thread::sleep(std::time::Duration::from_millis(1));
                g = imports::WORKERS.lock().unwrap();
                continue;
            }
            still = 0;
            let (ng, _) = imports::WORKER_CLOCK.wait_timeout(g, std::time::Duration::from_millis(10)).unwrap();
            g = ng;
            if !imports::QUEUE.lock().unwrap().is_empty() {
                return;
            }
            if start.elapsed() > std::time::Duration::from_millis(250) {
                static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("[page] a worker neither sleeps nor finishes; the clock goes on without it");
                }
                return;
            }
        }
    }

    // ---- the clock

    /// Runs the page until virtual time `end` (seconds), feeding `inputs` (sorted by time).
    pub fn run(&mut self, inputs: &[(f64, Input)], end: f64) {
        let mut next_input = 0;
        loop {
            if imports::STOP.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            self.drain();
            self.settle();
            self.drain();
            let now = imports::now();
            let mut t = end;
            if self.raf {
                t = t.min(self.next_vsync);
            }
            for tm in &self.timers {
                t = t.min(tm.next);
            }
            t = t.min(self.next_poll);
            if let Some((ti, _)) = inputs.get(next_input) {
                t = t.min(*ti);
            }
            if let Some(a) = &self.audio {
                t = t.min(a.next);
            }
            let t = t.max(now);
            if t >= end {
                imports::set_now(end);
                return;
            }
            imports::set_now(t);
            // audio quanta due (the worklet thread's callbacks), on the audio thread
            if let Some(a) = &mut self.audio {
                if a.next <= t {
                    let ctx = a.ctx;
                    let n = (((t - a.next) * AUDIO_RATE / AUDIO_QUANTUM as f64).floor() as u64 + 1).min(64);
                    a.next += n as f64 * AUDIO_QUANTUM as f64 / AUDIO_RATE;
                    a.frames += n * AUDIO_QUANTUM as u64;
                    let x = self.x;
                    let digest = std::thread::Builder::new().name("audio-worklet".into()).stack_size(16 << 20).spawn(move || {
                        let mut d = crate::gl::Digest::new();
                        for _ in 0..n {
                            let p = x.audio_output(ctx, AUDIO_QUANTUM, 2, AUDIO_RATE);
                            d.bytes(&read_bytes(p, AUDIO_QUANTUM * 2 * 4));
                        }
                        d.0
                    }).unwrap().join().unwrap_or(0);
                    let _ = digest;
                }
            }
            // the script's input due now
            while let Some((ti, inp)) = inputs.get(next_input) {
                if *ti > t {
                    break;
                }
                next_input += 1;
                self.input(inp.clone(), t);
            }
            // timers due
            let mut due: Vec<usize> = Vec::new();
            for tm in &mut self.timers {
                if tm.next <= t {
                    due.push(tm.id);
                    tm.next = if tm.repeats { tm.next + tm.interval } else { f64::INFINITY };
                }
            }
            self.timers.retain(|tm| tm.next.is_finite());
            for id in due {
                self.send(ToWasmTimerFired { timer_id: id });
                self.pump();
            }
            if self.next_poll <= t {
                self.next_poll += SIGNAL_POLL_S;
                self.deliver_signals();
            }
            if self.raf && self.next_vsync <= t {
                self.animation_frame(t);
            }
        }
    }

    /// A requestAnimationFrame callback (web.js FromWasmRequestAnimationFrame).
    fn animation_frame(&mut self, t: f64) {
        self.raf = false;
        self.gl.first_draw_this_frame = false;
        let flags = self.x.check_signal();
        if flags != 0 {
            self.send(ToWasmSignal { flags });
        }
        self.send(ToWasmAnimationFrame { time: t });
        self.pump();
        self.gl.end_frame();
        self.frames += 1;
        let (committed, live) = relax_compiler::zone_alloc::web::stats();
        if committed > self.peak_memory.0 && std::env::var_os("MAKEPAD_WEB_TRACE_MEM").is_some() {
            let (touched, slots, large, listed) = relax_compiler::zone_alloc::web::detail();
            eprintln!("[page] memory: {} MB committed, {} MB live; touched {} MB, slots {} MB, large {} MB, {} partial chunks",
                committed >> 20, live >> 20, touched >> 20, slots >> 20, large >> 20, listed);
        }
        self.peak_memory = (self.peak_memory.0.max(committed), self.peak_memory.1.max(live));
        if self.gl.first_draws_deferred {
            self.gl.first_draws_deferred = false;
            self.send(ToWasmRedrawAll {});
            self.request_frame();
        }
    }

    fn input(&mut self, i: Input, t: f64) {
        let mouse = |x: f64, y: f64, button: u32| WMouse { x, y, modifiers: 0, button, time: t };
        match i {
            Input::MouseDown { x, y, button } => self.send(ToWasmMouseDown { mouse: mouse(x, y, button) }),
            Input::MouseMove { x, y } => self.send(ToWasmMouseMove { was_out: false, mouse: mouse(x, y, 0) }),
            Input::MouseUp { x, y, button } => self.send(ToWasmMouseUp { mouse: mouse(x, y, button) }),
            Input::Scroll { x, y, dx, dy } => self.send(ToWasmScroll { x, y, modifiers: 0, is_touch: false, scroll_x: dx, scroll_y: dy, time: t }),
            Input::KeyDown { key_code, char_code } => self.send(ToWasmKeyDown { key: WKey { char_code, key_code, modifiers: 0, time: t, is_repeat: false } }),
            Input::KeyUp { key_code, char_code } => self.send(ToWasmKeyUp { key: WKey { char_code, key_code, modifiers: 0, time: t, is_repeat: false } }),
            Input::Text(s) => self.send(ToWasmTextInput { was_paste: false, replace_last: false, input: s }),
            Input::Resize(w) => {
                self.window = w;
                let window_info = self.window_info();
                self.send(ToWasmResizeWindow { window_info });
            }
        }
        self.pump();
    }

    pub fn summary(&self) -> String {
        format!("frames {}, peak memory {} MB committed / {} MB live, live GPU textures peak {} MB / at the end {} MB, draw calls {}, shaders {}, title {:?}, bridge code {} chars, unhandled {:?}",
            self.frames, self.peak_memory.0 >> 20, self.peak_memory.1 >> 20, self.gl.peak_texture_bytes >> 20, self.gl.live_textures.values().sum::<u64>() >> 20, self.gl.frames.iter().map(|f| f.draw_calls as u64).sum::<u64>(), self.gl.shaders_compiled, self.title, self.bridge_code_len, self.unhandled)
    }
}
