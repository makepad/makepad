//! Collect mode: the app run once, hidden, to learn what its web build
//! needs, written as one manifest directory (`makepad-web-manifest`).
//!
//! `MAKEPAD_RUN=collect-web MAKEPAD_COLLECT=<dir> <native app binary>`
//! runs the app on its normal desktop backend (hidden with
//! `MAKEPAD_HIDE_WINDOWS=1`, muted with `SANDBOX_MUTE=1`), with what the
//! web build would have:
//!
//! - the web font set (`app_main!`'s wasm choice), so the faces laid out
//!   are the faces the web build ships;
//! - the web shader compiler beside the native one: every draw shader that
//!   is applied is also compiled to GLSL ES with its reflection
//!   ([`crate::shader_pack`]), from before the app's first script module.
//!
//! The run: a few frames draw; then ONE [`Event::Scan`] goes through the
//! app and its widget tree. Every widget forwards it to its children;
//! widgets that create children from data spawn the variants they could
//! create (a `scan:` list names them) and forward into those. Whatever is
//! applied, laid out or emitted on the way is collected: draw shaders,
//! glyphs per font file, coverage ([`ScanEvent::need_font_full`] ...),
//! widget types, kernels, assets, the web runtime's JS sections and
//! diagnostics. Plug-in [`Scanner`]s (a runtime's own, the text stack's)
//! run after every frame until they report done. Then the manifest is
//! written and the app quits.
//!
//! Nothing here runs unless the run mode asks for it: an app sees
//! `Event::Scan` only in a collect run.

use crate::cx::Cx;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::rc::Rc;

/// Whether `MAKEPAD_RUN=collect-web` asks for a collect run (desktop only).
fn collect_web_requested() -> bool {
    cfg!(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))
        && std::env::var("MAKEPAD_RUN").as_deref() == Ok("collect-web")
}

/// What a scan collected, shared by the [`ScanEvent`]s and the collector.
#[derive(Default)]
pub struct ScanState {
    step: usize,
    by: Vec<String>,
    visited: HashSet<u64>,
    /// (face, spec, by)
    coverage: BTreeSet<(String, String, String)>,
    glyphs: BTreeMap<String, BTreeSet<u16>>,
    texts: BTreeMap<String, BTreeMap<String, u32>>,
    widgets: BTreeMap<String, (usize, String)>,
    js: BTreeMap<String, BTreeSet<String>>,
    assets: BTreeMap<String, String>,
    kernels: BTreeMap<String, (Vec<u8>, String)>,
    kernel_module: Option<(Vec<u8>, Vec<u8>)>,
    features: BTreeSet<String>,
    summary: BTreeMap<String, String>,
    /// (severity, file, line, col, message)
    diagnostics: Vec<(String, String, u32, u32, String)>,
}

/// The scan, as `Event::Scan` carries it: the one handle widgets, apps
/// and runtimes emit their needs through. Cheap to clone (a shared
/// handle); every emit records who emitted it ([`ScanEvent::by`]).
#[derive(Clone, Default)]
pub struct ScanEvent {
    state: Rc<RefCell<ScanState>>,
}

impl fmt::Debug for ScanEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ScanEvent(step {})", self.state.borrow().step)
    }
}

/// While alive, emits are recorded as coming from this emitter
/// ([`ScanEvent::by`]).
pub struct ScanBy<'a> {
    scan: &'a ScanEvent,
}

impl Drop for ScanBy<'_> {
    fn drop(&mut self) {
        self.scan.state.borrow_mut().by.pop();
    }
}

/// The FNV-1a 64 hash of a font file as 16 hex digits: how a scan names a
/// face (the manifest's `font_hash`).
pub fn font_hash(bytes: &[u8]) -> String {
    let h = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    format!("{h:016x}")
}

/// The characters of `text` as a coverage spec (`chars:` with the comma
/// as a range, since specs are comma-separated).
pub fn chars_spec(text: &str) -> String {
    let chars: BTreeSet<char> = text.chars().filter(|c| !c.is_control()).collect();
    let mut spec = String::from("chars:");
    let mut comma = false;
    for c in chars {
        if c == ',' {
            comma = true;
        } else {
            spec.push(c);
        }
    }
    if comma {
        spec.push_str(",U+002C-U+002C");
    }
    spec
}

impl ScanEvent {
    /// The step of the run this event belongs to.
    pub fn step(&self) -> usize {
        self.state.borrow().step
    }

    /// Emits from now until the guard drops come from `who` (a widget type
    /// and path, a runtime's name). Nested: the innermost one counts.
    pub fn by(&self, who: impl Into<String>) -> ScanBy<'_> {
        self.state.borrow_mut().by.push(who.into());
        ScanBy { scan: self }
    }

    /// Who an emit made now comes from: the innermost [`Self::by`], else
    /// the caller's source location.
    #[track_caller]
    pub fn current_by(&self) -> String {
        let at = std::panic::Location::caller();
        match self.state.borrow().by.last() {
            Some(by) => by.clone(),
            None => format!("{}:{}", at.file(), at.line()),
        }
    }

    /// True the first time `id` (a widget uid) is visited in this scan:
    /// the forwarding guard that makes the tree walk visit each widget once.
    pub fn first_visit(&self, id: u64) -> bool {
        self.state.borrow_mut().visited.insert(id)
    }

    /// The whole of a font face: text that cannot be bounded (an input, a
    /// name list) is shown in it. `face` is the font file's
    /// `font_hash` (the draw crate's helpers resolve a text style to it).
    #[track_caller]
    pub fn need_font_full(&self, face: &str) {
        self.need_font_range(face, "all");
    }

    /// These characters of a face.
    #[track_caller]
    pub fn need_glyphs(&self, face: &str, text: &str) {
        if text.chars().any(|c| !c.is_control()) {
            self.need_font_range(face, &chars_spec(text));
        }
    }

    /// A coverage spec of a face: `all`, `latin`, `digits`,
    /// `U+0041-U+005A`, `chars:...`, comma-separated.
    #[track_caller]
    pub fn need_font_range(&self, face: &str, spec: &str) {
        let by = self.current_by();
        self.state.borrow_mut().coverage.insert((face.to_string(), spec.to_string(), by));
    }

    /// Glyphs a layout returned for a font file (the text stack's record),
    /// with the texts it laid out.
    pub fn add_glyphs(&self, face: &str, glyphs: impl IntoIterator<Item = u16>, texts: impl IntoIterator<Item = String>) {
        let mut s = self.state.borrow_mut();
        s.glyphs.entry(face.to_string()).or_default().extend(glyphs);
        let rows = s.texts.entry(face.to_string()).or_default();
        for t in texts {
            *rows.entry(t).or_default() += 1;
        }
    }

    /// A compute kernel the app runs, as an opaque named blob (the
    /// runtime's own form; a packer that knows the runtime links them).
    #[track_caller]
    pub fn need_kernel(&self, name: &str, blob: &[u8]) {
        let by = self.current_by();
        let mut s = self.state.borrow_mut();
        s.features.insert("kernels".into());
        s.kernels.entry(name.to_string()).or_insert_with(|| (blob.to_vec(), by));
    }

    /// A runtime that links its kernels into one wasm module hands it over
    /// with its key table (`kernels.wasm`, `kernels.keys`).
    pub fn set_kernel_module(&self, wasm: Vec<u8>, keys: Vec<u8>) {
        let mut s = self.state.borrow_mut();
        s.features.insert("kernels".into());
        s.kernel_module = Some((wasm, keys));
    }

    /// A widget type reached, by its Rust type.
    #[track_caller]
    pub fn need_widget<T: ?Sized + 'static>(&self) {
        let name = std::any::type_name::<T>();
        let name = name.rsplit("::").next().unwrap_or(name);
        self.need_widget_type(name);
    }

    /// A widget type reached, by name.
    #[track_caller]
    pub fn need_widget_type(&self, name: &str) {
        let by = self.current_by();
        let mut s = self.state.borrow_mut();
        let e = s.widgets.entry(name.to_string()).or_insert((0, by));
        e.0 += 1;
    }

    /// A file the app loads at run time (a resource path or URL).
    #[track_caller]
    pub fn need_asset(&self, path: &str) {
        let by = self.current_by();
        self.state.borrow_mut().assets.entry(path.to_string()).or_insert(by);
    }

    /// A section of the web runtime (`// @section <name>` in `web.js`:
    /// "text-input", "clipboard", "storage", ...). An unknown name is a
    /// diagnostic.
    #[track_caller]
    pub fn need_js_feature(&self, section: &str) {
        let by = self.current_by();
        if !crate::collect::JS_SECTION_NAMES.contains(&section) {
            self.note("warning", &format!("{by}: `{section}` is not a section of the web runtime"));
            return;
        }
        self.state.borrow_mut().js.entry(section.to_string()).or_default().insert(by);
    }

    /// A subsystem used (`features.txt`).
    pub fn need_feature(&self, name: &str) {
        self.state.borrow_mut().features.insert(name.to_string());
    }

    /// A `name=value` line of the summary.
    pub fn summary(&self, name: &str, value: impl fmt::Display) {
        self.state.borrow_mut().summary.insert(name.to_string(), value.to_string());
    }

    /// A problem without a place (`severity`: note, warning, error).
    pub fn note(&self, severity: &str, message: &str) {
        self.diagnostic(severity, "", 0, 0, message);
    }

    /// A problem at a place in a file.
    pub fn diagnostic(&self, severity: &str, file: &str, line: u32, col: u32, message: &str) {
        self.state.borrow_mut().diagnostics.push((severity.to_string(), file.to_string(), line, col, message.to_string()));
    }
}

/// The web runtime's section names (the manifest crate's list, kept here
/// because the platform builds it on every target).
pub const JS_SECTION_NAMES: [&str; 14] = [
    "text-input",
    "clipboard",
    "file-dialog",
    "midi",
    "xr",
    "websocket",
    "live-reload",
    "storage",
    "crash-upload",
    "history",
    "geolocation",
    "legacy-http",
    "permissions",
    "video-playback",
];

/// A plug-in to a collect run: a runtime's or a library's own collection
/// (a film's frames, the text stack's glyph record). Registered with
/// [`Cx::add_scanner`].
pub trait Scanner {
    fn name(&self) -> &str;
    /// After every drawn frame of the run, from the first. Return true
    /// when it has nothing more to visit; the run ends when every scanner
    /// is done after the app's `Event::Scan`. A scanner that needs frames
    /// (drawing over time) returns false until it has them.
    fn step(&mut self, _cx: &mut Cx, _scan: &ScanEvent) -> bool {
        true
    }
    /// Before the manifest is written.
    fn finish(&mut self, _cx: &mut Cx, _scan: &ScanEvent) {}
}

/// Frames drawn before the scan (the first layout and draw settle).
#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
const WARMUP_FRAMES: usize = 2;
/// Frames drawn after it (spawned content draws, scanners finish).
#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
const SETTLE_FRAMES: usize = 2;

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Warmup,
    Scanned,
    Settle(usize),
    Written,
}

/// The collect run's driver, owned by `Cx` in collect mode.
pub struct Collector {
    scan: ScanEvent,
    scanners: Vec<Box<dyn Scanner>>,
    #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
    frames: usize,
    #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
    phase: Phase,
}

impl Collector {
    fn new() -> Self {
        Collector {
            scan: ScanEvent::default(),
            scanners: Vec::new(),
            #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
            frames: 0,
            #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
            phase: Phase::Warmup,
        }
    }
}

impl Cx {
    /// Whether this is a collect run: recorders a library keeps for the
    /// manifest (glyphs, kernels) turn on when this is true.
    pub fn is_collecting(&self) -> bool {
        self.collect.is_some()
    }

    /// Adds a plug-in to the collect run; dropped when not collecting.
    pub fn add_scanner(&mut self, scanner: Box<dyn Scanner>) {
        if let Some(c) = &mut self.collect {
            c.scanners.push(scanner);
        }
    }

    /// The run's scan handle (for a library that emits outside an event,
    /// e.g. while loading); `None` when not collecting.
    pub fn collect_scan(&self) -> Option<ScanEvent> {
        self.collect.as_ref().map(|c| c.scan.clone())
    }

    /// Records which script modules register and which the app uses (the
    /// script's `census`), from now on: before the first script module.
    /// A collect run does this itself; an app's own analysis calls it.
    pub fn record_module_census(&mut self) {
        self.with_vm(|vm| vm.census_record());
    }

    /// The script modules registered and used since
    /// [`Self::record_module_census`]; ends the recording. `app_crates`
    /// (crate names as in module paths) are the app's own: all their
    /// modules are used, and what they name.
    pub fn module_census(&mut self, app_crates: &[&str]) -> Option<makepad_script::census::ModuleUse> {
        self.with_vm(|vm| makepad_script::census::census_used_from(vm, app_crates))
    }

    /// Collect mode from the environment: called once as the Cx is made,
    /// before any script module runs, so the shader recording sees every
    /// shader.
    pub(crate) fn init_collect_from_env(&mut self) {
        if !collect_web_requested() {
            return;
        }
        self.collect = Some(Box::new(Collector::new()));
        self.record_module_census();
        #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
        self.record_shader_pack(true);
        crate::log!("collect-web: collecting into {}", collect_dir().display());
    }

    /// After every draw of a collect run: drive the scan, then write the
    /// manifest and quit.
    #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
    pub(crate) fn collect_after_draw(&mut self) {
        let Some(c) = self.collect.as_mut() else { return };
        c.frames += 1;
        let scan = c.scan.clone();
        scan.state.borrow_mut().step = c.frames;
        let phase = c.phase;
        // Plug-ins step after every frame.
        let mut scanners = std::mem::take(&mut c.scanners);
        let mut done = true;
        for s in &mut scanners {
            done &= s.step(self, &scan);
        }
        let Some(c) = self.collect.as_mut() else { return };
        scanners.append(&mut c.scanners);
        c.scanners = scanners;
        match phase {
            Phase::Warmup if c.frames >= WARMUP_FRAMES => {
                c.phase = Phase::Scanned;
                let t = crate::monotonic_seconds();
                self.call_event_handler(&crate::event::Event::Scan(scan));
                crate::log!("collect-web: scan at frame {} took {:.2} s", self.collect.as_ref().map_or(0, |c| c.frames), crate::monotonic_seconds() - t);
            }
            Phase::Scanned if done => c.phase = Phase::Settle(0),
            Phase::Settle(n) if n + 1 >= SETTLE_FRAMES => {
                c.phase = Phase::Written;
                self.collect_finish();
                return;
            }
            Phase::Settle(n) => c.phase = Phase::Settle(n + 1),
            _ => {}
        }
        if phase != Phase::Written {
            self.redraw_all();
        }
    }

    #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
    fn collect_finish(&mut self) {
        let Some(c) = self.collect.as_mut() else { return };
        let scan = c.scan.clone();
        let mut scanners = std::mem::take(&mut c.scanners);
        for s in &mut scanners {
            let _by = scan.by(s.name().to_string());
            s.finish(self, &scan);
        }
        // The app's crate: the binary's name as a crate name.
        let app = std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().replace('-', "_"))).unwrap_or_default();
        let modules = self.module_census(&[app.as_str()]).unwrap_or_default();
        let shaders = self.take_shader_pack();
        let shader_list: Vec<(u64, String)> = crate::shader_pack::read_shader_pack(&shaders).map(|e| e.into_iter().map(|e| (e.key, e.name)).collect()).unwrap_or_default();
        let entries = shader_list.len();
        let frames = self.collect.as_ref().map_or(0, |c| c.frames);
        let dir = collect_dir();
        let mut manifest = manifest_of(&scan.state.borrow(), shaders, entries, frames);
        manifest.shader_list = shader_list;
        manifest.summary.insert("modules_used".into(), format!("{} of {}", modules.used.len(), modules.registered.len()));
        manifest.modules = modules.used;
        manifest.modules_registered = modules.registered;
        match manifest.write(&dir) {
            Ok(()) => crate::log!(
                "collect-web: wrote {} ({} shaders, {} fonts, {} glyphs, {} widget types)",
                dir.display(),
                entries,
                manifest.glyphs.len(),
                manifest.glyphs.values().map(|g| g.len()).sum::<usize>(),
                manifest.widgets.len()
            ),
            Err(e) => crate::error!("collect-web: writing {}: {e}", dir.display()),
        }
        self.quit();
    }
}

/// Where the manifest goes: `MAKEPAD_COLLECT`, else `./makepad-collect`.
#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
pub fn collect_dir() -> std::path::PathBuf {
    std::env::var_os("MAKEPAD_COLLECT").map(Into::into).unwrap_or_else(|| "makepad-collect".into())
}

#[cfg(any(target_arch = "wasm32", target_os = "android", target_env = "ohos"))]
pub fn collect_dir() -> std::path::PathBuf {
    "makepad-collect".into()
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
fn manifest_of(s: &ScanState, shaders: Vec<u8>, shader_count: usize, frames: usize) -> makepad_web_manifest::Manifest {
    use makepad_web_manifest::*;
    let mut m = Manifest::default();
    m.glyphs = s.glyphs.clone();
    m.texts = s.texts.clone();
    m.coverage = s.coverage.iter().map(|(face, spec, by)| Coverage { face: face.clone(), spec: spec.clone(), by: by.clone() }).collect();
    if let Some((wasm, keys)) = &s.kernel_module {
        m.kernels_wasm = wasm.clone();
        m.kernel_keys = keys.clone();
    }
    m.kernel_blobs = s.kernels.iter().map(|(n, (b, by))| (n.clone(), KernelBlob { bytes: b.clone(), by: by.clone() })).collect();
    m.shaders = shaders;
    m.features = s.features.clone();
    m.widgets = s.widgets.iter().map(|(t, (n, by))| (t.clone(), WidgetUse { instances: *n, by: by.clone() })).collect();
    m.js = s.js.clone();
    m.assets = s.assets.clone();
    m.diagnostics = s
        .diagnostics
        .iter()
        .map(|(sev, file, line, col, msg)| Diagnostic { severity: Severity::parse(sev), file: file.clone(), line: *line, col: *col, message: msg.clone() })
        .collect();
    m.summary = s.summary.clone();
    m.summary.insert("frames".into(), frames.to_string());
    m.summary.insert("shaders".into(), shader_count.to_string());
    m.summary.insert("fonts".into(), m.glyphs.len().to_string());
    m.summary.insert("glyphs".into(), m.glyphs.values().map(|g| g.len()).sum::<usize>().to_string());
    m.summary.insert("widget_types".into(), m.widgets.len().to_string());
    m.summary.insert("kernels".into(), (m.kernel_blobs.len() + usize::from(!m.kernels_wasm.is_empty())).to_string());
    m
}
