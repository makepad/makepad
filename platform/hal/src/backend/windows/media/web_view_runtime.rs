//! The system web view: installed runtime discovery, asynchronous environment creation and
//! composition hosting. All COM objects stay on the shell's STA. There is no SDK loader
//! dependency and no work per frame: discovery/loading happens once, each session retains one
//! environment, and each web layer one composition controller whose root visual target is the
//! layer's DirectComposition visual (the window tree places that visual every frame, `dcomp`).
//! Placement changes (window, size, visibility) arrive as shell ops; pointer input inside a
//! layer's input region is routed here by the WndProc (`route_mouse`), keyboard input reaches
//! the web view through its own focus.

use super::super::win32::{Key, LayerMap, LayerRegion, Shared, WebVisual};
use crate::native_layer::{mode, op, state};
use crate::Id;
use makepad_windows_sys::core::{HRESULT, PCSTR, PCWSTR, PWSTR};
use makepad_windows_sys::system_web_view::TypeLibrary::*;
use makepad_windows_sys::system_web_view::Win32::System::{Com::CoTaskMemAlloc, LibraryLoader::*, Registry::*};
use makepad_windows_sys::Win32::Foundation::{FreeLibrary, HWND, POINT, RECT};
use makepad_windows_sys::Win32::System::Com::{CoTaskMemFree, IUnknown};
use makepad_windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, SetCursor, HCURSOR, HICON};
use makepad_windows_sys::system_web_view::Win32::UI::WindowsAndMessaging::CopyIcon;
use makepad_windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use makepad_windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW};
use makepad_windows_sys::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Instant;

const CLIENT_KEY: &str = "Software\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
const RUNTIME_FOLDER: &str = "MAKEPAD_SYSTEM_WEB_VIEW_RUNTIME";
// The target version specified by the pinned SDK's environment-options header.
const TARGET_VERSION: &str = "154.0.4258.31";

thread_local! {
    static RUNTIME: Cell<Option<(CreateEnvironment, u32)>> = const { Cell::new(None) };
    static HOSTS: RefCell<HashMap<u32, Rc<Host>>> = RefCell::new(HashMap::new());
}

fn wide(value: &std::ffi::OsStr) -> Result<Vec<u16>, String> {
    let mut result: Vec<u16> = value.encode_wide().collect();
    if result.contains(&0) { return Err("embedded NUL in system web view path".into()); }
    result.push(0);
    Ok(result)
}

fn registry_string(hive: HKEY, name: &str) -> Result<Option<OsString>, String> {
    let key: Vec<u16> = CLIENT_KEY.encode_utf16().chain(Some(0)).collect();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let flags = RRF_RT_REG_SZ | RRF_SUBKEY_WOW6432KEY;
    let mut bytes = 0;
    let mut value = Vec::<u16>::new();
    loop {
        let status = unsafe {
            RegGetValueW(hive, Some(PCWSTR(key.as_ptr())), Some(PCWSTR(name.as_ptr())), flags,
                None, if value.is_empty() {None} else {Some(value.as_mut_ptr().cast())}, Some(&mut bytes))
        };
        match status.0 {
            2 | 3 => return Ok(None), // absent registration/value
            0 if !value.is_empty() => {
                if bytes % 2 != 0 { return Err("invalid runtime registry string length".into()); }
                value.truncate(bytes as usize / 2);
                while value.last() == Some(&0) { value.pop(); }
                if value.is_empty() { return Ok(None); }
                if value.contains(&0) { return Err("invalid runtime registry string".into()); }
                return Ok(Some(OsString::from_wide(&value)));
            }
            0 | 234 => { // size query or a value that grew: resize and reread, never truncate
                if bytes == 0 { return Ok(None); }
                value.resize((bytes as usize).div_ceil(2), 0);
            }
            code => return Err(format!("runtime registration read failed ({code})")),
        }
    }
}

fn embedded_client(folder: &Path) -> Result<PathBuf, String> {
    let arch = if cfg!(target_arch = "x86_64") { "x64" }
        else if cfg!(target_arch = "aarch64") { "arm64" }
        else if cfg!(target_arch = "x86") { "x86" }
        else { return Err("unsupported system web view architecture".into()); };
    if !folder.is_absolute() { return Err("runtime folder must be absolute".into()); }
    let dll = folder.join("EBWebView").join(arch).join("EmbeddedBrowserWebView.dll");
    if !dll.is_file() { return Err(format!("runtime client not found: {}", dll.display())); }
    Ok(dll)
}

fn discover() -> Result<(PathBuf, u32), String> {
    // An explicit folder is authoritative and names the version directory of a fixed or
    // installed runtime. Do not silently use another installation when it is invalid.
    if let Some(folder) = std::env::var_os(RUNTIME_FOLDER) {
        return embedded_client(Path::new(&folder)).map(|dll| (dll, 1));
    }
    // The installed runtime registers in the updater's 32-bit client view, machine-wide
    // first and per-user second. `location` is the application directory; `pv` names the
    // version subdirectory. Verify both values and the architecture-specific client.
    let mut errors: Vec<String> = Vec::new();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let candidate = (|| {
            let version = registry_string(hive, "pv")?.ok_or("runtime version is not registered")?;
            let text = version.to_str().ok_or("invalid runtime version")?;
            let parts: Vec<u32> = text.split('.').map(str::parse).collect::<Result<_, _>>().map_err(|_| "invalid runtime version")?;
            if parts.len() != 4 || parts.iter().all(|v| *v == 0) { return Err("runtime version is not installed".into()); }
            let location = registry_string(hive, "location")?.ok_or("runtime location is not registered")?;
            let dll = embedded_client(&PathBuf::from(location).join(version))?;
            Ok(dll)
        })();
        match candidate {
            Ok(dll) => return Ok((dll, 0)),
            Err(error) => errors.push(error),
        }
    }
    Err(format!("installed system web view unavailable: {}", errors.join("; ")))
}

fn runtime() -> Result<(CreateEnvironment, u32), String> {
    if let Some(runtime) = RUNTIME.get() { return Ok(runtime); }
    let (dll, kind) = discover()?;
    let path = wide(dll.as_os_str())?;
    let module = unsafe { LoadLibraryExW(PCWSTR(path.as_ptr()), None,
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) }
        .map_err(|e| format!("load runtime client: {e}"))?;
    let entry = unsafe { GetProcAddress(module, PCSTR(b"CreateWebViewEnvironmentWithOptionsInternal\0".as_ptr())) };
    let Some(entry) = entry else {
        unsafe { let _ = FreeLibrary(module); }
        return Err("runtime environment entry point is unavailable".into());
    };
    // The generated function type records the pinned export ABI. The selected runtime's
    // module reference intentionally lasts until process exit: callbacks and COM releases
    // may still return into its code after the last environment owner has gone away.
    let entry: CreateEnvironment = unsafe { std::mem::transmute(entry) };
    RUNTIME.set(Some((entry, kind)));
    eprintln!("[hal-system-web-view] runtime found: {}", dll.display());
    Ok((entry, kind))
}

fn user_data() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA").filter(|p| !p.is_empty()).ok_or("app data directory is unavailable")?;
    let app = crate::app_id().map(OsString::from).or_else(|| std::env::current_exe().ok()
        .and_then(|path| path.file_stem().map(OsString::from))).ok_or("app identity is unavailable")?;
    let path = PathBuf::from(base).join("Makepad").join(app).join("system-web-view");
    if !path.is_absolute() { return Err("app data directory must be absolute".into()); }
    std::fs::create_dir_all(&path).map_err(|e| format!("create system web view data directory: {e}"))?;
    Ok(path)
}

struct Options {
    arguments: RefCell<Vec<u16>>,
    language: RefCell<Vec<u16>>,
    version: RefCell<Vec<u16>>,
    single_sign_on: Cell<i32>,
}

impl Options {
    fn new() -> Self {
        Self { arguments: RefCell::new(vec![0]), language: RefCell::new(vec![0]),
            version: RefCell::new(TARGET_VERSION.encode_utf16().chain(Some(0)).collect()), single_sign_on: Cell::new(0) }
    }
    fn get(value: &RefCell<Vec<u16>>) -> Result<PWSTR, HRESULT> {
        let value = value.borrow();
        let pointer = unsafe { CoTaskMemAlloc(value.len() * 2) } as *mut u16;
        if pointer.is_null() { return Err(HRESULT(0x8007000eu32 as i32)); }
        unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), pointer, value.len()); }
        Ok(PWSTR(pointer))
    }
    fn put(value: &RefCell<Vec<u16>>, input: PCWSTR) -> Result<(), HRESULT> {
        // COM's [in, string] contract guarantees a readable NUL-terminated string (or NULL).
        let mut copy = Vec::new();
        if !input.0.is_null() {
            let mut pointer = input.0;
            unsafe { while *pointer != 0 { copy.push(*pointer); pointer = pointer.add(1); } }
        }
        copy.push(0);
        *value.borrow_mut() = copy;
        Ok(())
    }
}

impl ICoreWebView2EnvironmentOptionsImpl for Options {
    fn get_AdditionalBrowserArguments(&self) -> Result<PWSTR, HRESULT> { Self::get(&self.arguments) }
    fn put_AdditionalBrowserArguments(&self, value: PCWSTR) -> Result<(), HRESULT> { Self::put(&self.arguments, value) }
    fn get_Language(&self) -> Result<PWSTR, HRESULT> { Self::get(&self.language) }
    fn put_Language(&self, value: PCWSTR) -> Result<(), HRESULT> { Self::put(&self.language, value) }
    fn get_TargetCompatibleBrowserVersion(&self) -> Result<PWSTR, HRESULT> { Self::get(&self.version) }
    fn put_TargetCompatibleBrowserVersion(&self, value: PCWSTR) -> Result<(), HRESULT> { Self::put(&self.version, value) }
    fn get_AllowSingleSignOnUsingOSPrimaryAccount(&self) -> Result<i32, HRESULT> { Ok(self.single_sign_on.get()) }
    fn put_AllowSingleSignOnUsingOSPrimaryAccount(&self, value: i32) -> Result<(), HRESULT> { self.single_sign_on.set(value); Ok(()) }
}

struct Host {
    session: u32,
    views: RefCell<HashMap<Id, Rc<View>>>,
    complete: Cell<bool>,
    error: RefCell<Option<String>>,
    environment: RefCell<Option<ICoreWebView2Environment>>,
    started: Instant,
}

/// One web layer's content.
struct View {
    key: Key,
    url: RefCell<Vec<u8>>,
    visual: RefCell<WebVisual>,
    /// the last placement: HWND, client origin, content size in pixels, visible
    placed: Cell<Option<(usize, [i32; 2], [i32; 2], bool)>>,
    /// the page asked for (`APPEARANCE`): dark, applied again after each navigation
    dark: Cell<Option<bool>>,
    /// its window is minimized: hidden whatever the frames say
    minimized: Cell<bool>,
    /// the page held the keyboard when its window was deactivated
    focus_on_activate: Cell<bool>,
    /// its window was activated while minimized: the page gets the keyboard back once shown
    focus_on_show: Cell<bool>,
    creating: Cell<bool>,
    hosted: RefCell<Option<Hosted>>,
    /// content ops that arrived before the controller
    ops: RefCell<Vec<(u32, u64, Vec<u8>)>>,
    /// our own copy of the page's cursor
    cursor: RefCell<Option<OwnedCursor>>,
    focused: Cell<bool>,
}

struct Hosted {
    composition: ICoreWebView2CompositionController,
    controller: ICoreWebView2Controller,
    web: ICoreWebView2,
    parent: Cell<usize>,
}

/// A copy of a cursor the runtime handed out (it may free its own handle): destroyed with the
/// view or when the page changes its cursor again.
struct OwnedCursor(HCURSOR);
impl Drop for OwnedCursor {
    fn drop(&mut self) {
        unsafe { let _ = DestroyIcon(HICON(self.0 .0)); }
    }
}

/// `MAKEPAD_HAL_LOG=1`: focus hand-overs between the window and its pages are logged.
fn trace(what: std::fmt::Arguments) {
    if super::super::win32::log_enabled() { eprintln!("[hal-system-web-view] {what}"); }
}

fn emit(key: Key, what: u32, a: u64, b: u64, data: &[u8]) {
    Shared::global().push(key, |q, id| q.native_layer_state(id, what, a, b, data));
}

fn wide_str(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// A string the runtime allocated: copied, then freed.
fn take_string(value: Result<PWSTR, HRESULT>) -> String {
    let Ok(value) = value else { return String::new() };
    if value.0.is_null() { return String::new(); }
    let mut text = Vec::new();
    unsafe {
        let mut p = value.0;
        while *p != 0 { text.push(*p); p = p.add(1); }
        CoTaskMemFree(Some(value.0 as *const _));
    }
    String::from_utf16_lossy(&text)
}

/// The session's main window: the parent of a web view before any frame placed it.
fn parent_of(view: &View) -> Option<usize> {
    view.placed.get().map(|p| p.0).or_else(|| super::super::win32::session_hwnd(view.key.0))
}

impl Host {
    fn report_error(&self, id: Id) {
        if let Some(view) = self.views.borrow().get(&id) { view.ops.borrow_mut().clear(); }
        let error = self.error.borrow();
        let reason = error.as_deref().unwrap_or("system web view environment unavailable");
        eprintln!("[hal-system-web-view] REALIZED UNSUPPORTED session={} layer={id:?}: {reason}", self.session);
        emit((self.session, id), state::REALIZED, mode::UNSUPPORTED, 0, reason.as_bytes());
    }
    fn finish(self: &Rc<Self>, result: Result<ICoreWebView2Environment, String>) {
        if self.complete.replace(true) { return; }
        match result {
            Ok(environment) => {
                *self.environment.borrow_mut() = Some(environment);
                eprintln!("[hal-system-web-view] environment completed session={} elapsed_ms={:.3}", self.session, self.started.elapsed().as_secs_f64() * 1000.0);
            }
            Err(error) => *self.error.borrow_mut() = Some(error),
        }
        let views: Vec<Rc<View>> = self.views.borrow().values().cloned().collect();
        for view in views { self.start(&view); }
    }
    fn begin(self: &Rc<Self>) -> Result<(), String> {
        let started = Instant::now();
        let (entry, kind) = runtime()?;
        let data = wide(user_data()?.as_os_str())?;
        let options = ICoreWebView2EnvironmentOptions::implement(Box::new(Options::new()));
        let completed = ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler::implement(Box::new(Completed(Rc::downgrade(self))));
        let status = unsafe { entry(true, kind, PCWSTR(data.as_ptr()), options.as_raw(), completed.as_raw()) };
        eprintln!("[hal-system-web-view] environment request returned sta_elapsed_ms={:.3}", started.elapsed().as_secs_f64() * 1000.0);
        status.ok().map_err(|e| format!("create system web view environment: {e}"))
    }
    /// Asks for the view's composition controller once the environment and a parent exist.
    fn start(self: &Rc<Self>, view: &Rc<View>) {
        if !self.complete.get() || view.creating.get() || view.hosted.borrow().is_some() { return; }
        if self.error.borrow().is_some() { self.report_error(view.key.1); return; }
        let Some(environment) = self.environment.borrow().clone() else { return };
        let Some(parent) = parent_of(view) else { return };
        // the runtime may complete before the call returns
        view.creating.set(true);
        let result = (|| unsafe {
            let environment = ICoreWebView2Environment3::query(environment.as_raw()).map_err(|e| format!("composition hosting needs a newer runtime: {e}"))?;
            let handler = ICoreWebView2CreateCoreWebView2CompositionControllerCompletedHandler::implement(Box::new(ControllerCompleted(Rc::downgrade(view))));
            environment.CreateCoreWebView2CompositionController(HWND(parent as _), &handler).map_err(|e| format!("create composition controller: {e}"))
        })();
        match result {
            Ok(()) => {}
            Err(error) => {
                view.creating.set(false);
                eprintln!("[hal-system-web-view] REALIZED UNSUPPORTED layer={:?}: {error}", view.key.1);
                emit(view.key, state::REALIZED, mode::UNSUPPORTED, 0, error.as_bytes());
            }
        }
    }
}

struct Completed(Weak<Host>);
impl ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandlerImpl for Completed {
    fn Invoke(&self, status: HRESULT, environment: Option<&ICoreWebView2Environment>) -> Result<(), HRESULT> {
        if let Some(host) = self.0.upgrade() {
            host.finish(status.ok().map_err(|e| format!("system web view environment completion: {e}"))
                .and_then(|_| environment.cloned().ok_or_else(|| "system web view returned no environment".into())));
        }
        Ok(())
    }
}

struct ControllerCompleted(Weak<View>);
impl ICoreWebView2CreateCoreWebView2CompositionControllerCompletedHandlerImpl for ControllerCompleted {
    fn Invoke(&self, status: HRESULT, composition: Option<&ICoreWebView2CompositionController>) -> Result<(), HRESULT> {
        let Some(view) = self.0.upgrade() else { return Ok(()) };
        view.creating.set(false);
        let result = status.ok().map_err(|e| format!("composition controller: {e}"))
            .and_then(|_| composition.cloned().ok_or_else(|| "no composition controller".to_string()))
            .and_then(|composition| unsafe { host(&view, composition) });
        match result {
            Ok(()) => {
                eprintln!("[hal-system-web-view] REALIZED COMPOSITED layer={:?}", view.key.1);
                emit(view.key, state::REALIZED, mode::COMPOSITED, 0, &[]);
                let ops = std::mem::take(&mut *view.ops.borrow_mut());
                for (op, arg, data) in ops { apply(&view, op, arg, &data); }
            }
            Err(error) => {
                eprintln!("[hal-system-web-view] REALIZED UNSUPPORTED layer={:?}: {error}", view.key.1);
                view.ops.borrow_mut().clear();
                emit(view.key, state::REALIZED, mode::UNSUPPORTED, 0, error.as_bytes());
            }
        }
        Ok(())
    }
}

/// The controller renders into the layer's visual; its events become layer states.
/// The controller's bounds: where the layer's content is in the parent's client, in pixels.
/// Our visual places the content (frame-synced); the runtime uses the bounds for what it puts
/// on screen itself (dropdowns, menus, the IME candidate window) and for screen coordinates.
fn bounds(origin: [i32; 2], size: [i32; 2]) -> RECT {
    RECT { left: origin[0], top: origin[1], right: origin[0] + size[0].max(1), bottom: origin[1] + size[1].max(1) }
}

fn err(what: &'static str) -> impl Fn(HRESULT) -> String {
    move |e| format!("{what}: {e}")
}

unsafe fn host(view: &Rc<View>, composition: ICoreWebView2CompositionController) -> Result<(), String> {
    let e = err;
    let controller = ICoreWebView2Controller::query(composition.as_raw()).map_err(e("controller"))?;
    let web = controller.get_CoreWebView2().map_err(e("web view"))?;
    composition.put_RootVisualTarget(view.visual.borrow().0.as_IUnknown()).map_err(e("root visual target"))?;
    let (origin, size, visible) = view.placed.get().map_or(([0, 0], [1, 1], false), |p| (p.1, p.2, p.3));
    controller.put_Bounds(bounds(origin, size)).map_err(e("bounds"))?;
    controller.put_IsVisible((visible && !view.minimized.get()) as i32).map_err(e("visibility"))?;
    let key = view.key;
    let weak = Rc::downgrade(view);
    web.add_DocumentTitleChanged(&ICoreWebView2DocumentTitleChangedEventHandler::implement(Box::new(OnTitle(key)))).map_err(e("title events"))?;
    web.add_SourceChanged(&ICoreWebView2SourceChangedEventHandler::implement(Box::new(OnSource(key)))).map_err(e("source events"))?;
    web.add_HistoryChanged(&ICoreWebView2HistoryChangedEventHandler::implement(Box::new(OnHistory(key)))).map_err(e("history events"))?;
    web.add_NavigationStarting(&ICoreWebView2NavigationStartingEventHandler::implement(Box::new(OnStarting(key)))).map_err(e("navigation events"))?;
    web.add_NavigationCompleted(&ICoreWebView2NavigationCompletedEventHandler::implement(Box::new(OnCompleted(key)))).map_err(e("navigation events"))?;
    web.add_ProcessFailed(&ICoreWebView2ProcessFailedEventHandler::implement(Box::new(OnFailed(key)))).map_err(e("process events"))?;
    controller.add_GotFocus(&ICoreWebView2FocusChangedEventHandler::implement(Box::new(OnFocus(weak.clone(), true)))).map_err(e("focus events"))?;
    controller.add_LostFocus(&ICoreWebView2FocusChangedEventHandler::implement(Box::new(OnFocus(weak.clone(), false)))).map_err(e("focus events"))?;
    controller.add_MoveFocusRequested(&ICoreWebView2MoveFocusRequestedEventHandler::implement(Box::new(OnMoveFocus(weak.clone())))).map_err(e("focus events"))?;
    composition.add_CursorChanged(&ICoreWebView2CursorChangedEventHandler::implement(Box::new(OnCursor(weak)))).map_err(e("cursor events"))?;
    let parent = parent_of(view).unwrap_or(0);
    *view.hosted.borrow_mut() = Some(Hosted { composition, controller, web: web.clone(), parent: Cell::new(parent) });
    let url = String::from_utf8_lossy(&view.url.borrow()).into_owned();
    if !url.is_empty() { web.Navigate(PCWSTR(wide_str(&url).as_ptr())).map_err(e("navigate"))?; }
    Ok(())
}

fn nav_state(key: Key, web: &ICoreWebView2) {
    let back = unsafe { web.get_CanGoBack() }.unwrap_or(0) != 0;
    let forward = unsafe { web.get_CanGoForward() }.unwrap_or(0) != 0;
    emit(key, state::NAV, back as u64, forward as u64, &[]);
}

/// A web view event handler of layer `key`: `$body` runs with the sending web view.
macro_rules! web_event {
    ($name:ident, $handler:ident, $args:ty, |$key:ident, $web:ident, $a:ident| $body:block) => {
        struct $name(Key);
        impl $handler for $name {
            fn Invoke(&self, sender: Option<&ICoreWebView2>, $a: Option<&$args>) -> Result<(), HRESULT> {
                let $key = self.0;
                if let Some($web) = sender { $body }
                Ok(())
            }
        }
    };
}
web_event!(OnTitle, ICoreWebView2DocumentTitleChangedEventHandlerImpl, IUnknown, |key, web, _args| {
    emit(key, state::TITLE, 0, 0, take_string(unsafe { web.get_DocumentTitle() }).as_bytes());
});
web_event!(OnSource, ICoreWebView2SourceChangedEventHandlerImpl, ICoreWebView2SourceChangedEventArgs, |key, web, _args| {
    emit(key, state::URL, 0, 0, take_string(unsafe { web.get_Source() }).as_bytes());
});
web_event!(OnHistory, ICoreWebView2HistoryChangedEventHandlerImpl, IUnknown, |key, web, _args| {
    nav_state(key, web);
});
web_event!(OnStarting, ICoreWebView2NavigationStartingEventHandlerImpl, ICoreWebView2NavigationStartingEventArgs, |key, _web, _args| {
    emit(key, state::LOADING, 1, 0, &[]);
});
web_event!(OnCompleted, ICoreWebView2NavigationCompletedEventHandlerImpl, ICoreWebView2NavigationCompletedEventArgs, |key, web, _args| {
    emit(key, state::LOADING, 0, 0, &[]);
    nav_state(key, web);
    if let Some(dark) = view_of(key).and_then(|(_, v)| v.dark.get()) { color_scheme(web, dark); }
});
web_event!(OnFailed, ICoreWebView2ProcessFailedEventHandlerImpl, ICoreWebView2ProcessFailedEventArgs, |key, _web, args| {
    let kind = args.and_then(|a| unsafe { a.get_ProcessFailedKind() }.ok()).map_or(-1, |k| k.0);
    emit(key, state::CRASHED, 0, 0, format!("web view process failed (kind {kind})").as_bytes());
});

struct OnFocus(Weak<View>, bool);
impl ICoreWebView2FocusChangedEventHandlerImpl for OnFocus {
    fn Invoke(&self, _: Option<&ICoreWebView2Controller>, _: Option<&IUnknown>) -> Result<(), HRESULT> {
        if let Some(view) = self.0.upgrade() {
            trace(format_args!("layer {:?} {}", view.key.1, if self.1 { "got the keyboard" } else { "lost the keyboard" }));
            if view.focused.replace(self.1) != self.1 {
                emit(view.key, if self.1 { state::FOCUS_IN } else { state::FOCUS_OUT }, 0, 0, &[]);
            }
        }
        Ok(())
    }
}

/// Tab out of the page: the window takes the focus back, and Makepad's focus moves on.
struct OnMoveFocus(Weak<View>);
impl ICoreWebView2MoveFocusRequestedEventHandlerImpl for OnMoveFocus {
    fn Invoke(&self, _: Option<&ICoreWebView2Controller>, args: Option<&ICoreWebView2MoveFocusRequestedEventArgs>) -> Result<(), HRESULT> {
        let Some(view) = self.0.upgrade() else { return Ok(()) };
        let Some(args) = args else { return Ok(()) };
        let reason = unsafe { args.get_reason() }.map_or(0, |r| r.0);
        unsafe { let _ = args.put_Handled(1); }
        trace(format_args!("layer {:?} moves the keyboard out ({})", view.key.1, if reason == COREWEBVIEW2_MOVE_FOCUS_REASON_PREVIOUS.0 { "backwards" } else { "forwards" }));
        let parent = view.hosted.borrow().as_ref().map(|h| h.parent.get());
        if let Some(hwnd) = parent {
            super::super::win32::focus_window(hwnd, Some(reason == COREWEBVIEW2_MOVE_FOCUS_REASON_PREVIOUS.0));
        }
        Ok(())
    }
}

struct OnCursor(Weak<View>);
impl ICoreWebView2CursorChangedEventHandlerImpl for OnCursor {
    fn Invoke(&self, sender: Option<&ICoreWebView2CompositionController>, _: Option<&IUnknown>) -> Result<(), HRESULT> {
        let (Some(view), Some(sender)) = (self.0.upgrade(), sender) else { return Ok(()) };
        // the runtime's cursor handle is an icon handle type in its type library; it may free it,
        // so the view keeps its own copy
        let copy = unsafe { sender.get_Cursor() }.ok().and_then(|icon| unsafe { CopyIcon(icon) }.ok()).map(|icon| OwnedCursor(HCURSOR(icon.0)));
        if HOVER.with_borrow(|h| h.values().any(|k| *k == view.key)) {
            unsafe { SetCursor(copy.as_ref().map(|c| c.0)); }
        }
        // the previous copy goes once the new one is set
        *view.cursor.borrow_mut() = copy;
        let id = unsafe { sender.get_SystemCursorId() }.unwrap_or(0);
        // IDC_* resource ids
        let makepad = match id {
            32513 => crate::types::cursor::TEXT,
            32649 => crate::types::cursor::HAND,
            32514 | 32650 => crate::types::cursor::WAIT,
            32515 => crate::types::cursor::CROSSHAIR,
            32648 => crate::types::cursor::NOT_ALLOWED,
            32644 => crate::types::cursor::EW_RESIZE,
            32645 => crate::types::cursor::NS_RESIZE,
            32646 => crate::types::cursor::MOVE,
            _ => crate::types::cursor::DEFAULT,
        };
        emit(view.key, state::CURSOR, makepad as u64, 0, &[]);
        Ok(())
    }
}

/// A content op on a hosted view.
fn apply(view: &View, op: u32, arg: u64, data: &[u8]) {
    let hosted = view.hosted.borrow();
    let Some(h) = hosted.as_ref() else {
        view.ops.borrow_mut().push((op, arg, data.to_vec()));
        return;
    };
    let result = unsafe {
        match op {
            op::NAVIGATE => {
                let url = String::from_utf8_lossy(data).into_owned();
                *view.url.borrow_mut() = data.to_vec();
                if arg != 0 {
                    // replace the current history entry: the page's own navigation does that;
                    // where the page cannot run it (an error page, a document still loading)
                    // the navigation still happens, as a new entry
                    let web = h.web.clone();
                    let wide = wide_str(&url);
                    script(&h.web, &format!("(function(){{location.replace({});return 1}})()", json_string(&url)), "replace navigation",
                        Box::new(move || { let _ = web.Navigate(PCWSTR(wide.as_ptr())); }))
                } else {
                    h.web.Navigate(PCWSTR(wide_str(&url).as_ptr()))
                }
            }
            op::HISTORY => match arg as u32 as i32 {
                -1 => h.web.GoBack(),
                1 => h.web.GoForward(),
                0 => Ok(()),
                // one traversal of `delta` entries, as the page's history does it; step by step
                // where the page cannot run it
                delta => {
                    let web = h.web.clone();
                    script(&h.web, &format!("(function(){{history.go({delta});return 1}})()"), "history traversal", Box::new(move || {
                        for _ in 0..delta.unsigned_abs() {
                            let r = if delta < 0 { web.GoBack() } else { web.GoForward() };
                            if r.is_err() { break; }
                        }
                    }))
                }
            },
            op::RELOAD => h.web.Reload(),
            op::STOP => h.web.Stop(),
            op::APPEARANCE => {
                let dark = arg & 1 != 0;
                view.dark.set(Some(dark));
                color_scheme(&h.web, dark);
                if data.len() == 16 {
                    let f = |i: usize| f32::from_le_bytes([data[i * 4], data[i * 4 + 1], data[i * 4 + 2], data[i * 4 + 3]]);
                    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    match ICoreWebView2Controller2::query(h.controller.as_raw()) {
                        // the runtime takes an opaque or a fully transparent background only
                        Ok(c2) => c2.put_DefaultBackgroundColor(COREWEBVIEW2_COLOR { A: if f(3) < 0.5 { 0 } else { 255 }, R: byte(f(0)), G: byte(f(1)), B: byte(f(2)) }),
                        Err(e) => Err(e),
                    }
                } else {
                    Ok(())
                }
            }
            op::FOCUS => {
                trace(format_args!("layer {:?} FOCUS", view.key.1));
                h.controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
            }
            op::BLUR => {
                trace(format_args!("layer {:?} BLUR (page has the keyboard: {})", view.key.1, view.focused.get()));
                if view.focused.get() { super::super::win32::focus_window(h.parent.get(), None); }
                Ok(())
            }
            // pointer input is routed by the window itself (host-routed in the HAL)
            _ => Ok(()),
        }
    };
    if let Err(e) = result {
        eprintln!("[hal-system-web-view] layer {:?} op {op}: {e}", view.key.1);
    }
}

/// A JSON (and JavaScript) string literal.
fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Ignore;
impl ICoreWebView2CallDevToolsProtocolMethodCompletedHandlerImpl for Ignore {
    fn Invoke(&self, _: HRESULT, _: PCWSTR) -> Result<(), HRESULT> { Ok(()) }
}

/// A string the runtime lends for the duration of a call.
fn lent_string(value: PCWSTR) -> String {
    let mut text = Vec::new();
    if !value.0.is_null() {
        unsafe {
            let mut p = value.0;
            while *p != 0 { text.push(*p); p = p.add(1); }
        }
    }
    String::from_utf16_lossy(&text)
}

/// The completion of a script that answers 1 once it did its work; anything else (it did not
/// run, or the page refused it) runs `otherwise`.
struct Otherwise(&'static str, RefCell<Option<Box<dyn FnOnce()>>>);
impl ICoreWebView2ExecuteScriptCompletedHandlerImpl for Otherwise {
    fn Invoke(&self, status: HRESULT, result: PCWSTR) -> Result<(), HRESULT> {
        let answer = lent_string(result);
        if status.ok().is_err() || answer != "1" {
            eprintln!("[hal-system-web-view] {} by the page failed ({status:?}, {answer}); done by the controller instead", self.0);
            if let Some(f) = self.1.borrow_mut().take() { f(); }
        }
        Ok(())
    }
}

/// Runs `code` in the page; when it cannot, `otherwise` does the work.
unsafe fn script(web: &ICoreWebView2, code: &str, what: &'static str, otherwise: Box<dyn FnOnce()>) -> Result<(), HRESULT> {
    let completion = ICoreWebView2ExecuteScriptCompletedHandler::implement(Box::new(Otherwise(what, RefCell::new(Some(otherwise)))));
    web.ExecuteScript(PCWSTR(wide_str(code).as_ptr()), &completion)
}

/// The page's `prefers-color-scheme`, for this web view only (the DevTools protocol's media
/// emulation; the runtime keeps it across the view's navigations, and it is applied again after
/// each one).
fn color_scheme(web: &ICoreWebView2, dark: bool) {
    let params = format!("{{\"features\":[{{\"name\":\"prefers-color-scheme\",\"value\":\"{}\"}}]}}", if dark { "dark" } else { "light" });
    let result = unsafe {
        web.CallDevToolsProtocolMethod(PCWSTR(wide_str("Emulation.setEmulatedMedia").as_ptr()), PCWSTR(wide_str(&params).as_ptr()),
            &ICoreWebView2CallDevToolsProtocolMethodCompletedHandler::implement(Box::new(Ignore)))
    };
    if let Err(e) = result { eprintln!("[hal-system-web-view] color scheme: {e}"); }
}

fn view_of(key: Key) -> Option<(Rc<Host>, Rc<View>)> {
    HOSTS.with_borrow(|hosts| hosts.get(&key.0).and_then(|h| h.views.borrow().get(&key.1).cloned().map(|v| (h.clone(), v))))
}

/// Shell STA only, outside the HAL lock. The message loop delivers completion asynchronously.
pub(crate) fn create(key: Key, url: Vec<u8>, visual: WebVisual) {
    let (host, start) = HOSTS.with_borrow_mut(|hosts| {
        let start = !hosts.contains_key(&key.0);
        let host = hosts.entry(key.0).or_insert_with(|| Rc::new(Host {
            session: key.0, views: RefCell::new(HashMap::new()), complete: Cell::new(false),
            error: RefCell::new(None), environment: RefCell::new(None), started: Instant::now(),
        })).clone();
        (host, start)
    });
    let view = Rc::new(View {
        key, url: RefCell::new(url), visual: RefCell::new(visual), placed: Cell::new(None), dark: Cell::new(None),
        minimized: Cell::new(false), focus_on_activate: Cell::new(false), focus_on_show: Cell::new(false), creating: Cell::new(false),
        hosted: RefCell::new(None), ops: RefCell::new(Vec::new()), cursor: RefCell::new(None), focused: Cell::new(false),
    });
    host.views.borrow_mut().insert(key.1, view.clone());
    if start {
        if let Err(error) = host.begin() { host.finish(Err(error)); }
    } else {
        host.start(&view);
    }
}

pub(crate) fn operation(key: Key, op: u32, arg: u64, data: &[u8]) {
    if let Some((_, view)) = view_of(key) { apply(&view, op, arg, data); }
}

/// A committed frame placed the layer: the controller follows its window, bounds and visibility.
pub(crate) fn place(key: Key, hwnd: usize, origin: [i32; 2], size: [i32; 2], visible: bool) {
    let Some((host, view)) = view_of(key) else { return };
    let previous = view.placed.replace(Some((hwnd, origin, size, visible)));
    let hosted = view.hosted.borrow();
    if let Some(h) = hosted.as_ref() {
        unsafe {
            if h.parent.get() != hwnd {
                h.parent.set(hwnd);
                let _ = h.controller.put_ParentWindow(HWND(hwnd as _));
            }
            if previous.map(|p| (p.1, p.2)) != Some((origin, size)) {
                let _ = h.controller.put_Bounds(bounds(origin, size));
            }
            if previous.map(|p| p.3) != Some(visible) {
                let _ = h.controller.put_IsVisible((visible && !view.minimized.get()) as i32);
            }
        }
        if !visible && view.focused.get() { super::super::win32::focus_window(hwnd, None); }
    } else {
        drop(hosted);
        host.start(&view);
    }
}

/// The composition device was made again: the content renders into the layer's new visual.
pub(crate) fn retarget(key: Key, visual: WebVisual) {
    let Some((_, view)) = view_of(key) else { return };
    if let Some(h) = view.hosted.borrow().as_ref() {
        if let Err(e) = unsafe { h.composition.put_RootVisualTarget(visual.0.as_IUnknown()) } {
            eprintln!("[hal-system-web-view] layer {:?} root visual target: {e}", key.1);
        }
    }
    *view.visual.borrow_mut() = visual;
}

/// The hosted views whose parent is `hwnd`.
fn views_in(hwnd: usize) -> Vec<Rc<View>> {
    HOSTS.with_borrow(|hosts| hosts.values().flat_map(|h| h.views.borrow().values().cloned().collect::<Vec<_>>())
        .filter(|v| v.hosted.borrow().as_ref().is_some_and(|h| h.parent.get() == hwnd)).collect())
}

/// The window moved on screen: the runtime places its own popups and dialogs from it.
pub(crate) fn parent_moved(hwnd: usize) {
    for view in views_in(hwnd) {
        if let Some(h) = view.hosted.borrow().as_ref() { unsafe { let _ = h.controller.NotifyParentWindowPositionChanged(); } }
    }
}

/// The window was minimized or shown again: its web views hide (and stop rendering) meanwhile.
pub(crate) fn minimized(hwnd: usize, on: bool) {
    for view in views_in(hwnd) {
        if view.minimized.replace(on) == on { continue; }
        trace(format_args!("window {}; layer {:?} {}", if on { "minimized" } else { "restored" }, view.key.1, if on { "hidden" } else { "shown as placed" }));
        let visible = !on && view.placed.get().is_some_and(|p| p.3);
        if let Some(h) = view.hosted.borrow().as_ref() {
            unsafe { let _ = h.controller.put_IsVisible(visible as i32); }
            if !on && view.focus_on_show.replace(false) {
                trace(format_args!("layer {:?} shown; gets the keyboard back", view.key.1));
                unsafe { let _ = h.controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC); }
            }
        }
    }
}

/// The window was deactivated while a page held the keyboard: Makepad's window loses its focus
/// (true), and the page gets the keyboard back when the window is activated again.
pub(crate) fn deactivated(hwnd: usize) -> bool {
    let mut any = false;
    for view in views_in(hwnd) {
        // a keyboard return still waiting for the window to be shown waits for the next activation
        let focused = view.focused.get() || view.focus_on_show.replace(false);
        view.focus_on_activate.set(focused);
        trace(format_args!("window deactivated; layer {:?} had the keyboard: {focused}", view.key.1));
        any |= focused;
    }
    any
}

pub(crate) fn activated(hwnd: usize) {
    for view in views_in(hwnd) {
        if view.focus_on_activate.replace(false) {
            if view.minimized.get() {
                // a hidden page does not take the keyboard (activation comes before the restore)
                trace(format_args!("window activated while minimized; layer {:?} gets the keyboard once shown", view.key.1));
                view.focus_on_show.set(true);
                continue;
            }
            trace(format_args!("window activated; layer {:?} gets the keyboard back", view.key.1));
            if let Some(h) = view.hosted.borrow().as_ref() { unsafe { let _ = h.controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC); } }
        }
    }
}

pub(crate) fn destroy(key: Key) {
    let removed = HOSTS.with_borrow_mut(|hosts| {
        let host = hosts.get(&key.0)?;
        let view = host.views.borrow_mut().remove(&key.1);
        if host.views.borrow().is_empty() && host.complete.get() { hosts.remove(&key.0); }
        view
    });
    HOVER.with_borrow_mut(|h| h.retain(|_, k| *k != key));
    CAPTURE.with_borrow_mut(|c| c.retain(|_, k| k.key != key));
    POINTERS.with_borrow_mut(|c| c.retain(|_, k| k.0 != key));
    if let Some(view) = removed {
        if let Some(h) = view.hosted.borrow_mut().take() {
            unsafe { let _ = h.controller.Close(); }
        }
    }
}
pub(crate) fn session_closed(session: u32) {
    let host = HOSTS.with_borrow_mut(|hosts| hosts.remove(&session));
    if let Some(host) = host {
        for view in host.views.borrow().values() {
            if let Some(h) = view.hosted.borrow_mut().take() { unsafe { let _ = h.controller.Close(); } }
        }
    }
    HOVER.with_borrow_mut(|h| h.retain(|_, k| k.0 != session));
    CAPTURE.with_borrow_mut(|c| c.retain(|_, k| k.key.0 != session));
    POINTERS.with_borrow_mut(|c| c.retain(|_, k| k.0 .0 != session));
}
pub(crate) fn shutdown() {
    let hosts = HOSTS.with_borrow_mut(std::mem::take);
    for host in hosts.values() {
        for view in host.views.borrow().values() {
            if let Some(h) = view.hosted.borrow_mut().take() { unsafe { let _ = h.controller.Close(); } }
        }
    }
}

thread_local! {
    /// the layer under the pointer, by HWND
    static HOVER: RefCell<HashMap<usize, Key>> = RefCell::new(HashMap::new());
    /// the layer a press went to, by HWND: it gets the pointer until every button is up
    static CAPTURE: RefCell<HashMap<usize, Capture>> = RefCell::new(HashMap::new());
    /// the layer each touch or pen contact went to, by (HWND, pointer id), until it lifts
    static POINTERS: RefCell<HashMap<(usize, u32), (Key, LayerMap)>> = RefCell::new(HashMap::new());
    /// the last press routed to a layer, by HWND: message, time, client pixel
    static LAST_DOWN: RefCell<HashMap<usize, (u32, Instant, (f32, f32))>> = RefCell::new(HashMap::new());
}

/// A press that went to a layer: the buttons still down and where the pointer last was there.
#[derive(Clone, Copy)]
struct Capture {
    key: Key,
    keys: u32,
    point: (f32, f32),
    /// the layer's mapping at the press (used while its region is gone)
    map: LayerMap,
}

/// The window lost the pointer capture (another window took it, a modal loop, a cancel): a
/// press that went to a layer ends there, and the page hears its buttons come up.
pub(crate) fn capture_lost(hwnd: usize) {
    let Some(c) = CAPTURE.with_borrow_mut(|c| c.remove(&hwnd)) else { return };
    release_buttons(c);
}

/// Button-ups for every button of a capture that is still down (MK_ flags to messages).
fn release_buttons(c: Capture) {
    for (mask, up, data) in [(0x1, 0x202, 0), (0x2, 0x205, 0), (0x10, 0x208, 0), (0x20, 0x20c, 1), (0x40, 0x20c, 2)] {
        if c.keys & mask != 0 { hosted_send(c.key, up, c.keys & !mask, data, c.point); }
    }
}

/// A press for Makepad in `hwnd` while a web view there holds the keyboard: the window takes it.
pub(crate) fn press_outside(hwnd: usize) {
    let focused = HOSTS.with_borrow(|hosts| hosts.values().any(|h| h.views.borrow().values().any(|v| {
        v.focused.get() && v.hosted.borrow().as_ref().is_some_and(|h| h.parent.get() == hwnd)
    })));
    if focused {
        trace(format_args!("press outside the page: the window takes the keyboard"));
        super::super::win32::focus_window(hwnd, None);
    }
}

/// What the WndProc does with a pointer message after `route_mouse`.
pub(crate) enum Routed {
    /// not a layer's: Makepad handles it (the pointer may just have left a layer)
    Makepad,
    /// a layer took it
    Layer,
    /// a layer took it and the pointer came from Makepad's area: Makepad hears a leave
    Entered,
}

fn hosted_send(key: Key, kind: i32, keys: u32, data: u32, point: (f32, f32)) -> bool {
    let Some((_, view)) = view_of(key) else { return false };
    let hosted = view.hosted.borrow();
    let Some(h) = hosted.as_ref() else { return false };
    let p = POINT { x: point.0.floor() as i32, y: point.1.floor() as i32 };
    if let Err(e) = unsafe { h.composition.SendMouseInput(COREWEBVIEW2_MOUSE_EVENT_KIND(kind), COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS(keys as i32), data, p) } {
        eprintln!("[hal-system-web-view] layer {:?} mouse input: {e}", key.1);
    }
    true
}

/// Routes a client-area pointer message of `hwnd` (client pixel `x`, `y`) to the native layer
/// whose input region holds it, or to the layer a press went to. Message ids are the web view's
/// mouse event kinds; the virtual keys are the message's MK_ flags.
pub(crate) fn route_mouse(hwnd: usize, msg: u32, keys: u32, data: u32, x: f32, y: f32) -> Routed {
    const WM_MOUSELEAVE: u32 = 0x2a3;
    let regions = Shared::global().layer_regions(hwnd);
    let captured = CAPTURE.with_borrow(|c| c.get(&hwnd).copied());
    let up = matches!(msg, 0x202 | 0x205 | 0x208 | 0x20c);
    // a layer without content yet (or without support) is Makepad's placeholder
    let hosted = |key: Key| view_of(key).is_some_and(|(_, v)| v.hosted.borrow().is_some());
    let hit = |regions: &[LayerRegion]| regions.iter().rev().find_map(|r| r.local(x, y).filter(|_| hosted(r.key)).map(|p| (r.key, p, r.map())));
    let target = match captured {
        Some(c) => {
            // the layer went away mid-press: the rest of the press maps as it did at the press
            let map = regions.as_ref().and_then(|r| r.iter().find(|r| r.key == c.key)).map_or(c.map, |r| r.map());
            Some((c.key, map.local(x, y), map))
        }
        None if msg == WM_MOUSELEAVE => None,
        None => regions.as_ref().and_then(|r| hit(r.as_slice())),
    };
    let hover = HOVER.with_borrow(|h| h.get(&hwnd).copied());
    if hover.is_some() && hover != target.map(|t| t.0) {
        // the pointer left that layer
        hosted_send(hover.unwrap(), WM_MOUSELEAVE as i32, 0, 0, (0.0, 0.0));
        HOVER.with_borrow_mut(|h| h.remove(&hwnd));
    }
    let Some((key, point, map)) = target else { return Routed::Makepad };
    if msg == WM_MOUSELEAVE { return Routed::Layer; }
    let entered = hover != Some(key);
    if entered {
        HOVER.with_borrow_mut(|h| h.insert(hwnd, key));
        if let Some((_, view)) = view_of(key) { if let Some(c) = view.cursor.borrow().as_ref() { unsafe { SetCursor(Some(c.0)); } } }
    }
    let down = matches!(msg, 0x201 | 0x204 | 0x207 | 0x20b);
    if down || captured.is_some() {
        CAPTURE.with_borrow_mut(|c| c.insert(hwnd, Capture { key, keys, point, map }));
    }
    if down {
        if let Some((_, view)) = view_of(key) {
            if !view.focused.get() {
                if let Some(h) = view.hosted.borrow().as_ref() { unsafe { let _ = h.controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC); } }
            }
        }
    }
    // the window class takes no double clicks: a second press soon and close by is one here
    let mut kind = msg;
    if down {
        let now = Instant::now();
        let double = LAST_DOWN.with_borrow(|l| l.get(&hwnd).is_some_and(|&(m, t, p)| m == msg && now.duration_since(t).as_millis() < 500
            && (p.0 - x).abs() <= 4.0 && (p.1 - y).abs() <= 4.0));
        if double {
            kind = msg + 2;
            LAST_DOWN.with_borrow_mut(|l| l.remove(&hwnd));
        } else {
            LAST_DOWN.with_borrow_mut(|l| l.insert(hwnd, (msg, now, (x, y))));
        }
    }
    let sent = hosted_send(key, kind as i32, keys, data, point);
    // MK_LBUTTON | MK_RBUTTON | MK_MBUTTON | MK_XBUTTON1 | MK_XBUTTON2
    if up && keys & 0x73 == 0 {
        CAPTURE.with_borrow_mut(|c| c.remove(&hwnd));
    }
    if !sent && captured.is_none() { return Routed::Makepad; }
    if entered && hover.is_none() { Routed::Entered } else { Routed::Layer }
}

/// Routes a touch or pen pointer message of `hwnd` (client pixel `x`, `y`) to the layer whose
/// input region holds its contact (or the layer its contact went down in) with the runtime's
/// pointer input. True when a layer took it: the window then does not turn it into mouse input.
pub(crate) fn route_pointer(hwnd: usize, msg: u32, id: u32, x: f32, y: f32) -> bool {
    use makepad_windows_sys::system_web_view::Win32::UI::Input::Pointer::*;
    const DOWN: u32 = 0x246;
    const UP: u32 = 0x247;
    const LEAVE: u32 = 0x24a;
    const CAPTURE_CHANGED: u32 = 0x24c;
    let regions = Shared::global().layer_regions(hwnd);
    let held = POINTERS.with_borrow(|p| p.get(&(hwnd, id)).copied());
    let hosted = |key: Key| view_of(key).is_some_and(|(_, v)| v.hosted.borrow().is_some());
    // a held contact maps as its layer does now, or as it did when its region went away
    let target = match held {
        Some((key, map)) => Some((key, regions.as_ref().and_then(|r| r.iter().find(|r| r.key == key)).map_or(map, |r| r.map()))),
        None if msg == DOWN => regions.as_ref().and_then(|r| r.iter().rev().find(|r| r.local(x, y).is_some() && hosted(r.key)).map(|r| (r.key, r.map()))),
        None => None,
    };
    let Some((key, map)) = target else { return false };
    if msg == DOWN || held.is_some() { POINTERS.with_borrow_mut(|p| p.insert((hwnd, id), (key, map))); }
    if matches!(msg, UP | LEAVE | CAPTURE_CHANGED) { POINTERS.with_borrow_mut(|p| p.remove(&(hwnd, id))); }
    let Some((host, view)) = view_of(key) else { return true };
    let hosted = view.hosted.borrow();
    let (Some(h), Some(environment)) = (hosted.as_ref(), host.environment.borrow().clone()) else { return true };
    let result = (|| unsafe {
        let mut info = POINTER_INFO::default();
        GetPointerInfo(id, &mut info)?;
        let environment = ICoreWebView2Environment3::query(environment.as_raw())?;
        let p = environment.CreateCoreWebView2PointerInfo()?;
        // pixel locations in the layer's content (as the mouse path maps them), the rest as the
        // system reports it
        let to_layer = |q: POINT| {
            let mut c = q;
            let _ = ScreenToClient(HWND(hwnd as _), &mut c);
            let (lx, ly) = map.local(c.x as f32, c.y as f32);
            POINT { x: lx.round() as i32, y: ly.round() as i32 }
        };
        p.put_PointerKind(info.pointerType.0 as u32)?;
        p.put_PointerId(info.pointerId)?;
        p.put_FrameId(info.frameId)?;
        p.put_PointerFlags(info.pointerFlags.0)?;
        let mut device = RECT::default();
        let mut display = RECT::default();
        if GetPointerDeviceRects(info.sourceDevice, &mut device, &mut display).is_ok() {
            p.put_PointerDeviceRect(device)?;
            p.put_DisplayRect(display)?;
        }
        p.put_PixelLocation(to_layer(info.ptPixelLocation))?;
        p.put_HimetricLocation(info.ptHimetricLocation)?;
        p.put_PixelLocationRaw(to_layer(info.ptPixelLocationRaw))?;
        p.put_HimetricLocationRaw(info.ptHimetricLocationRaw)?;
        p.put_Time(info.dwTime)?;
        p.put_HistoryCount(info.historyCount)?;
        p.put_InputData(info.InputData)?;
        p.put_KeyStates(info.dwKeyStates)?;
        p.put_PerformanceCount(info.PerformanceCount)?;
        p.put_ButtonChangeKind(info.ButtonChangeType.0)?;
        if info.pointerType == makepad_windows_sys::system_web_view::Win32::UI::WindowsAndMessaging::PT_PEN {
            let mut pen = POINTER_PEN_INFO::default();
            if GetPointerPenInfo(id, &mut pen).is_ok() {
                p.put_PenFlags(pen.penFlags)?;
                p.put_PenMask(pen.penMask)?;
                p.put_PenPressure(pen.pressure)?;
                p.put_PenRotation(pen.rotation)?;
                p.put_PenTiltX(pen.tiltX)?;
                p.put_PenTiltY(pen.tiltY)?;
            }
        } else {
            let mut touch = POINTER_TOUCH_INFO::default();
            if GetPointerTouchInfo(id, &mut touch).is_ok() {
                // the contact's corners mapped, as a box again (a rotated layer turns it)
                let rect = |r: RECT| {
                    let c = [(r.left, r.top), (r.right, r.top), (r.left, r.bottom), (r.right, r.bottom)].map(|(x, y)| to_layer(POINT { x, y }));
                    RECT { left: c.iter().map(|p| p.x).min().unwrap(), top: c.iter().map(|p| p.y).min().unwrap(),
                        right: c.iter().map(|p| p.x).max().unwrap(), bottom: c.iter().map(|p| p.y).max().unwrap() }
                };
                p.put_TouchFlags(touch.touchFlags)?;
                p.put_TouchMask(touch.touchMask)?;
                p.put_TouchContact(rect(touch.rcContact))?;
                p.put_TouchContactRaw(rect(touch.rcContactRaw))?;
                p.put_TouchOrientation(touch.orientation)?;
                p.put_TouchPressure(touch.pressure)?;
            }
        }
        h.composition.SendPointerInput(COREWEBVIEW2_POINTER_EVENT_KIND(msg as i32), &p)
    })();
    if let Err(e) = result {
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) { eprintln!("[hal-system-web-view] layer {:?} pointer input: {e}", key.1); }
    }
    if msg == DOWN && !view.focused.get() {
        unsafe { let _ = h.controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC); }
    }
    true
}

/// The cursor of the layer under the pointer in `hwnd`, if one is.
pub(crate) fn hover_cursor(hwnd: usize) -> Option<Option<HCURSOR>> {
    let key = HOVER.with_borrow(|h| h.get(&hwnd).copied())?;
    Some(view_of(key).and_then(|(_, v)| v.cursor.borrow().as_ref().map(|c| c.0)))
}

/// Whether a press in `hwnd` went to a layer and its buttons are still down.
pub(crate) fn captured(hwnd: usize) -> bool {
    CAPTURE.with_borrow(|c| c.contains_key(&hwnd))
}
