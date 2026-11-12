use {
    crate::makepad_draw::*,
    crate::makepad_draw::makepad_platform::script::std::ScriptStd,
    crate::makepad_draw::makepad_platform::script::timer::CxScriptTimer,
    crate::makepad_script::{script_err_not_found, ScriptFnRef, ScriptThreadId},
    crate::widget::{WidgetRef, WidgetUid},
    crate::widget_tree::CxWidgetExt,
    crate::{run_units, IsolateWidgets, UiUnit},
    std::cell::RefCell,
    std::collections::{HashMap, VecDeque},
    std::sync::atomic::{AtomicU64, Ordering},
};

static SCRIPT_ASYNC_COUNTER: AtomicU64 = AtomicU64::new(1);
pub(crate) const WIDGET_SCRIPT_INSTRUCTION_LIMIT: usize = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScriptAsyncId(u64);

impl ScriptAsyncId {
    pub(crate) fn new() -> Self {
        Self(SCRIPT_ASYNC_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SplashVmId(pub u64);

pub const MAIN_SPLASH_VM_ID: SplashVmId = SplashVmId(0);

/// The widget theme a Splash isolate boots with. A host picks its own theme
/// after `theme_mod`, which an isolate's prelude never sees, so it says here.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SplashTheme {
    #[default]
    Dark,
    Light,
    Skeleton,
}

/// Host-trusted Splash code that isolates run on top of the host's theme, in
/// the two phases a desktop stylesheet has: `theme` right after the isolate
/// picks its theme (for the host's own tokens), `widgets` right after it
/// registers its widgets (to restyle them).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SplashThemeScript {
    pub theme: String,
    pub widgets: String,
}

thread_local! {
    /// The host's theme for its isolates, which live with it on the UI thread.
    static SPLASH_THEME: std::cell::Cell<SplashTheme> = const { std::cell::Cell::new(SplashTheme::Dark) };
    static SPLASH_THEME_SCRIPT: RefCell<Option<std::rc::Rc<SplashThemeScript>>> = const { RefCell::new(None) };
    /// Bumped whenever the host's theme or its script changes, so that running
    /// isolates know to check whether they must restyle.
    static SPLASH_THEME_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Sets the theme this thread's Splash isolates use. Running ones restyle to it
/// on their next draw, unless pinned to a theme of their own (`Splash::set_theme`).
pub fn set_splash_theme(theme: SplashTheme) {
    if SPLASH_THEME.with(|current| current.replace(theme)) != theme {
        SPLASH_THEME_GENERATION.with(|generation| generation.set(generation.get() + 1));
    }
}

/// Sets the host's [`SplashThemeScript`]. Running isolates restyle to it on
/// their next draw, just as they do for a new [`set_splash_theme`].
pub fn set_splash_theme_script(script: SplashThemeScript) {
    let is_new = SPLASH_THEME_SCRIPT.with(|current| {
        let mut current = current.borrow_mut();
        let is_new = current.as_deref() != Some(&script);
        if is_new {
            *current = Some(std::rc::Rc::new(script));
        }
        is_new
    });
    if is_new {
        SPLASH_THEME_GENERATION.with(|generation| generation.set(generation.get() + 1));
    }
}

pub(crate) fn splash_theme() -> SplashTheme {
    SPLASH_THEME.with(|theme| theme.get())
}

pub(crate) fn splash_theme_script() -> Option<std::rc::Rc<SplashThemeScript>> {
    SPLASH_THEME_SCRIPT.with(|script| script.borrow().clone())
}

pub(crate) fn splash_theme_generation() -> u64 {
    SPLASH_THEME_GENERATION.with(|generation| generation.get())
}

/// Puts an isolate on `theme` and registers its widgets there, with the
/// host's `script` (if any) around that registration. Runs at the isolate's UI
/// expansion, and again in reload mode when a running isolate restyles.
pub(crate) fn style_isolate(
    vm: &mut ScriptVm,
    theme: SplashTheme,
    script: Option<&SplashThemeScript>,
    host_io_only: bool,
    set: IsolateWidgets,
) {
    let mut units = Vec::new();
    style_isolate_units(vm, &mut units, theme, script.map(|script| std::rc::Rc::new(script.clone())), host_io_only, set);
    run_units(vm, units);
}

pub(crate) fn style_isolate_units(
    vm: &mut ScriptVm,
    units: &mut Vec<UiUnit>,
    theme: SplashTheme,
    script: Option<std::rc::Rc<SplashThemeScript>>,
    host_io_only: bool,
    set: IsolateWidgets,
) {
    let theme_script = script.clone();
    units.push(UiUnit::new("style: theme", move |vm| {
        // Set directly: evaluating the same `script!` again on a restyle errors in the VM.
        let name = match theme {
            SplashTheme::Dark => id!(dark),
            SplashTheme::Light => id!(light),
            SplashTheme::Skeleton => id!(skeleton),
        };
        let themes = vm.module(id!(themes));
        let picked = vm.value(themes, name);
        let modules = vm.modules();
        vm.set_value_def(modules, id!(theme), picked);
        if let Some(script) = &theme_script {
            eval_theme_script(vm, "theme", &script.theme);
        }
    }));
    crate::widgets_mod_with_host_io_units(vm, units, host_io_only, set);
    if let Some(script) = script {
        units.push(UiUnit::new("style: widgets script", move |vm| {
            eval_theme_script(vm, "widgets", &script.widgets);
        }));
    }
}

/// Styles a running isolate again onto `theme` and the host's `script`:
/// [`style_isolate`] in reload mode, then the installed desktop stylesheet's
/// widgets (if any), then the isolate's allocation-time host mods, as at boot.
/// An isolate whose UI is still lazy only records the new style, which its
/// coming expansion installs.
pub(crate) fn restyle_isolate(
    vm: &mut ScriptVm,
    theme: SplashTheme,
    script: Option<std::rc::Rc<SplashThemeScript>>,
    loader: Option<&ScriptObjectRef>,
) {
    let heap_key = vm.heap_key();
    let lazy = vm.has_lazy_modules();
    let (host_io_only, set, host_mods) = match vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get_mut(&heap_key) {
        Some(ui) => {
            ui.theme = theme;
            ui.script = script.clone();
            (ui.host_io_only, ui.set, ui.host_mods.clone())
        }
        None => (vm.cx_mut().script_data.std.host_io_only(), IsolateWidgets::Library, Vec::new()),
    };
    if lazy {
        return;
    }
    // The registration is the host's own code (the library's fonts load through the loader the
    // isolate was stripped of), and the restriction is back before any guest code runs again.
    let restricted = vm.cx_mut().script_data.std.set_host_io_only_for_host_code(false);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        vm.with_reload(|vm| {
            with_loader(vm, loader, |vm| {
                style_isolate(vm, theme, script.as_deref(), host_io_only, set);
                crate::desktop_style::apply_widgets(vm);
                // Widget registration replaces mod.widgets: put this isolate's own host mods back.
                for install in host_mods {
                    install(vm);
                }
            })
        })
    }));
    vm.cx_mut().script_data.std.set_host_io_only_for_host_code(restricted);
    if let Err(payload) = out {
        std::panic::resume_unwind(payload);
    }
}

fn eval_theme_script(vm: &mut ScriptVm, phase: &str, code: &str) {
    if code.is_empty() {
        return;
    }
    // A script's last statement is its value, which an assignment there
    // never stores; ending the script on an empty statement avoids that.
    let code = format!("{code}\n;");
    vm.eval(ScriptMod::new("", &format!("splash_theme_{phase}"), &format!("splash_theme/{phase}.splash"), 0, 0, &code, &[]));
}

thread_local! {
    /// Splash isolate VMs whose owning `Splash` widget has been dropped, awaiting
    /// reclamation. `Drop` can't reach `Cx`, so it only records the id here; the
    /// real teardown happens in [`gc_dead_splash_isolates`] on the next isolate
    /// allocation. We defer rather than free in `Drop` because the isolate owns a
    /// live `ScriptHeap`/`ScriptStd` that must be dropped with a `Cx` in hand and
    /// while nothing is executing in it.
    static DEAD_SPLASH_ISOLATES: RefCell<Vec<SplashVmId>> = const { RefCell::new(Vec::new()) };
}

thread_local! {
    /// Script mods the embedding host installs into every Splash isolate, in
    /// registration order. Registered from host code that has no `Cx` in hand,
    /// hence a thread-local rather than `CxWidgetAsync` state.
    static HOST_ISOLATE_MODS: RefCell<Vec<fn(&mut ScriptVm)>> = const { RefCell::new(Vec::new()) };
}

/// Install a script mod into every Splash isolate allocated from here on.
///
/// An isolate receives makepad's own mods and nothing else, so a widget type
/// defined out in host code is otherwise unnameable from a mounted body — the
/// body has no way to say it, and the host is forced to substitute a built-in.
/// Pass the `script_mod` fn of the crate whose widgets those bodies should reach.
///
/// An isolate takes its mods at allocation, so a registration only reaches
/// isolates allocated after it: register before the first mount. Registering the
/// same mod twice installs it twice, which is wasteful but harmless — the second
/// pass rebinds the same names. The isolate replays its allocation-time mods in
/// reload mode when its host theme changes, restoring their widget definitions.
///
/// Host mods install last: after the ambient-authority strip (`fs`, `run`, `res`,
/// `cx.quit`) and after the jailed `fs` and brokered `host` re-registrations. A
/// host mod therefore cannot be stripped by that pass, and it sees the isolate's
/// final namespace. This does mean a host mod could re-expose ambient authority
/// if it deliberately handed one back — the host is trusted code, and it already
/// chose what to install.
pub fn register_splash_isolate_mod(f: fn(&mut ScriptVm)) {
    HOST_ISOLATE_MODS.with(|g| g.borrow_mut().push(f));
}

/// Queue a Splash isolate for reclamation on the next isolate alloc. Called from
/// the `Drop` of an isolate's owner, which has no `Cx` (`Splash`, a game's script
/// host). Ignores the main VM (id 0), never an isolate.
pub fn mark_splash_isolate_dead(vm_id: SplashVmId) {
    if vm_id == MAIN_SPLASH_VM_ID {
        return;
    }
    DEAD_SPLASH_ISOLATES.with(|g| g.borrow_mut().push(vm_id));
}

/// Reclaim any isolates queued by [`mark_splash_isolate_dead`]. Called on each new
/// isolate allocation so the live count tracks live Splashes instead of growing
/// unboundedly. Dropping an isolate frees its `ScriptHeap` (and every widget object
/// minted in it) plus its `ScriptStd`, so we must also purge every queue/map that
/// could later swap into the now-missing VM (which would panic in
/// `with_script_vm_id`) or dereference its freed heap. Cheap no-op when nothing is
/// queued.
pub fn gc_dead_splash_isolates(cx: &mut Cx) {
    let dead: Vec<SplashVmId> = DEAD_SPLASH_ISOLATES.with(|g| {
        let mut g = g.borrow_mut();
        std::mem::take(&mut *g)
    });
    if dead.is_empty() {
        return;
    }
    let dead_heaps: Vec<usize> = {
        let state = cx.global::<CxWidgetAsync>();
        state
            .heap_to_vm
            .iter()
            .filter(|(_, v)| dead.contains(v))
            .map(|(k, _)| *k)
            .collect()
    };
    // Stop and drop script timers whose callbacks live in a dying heap. Their fn refs
    // hold the heap's roots Rc alive, and firing one later would deref a freed heap.
    let stale_timers: Vec<_> = cx
        .script_data
        .timers
        .timers
        .iter()
        .filter(|t| dead_heaps.contains(&t.callback.heap_key()))
        .map(|t| (t.id, t.timer))
        .collect();
    for (id, timer) in stale_timers {
        cx.stop_timer(timer);
        cx.script_data.timers.timers.retain(|t| t.id != id);
    }
    // Sandbox roots and host-bridge state die with their isolates.
    crate::splash_storage::gc_roots(&dead_heaps);
    crate::splash_host::gc_bridge(&dead_heaps);
    // Script tweens: the isolate's host (its rooted apply objects), its
    // callback closures and its handle type go with it.
    crate::tween_script::gc_vms(cx, &dead);
    crate::desktop_style::gc_heaps(cx,&dead_heaps);
    crate::cached_widget::gc_heaps(cx, &dead_heaps);
    for heap in &dead_heaps {
        cx.forget_kept_for_reload(*heap);
    }
    // And the resource cache, which is keyed by heap ADDRESS: dropping a heap
    // frees that address for the next isolate, and a leftover entry would hand
    // the newcomer a dead heap's handle. See `CxScriptResources::gc_heaps`.
    cx.script_data.resources.gc_heaps(&dead_heaps);
    let state = cx.global::<CxWidgetAsync>();
    state.dead_heaps.extend(dead_heaps.iter().copied());
    for heap in &dead_heaps {
        state.isolate_ui.remove(heap);
    }
    for vm_id in dead {
        // Purge everything that HOLDS the isolate's values before dropping the
        // heap those values live in: `script_to_widget_calls` carries a
        // `ScriptObjectRef` into it, and `pending_script_to_widget_returns` a
        // bare `ScriptValue`. Same reason `gc_bridge` above runs first.
        state.heap_to_vm.retain(|_, v| *v != vm_id);
        state.ui_handle_types.remove(&vm_id);
        state.vm_root_uids.remove(&vm_id);
        state.loaders.remove(&vm_id);
        state.ui_in_progress.remove(&vm_id);
        state.done.retain(|d| d.vm_id != vm_id);
        state.widget_to_script_calls.retain(|r| r.vm_id != vm_id);
        state.script_to_widget_calls.retain(|r| r.vm_id != vm_id);
        state
            .pending_script_to_widget_returns
            .retain(|(v, _), _| *v != vm_id);
        state.thread_map.retain(|(v, _), _| *v != vm_id);
        state.isolated_vms.vms.remove(&vm_id);
    }
}

/// A script value a queue or a widget keeps from one pump to the next, rooted. A collection --
/// the paint loop's after the draw that rendered, an isolate's round-robin one -- can run before
/// it is read, and a bare `ScriptValue` keeps nothing alive: a render's `me` and result were
/// freed that way and their delivery panicked on the freed slots. An object is held by its own
/// ref, any other heap value (array, string, pod, handle, regex) in a rooted one-slot box;
/// numbers, ids and inline strings need nothing.
#[derive(Clone)]
pub(crate) struct HeldValue {
    value: ScriptValue,
    _root: Option<ScriptObjectRef>,
}

impl HeldValue {
    pub(crate) fn new(vm: &mut ScriptVm, value: ScriptValue) -> Self {
        let root = if let Some(obj) = value.as_object() {
            (obj != ScriptObject::ZERO).then(|| vm.new_object_ref(obj))
        } else if value.is_array() || value.is_string() || value.is_pod() || value.is_handle() || value.is_regex() {
            let held = vm.new_object();
            vm.vec_push(held, NIL, value);
            Some(vm.new_object_ref(held))
        } else {
            None
        };
        Self { value, _root: root }
    }

    pub(crate) fn get(&self) -> ScriptValue {
        self.value
    }
}

#[derive(Clone)]
pub struct ScriptAsyncCall {
    id: ScriptAsyncId,
    method: LiveId,
    me: HeldValue,
    thread_id: Option<ScriptThreadId>,
}

#[derive(Clone, Default)]
pub struct ScriptAsyncCalls {
    calls: Vec<ScriptAsyncCall>,
}

impl ScriptAsyncCalls {
    pub fn take(&mut self, id: ScriptAsyncId) -> Option<ScriptAsyncCall> {
        if let Some(pos) = self.calls.iter().position(|v| v.id == id) {
            Some(self.calls.swap_remove(pos))
        } else {
            None
        }
    }
}

impl ScriptAsyncCall {
    pub fn id(&self) -> ScriptAsyncId {
        self.id
    }

    pub fn method(&self) -> LiveId {
        self.method
    }

    pub fn me(&self) -> ScriptValue {
        self.me.get()
    }

    pub fn thread_id(&self) -> Option<ScriptThreadId> {
        self.thread_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptAsyncResult {
    Return(ScriptValue),
    Pending,
    MethodNotFound,
}

struct WidgetAsyncDone {
    vm_id: SplashVmId,
    target_uid: WidgetUid,
    id: ScriptAsyncId,
    result: HeldValue,
}

struct ScriptToWidgetCallRequest {
    vm_id: SplashVmId,
    target_uid: WidgetUid,
    method: LiveId,
    caller_thread: ScriptThreadId,
    args: ScriptObjectRef,
}

struct ScriptToWidgetReturn {
    vm_id: SplashVmId,
    target_uid: WidgetUid,
    method: LiveId,
    result: HeldValue,
}

struct WidgetToScriptCallRequest {
    vm_id: SplashVmId,
    target_uid: WidgetUid,
    me: HeldValue,
    source: ScriptObjectRef,
    script_fn: ScriptFnRef,
    args: HeldValue,
}

/// An isolate's UI registration, as it was asked for at allocation.
struct IsolateUi {
    set: IsolateWidgets,
    sheet: Option<crate::desktop_style::StyleSheet>,
    host_io_only: bool,
    theme: SplashTheme,
    /// the host's theme script (none when the Splash pins a stock theme)
    script: Option<std::rc::Rc<SplashThemeScript>>,
    loader: Option<ScriptObjectRef>,
    /// the host's isolate mods as they were at allocation
    host_mods: Vec<fn(&mut ScriptVm)>,
    /// the host's registration instead of the stock one (`alloc_splash_vm_with_ui`), kept for
    /// the isolate's reload passes ([`reload_isolate`])
    custom: Option<std::rc::Rc<dyn Fn(&mut ScriptVm)>>,
    /// the sheet was set by a restyle before the expansion: its widget half goes over the library
    apply_sheet_widgets: bool,
    /// the registration under way a unit at a time, and its next unit: made at the first unit
    /// from this record as it stands then (see [`step_isolate_ui_unit`])
    units: Option<(Vec<UiUnit>, usize)>,
    /// a `mod` key of the lazy group the registration is declared as, to step or finish it by
    lazy_key: Option<LiveId>,
}

/// A Splash restyling an isolate whose UI modules are still lazy: the sheet is what the coming
/// registration installs (theme and widget halves), so the library is registered once, styled.
/// False when the isolate's UI is already registered (the caller restyles it).
pub(crate) fn restyle_lazy_isolate(cx: &mut Cx, vm_id: SplashVmId, sheet: &crate::desktop_style::StyleSheet) -> bool {
    let (lazy, heap_key) = cx.with_script_vm_id(vm_id, |vm| (vm.has_lazy_modules(), vm.heap_key()));
    if !lazy {
        return false;
    }
    match cx.global::<CxWidgetAsync>().isolate_ui.get_mut(&heap_key) {
        Some(ui) if ui.custom.is_none() => {
            ui.sheet = Some(sheet.clone());
            ui.apply_sheet_widgets = true;
            true
        }
        _ => false,
    }
}

thread_local! {
    // The `mod` keys an isolate's UI registration makes, learned from the first isolate
    // by the registration's shape: widget set, families and host mods, host-io, sheet or not
    static ISOLATE_UI_KEYS: RefCell<Vec<(Vec<usize>, Vec<LiveId>)>> = const { RefCell::new(Vec::new()) };
}

/// Registers an isolate's UI modules: the parent's sheet, `theme_mod`, the Splash theme, the
/// widget library (families as the host chose) and the host's isolate mods, with the isolate's
/// resource loader on `mod.res` while each unit runs (fonts load through it). Eagerly, all at
/// once, for the first isolate of the process; otherwise a lazy group, run on the first read of
/// one of its modules, or a unit at a time by a host that steps it ([`CxSplashVmExt::step_isolate_ui`]).
fn expand_isolate_ui(vm: &mut ScriptVm) {
    while step_isolate_ui_unit(vm) {}
}

unsafe extern "C-unwind" fn step_isolate_ui_lazily(vm: *mut ScriptVm) -> bool {
    step_isolate_ui_unit(&mut *vm)
}

/// Runs the next unit of the isolate's UI registration: true while units remain. The unit list
/// is made at the first unit from the isolate's [`IsolateUi`]; a unit that panics leaves no list,
/// so the next run starts it over (the whole registration again over what the heap has).
fn step_isolate_ui_unit(vm: &mut ScriptVm) -> bool {
    let heap_key = vm.heap_key();
    match vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get(&heap_key).map(|ui| ui.units.is_some()) {
        Some(true) => (),
        Some(false) => {
            let units = isolate_ui_units_of(vm).expect("the isolate's record");
            if let Some(ui) = vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get_mut(&heap_key) {
                ui.units = Some((units, 0));
            }
        }
        None => {
            // No record to keep a cursor in: the registration with the defaults, at once.
            for unit in isolate_ui_units(vm, None, true, IsolateWidgets::Library, splash_theme(), None, Vec::new(), None, false) {
                run_isolate_ui_unit(vm, &unit, None);
            }
            return false;
        }
    }
    let next = vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get_mut(&heap_key).and_then(|ui| {
        let loader = ui.loader.clone();
        let (units, next) = ui.units.as_mut()?;
        let unit = units.get(*next).cloned();
        *next += 1;
        let more = *next < units.len();
        if !more {
            ui.units = None;
        }
        Some((unit?, loader, more))
    });
    let Some((unit, loader, more)) = next else { return false };
    let start = Cx::monotonic_now();
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_isolate_ui_unit(vm, &unit, loader.as_ref())));
    if let Err(payload) = out {
        if let Some(ui) = vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get_mut(&heap_key) {
            ui.units = None;
        }
        std::panic::resume_unwind(payload);
    }
    if crate::makepad_script::makepad_error_log::trace_enabled("script.lazy") {
        eprintln!("script.lazy: isolate ui unit `{}` in {:.1} ms", unit.name, (Cx::monotonic_now() - start) * 1000.0);
    }
    more
}

/// The unit list of the isolate's UI registration as its record stands now; None without one.
fn isolate_ui_units_of(vm: &mut ScriptVm) -> Option<Vec<UiUnit>> {
    let heap_key = vm.heap_key();
    let ui = vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get(&heap_key)?;
    let (sheet, host_io_only, set, theme, script, host_mods, custom, apply_sheet_widgets) = (ui.sheet.clone(), ui.host_io_only, ui.set, ui.theme, ui.script.clone(), ui.host_mods.clone(), ui.custom.clone(), ui.apply_sheet_widgets);
    Some(isolate_ui_units(vm, sheet, host_io_only, set, theme, script, host_mods, custom, apply_sheet_widgets))
}

/// One unit of an isolate's UI registration. The registration is the host's own code: it loads
/// the library's fonts through the isolate's loader like before the isolate was restricted, and
/// the restriction is back before any guest code runs again.
fn run_isolate_ui_unit(vm: &mut ScriptVm, unit: &UiUnit, loader: Option<&ScriptObjectRef>) {
    let restricted = vm.cx_mut().script_data.std.set_host_io_only_for_host_code(false);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| with_loader(vm, loader, |vm| (unit.run)(vm))));
    vm.cx_mut().script_data.std.set_host_io_only_for_host_code(restricted);
    if let Err(payload) = out {
        std::panic::resume_unwind(payload);
    }
}

/// An isolate entry's first step when the host left its UI registration part way
/// ([`CxSplashVmExt::step_isolate_ui`]): the rest of it, so nothing but the host's own steps sees
/// it half done. Before a live edit's reload pass, which then finds it registered.
fn finish_isolate_ui(vm: &mut ScriptVm) {
    let heap_key = vm.heap_key();
    let key = vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get(&heap_key).and_then(|ui| ui.lazy_key);
    if let Some(key) = key {
        vm.expand_lazy_key(key);
    }
}

/// Whether entering isolate `vm_id` must finish its UI registration first; it is out of the
/// in-progress set from here on.
fn take_isolate_ui_in_progress(cx: &mut Cx, vm_id: SplashVmId) -> bool {
    let state = cx.global::<CxWidgetAsync>();
    !state.ui_in_progress.is_empty() && state.ui_in_progress.remove(&vm_id)
}

fn isolate_ui_units(
    vm: &mut ScriptVm,
    sheet: Option<crate::desktop_style::StyleSheet>,
    host_io_only: bool,
    set: IsolateWidgets,
    theme: SplashTheme,
    script: Option<std::rc::Rc<SplashThemeScript>>,
    host_mods: Vec<fn(&mut ScriptVm)>,
    custom: Option<std::rc::Rc<dyn Fn(&mut ScriptVm)>>,
    apply_sheet_widgets: bool,
) -> Vec<UiUnit> {
    let mut units = Vec::new();
    if let Some(custom) = custom {
        units.push(UiUnit::new("ui: custom", move |vm| custom(vm)));
        for install in host_mods {
            units.push(UiUnit::new("ui: host mod", install));
        }
        return units;
    }
    // The allocating VM's sheet, or its plain theme: never the
    // environment's sheet over a VM that took it off.
    let sheet = sheet.map(std::rc::Rc::new);
    units.push(UiUnit::new("ui: sheet", move |vm| {
        match &sheet {
            Some(sheet) => crate::desktop_style::install(vm, sheet.as_ref().clone()),
            None => crate::desktop_style::uninstall(vm),
        }
    }));
    crate::theme_mod_units(&mut units);
    style_isolate_units(vm, &mut units, theme, script, host_io_only, set);
    if apply_sheet_widgets {
        units.push(UiUnit::new("ui: sheet widgets", crate::desktop_style::apply_widgets));
    }
    // Mods the embedding host registered through `register_splash_isolate_mod` by the
    // isolate's allocation, last, so they see the final namespace.
    for install in host_mods {
        units.push(UiUnit::new("ui: host mod", install));
    }
    units
}

/// A live edit's reload pass in the installed isolate, as the main VM's runs its app's
/// `script_mod` again: its UI registration again in reload mode (the host's own, or the theme,
/// the Splash theme, the library and the installed sheet's widget half, as a restyle does),
/// unless its UI is still lazy (the coming expansion registers the edited code), then the
/// host's isolate mods and what the host and libraries kept for the pass
/// (`ScriptVmReload::keep_for_reload`). The base platform modules are left alone, as the main
/// VM's are: the strip took ambient authority out of them. Runs where the isolate is first
/// entered after the edit ([`with_isolate_installed`]); the trees built on the old
/// registration are their hosts' to apply again (a Splash runs its body again).
fn reload_isolate(vm: &mut ScriptVm) {
    let heap_key = vm.heap_key();
    let lazy = vm.has_lazy_modules();
    let ui = vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get(&heap_key).map(|ui| (ui.host_io_only, ui.set, ui.theme, ui.script.clone(), ui.loader.clone(), ui.host_mods.clone(), ui.custom.clone()));
    let restricted = vm.cx_mut().script_data.std.set_host_io_only_for_host_code(false);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        vm.with_reload(|vm| {
            if let Some((host_io_only, set, theme, script, loader, host_mods, custom)) = ui.filter(|_| !lazy) {
                with_loader(vm, loader.as_ref(), |vm| {
                    match custom {
                        Some(custom) => custom(vm),
                        None => {
                            crate::theme_mod(vm);
                            style_isolate(vm, theme, script.as_deref(), host_io_only, set);
                            crate::desktop_style::apply_widgets(vm);
                        }
                    }
                    for install in host_mods {
                        install(vm);
                    }
                });
            }
            vm.run_kept_for_reload();
        })
    }));
    vm.cx_mut().script_data.std.set_host_io_only_for_host_code(restricted);
    if let Err(payload) = out {
        std::panic::resume_unwind(payload);
    }
}

struct IsolatedSplashVm {
    network_enabled: bool,
    std: ScriptStd,
    vm: Option<Box<ScriptVmBase>>,
    /// The reload pass (`Cx::reload_generation`) its registration last followed.
    reloaded_at: u64,
    /// Its host follows live edits itself ([`isolate_reloads_with_host`]).
    host_reloads: bool,
}

/// The host of isolate `vm_id` follows live edits itself: it registers the isolate's UI again
/// and rebases its trees onto it in one restyle (flow's faces take a live edit as an
/// appearance change, indexing the old prototypes first), so the isolate's first entry after
/// an edit runs no reload pass of its own ([`reload_isolate`]).
pub fn isolate_reloads_with_host(cx: &mut Cx, vm_id: SplashVmId) {
    if let Some(isolated) = cx.global::<CxWidgetAsync>().isolated_vms.vms.get_mut(&vm_id) {
        isolated.host_reloads = true;
    }
}

#[derive(Default)]
struct IsolatedScriptVms {
    next_id: u64,
    vms: HashMap<SplashVmId, IsolatedSplashVm>,
}

#[derive(Default)]
struct CxWidgetAsync {
    done: VecDeque<WidgetAsyncDone>,
    widget_to_script_calls: VecDeque<WidgetToScriptCallRequest>,
    script_to_widget_calls: VecDeque<ScriptToWidgetCallRequest>,
    pending_script_to_widget_returns: HashMap<(SplashVmId, usize), ScriptToWidgetReturn>,
    thread_map: HashMap<(SplashVmId, usize), (WidgetUid, ScriptAsyncId)>,
    ui_handle_types: HashMap<SplashVmId, ScriptHandleType>,
    global_ui_root_uid: WidgetUid,
    /// Maps a heap identity (see [`ScriptObjectRef::heap_key`]) to the isolate VM
    /// that owns it. Only isolate heaps are inserted, and an entry is removed the
    /// moment its isolate dies — so a ref that misses here is either the main app
    /// heap's (checked against the app VM's own key) or a dead isolate's, whose
    /// calls are dropped rather than routed anywhere. This replaces per-widget uid
    /// registration: a widget's owning VM is derived directly from its own
    /// `source` ref, so there are no coverage gaps for lazily-created widgets.
    heap_to_vm: HashMap<usize, SplashVmId>,
    /// Each isolate's own view-root uid (set by [`inject_splash_ui_handle`]). Isolate `ui`
    /// handles are confined to this subtree so a mini-app can't reach host/sibling widgets.
    vm_root_uids: HashMap<SplashVmId, WidgetUid>,
    /// Each isolate's resource loader (`mod.res`), held here because the
    /// isolate's own namespace is stripped of it before a body first runs.
    /// Only the host's restyle hands it back, while it runs: a sheet's theme
    /// half loads its fonts through `crate_resource` (see [`with_loader`]).
    loaders: HashMap<SplashVmId, ScriptObjectRef>,
    isolated_vms: IsolatedScriptVms,
    current_vm_id: SplashVmId,
    /// Round-robin cursor for the per-pump isolate GC pass (last vm id serviced).
    gc_rr_last: u64,
    /// What each isolate's lazy UI expansion needs, by heap key (see `expand_isolate_ui`).
    isolate_ui: HashMap<usize, IsolateUi>,
    /// Isolates whose host stepped their UI registration part way
    /// ([`CxSplashVmExt::step_isolate_ui`]): any other entry finishes it first.
    ui_in_progress: std::collections::HashSet<SplashVmId>,
    /// Heaps of isolates that have been reclaimed. A widget can outlive the
    /// isolate that minted it by a frame or two and still try to call back into
    /// it; this is what tells such a ref apart from an app-VM one, so it can be
    /// dropped instead of misrouted. Keys are only added once their isolate is
    /// gone and removed again if a later heap is allocated at the same address.
    dead_heaps: std::collections::HashSet<usize>,
}

#[derive(Default)]
struct CxWidgetAsyncHooksInstalled(pub bool);

struct CxWidgetHandleGc {
    handle: ScriptHandle,
    uid: WidgetUid,
}

impl ScriptHandleGc for CxWidgetHandleGc {
    fn gc(&mut self) {}

    fn set_handle(&mut self, handle: ScriptHandle) {
        self.handle = handle;
    }
}

/// Swap isolate `vm_id` onto `Cx` — its `ScriptStd` into `cx.script_data.std` and its
/// `ScriptVmBase` into `cx.script_vm` — for the duration of `f`, then put the previous
/// pair back.
///
/// **Every path that executes an isolate's script must go through this.** A `ScriptVm`
/// hands `&mut Cx` to native code via `with_cx`/`with_cx_mut`, which park the executing
/// `bx` into `cx.script_vm` and take it back out. Run an isolate while the app VM still
/// occupies that slot and the park silently drops the app VM's entire heap, nulls the
/// slot, and leaves every subsequent `Cx`-mediated script access resolving isolate
/// object pointers against the wrong heap. Handing the isolate's `std`/`vm` to a
/// function as side-channel `&mut` args is exactly that mistake.
///
/// Nested installs are fine: the enclosing `with_vm` owns the outer `bx` (so the slot
/// reads `None` here) and restores it on the way out.
fn with_isolate_installed<R>(cx: &mut Cx, vm_id: SplashVmId, f: impl FnOnce(&mut Cx) -> R) -> R {
    let mut isolated = cx
        .global::<CxWidgetAsync>()
        .isolated_vms
        .vms
        .remove(&vm_id)
        .unwrap_or_else(|| panic!("missing Splash VM {:?}", vm_id));

    let previous_vm_id = cx.global::<CxWidgetAsync>().current_vm_id;
    cx.global::<CxWidgetAsync>().current_vm_id = vm_id;
    // The first entry after a live edit runs the isolate's reload pass before its work.
    let reload = isolated.reloaded_at != cx.reload_generation && !isolated.host_reloads;
    isolated.reloaded_at = cx.reload_generation;
    let finish = take_isolate_ui_in_progress(cx, vm_id);

    let outer_std = std::mem::replace(&mut cx.script_data.std, isolated.std);
    let outer_vm = cx.script_vm.take();
    cx.script_vm = isolated.vm.take();

    // Whether `f` returns or unwinds, the isolate goes back into the table
    // and the outer VM back onto `Cx`: a panic caught above this frame (a
    // platform's event catcher, a host isolating a guest) must not cost the
    // app its VM and the table its isolate. Catch, restore, then let the
    // panic continue to the containment layer at the entry funnel.
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if finish {
            cx.with_vm(finish_isolate_ui);
        }
        if reload {
            contain_isolate_panic("live edit reload", || cx.with_vm(reload_isolate));
        }
        f(cx)
    }));

    isolated.vm = cx.script_vm.take();
    cx.script_vm = outer_vm;
    isolated.std = std::mem::replace(&mut cx.script_data.std, outer_std);

    cx.global::<CxWidgetAsync>().current_vm_id = previous_vm_id;
    cx.global::<CxWidgetAsync>()
        .isolated_vms
        .vms
        .insert(vm_id, isolated);

    match out {
        Ok(out) => out,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// The resource loader the isolate `vm_id` was stripped of, if it had one.
pub(crate) fn isolate_loader(cx: &mut Cx, vm_id: SplashVmId) -> Option<ScriptObjectRef> {
    cx.global::<CxWidgetAsync>().loaders.get(&vm_id).cloned()
}

/// Runs `f`, the host's own work in an isolate, with the isolate's resource
/// loader back on `mod.res`, and strips it again after, a panic included, so
/// no body code runs while it is there.
pub(crate) fn with_loader<R>(vm: &mut ScriptVm, loader: Option<&ScriptObjectRef>, f: impl FnOnce(&mut ScriptVm) -> R) -> R {
    let Some(loader) = loader else {
        return f(vm);
    };
    let modules = vm.modules();
    vm.set_value_def(modules, id!(res), loader.as_object());
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(vm)));
    vm.set_value_def(modules, id!(res), NIL);
    match out {
        Ok(out) => out,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Runs isolate-side work and CONTAINS any panic it raises: the host app must
/// survive anything a mini-app isolate does. Returns false when a panic was
/// contained; the isolate is left degraded (a Force Stop cleans it up).
pub fn contain_isolate_panic(what: &str, f: impl FnOnce()) -> bool {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(()) => true,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("(non-string panic)");
            crate::makepad_platform::error!(
                "contained a mini-app isolate panic in {what}: {msg}"
            );
            false
        }
    }
}

pub trait CxSplashVmExt {
    fn alloc_splash_vm(&mut self) -> SplashVmId;
    fn alloc_splash_vm_with_network(&mut self, network_enabled: bool) -> SplashVmId;
    /// Allocate an isolate with the chosen widget set, using the host's theme and script.
    /// A smaller set reduces UI registration and the isolate's live heap; a name outside
    /// that set is an ordinary unknown name, with no fallback registration.
    fn alloc_splash_vm_with_widgets(&mut self, network_enabled: bool, set: IsolateWidgets) -> SplashVmId;
    /// Allocate an isolate whose external I/O must use the host service bridge.
    fn alloc_splash_vm_with_host_io(&mut self) -> SplashVmId;
    fn alloc_splash_vm_with_io(&mut self, network_enabled: bool, host_io_only: bool) -> SplashVmId;
    /// An isolate whose UI registration is the host's own `ui` (a WM sheet, an appearance)
    /// instead of the stock theme + widgets, run once, at allocation: so a host that restyles a
    /// fresh isolate does not register the library twice. `ui` runs as host code (the isolate's
    /// loader on `mod.res`, no host-io restriction); the host's isolate mods follow it. It runs
    /// again in reload mode on each of the isolate's live-edit reload passes, so what it takes
    /// from the host it reads as it is then (a sheet restyled since stays).
    fn alloc_splash_vm_with_ui(&mut self, network_enabled: bool, ui: std::rc::Rc<dyn Fn(&mut ScriptVm)>) -> SplashVmId;
    /// Like [`alloc_splash_vm_with_io`](Self::alloc_splash_vm_with_io), but
    /// `Some(theme)` pins the isolate to that stock theme, without the host's
    /// [`SplashThemeScript`]. `None` takes the host's theme and script.
    fn alloc_splash_vm_with_theme(
        &mut self,
        network_enabled: bool,
        host_io_only: bool,
        theme: Option<SplashTheme>,
    ) -> SplashVmId;
    /// The next unit of isolate `vm_id`'s lazy UI registration (its theme, widgets and prelude),
    /// for a host that spreads it over frames: true while units remain. Any other entry to the
    /// isolate runs the rest first, so nothing but these steps sees it half done. False when there
    /// is nothing to step: the main VM, or an isolate whose UI is registered.
    fn step_isolate_ui(&mut self, vm_id: SplashVmId) -> bool;
    fn with_script_vm_id<R>(&mut self, vm_id: SplashVmId, f: impl FnOnce(&mut ScriptVm) -> R) -> R;
    fn with_script_vm_id_thread<R>(
        &mut self,
        vm_id: SplashVmId,
        thread_id: ScriptThreadId,
        f: impl FnOnce(&mut ScriptVm) -> R,
    ) -> R;
    /// Resolve the VM that owns a widget's script objects directly from a ref
    /// minted by that widget (its `source`, a template, an `on_click` fn). This
    /// is exact — the heap identity comes from the ref itself — so it never
    /// mis-routes lazily-created widgets the way a uid registry could.
    ///
    /// `None` means the ref belongs to a heap that is gone: an isolate was torn
    /// down while widgets it minted were still in the tree. Such a call must be
    /// dropped, never redirected — see the note on the impl.
    fn script_ref_vm_id(&mut self, script_ref: &ScriptObjectRef) -> Option<SplashVmId>;
    /// Tear an isolate down now: its script timers stop, its storage jail
    /// and host-bridge state go, its heap is freed. Every widget minted in
    /// it must be dropped by then (or about to be): a ref that outlives
    /// the heap is dropped, never redirected — see `script_ref_vm_id`.
    fn free_splash_vm(&mut self, vm_id: SplashVmId);
}

// The bodies of `CxSplashVmExt::with_script_vm_id*`, compiled once: the trait
// methods are generic over their closure and result, so each call site would
// otherwise carry its own copy of the VM routing below.

/// The body of `alloc_splash_vm_with_io` / `alloc_splash_vm_with_ui`.
/// `pin`: a stock theme the isolate keeps, without the host's [`SplashThemeScript`];
/// `None` takes the host's theme and script.
fn alloc_splash_vm_impl(cx: &mut Cx, network_enabled: bool, host_io_only: bool, set: IsolateWidgets, custom_ui: Option<std::rc::Rc<dyn Fn(&mut ScriptVm)>>, pin: Option<SplashTheme>) -> SplashVmId {

    // A nested Splash cannot acquire authority its parent does not have.
    let host_io_only = host_io_only || cx.script_data.std.host_io_only();
    let network_enabled = network_enabled && !host_io_only;
    ensure_widget_async_hooks_registered(cx);
    // Reclaim isolates from dropped Splashes before growing, so the live count
    // tracks the number of live Splash widgets rather than accumulating.
    gc_dead_splash_isolates(cx);

    let id = {
        let state = cx.global::<CxWidgetAsync>();
        if state.isolated_vms.next_id == 0 {
            state.isolated_vms.next_id = 1;
        }
        let id = SplashVmId(state.isolated_vms.next_id);
        state.isolated_vms.next_id += 1;
        id
    };

    let std = if network_enabled {
        ScriptStd::with_network_runtime(cx.net.clone())
    } else {
        ScriptStd::new()
    };
    let outer_std = std::mem::replace(&mut cx.script_data.std, std);
    let outer_vm = cx.script_vm.take();
    // An isolate wears the stylesheet of the VM that allocates it, as an
    // embedded `Splash` does on reapply: panels a guest builds from the
    // stock widgets match the window they sit in.
    let inherited = outer_vm.as_ref().and_then(|outer| crate::desktop_style::sheet_of_heap(cx, outer.heap_ref().heap_key()));
    cx.script_vm = Some(Box::new(ScriptVmBase::new()));
    let (bx, loader) = {
        let bx = cx.script_vm.take().unwrap();
        let mut vm = ScriptVm {
            host: cx.host_context(),
            bx,
        };
        crate::makepad_draw::makepad_platform::script::script_mod(&mut vm);
        // The isolate's UI modules (theme, widgets, the prelude, the host's isolate mods) are
        // lazy: declared now, registered on the first read of any of them, once, in this heap.
        // The first isolate of the process registers them eagerly to learn their `mod` keys.
        let heap_key = vm.heap_key();
        let script = if pin.is_none() { splash_theme_script() } else { None };
        vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.insert(heap_key, IsolateUi { set, sheet: inherited, host_io_only, theme: pin.unwrap_or_else(splash_theme), script: script.clone(), loader: None, host_mods: HOST_ISOLATE_MODS.with(|g| g.borrow().clone()), custom: None, apply_sheet_widgets: false, units: None, lazy_key: None });
        let shape: Vec<usize> = {
            let families = vm.cx_mut().global::<crate::widget_hooks::IsolateFamilies>().0.clone();
            let mut shape: Vec<usize> = families.iter().map(|f| *f as usize).collect();
            HOST_ISOLATE_MODS.with(|g| shape.extend(g.borrow().iter().map(|f| *f as usize)));
            shape.push(set as usize);
            shape.push(host_io_only as usize);
            shape.push(script.as_ref().map_or(0, |script| {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                script.hash(&mut hasher);
                hasher.finish() as usize | 1
            }));
            shape.push(vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get(&heap_key).map_or(0, |ui| ui.sheet.is_some() as usize));
            shape
        };
        let known = ISOLATE_UI_KEYS.with(|k| k.borrow().iter().find(|(s, _)| *s == shape).map(|(_, keys)| keys.clone()));
        if let Some(ui) = custom_ui {
            // the host's own registration, now: it restyles what it registers, so nothing is
            // registered before it (and its keys are its own: nothing to learn)
            vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get_mut(&heap_key).expect("just inserted").custom = Some(ui);
            expand_isolate_ui(&mut vm);
        } else { match known {
            Some(keys) => {
                vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get_mut(&heap_key).expect("just inserted").lazy_key = keys.first().copied();
                vm.declare_lazy_modules_stepped("isolate ui (theme, widgets, prelude)", &keys, step_isolate_ui_lazily);
            }
            None => {
                let modules = vm.modules();
                let before: Vec<LiveId> = vm.heap().map_entries(modules).iter().filter_map(|e| e.key.as_id()).collect();
                expand_isolate_ui(&mut vm);
                let keys: Vec<LiveId> = vm.heap().map_entries(modules).iter().filter_map(|e| e.key.as_id()).filter(|k| !before.contains(k)).collect();
                ISOLATE_UI_KEYS.with(|k| k.borrow_mut().push((shape, keys)));
            }
        } }
        // Splash isolates run untrusted-ish mini-app script; strip the
        // ambient-authority modules from the isolate's namespace entirely:
        // filesystem access (`fs`), child processes (`run`), and the resource
        // loader (`res`), whose handles reach BOTH the filesystem (abs_path
        // loads) and the network (web_url/http resources) without going
        // through the gated net runtime. Raw sockets are gated separately:
        // the stdlib's `net.socket_stream` errors when no net runtime is
        // configured, same as `net.http_request`.
        // `cx.quit` would let any mini-app close the whole host process.
        let strip = crate::makepad_script::script! {
            mod.fs = nil
            mod.run = nil
            mod.res = nil
            mod.cx.quit = nil
        };
        // The loader itself is kept where no script can name it, for the
        // host's own restyle: see `CxWidgetAsync::loaders`.
        let loader = vm.heap_mut().module(id!(res));
        let loader = (loader != ScriptObject::ZERO).then(|| vm.new_object_ref(loader));
        vm.eval(strip);
        // Re-register `fs` as the JAILED per-app storage module: inside an
        // isolate, "the filesystem" is the app's private sandbox directory
        // (assigned by the host via Splash::set_sandbox_dir; without one,
        // every call errors). See splash_storage.rs for the containment.
        crate::splash_storage::script_mod(&mut vm);
        // `host` is the brokered doorway to host services (location,
        // clipboard, IPC, ...); requests queue for the embedding host to
        // answer, and no host = nothing resolves. See splash_host.rs.
        crate::splash_host::script_mod(&mut vm);
        (vm.bx, loader)
    };
    let mut std = std::mem::replace(&mut cx.script_data.std, outer_std);
    if host_io_only {
        std.restrict_to_host_io();
    }
    cx.script_vm = outer_vm;

    // Record this isolate's heap identity so any ref minted here (widget
    // sources, templates, on_click fns) routes back to this VM.
    let heap_key = bx.heap_ref().heap_key();
    let reloaded_at = cx.reload_generation;
    let state = cx.global::<CxWidgetAsync>();
    // A heap key is an allocation address, so a fresh heap can land on one
    // a dead isolate used to own. Live registration wins over the memory of
    // the dead one.
    state.dead_heaps.remove(&heap_key);
    state.heap_to_vm.insert(heap_key, id);
    if let Some(loader) = loader {
        if let Some(ui) = state.isolate_ui.get_mut(&heap_key) {
            ui.loader = Some(loader.clone());
        }
        state.loaders.insert(id, loader);
    }
    state.isolated_vms.vms.insert(
        id,
        IsolatedSplashVm {
            network_enabled,
            std,
            vm: Some(bx),
            reloaded_at,
            host_reloads: false,
        },
    );
    if crate::makepad_script::makepad_error_log::trace_enabled("script.heaps") {
        let state = cx.global::<CxWidgetAsync>();
        let isolates = state.isolated_vms.vms.len();
        let live = state.isolated_vms.vms.get(&id).and_then(|iso| iso.vm.as_ref()).map_or(0, |bx| bx.heap_ref().gc_live_len());
        eprintln!("script.heaps: + isolate {} ({} live values), {isolates} isolates, {} heaps", id.0, live, crate::makepad_script::heap_tag::live_heaps());
    }

    id
}

#[inline(never)]
fn with_script_vm_id_dyn(cx: &mut Cx, vm_id: SplashVmId, f: &mut dyn FnMut(&mut ScriptVm)) {
    // "Already installed?" comes first, and the main-VM case has to prove
    // it too: `with_vm` runs against whatever VM is currently parked on
    // `Cx`, which during an isolate's own execution is that ISOLATE's. So
    // an unguarded main-VM branch here silently runs app-VM work in an
    // isolate's heap.
    let current = cx.global::<CxWidgetAsync>().current_vm_id;
    if current == vm_id {
        return cx.with_vm(f);
    }
    if vm_id == MAIN_SPLASH_VM_ID {
        if current != MAIN_SPLASH_VM_ID {
            // Running main-VM work here would execute against the
            // installed ISOLATE's heap and plant its values there; the
            // fault would only surface later, in a GC, with nothing left
            // pointing at this call.
            error!(
                "BUG: main-VM script call while isolate {current:?} is installed on Cx"
            );
        }
        return cx.with_vm(f);
    }

    with_isolate_installed(cx, vm_id, |cx| cx.with_vm(f))
}

#[inline(never)]
fn with_script_vm_id_thread_dyn(
    cx: &mut Cx,
    vm_id: SplashVmId,
    thread_id: ScriptThreadId,
    f: &mut dyn FnMut(&mut ScriptVm),
) {
    let current = cx.global::<CxWidgetAsync>().current_vm_id;
    if current == vm_id {
        return cx.with_vm_thread(thread_id, f);
    }
    if vm_id == MAIN_SPLASH_VM_ID {
        if current != MAIN_SPLASH_VM_ID {
            error!(
                "BUG: main-VM script call while isolate {current:?} is installed on Cx"
            );
        }
        return cx.with_vm_thread(thread_id, f);
    }

    with_isolate_installed(cx, vm_id, |cx| cx.with_vm_thread(thread_id, f))
}

impl CxSplashVmExt for Cx {
    fn alloc_splash_vm(&mut self) -> SplashVmId {
        self.alloc_splash_vm_with_network(false)
    }

    fn alloc_splash_vm_with_network(&mut self, network_enabled: bool) -> SplashVmId {
        self.alloc_splash_vm_with_io(network_enabled, false)
    }

    fn alloc_splash_vm_with_widgets(&mut self, network_enabled: bool, set: IsolateWidgets) -> SplashVmId {
        alloc_splash_vm_impl(self, network_enabled, false, set, None, None)
    }

    fn alloc_splash_vm_with_host_io(&mut self) -> SplashVmId {
        self.alloc_splash_vm_with_io(false, true)
    }

    fn alloc_splash_vm_with_io(&mut self, network_enabled: bool, host_io_only: bool) -> SplashVmId {
        alloc_splash_vm_impl(self, network_enabled, host_io_only, IsolateWidgets::Library, None, None)
    }

    fn alloc_splash_vm_with_ui(&mut self, network_enabled: bool, ui: std::rc::Rc<dyn Fn(&mut ScriptVm)>) -> SplashVmId {
        alloc_splash_vm_impl(self, network_enabled, false, IsolateWidgets::Library, Some(ui), None)
    }

    fn alloc_splash_vm_with_theme(
        &mut self,
        network_enabled: bool,
        host_io_only: bool,
        theme: Option<SplashTheme>,
    ) -> SplashVmId {
        alloc_splash_vm_impl(self, network_enabled, host_io_only, IsolateWidgets::Library, None, theme)
    }

    fn step_isolate_ui(&mut self, vm_id: SplashVmId) -> bool {
        if vm_id == MAIN_SPLASH_VM_ID {
            return false;
        }
        // Out of the in-progress set while its own step enters, so the entry does not finish it;
        // left in when a live edit is due, so the entry finishes it before the reload pass.
        let generation = self.reload_generation;
        let state = self.global::<CxWidgetAsync>();
        let reload_due = state.isolated_vms.vms.get(&vm_id).is_some_and(|isolated| isolated.reloaded_at != generation && !isolated.host_reloads);
        if !reload_due {
            state.ui_in_progress.remove(&vm_id);
        }
        let more = self.with_script_vm_id(vm_id, |vm| {
            let heap_key = vm.heap_key();
            let key = vm.cx_mut().global::<CxWidgetAsync>().isolate_ui.get(&heap_key).and_then(|ui| ui.lazy_key);
            key.is_some_and(|key| vm.step_lazy_key(key))
        });
        let state = self.global::<CxWidgetAsync>();
        if more {
            state.ui_in_progress.insert(vm_id);
        } else {
            state.ui_in_progress.remove(&vm_id);
        }
        more
    }

    fn with_script_vm_id<R>(&mut self, vm_id: SplashVmId, f: impl FnOnce(&mut ScriptVm) -> R) -> R {
        let mut f = Some(f);
        let mut out = None;
        with_script_vm_id_dyn(self, vm_id, &mut |vm| out = Some((f.take().unwrap())(vm)));
        out.unwrap()
    }

    fn with_script_vm_id_thread<R>(
        &mut self,
        vm_id: SplashVmId,
        thread_id: ScriptThreadId,
        f: impl FnOnce(&mut ScriptVm) -> R,
    ) -> R {
        let mut f = Some(f);
        let mut out = None;
        with_script_vm_id_thread_dyn(self, vm_id, thread_id, &mut |vm| {
            out = Some((f.take().unwrap())(vm))
        });
        out.unwrap()
    }

    fn free_splash_vm(&mut self, vm_id: SplashVmId) {
        mark_splash_isolate_dead(vm_id);
        gc_dead_splash_isolates(self);
    }

    fn script_ref_vm_id(&mut self, script_ref: &ScriptObjectRef) -> Option<SplashVmId> {
        let heap_key = script_ref.heap_key();
        if heap_key == 0 {
            return Some(MAIN_SPLASH_VM_ID);
        }
        let state = self.global::<CxWidgetAsync>();
        if let Some(vm_id) = state.heap_to_vm.get(&heap_key).copied() {
            return Some(vm_id);
        }
        // Not a live isolate's heap. Either the app VM's own — the common case,
        // since the app VM is never in `heap_to_vm` — or one that has been
        // reclaimed, and those two must not be confused.
        //
        // An isolate's widgets can outlive it by a frame: a tile dropped
        // mid-gesture, an app force-stopped while its buttons are still in the
        // tree. Each of those holds refs minted by the dead heap. Treating them
        // as "not an isolate, therefore the app VM" routes a dead heap's
        // objects INTO the app VM, which stores them in an args object of its
        // own. Nothing complains at the time — the checked stores skip indices
        // they cannot resolve — and then the next GC walks that object,
        // indexes the app heap with a foreign heap's index, and panics
        // somewhere else entirely, in code that did nothing wrong.
        //
        // So a heap we have reclaimed is remembered, and its calls are dropped
        // rather than redirected — the same treatment
        // `script_timer_dispatch_hook` already gives a dead isolate's timers.
        if state.dead_heaps.contains(&heap_key) {
            return None;
        }
        Some(MAIN_SPLASH_VM_ID)
    }
}

/// The isolate (if any) that owns a heap, for host-bridge response routing.
pub(crate) fn vm_for_heap(cx: &mut Cx, heap_key: usize) -> Option<SplashVmId> {
    cx.global::<CxWidgetAsync>().heap_to_vm.get(&heap_key).copied()
}

/// Whether `vm_id` can be entered with `with_script_vm_id` right now: the
/// main VM always, an isolate while it is allocated and parked in the table
/// (one that is installed on `Cx` is out of the table and answers false).
pub(crate) fn splash_vm_is_live(cx: &mut Cx, vm_id: SplashVmId) -> bool {
    vm_id == MAIN_SPLASH_VM_ID
        || cx
            .global::<CxWidgetAsync>()
            .isolated_vms
            .vms
            .contains_key(&vm_id)
}

/// The widget a script `ui` handle names (`ui`, `ui.card`): its uid, read
/// exactly as the handle's own getter reads it. `None` for any other value.
pub(crate) fn ui_handle_uid(vm: &ScriptVm, value: ScriptValue) -> Option<WidgetUid> {
    let handle = value.as_handle()?;
    vm.downcast_handle_gc::<CxWidgetHandleGc>(handle)
        .map(|gc| gc.uid)
}

/// Deliver `Event::NetworkResponses` to a Splash isolate's script (resolving its
/// `net.http_request` callbacks / promises). Responses that belong to other VMs simply
/// find no matching request id in this isolate's `ScriptStd` and are ignored.
pub(crate) fn handle_splash_network_responses(
    cx: &mut Cx,
    vm_id: SplashVmId,
    responses: &[NetworkResponse],
) {
    if vm_id == MAIN_SPLASH_VM_ID || responses.is_empty() {
        return;
    }

    match cx.global::<CxWidgetAsync>().isolated_vms.vms.get(&vm_id) {
        Some(isolated) if isolated.network_enabled => {}
        _ => return,
    }

    // The isolate has to be installed on `Cx` while its handlers run — see
    // `with_isolate_installed`.
    with_isolate_installed(cx, vm_id, |cx| {
        cx.handle_script_network_events_for_current_vm(responses)
    });
}

#[doc(hidden)]
pub fn set_widget_async_trace(_cx: &mut Cx, _enabled: bool) {}

fn force_set_map_value(heap: &mut ScriptHeap, obj: ScriptObject, key: LiveId, value: ScriptValue) {
    heap.map_insert_own(obj, key.into(), value);
}

#[doc(hidden)]
pub fn ensure_widget_async_hooks_registered(cx: &mut Cx) {
    cx.global::<CxWidgetAsync>();
    if cx.global::<CxWidgetAsyncHooksInstalled>().0 {
        return;
    }
    register_task_hooks(cx);
    cx.global::<CxWidgetAsyncHooksInstalled>().0 = true;
}

/// Inject `ui` as a real global into an isolated Splash VM, resolving against that splash's own
/// view root. The on_click/on_return callback path already injects `ui` into the *closure* scope,
/// but a closure that calls a helper `fn` (the natural way to write e.g. a calculator) leaves the
/// helper unable to see `ui`. Making `ui` a global on the splash VM fixes that so `ui.<id>` works
/// everywhere inside a runsplash block, not just inline in the handler.
pub(crate) fn inject_splash_ui_handle(cx: &mut Cx, vm_id: SplashVmId, root_uid: WidgetUid) {
    if vm_id == MAIN_SPLASH_VM_ID {
        return;
    }
    ensure_widget_async_hooks_registered(cx);
    // Remember this isolate's view root so its ui handles stay confined to that subtree.
    cx.global::<CxWidgetAsync>()
        .vm_root_uids
        .insert(vm_id, root_uid);
    cx.with_script_vm_id(vm_id, |vm| {
        let ui_handle = vm.build_ui_handle_for_uid(root_uid);
        vm.set_injected_global(id!(ui), ui_handle);
    });
}

pub(crate) fn update_global_ui_handle(cx: &mut Cx, root_uid: WidgetUid) {
    ensure_widget_async_hooks_registered(cx);
    if cx.global::<CxWidgetAsync>().global_ui_root_uid == root_uid {
        return;
    }
    // `with_vm` below runs against whatever VM is installed; with an isolate
    // installed this would mint the main `ui` handle in the isolate's heap
    // and leave current_vm_id clobbered to MAIN for the rest of the isolate's
    // execution. Defer to the next main-context call instead.
    if cx.global::<CxWidgetAsync>().current_vm_id != MAIN_SPLASH_VM_ID {
        error!(
            "BUG: update_global_ui_handle while isolate {:?} is installed; deferred",
            cx.global::<CxWidgetAsync>().current_vm_id
        );
        return;
    }
    cx.global::<CxWidgetAsync>().global_ui_root_uid = root_uid;
    cx.with_vm(|vm| {
        let ui_handle = vm.build_ui_handle_for_uid(root_uid);
        vm.set_injected_global(id!(ui), ui_handle);
    });
}

trait WidgetToScriptCallExt {
    fn build_ui_handle_for_uid(&mut self, target_uid: WidgetUid) -> ScriptValue;

    fn make_call_args_object_with_context(
        &mut self,
        source: ScriptObject,
        ui: ScriptValue,
        forwarded_args: ScriptValue,
    ) -> ScriptObject;

    fn widget_to_script_async_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
        from_method: LiveId,
    ) -> ScriptAsyncResult;

    fn widget_to_script_async_call(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
        from_method: LiveId,
    ) -> ScriptAsyncResult;

    fn widget_to_script_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
    );

    fn widget_to_script_call(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
    );

    fn enqueue_script_to_widget_call(
        &mut self,
        target_uid: WidgetUid,
        method: LiveId,
        args: ScriptValue,
    );
}

impl<'a> WidgetToScriptCallExt for ScriptVm<'a> {
    fn build_ui_handle_for_uid(&mut self, target_uid: WidgetUid) -> ScriptValue {
        ensure_widget_async_hooks_registered(self.cx_mut());
        let vm_id = self.cx_mut().global::<CxWidgetAsync>().current_vm_id;
        if self
            .cx_mut()
            .global::<CxWidgetAsync>()
            .ui_handle_types
            .get(&vm_id)
            .is_none()
        {
            register_ui_handle(self);
        }

        let ui_type = self
            .cx_mut()
            .global::<CxWidgetAsync>()
            .ui_handle_types
            .get(&vm_id)
            .copied()
            .expect("ui handle type not registered");

        let gc = CxWidgetHandleGc {
            handle: ScriptHandle::ZERO,
            uid: target_uid,
        };
        self.heap_mut().new_handle(ui_type, Box::new(gc)).into()
    }

    fn make_call_args_object_with_context(
        &mut self,
        source: ScriptObject,
        ui: ScriptValue,
        forwarded_args: ScriptValue,
    ) -> ScriptObject {
        let args_obj = self.new_object();
        // Keep mixed (map + vec) semantics so named context vars like `ui` and `self`
        // are stored in map keys, while positional forwarded args stay in vec.
        self.heap_mut().set_object_storage_auto(args_obj);
        self.heap_mut().clear_object_deep(args_obj);

        if let Some(obj) = forwarded_args.as_object() {
            self.merge_object_trap(args_obj, obj);
        } else if let Some(arr) = forwarded_args.as_array() {
            let len = self.array_len(arr);
            for index in 0..len {
                let value = self.array_index_trap(arr, index);
                self.vec_push_trap(args_obj, NIL, value);
            }
        } else if !forwarded_args.is_nil() {
            self.vec_push_trap(args_obj, NIL, forwarded_args);
        }

        self.set_value_trap(args_obj, id!(self), source);
        self.set_value_trap(args_obj, id!(ui), ui);

        args_obj
    }

    fn widget_to_script_async_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
        from_method: LiveId,
    ) -> ScriptAsyncResult {
        if script_fn.as_object() == ScriptObject::ZERO {
            return ScriptAsyncResult::MethodNotFound;
        }

        let async_id = ScriptAsyncId::new();
        let ui_handle = self.build_ui_handle_for_uid(target_uid);
        let call_args =
            self.make_call_args_object_with_context(source.as_object(), ui_handle, args);
        let result = self.with_instruction_limit(WIDGET_SCRIPT_INSTRUCTION_LIMIT as u64, |vm| {
            vm.call_with_args_object_with_me(script_fn.clone().into(), call_args, me)
        });

        if self.is_paused() {
            let thread_id = self.thread_id();
            let me = HeldValue::new(self, me);
            script_async.calls.push(ScriptAsyncCall {
                id: async_id,
                method: from_method,
                me,
                thread_id: Some(thread_id),
            });
            let vm_id = self.cx_mut().global::<CxWidgetAsync>().current_vm_id;
            self.cx_mut()
                .global::<CxWidgetAsync>()
                .thread_map
                .insert((vm_id, thread_id.to_index()), (target_uid, async_id));
            ScriptAsyncResult::Pending
        } else {
            let me = HeldValue::new(self, me);
            script_async.calls.push(ScriptAsyncCall {
                id: async_id,
                method: from_method,
                me,
                thread_id: None,
            });
            let held = HeldValue::new(self, result);
            let vm_id = self.cx_mut().global::<CxWidgetAsync>().current_vm_id;
            self.cx_mut()
                .global::<CxWidgetAsync>()
                .done
                .push_back(WidgetAsyncDone {
                    vm_id,
                    target_uid,
                    id: async_id,
                    result: held,
                });
            ScriptAsyncResult::Return(result)
        }
    }

    fn widget_to_script_async_call(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
        from_method: LiveId,
    ) -> ScriptAsyncResult {
        let args_obj = self.new_object();
        self.heap_mut().set_object_storage_vec2(args_obj);
        self.heap_mut().clear_object_deep(args_obj);
        for value in args {
            self.vec_push_trap(args_obj, NIL, *value);
        }
        self.widget_to_script_async_call_fwd(
            target_uid,
            script_async,
            me,
            source,
            script_fn,
            args_obj.into(),
            from_method,
        )
    }

    fn widget_to_script_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
    ) {
        if script_fn.as_object() == ScriptObject::ZERO {
            return;
        }
        let (me, args) = (HeldValue::new(self, me), HeldValue::new(self, args));
        let vm_id = self.cx_mut().global::<CxWidgetAsync>().current_vm_id;
        self.cx_mut()
            .global::<CxWidgetAsync>()
            .widget_to_script_calls
            .push_back(WidgetToScriptCallRequest {
                vm_id,
                target_uid,
                me,
                source,
                script_fn,
                args,
            });
    }

    fn widget_to_script_call(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
    ) {
        let args_obj = self.new_object();
        self.heap_mut().set_object_storage_vec2(args_obj);
        self.heap_mut().clear_object_deep(args_obj);
        for value in args {
            self.vec_push_trap(args_obj, NIL, *value);
        }
        self.widget_to_script_call_fwd(target_uid, me, source, script_fn, args_obj.into());
    }

    fn enqueue_script_to_widget_call(
        &mut self,
        target_uid: WidgetUid,
        method: LiveId,
        args: ScriptValue,
    ) {
        let args_ref = if let Some(args_obj) = args.as_object() {
            self.new_object_ref(args_obj)
        } else {
            let obj = self.new_object();
            self.heap_mut().set_object_storage_vec2(obj);
            self.heap_mut().clear_object_deep(obj);
            if !args.is_nil() {
                self.vec_push_trap(obj, NIL, args);
            }
            self.new_object_ref(obj)
        };

        let caller_thread = self.thread_id();
        let vm_id = self.cx_mut().global::<CxWidgetAsync>().current_vm_id;
        self.cx_mut()
            .global::<CxWidgetAsync>()
            .script_to_widget_calls
            .push_back(ScriptToWidgetCallRequest {
                vm_id,
                target_uid,
                method,
                caller_thread,
                args: args_ref,
            });

        self.pause();
    }
}

pub trait CxWidgetToScriptCallExt {
    fn widget_to_script_async_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
        from_method: LiveId,
    ) -> ScriptAsyncResult;

    fn widget_to_script_async_call(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
        from_method: LiveId,
    ) -> ScriptAsyncResult;

    fn widget_to_script_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
    );

    fn widget_to_script_call(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
    );
}

impl CxWidgetToScriptCallExt for Cx {
    fn widget_to_script_async_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
        from_method: LiveId,
    ) -> ScriptAsyncResult {
        let Some(vm_id) = self.script_ref_vm_id(&source) else {
            error!(
                "widget->script call {:?} dropped: widget {:?} source belongs to a reclaimed isolate heap",
                from_method, target_uid
            );
            return ScriptAsyncResult::MethodNotFound;
        };
        self.with_script_vm_id(vm_id, |vm| {
            let src_key = source.heap_key();
            if src_key != 0 && src_key != vm.heap_key() {
                error!(
                    "BUG: widget->script call {:?} for widget {:?} routed to vm {:?} whose heap does not own the widget's source",
                    from_method, target_uid, vm_id
                );
            }
            vm.widget_to_script_async_call_fwd(
                target_uid,
                script_async,
                me,
                source,
                script_fn,
                args,
                from_method,
            )
        })
    }

    fn widget_to_script_async_call(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
        from_method: LiveId,
    ) -> ScriptAsyncResult {
        let Some(vm_id) = self.script_ref_vm_id(&source) else {
            return ScriptAsyncResult::MethodNotFound;
        };
        self.with_script_vm_id(vm_id, |vm| {
            vm.widget_to_script_async_call(
                target_uid,
                script_async,
                me,
                source,
                script_fn,
                args,
                from_method,
            )
        })
    }

    fn widget_to_script_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
    ) {
        let Some(vm_id) = self.script_ref_vm_id(&source) else {
            error!(
                "widget->script call dropped: widget {:?} source belongs to a reclaimed isolate heap",
                target_uid
            );
            return;
        };
        self.with_script_vm_id(vm_id, |vm| {
            let src_key = source.heap_key();
            if src_key != 0 && src_key != vm.heap_key() {
                error!(
                    "BUG: widget->script call for widget {:?} routed to vm {:?} whose heap does not own the widget's source",
                    target_uid, vm_id
                );
            }
            vm.widget_to_script_call_fwd(target_uid, me, source, script_fn, args);
        });
    }

    fn widget_to_script_call(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
    ) {
        let Some(vm_id) = self.script_ref_vm_id(&source) else {
            return;
        };
        self.with_script_vm_id(vm_id, |vm| {
            vm.widget_to_script_call(target_uid, me, source, script_fn, args);
        });
    }
}

fn register_ui_handle(vm: &mut ScriptVm) {
    let vm_id = vm.cx_mut().global::<CxWidgetAsync>().current_vm_id;
    if vm
        .cx_mut()
        .global::<CxWidgetAsync>()
        .ui_handle_types
        .contains_key(&vm_id)
    {
        return;
    }

    let ui_type = vm.new_handle_type(id_lut!(ui));

    vm.set_handle_getter(ui_type, move |vm, pself, prop| {
        if let Some(handle) = pself.as_handle() {
            let Some(target_uid) = vm
                .downcast_handle_gc::<CxWidgetHandleGc>(handle)
                .map(|gc| gc.uid)
            else {
                return script_err_not_found!(vm.trap(), "invalid ui handle");
            };

            // Isolate VMs are confined to their own splash subtree: `ui.root` resolves to
            // the splash's view root (never the app root), and name lookups only search
            // within that subtree. Without this, a mini-app could reach host widgets or
            // widgets belonging to a sibling mini-app.
            let cur_vm_id = vm.cx_mut().global::<CxWidgetAsync>().current_vm_id;
            let confine_root = if cur_vm_id == MAIN_SPLASH_VM_ID {
                None
            } else {
                Some(
                    vm.cx_mut()
                        .global::<CxWidgetAsync>()
                        .vm_root_uids
                        .get(&cur_vm_id)
                        .copied()
                        .unwrap_or(target_uid),
                )
            };

            if prop == live_id!(root) {
                let root_uid = match confine_root {
                    Some(root_uid) => root_uid,
                    None => vm.with_cx(|cx| cx.widget_tree().root_uid()),
                };
                if root_uid == WidgetUid(0) {
                    return script_err_not_found!(vm.trap(), "ui root not found");
                }
                return vm.build_ui_handle_for_uid(root_uid);
            }

            // Script UI handles intentionally use upward flood search semantics:
            // look in current subtree first, then expand outward through ancestors.
            let child_ref: Result<WidgetRef, String> = vm.with_cx(|cx| {
                if let Some(confine_root) = confine_root {
                    // Confined (isolate) search: the target's subtree first, then the
                    // splash root's subtree. Never the whole tree.
                    let child_ref = cx.widget_tree().find_within(target_uid, &[prop]);
                    if !child_ref.is_empty() {
                        return Ok(child_ref);
                    }
                    return Ok(cx.widget_tree().find_within(confine_root, &[prop]));
                }

                let child_ref = cx.widget_tree().find_flood(target_uid, &[prop]);
                if !child_ref.is_empty() {
                    return Ok(child_ref);
                }

                let mut matches = cx
                    .widget_tree()
                    .find_all_anywhere_including_skipped(&[prop]);
                if matches.len() == 1 {
                    return Ok(matches.pop().unwrap());
                }
                if matches.len() > 1 {
                    let tree = cx.widget_tree();
                    let mut paths: Vec<String> = matches
                        .iter()
                        .map(|m| {
                            tree.scope_path(m.widget_uid())
                                .into_iter()
                                .map(crate::widget_tree::live_id_token)
                                .collect::<Vec<_>>()
                                .join(".")
                        })
                        .collect();
                    paths.sort();
                    return Err(format!(
                        "widget `{}` is ambiguous: {}; write ui.{}",
                        crate::widget_tree::live_id_token(prop),
                        paths.join(", "),
                        paths[0]
                    ));
                }

                Ok(WidgetRef::empty())
            });
            let child_ref = match child_ref {
                Ok(child_ref) => child_ref,
                Err(message) => return script_err_not_found!(vm.trap(), "{}", message),
            };
            if child_ref.is_empty() {
                return script_err_not_found!(vm.trap(), "widget '{:?}' not found in tree", prop);
            }

            let child_uid = child_ref.widget_uid();
            if child_uid == WidgetUid(0) {
                return script_err_not_found!(vm.trap(), "widget has no uid");
            }

            let gc = CxWidgetHandleGc {
                handle: ScriptHandle::ZERO,
                uid: child_uid,
            };
            let child_handle = vm.heap_mut().new_handle(ui_type, Box::new(gc));
            return child_handle.into();
        }

        script_err_not_found!(vm.trap(), "invalid ui handle")
    });

    vm.set_handle_call(ui_type, move |vm, args, method| {
        let pself = script_value!(vm, args.self);
        if let Some(handle) = pself.as_handle() {
            let Some(uid) = vm
                .downcast_handle_gc::<CxWidgetHandleGc>(handle)
                .map(|gc| gc.uid)
            else {
                return script_err_not_found!(vm.trap(), "invalid ui handle");
            };

            let ui_handle = vm.build_ui_handle_for_uid(uid);
            force_set_map_value(vm.heap_mut(), args, id!(ui), ui_handle);

            let caller_thread = vm.thread_id();
            let vm_id = vm.cx_mut().global::<CxWidgetAsync>().current_vm_id;
            if let Some(pending) = vm
                .cx_mut()
                .global::<CxWidgetAsync>()
                .pending_script_to_widget_returns
                .remove(&(vm_id, caller_thread.to_index()))
            {
                if pending.vm_id == vm_id && pending.target_uid == uid && pending.method == method {
                    return pending.result.get();
                }
                vm.cx_mut()
                    .global::<CxWidgetAsync>()
                    .pending_script_to_widget_returns
                    .insert((vm_id, caller_thread.to_index()), pending);
            }

            vm.enqueue_script_to_widget_call(uid, method, args.into());
            return NIL;
        }

        script_err_not_found!(vm.trap(), "invalid ui handle for method call")
    });

    vm.cx_mut()
        .global::<CxWidgetAsync>()
        .ui_handle_types
        .insert(vm_id, ui_type);
}

fn on_widget_script_thread_completed(
    cx: &mut Cx,
    vm_id: SplashVmId,
    thread_id: ScriptThreadId,
    result: ScriptValue,
) -> bool {
    cx.global::<CxWidgetAsync>()
        .pending_script_to_widget_returns
        .remove(&(vm_id, thread_id.to_index()));

    let Some((target_uid, async_id)) = cx
        .global::<CxWidgetAsync>()
        .thread_map
        .remove(&(vm_id, thread_id.to_index()))
    else {
        return false;
    };

    let result = cx.with_script_vm_id(vm_id, |vm| HeldValue::new(vm, result));
    cx.global::<CxWidgetAsync>()
        .done
        .push_back(WidgetAsyncDone {
            vm_id,
            target_uid,
            id: async_id,
            result,
        });
    true
}

fn pump_widget_async(cx: &mut Cx) -> bool {
    let mut progressed = false;

    loop {
        let req = cx
            .global::<CxWidgetAsync>()
            .widget_to_script_calls
            .pop_front();
        if let Some(req) = req {
            progressed = true;
            contain_isolate_panic("widget->script dispatch", || {
                cx.with_script_vm_id(req.vm_id, |vm| {
                    if req.script_fn.as_object() != ScriptObject::ZERO {
                        let ui_handle = vm.build_ui_handle_for_uid(req.target_uid);
                        let call_args = vm.make_call_args_object_with_context(
                            req.source.as_object(),
                            ui_handle,
                            req.args.get(),
                        );
                        let _ = vm.with_instruction_limit(WIDGET_SCRIPT_INSTRUCTION_LIMIT as u64, |vm| {
                            vm.call_with_args_object_with_me(
                                req.script_fn.clone().into(),
                                call_args,
                                req.me.get(),
                            )
                        });
                    }
                });
            });
            continue;
        }

        let req = cx
            .global::<CxWidgetAsync>()
            .script_to_widget_calls
            .pop_front();
        if let Some(req) = req {
            progressed = true;
            contain_isolate_panic("script->widget dispatch", || {
            let ret = cx.with_script_vm_id_thread(req.vm_id, req.caller_thread, |vm| {
                let widget_ref = vm.with_cx(|cx| cx.widget_tree().widget(req.target_uid));
                if widget_ref.is_empty() {
                    error!(
                        "script->widget call {:?} dropped: widget {:?} not in the widget tree (vm {:?})",
                        req.method, req.target_uid, req.vm_id
                    );
                }
                let value = match widget_ref.script_call(vm, req.method, req.args.as_object().into()) {
                    ScriptAsyncResult::Return(value) => value,
                    ScriptAsyncResult::Pending => NIL,
                    ScriptAsyncResult::MethodNotFound => script_err_not_found!(
                        vm.trap(),
                        "widget method {:?} not found for uid {:?}",
                        req.method,
                        req.target_uid
                    ),
                };
                HeldValue::new(vm, value)
            });
            cx.global::<CxWidgetAsync>()
                .pending_script_to_widget_returns
                .insert(
                    (req.vm_id, req.caller_thread.to_index()),
                    ScriptToWidgetReturn {
                        vm_id: req.vm_id,
                        target_uid: req.target_uid,
                        method: req.method,
                        result: ret,
                    },
                );
            let result = cx.with_script_vm_id_thread(req.vm_id, req.caller_thread, |vm| vm.resume());
            let is_paused = cx.with_script_vm_id_thread(req.vm_id, req.caller_thread, |vm| {
                vm.is_paused()
            });
            if !is_paused {
                on_widget_script_thread_completed(cx, req.vm_id, req.caller_thread, result);
            }
            });
            continue;
        }

        let done = cx.global::<CxWidgetAsync>().done.pop_front();
        if let Some(done) = done {
            progressed = true;
            contain_isolate_panic("async result delivery", || {
                cx.with_script_vm_id(done.vm_id, |vm| {
                    let widget_ref = vm.with_cx(|cx| cx.widget_tree().widget(done.target_uid));
                    if widget_ref.is_empty() {
                        error!(
                            "script_result dropped: widget {:?} not in the widget tree (vm {:?})",
                            done.target_uid, done.vm_id
                        );
                    }
                    widget_ref.script_result(vm, done.id, done.result.get());
                });
            });
            continue;
        }

        break;
    }

    // Isolate maintenance — runs on every pump, cheap when idle. Without
    // this, isolated Splash VMs never garbage-collect at all (only the app
    // VM has a paint-loop GC): a 60Hz script host accumulates per-tick
    // objects forever, and dead isolates only reclaimed on the next alloc.
    gc_dead_splash_isolates(cx);
    let state = cx.global::<CxWidgetAsync>();
    if !state.isolated_vms.vms.is_empty() {
        // Round-robin: give at most one isolate a GC opportunity per pump,
        // gated on the heap's own growth heuristic (needs_gc). Mark/sweep
        // runs directly on the parked ScriptVmBase — no Cx install needed.
        // An isolate currently installed on Cx is absent from the map and
        // naturally skipped.
        let mut ids: Vec<u64> = state.isolated_vms.vms.keys().map(|v| v.0).collect();
        ids.sort_unstable();
        let last = state.gc_rr_last;
        let next = ids.iter().copied().find(|id| *id > last).unwrap_or(ids[0]);
        state.gc_rr_last = next;
        if let Some(iso) = state.isolated_vms.vms.get_mut(&SplashVmId(next)) {
            if let Some(bx) = iso.vm.as_mut() {
                if bx.heap_ref().needs_gc() {
                    contain_isolate_panic("isolate gc", || bx.gc_if_needed());
                }
            }
        }
    }

    progressed
}

fn register_task_hooks(cx: &mut Cx) {
    // These static callbacks only borrow the supplied host; context is unused.
    unsafe {
        cx.add_script_task_on_thread_completed_hook(
            makepad_platform::script::std::CxScriptTaskOnThreadCompletedHook::new(
                std::ptr::null_mut(),
                on_widget_script_thread_completed_hook,
            ),
        );
        cx.add_script_task_pump_hook(makepad_platform::script::std::CxScriptTaskPumpHook::new(
            std::ptr::null_mut(),
            pump_widget_async_hook,
        ));
    }
    cx.add_script_timer_dispatch_hook(script_timer_dispatch_hook);
    cx.set_script_heap_route(script_heap_route);
    cx.set_script_vms_any(script_vms_any);
}

/// Whether `f` holds for an isolate's VM (`Cx::set_script_vms_any`): those not running now.
fn script_vms_any(cx: &mut Cx, f: &mut dyn FnMut(&ScriptVmBase) -> bool) -> bool {
    cx.global::<CxWidgetAsync>().isolated_vms.vms.values().filter_map(|isolated| isolated.vm.as_deref()).any(|vm| f(vm))
}

/// Enters the isolate that owns heap `heap_key` for `f` (`Cx::with_vm_of_heap`): a draw made
/// in an isolate takes a live edit there. Not a live isolate's heap (a dead one's, or the main
/// VM's from inside an isolate, which `with_script_vm_id` must not run), or an isolate that is
/// running further up the stack: false, nothing runs.
fn script_heap_route(cx: &mut Cx, heap_key: usize, f: &mut dyn FnMut(&mut ScriptVm)) -> bool {
    let state = cx.global::<CxWidgetAsync>();
    let Some(vm_id) = state.heap_to_vm.get(&heap_key).copied() else { return false };
    if state.current_vm_id != vm_id && !state.isolated_vms.vms.contains_key(&vm_id) {
        return false;
    }
    with_script_vm_id_dyn(cx, vm_id, f);
    true
}

/// Routes a firing script timer to the isolate VM that owns its callback. Without this,
/// `std.start_timeout`/`start_interval` called inside a Splash isolate would run their
/// callbacks on the main VM against the wrong heap.
fn script_timer_dispatch_hook(cx: &mut Cx, timer: &CxScriptTimer, time: ScriptValue) -> bool {
    let heap_key = timer.callback.heap_key();
    if heap_key == 0 {
        return false;
    }
    let vm_id = cx
        .global::<CxWidgetAsync>()
        .heap_to_vm
        .get(&heap_key)
        .copied();
    match vm_id {
        Some(vm_id) => {
            // The same instruction limit as any other isolate entry, so a runaway
            // timer callback can't hang the host.
            contain_isolate_panic("timer callback", || {
                cx.with_script_vm_id(vm_id, |vm| {
                    vm.with_instruction_limit(WIDGET_SCRIPT_INSTRUCTION_LIMIT as u64, |vm| {
                        vm.call(timer.callback.as_object().into(), &[time]);
                    });
                });
            });
            true
        }
        None => {
            // Not a live isolate's heap. The main VM's own timers fall through to the
            // default dispatch; anything else is a stale timer from a dead isolate.
            let main_heap_key = cx.with_vm(|vm| vm.heap_key());
            if heap_key == main_heap_key {
                false
            } else {
                cx.stop_timer(timer.timer);
                let id = timer.id;
                cx.script_data.timers.timers.retain(|t| t.id != id);
                true
            }
        }
    }
}

unsafe extern "C-unwind" fn on_widget_script_thread_completed_hook(
    _context: *mut u8,
    host: &mut ScriptHostContext,
    thread_id: ScriptThreadId,
    result: ScriptValue,
) -> bool {
    host.get_mut::<Cx>()
        .map(|cx| on_widget_script_thread_completed(cx, MAIN_SPLASH_VM_ID, thread_id, result))
        .unwrap_or(false)
}

unsafe extern "C-unwind" fn pump_widget_async_hook(
    _context: *mut u8,
    host: &mut ScriptHostContext,
) -> bool {
    host.get_mut::<Cx>().map(pump_widget_async).unwrap_or(false)
}

/// An isolate installed on `Cx` by [`enter_isolate`]; hand it back to
/// [`leave_isolate`] on the same `Cx`, in the same frame, with nothing of
/// the isolate's left executing.
pub struct IsolateEntry {
    vm_id: SplashVmId,
    previous_vm_id: SplashVmId,
    network_enabled: bool,
    reloaded_at: u64,
    host_reloads: bool,
    outer_std: Option<ScriptStd>,
    outer_vm: Option<Box<ScriptVmBase>>,
}

/// The isolate installed on `Cx` right now — [`MAIN_SPLASH_VM_ID`] when
/// none is: what a host checks after a guest's panic was caught.
pub fn current_splash_vm_id(cx: &mut Cx) -> SplashVmId {
    cx.global::<CxWidgetAsync>().current_vm_id
}

/// A context a host draws or dispatches through that reaches `Cx`: the
/// bare `Cx` of an event, the `Cx2d` of a draw. What [`with_isolate`] is
/// generic over, so one entry serves both.
pub trait IsolateCx {
    fn isolate_cx(&mut self) -> &mut Cx;
}

impl IsolateCx for Cx {
    fn isolate_cx(&mut self) -> &mut Cx {
        self
    }
}

impl<'a> IsolateCx for CxDraw<'a> {
    fn isolate_cx(&mut self) -> &mut Cx {
        self.cx
    }
}

impl<'a, 'b> IsolateCx for Cx2d<'a, 'b> {
    fn isolate_cx(&mut self) -> &mut Cx {
        self.cx.cx
    }
}

/// Run a stretch of HOST code that draws or dispatches events to widgets
/// minted in isolate `vm_id` — a module tile's draw pass, its event
/// delivery — with that isolate installed on `Cx`. While it is, every
/// `cx.with_vm` those widgets make (a lazily created child, a shader
/// compiled on first draw, an `on_click` callback) resolves against the
/// isolate's own heap, the only heap their objects mean anything in.
///
/// The outer VM comes back whether `f` returns or unwinds: a host that
/// catches a guest's panic finds `Cx` as it was, the isolate back in the
/// table, ready to be torn down. The main VM is a no-op entry.
pub fn with_isolate<C: IsolateCx, R>(cx: &mut C, vm_id: SplashVmId, f: impl FnOnce(&mut C) -> R) -> R {
    let entry = enter_isolate(cx.isolate_cx(), vm_id);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(cx)));
    leave_isolate(cx.isolate_cx(), entry);
    match out {
        Ok(out) => out,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// The unguarded half of [`with_isolate`]: install isolate `vm_id` on
/// `Cx` and hand back what [`leave_isolate`] needs to put the outer VM
/// back. For straight-line code that cannot unwind between the two (a
/// test poking at an isolate's objects); a host dispatching or drawing a
/// guest uses `with_isolate`, which also leaves on unwind — an entry
/// dropped without its leave takes the outer VM's heap with it.
pub fn enter_isolate(cx: &mut Cx, vm_id: SplashVmId) -> IsolateEntry {
    let previous_vm_id = cx.global::<CxWidgetAsync>().current_vm_id;
    if vm_id == MAIN_SPLASH_VM_ID || previous_vm_id == vm_id {
        return IsolateEntry { vm_id: MAIN_SPLASH_VM_ID, previous_vm_id, network_enabled: false, reloaded_at: 0, host_reloads: false, outer_std: None, outer_vm: None };
    }
    // Out of the table while installed, exactly as `with_isolate_installed`
    // keeps it: a nested entry by id takes the "already current" path.
    let isolated = cx
        .global::<CxWidgetAsync>()
        .isolated_vms
        .vms
        .remove(&vm_id)
        .unwrap_or_else(|| panic!("missing Splash VM {:?}", vm_id));
    cx.global::<CxWidgetAsync>().current_vm_id = vm_id;
    let outer_std = std::mem::replace(&mut cx.script_data.std, isolated.std);
    let outer_vm = cx.script_vm.take();
    cx.script_vm = isolated.vm;
    // A UI registration the host left part way, then the first entry after a live edit, as in
    // `with_isolate_installed`.
    if take_isolate_ui_in_progress(cx, vm_id) {
        cx.with_vm(finish_isolate_ui);
    }
    let reloaded_at = cx.reload_generation;
    if isolated.reloaded_at != reloaded_at && !isolated.host_reloads {
        contain_isolate_panic("live edit reload", || cx.with_vm(reload_isolate));
    }
    IsolateEntry { vm_id, previous_vm_id, network_enabled: isolated.network_enabled, reloaded_at, host_reloads: isolated.host_reloads, outer_std: Some(outer_std), outer_vm }
}

/// Put the outer VM back. See [`enter_isolate`].
pub fn leave_isolate(cx: &mut Cx, entry: IsolateEntry) {
    if entry.vm_id == MAIN_SPLASH_VM_ID {
        return;
    }
    let IsolateEntry { vm_id, previous_vm_id, network_enabled, reloaded_at, host_reloads, outer_std, outer_vm } = entry;
    let isolate_vm = cx.script_vm.take();
    cx.script_vm = outer_vm;
    let isolate_std = std::mem::replace(&mut cx.script_data.std, outer_std.expect("an entered isolate has an outer std"));
    cx.global::<CxWidgetAsync>().current_vm_id = previous_vm_id;
    cx.global::<CxWidgetAsync>()
        .isolated_vms
        .vms
        .insert(vm_id, IsolatedSplashVm { network_enabled, std: isolate_std, vm: isolate_vm, reloaded_at, host_reloads });
}

#[cfg(test)]
mod isolate_entry_tests {

    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    /// A live edit's reload pass (a later `Cx::reload_generation`) reaches an
    /// isolate at its first entry after the edit: its UI library is registered
    /// again (fresh widget prototypes) and what its host kept for the pass runs
    /// again, in reload mode, once. An isolate whose host follows edits itself
    /// is left to it. A draw made in an isolate is routed back to its heap.
    #[test]
    fn an_isolate_registers_again_on_a_live_edits_reload_pass() {
        thread_local! {
            static RUNS: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
        }
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let (vm_id, by_host) = (cx.alloc_splash_vm(), cx.alloc_splash_vm());
        isolate_reloads_with_host(&mut cx, by_host);
        let button = |cx: &mut Cx, vm_id| cx.with_script_vm_id(vm_id, |vm| {
            let widgets = vm.module(id!(widgets));
            vm.value(widgets, id!(Button)).as_object().expect("premise: the isolate has the library")
        });
        let (before, by_host_before) = (button(&mut cx, vm_id), button(&mut cx, by_host));
        cx.with_script_vm_id(vm_id, |vm| {
            vm.keep_for_reload(live_id!(probe), |vm| RUNS.with(|runs| {
                let (n, in_reload) = runs.get();
                runs.set((n + 1, in_reload + vm.is_reload() as u32));
            }));
        });
        assert_eq!(button(&mut cx, vm_id), before, "no edit: the library it had");
        assert_eq!(RUNS.with(|r| r.get()), (0, 0), "kept, not run");
        cx.reload_generation += 1;
        let after = button(&mut cx, vm_id);
        assert_ne!(after, before, "the pass registered the library again");
        assert_eq!(RUNS.with(|r| r.get()), (1, 1), "the kept registration ran, in reload mode");
        assert_eq!(button(&mut cx, vm_id), after, "once per pass");
        assert_eq!(RUNS.with(|r| r.get()), (1, 1));
        assert_eq!(button(&mut cx, by_host), by_host_before, "its host's to follow");

        let heap = cx.with_script_vm_id(vm_id, |vm| vm.heap_key());
        let mut seen = 0;
        assert!(cx.with_vm_of_heap(heap, &mut |vm| seen = vm.heap_key()));
        assert_eq!(seen, heap, "the isolate's own VM");
        cx.free_splash_vm(vm_id);
        assert!(!cx.with_vm_of_heap(heap, &mut |_| panic!("a freed isolate runs nothing")));
        cx.free_splash_vm(by_host);
    }

    /// A guest panics while its isolate is installed — through
    /// `with_isolate` (a tile's dispatch) and through
    /// `with_script_vm_id` (a host's own entry): the main VM is
    /// back on `Cx`, the isolate is back in the table with its heap, and
    /// both keep working.
    #[test]
    fn a_panic_inside_an_installed_isolate_restores_the_outer_vm_and_the_table() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let main_heap = cx.with_vm(|vm| vm.heap_key());
        let vm_id = cx.alloc_splash_vm();
        let isolate_heap = cx.with_script_vm_id(vm_id, |vm| vm.heap_key());

        let caught = catch_unwind(AssertUnwindSafe(|| {
            with_isolate(&mut cx, vm_id, |cx| {
                assert_eq!(cx.global::<CxWidgetAsync>().current_vm_id, vm_id);
                cx.with_vm(|_vm| panic!("a guest widget panicked"))
            })
        }));
        assert!(caught.is_err());
        assert!(!cx.is_script_vm_held(), "the outer VM is back on Cx");
        assert_eq!(cx.global::<CxWidgetAsync>().current_vm_id, MAIN_SPLASH_VM_ID);
        assert_eq!(cx.with_vm(|vm| vm.heap_key()), main_heap, "and it is the main VM");
        assert_eq!(cx.with_script_vm_id(vm_id, |vm| vm.heap_key()), isolate_heap, "the isolate kept its heap");

        let caught = catch_unwind(AssertUnwindSafe(|| {
            cx.with_script_vm_id(vm_id, |_vm| panic!("a module's create panicked"))
        }));
        assert!(caught.is_err());
        assert!(!cx.is_script_vm_held());
        assert_eq!(cx.global::<CxWidgetAsync>().current_vm_id, MAIN_SPLASH_VM_ID);
        assert_eq!(cx.with_vm(|vm| vm.heap_key()), main_heap);
        assert_eq!(cx.with_script_vm_id(vm_id, |vm| vm.heap_key()), isolate_heap);

        // A draw context is a valid entry too, and the isolate can be torn
        // down in the host's order afterwards.
        let event = DrawEvent::default();
        {
            let mut draw = CxDraw::new(&mut cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            let caught = catch_unwind(AssertUnwindSafe(|| {
                with_isolate(&mut cx2d, vm_id, |cx| cx.with_vm(|_vm| panic!("a guest's draw panicked")))
            }));
            assert!(caught.is_err());
            assert_eq!(cx2d.global::<CxWidgetAsync>().current_vm_id, MAIN_SPLASH_VM_ID);
        }
        cx.free_splash_vm(vm_id);
        cx.with_vm(|vm| {
            let value = vm.eval(script! { 2 * 21 });
            assert_eq!(value.as_f64(), Some(42.0));
        });
    }

    #[test]
    fn a_host_registered_mod_reaches_isolates_allocated_after_it() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        static INSTALLS: AtomicUsize = AtomicUsize::new(0);

        fn install_probe(vm: &mut ScriptVm) {
            INSTALLS.fetch_add(1, AtomicOrdering::Relaxed);
            vm.eval(crate::makepad_script::script! {
                mod.host_probe = 7
            });
        }

        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);

        // Allocated before any registration: the host mod is not in its
        // namespace, and nothing was installed.
        let before = cx.alloc_splash_vm();
        let missing = cx.with_script_vm_id(before, |vm| {
            vm.eval(crate::makepad_script::script! { mod.host_probe })
        });
        assert!(missing.is_err(), "probe must not exist before registration");
        assert_eq!(INSTALLS.load(AtomicOrdering::Relaxed), 0);

        register_splash_isolate_mod(install_probe);

        // Allocated after: the mod runs for this isolate (with its UI modules, on first touch at
        // the latest) and its binding resolves.
        let after = cx.alloc_splash_vm();
        let found = cx.with_script_vm_id(after, |vm| {
            vm.eval(crate::makepad_script::script! { mod.host_probe })
        });
        assert!(!found.is_err(), "probe must resolve inside the isolate");
        assert_eq!(INSTALLS.load(AtomicOrdering::Relaxed), 1);

        // Each further isolate gets its own install — the registry is not
        // consumed by the first allocation.
        let third = cx.alloc_splash_vm();
        cx.with_script_vm_id(third, |vm| vm.eval(crate::makepad_script::script! { mod.host_probe }));
        assert_eq!(INSTALLS.load(AtomicOrdering::Relaxed), 2);

        // The earlier isolate is unchanged: mods are taken at allocation.
        let still_missing = cx.with_script_vm_id(before, |vm| {
            vm.eval(crate::makepad_script::script! { mod.host_probe })
        });
        assert!(still_missing.is_err());

        cx.free_splash_vm(before);
        cx.free_splash_vm(after);
        cx.free_splash_vm(third);
    }

    #[test]
    fn entering_an_isolate_makes_it_the_current_vm_and_leaving_restores_the_outer_one() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let main_heap = cx.with_vm(|vm| vm.heap_key());
        let vm_id = cx.alloc_splash_vm();
        let isolate_heap = cx.with_script_vm_id(vm_id, |vm| vm.heap_key());
        assert_ne!(main_heap, isolate_heap);
        // Entered: a plain `with_vm` — what a lazily created child or a
        // first-draw shader compile does — runs in the isolate's heap.
        let entry = enter_isolate(&mut cx, vm_id);
        assert_eq!(cx.global::<CxWidgetAsync>().current_vm_id, vm_id);
        assert_eq!(cx.with_vm(|vm| vm.heap_key()), isolate_heap);
        // A nested entry by id takes the already-current path.
        assert_eq!(cx.with_script_vm_id(vm_id, |vm| vm.heap_key()), isolate_heap);
        leave_isolate(&mut cx, entry);
        assert_eq!(cx.global::<CxWidgetAsync>().current_vm_id, MAIN_SPLASH_VM_ID);
        assert_eq!(cx.with_vm(|vm| vm.heap_key()), main_heap);
        // The isolate is whole again: usable by id, and freeable.
        assert_eq!(cx.with_script_vm_id(vm_id, |vm| vm.heap_key()), isolate_heap);
        // The main VM is a no-op entry.
        let entry = enter_isolate(&mut cx, MAIN_SPLASH_VM_ID);
        assert_eq!(cx.with_vm(|vm| vm.heap_key()), main_heap);
        leave_isolate(&mut cx, entry);
        cx.free_splash_vm(vm_id);
    }
}

#[cfg(test)]
mod isolate_tests {

    use super::*;
    use crate::splash::Splash;
    use crate::view::View;
    use crate::widget_tree::set_ui_root;

    const BODY: &str = r#"
    let items = []
    fn go(){ ui.item_list.render() }
    fn load(){
        host.request("matrix.room_threads", {limit: 20}, fn(r){
            if r.is_ok {
                items = r.data.threads
                ui.header.set_text("" + items.len() + " threads")
            } else {
                items = []
                ui.header.set_text(r.error)
            }
            ui.item_list.render()
        })
    }
    header := Label{ text: "Loading" }
    item_list := View{ height: Fit, on_render: || {
        for it in items {
            Label{text: it.body}
        }
    } }
"#;

    fn item_list_children(cx: &Cx, host: &WidgetRef) -> usize {
        let w = host.widget(cx, &[live_id!(item_list)]);
        w.borrow::<View>()
            .map(|v| v.children.len())
            .unwrap_or(usize::MAX)
    }

    fn render_cycle(cx: &mut Cx, host: &WidgetRef, json: &'static str) -> usize {
        let splash = host.widget(cx, &[live_id!(splash)]);
        let item_list = host.widget(cx, &[live_id!(item_list)]);
        render_cycle_no_heal(cx, &splash, &item_list, json)
    }

    // Triggers load()+respond via direct WidgetRefs, so no tree lookup can
    // re-seed graph nodes; this is the app's own timer/callback view of the
    // world, where ui.X resolution relies purely on the existing graph.
    fn render_cycle_no_heal(
        cx: &mut Cx,
        splash: &WidgetRef,
        item_list: &WidgetRef,
        json: &'static str,
    ) -> usize {
        let called = splash
            .borrow_mut::<Splash>()
            .expect("splash widget")
            .call_script_fn(cx, live_id!(load), &[]);
        assert!(called, "load() not found in body scope");
        pump_widget_async(cx);
        let reqs = crate::splash_host::take_splash_host_requests();
        assert_eq!(reqs.len(), 1, "expected one bridge request");
        let req = &reqs[0];
        let outcome =
            crate::splash_host::splash_host_respond(cx, req.heap_key, req.req_id, Ok(json));
        eprintln!("### respond outcome {outcome:?}");
        pump_widget_async(cx);
        item_list
            .borrow::<View>()
            .map(|v| v.children.len())
            .unwrap_or(usize::MAX)
    }

    #[test]
    fn force_stop_midflight_then_stale_respond() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let template = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{ splash := Splash{} }
            });
            let obj = v.as_object().expect("template did not eval to an object");
            vm.new_object_ref(obj)
        });
        let pane = cx.with_vm(|vm| {
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                View{ height: Fit }
            });
            WidgetRef::script_from_value(vm, v)
        });
        set_ui_root(&mut cx, &pane);
        let pane_uid = pane.widget_uid();

        // run 1: request made, ANSWERED, but force-stopped BEFORE the pump
        // (paused callback thread + queued ui calls die with the isolate)
        let host = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template.as_object().into()));
        cx.widget_tree_insert_child_deep(pane_uid, live_id!(apphost), host.clone());
        host.widget(&cx, &[live_id!(splash)]).set_text(&mut cx, BODY);
        let hk1 = host
            .widget(&cx, &[live_id!(splash)])
            .borrow_mut::<Splash>()
            .unwrap()
            .isolate_heap_key(&mut cx)
            .unwrap();
        let _ = host.widget(&cx, &[live_id!(item_list)]);
        assert!(host
            .widget(&cx, &[live_id!(splash)])
            .borrow_mut::<Splash>()
            .unwrap()
            .call_script_fn(&mut cx, live_id!(load), &[]));
        pump_widget_async(&mut cx);
        // a second request left UNANSWERED at force stop
        assert!(host
            .widget(&cx, &[live_id!(splash)])
            .borrow_mut::<Splash>()
            .unwrap()
            .call_script_fn(&mut cx, live_id!(load), &[]));
        pump_widget_async(&mut cx);
        let reqs = crate::splash_host::take_splash_host_requests();
        assert_eq!(reqs.len(), 2);
        // answer the FIRST request but do NOT pump: callback ran, ui calls queued
        let outcome = crate::splash_host::splash_host_respond(
            &mut cx,
            reqs[0].heap_key,
            reqs[0].req_id,
            Ok(r#"{"threads":[{"sender":"a","body":"one"}]}"#),
        );
        eprintln!("### run1 respond outcome {outcome:?}");
        // force stop NOW, queues still full
        drop(host);
        gc_dead_splash_isolates(&mut cx);

        // run 2
        let host2 = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template.as_object().into()));
        cx.widget_tree_insert_child_deep(pane_uid, live_id!(apphost), host2.clone());
        host2.widget(&cx, &[live_id!(splash)]).set_text(&mut cx, BODY);
        let hk2 = host2
            .widget(&cx, &[live_id!(splash)])
            .borrow_mut::<Splash>()
            .unwrap()
            .isolate_heap_key(&mut cx)
            .unwrap();
        eprintln!("### heap reuse: hk1={hk1} hk2={hk2} same={}", hk1 == hk2);

        // stale respond for the dead isolate's outstanding request arrives late
        let stale = crate::splash_host::splash_host_respond(
            &mut cx,
            reqs[1].heap_key,
            reqs[1].req_id,
            Ok(r#"{"threads":[{"sender":"z","body":"stale"}]}"#),
        );
        eprintln!("### stale respond outcome {stale:?}");

        let n = render_cycle(
            &mut cx,
            &host2,
            r#"{"threads":[{"sender":"a","body":"one"},{"sender":"b","body":"two"}]}"#,
        );
        eprintln!("### run2 children = {n}");

        // GC everything hard, hunting the cross-heap panic
        cx.with_vm(|vm| vm.gc());
        let ids: Vec<SplashVmId> = cx
            .global::<CxWidgetAsync>()
            .isolated_vms
            .vms
            .keys()
            .copied()
            .collect();
        for id in ids {
            if let Some(iso) = cx.global::<CxWidgetAsync>().isolated_vms.vms.get_mut(&id) {
                if let Some(bx) = iso.vm.as_mut() {
                    bx.heap.mark(&bx.threads, &bx.code);
                    bx.heap.sweep(false);
                }
            }
        }
        assert_eq!(n, 2, "run2 render did not commit");
    }

    #[test]
    fn pane_refresh_drops_inserted_host() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let template = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{ splash := Splash{} }
            });
            let obj = v.as_object().expect("template did not eval to an object");
            vm.new_object_ref(obj)
        });
        let pane = cx.with_vm(|vm| {
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                View{ height: Fit }
            });
            WidgetRef::script_from_value(vm, v)
        });
        set_ui_root(&mut cx, &pane);
        let pane_uid = pane.widget_uid();

        let host = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template.as_object().into()));
        cx.widget_tree_insert_child_deep(pane_uid, live_id!(apphost), host.clone());
        host.widget(&cx, &[live_id!(splash)]).set_text(&mut cx, BODY);
        let n = render_cycle(
            &mut cx,
            &host,
            r#"{"threads":[{"sender":"a","body":"one"}]}"#,
        );
        eprintln!("### baseline children = {n}");
        assert_eq!(n, 1);

        let splash = host.widget(&cx, &[live_id!(splash)]);
        let item_list = host.widget(&cx, &[live_id!(item_list)]);

        let topdown0 = !cx
            .widget_tree()
            .find_within(pane_uid, &[live_id!(item_list)])
            .is_empty();
        eprintln!("### before flush: top-down find from pane: {topdown0}");

        // what teardown / any structural event does to the owner
        cx.widget_tree_mark_dirty(pane_uid);
        // any flood search flushes with mark_structure_dirty=true
        let _ = cx.widget_tree().root_uid();
        let in_graph = !cx.widget_tree().widget(splash.widget_uid()).is_empty();
        let topdown = !cx
            .widget_tree()
            .find_within(pane_uid, &[live_id!(item_list)])
            .is_empty();
        eprintln!("### after flush: splash node in graph: {in_graph}, top-down find from pane: {topdown}");

        let n = render_cycle_no_heal(
            &mut cx,
            &splash,
            &item_list,
            r#"{"threads":[{"sender":"a","body":"one"},{"sender":"b","body":"two"}]}"#,
        );
        eprintln!("### after pane refresh children = {n}");
        assert_eq!(n, 2, "render after pane refresh did not commit");
    }

    #[test]
    fn second_isolate_render_commits() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let template = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{ splash := Splash{} }
            });
            let obj = v.as_object().expect("template did not eval to an object");
            vm.new_object_ref(obj)
        });
        let pane = cx.with_vm(|vm| {
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                View{ height: Fit }
            });
            WidgetRef::script_from_value(vm, v)
        });
        set_ui_root(&mut cx, &pane);
        let pane_uid = pane.widget_uid();

        for run in 0..2 {
            let host =
                cx.with_vm(|vm| WidgetRef::script_from_value(vm, template.as_object().into()));
            cx.widget_tree_insert_child_deep(pane_uid, live_id!(apphost), host.clone());
            host.widget(&cx, &[live_id!(splash)]).set_text(&mut cx, BODY);
            let hk = host
                .widget(&cx, &[live_id!(splash)])
                .borrow_mut::<Splash>()
                .unwrap()
                .isolate_heap_key(&mut cx);
            eprintln!("### run {run}: heap_key {hk:?}");
            // warm the graph so the confined ui getter can resolve while the
            // Splash itself is mut-borrowed below
            let warm = host.widget(&cx, &[live_id!(item_list)]);
            assert!(!warm.is_empty(), "run {run}: item_list not found in tree");
            drop(warm);
            let called = host
                .widget(&cx, &[live_id!(splash)])
                .borrow_mut::<Splash>()
                .expect("splash widget")
                .call_script_fn(&mut cx, live_id!(load), &[]);
            assert!(called, "run {run}: load() not found in body scope");
            pump_widget_async(&mut cx);
            // the host drains and answers the bridge request, like robrix's broker
            let reqs = crate::splash_host::take_splash_host_requests();
            assert_eq!(reqs.len(), 1, "run {run}: expected one bridge request");
            let req = &reqs[0];
            let outcome = crate::splash_host::splash_host_respond(
                &mut cx,
                req.heap_key,
                req.req_id,
                Ok(r#"{"threads":[{"sender":"a","body":"one"},{"sender":"b","body":"two"}]}"#),
            );
            eprintln!("### run {run}: respond outcome {outcome:?}");
            pump_widget_async(&mut cx);
            let n = item_list_children(&cx, &host);
            eprintln!("### run {run}: item_list children = {n}");

            // aggressive GC afterwards, hunting the cross-heap panic
            cx.with_vm(|vm| vm.gc());
            {
                let state = cx.global::<CxWidgetAsync>();
                let ids: Vec<SplashVmId> =
                    state.isolated_vms.vms.keys().copied().collect();
                for id in ids {
                    if let Some(iso) = cx
                        .global::<CxWidgetAsync>()
                        .isolated_vms
                        .vms
                        .get_mut(&id)
                    {
                        if let Some(bx) = iso.vm.as_mut() {
                            bx.heap.mark(&bx.threads, &bx.code);
                            bx.heap.sweep(false);
                        }
                    }
                }
            }
            assert!(n >= 1, "run {run}: render did not commit (children={n})");

            // force stop: drop every strong ref, then reclaim like the
            // end-of-cycle pump does
            drop(host);
            pump_widget_async(&mut cx);
        }
    }
}

#[cfg(test)]
mod host_io_tests {
    use super::*;
    use crate::splash_host::{take_splash_host_requests, splash_host_respond, SplashRespondOutcome};

    fn context() -> (Box<Cx>, SplashVmId) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let id = cx.alloc_splash_vm_with_host_io();
        (Box::new(cx), id)
    }

    #[test]
    fn base_widgets_are_the_library_prefix_in_a_smaller_isolate() {
        fn family(vm: &mut ScriptVm) {
            crate::script_eval!(vm, {
                mod.widgets.TestFamilyWidget = mod.widgets.View {}
            });
        }

        let (mut cx, initial) = context();
        cx.free_splash_vm(initial);
        // Exercise the family hook without making the core depend on a family crate.
        cx.global::<crate::widget_hooks::IsolateFamilies>()
            .0
            .push(family);
        // Learn this shape first, so the measured base expands under the error sink.
        let warm = cx.alloc_splash_vm_with_widgets(false, IsolateWidgets::Base);
        cx.free_splash_vm(warm);
        let base = cx.alloc_splash_vm_with_widgets(false, IsolateWidgets::Base);
        let library = cx.alloc_splash_vm_with_widgets(false, IsolateWidgets::Library);
        let inspect = |cx: &mut Cx, id, set| {
            with_isolate(cx, id, |cx| {
                cx.with_vm(|vm| {
                    vm.capture_errors();
                    let widgets = vm.module(id!(widgets));
                    for name in [id!(View), id!(Label), id!(Button), id!(Image), id!(Icon)] {
                        assert!(
                            vm.value(widgets, name).as_object().is_some(),
                            "{set:?}: {name} resolves"
                        );
                    }
                    let errors = vm.take_errors();
                    assert!(
                        errors.is_empty(),
                        "{set:?}: registration has zero script errors: {errors:?}"
                    );
                    let names: Vec<_> = vm
                        .heap()
                        .map_entries(widgets)
                        .iter()
                        .map(|e| e.key)
                        .collect();
                    for name in [id!(TestFamilyWidget), id!(TextInput), id!(Slider)] {
                        assert_eq!(
                            names.contains(&name.into()),
                            set == IsolateWidgets::Library,
                            "{set:?}: {name} belongs only to the library"
                        );
                    }
                    let button = vm
                        .value(widgets, id!(Button))
                        .as_object()
                        .expect("Button template");
                    let mut keys: Vec<_> = vm
                        .heap()
                        .proto_map_entries(button)
                        .iter()
                        .filter_map(|e| e.key.as_id())
                        .collect();
                    keys.sort_by_key(|key| key.0);
                    keys.dedup();
                    assert!(
                        !keys.is_empty(),
                        "{set:?}: Button has property keys to compare"
                    );
                    vm.gc();
                    (keys, vm.heap().alloced_object_ids().len())
                })
            })
        };
        let (base_keys, base_live) = inspect(&mut cx, base, IsolateWidgets::Base);
        let (library_keys, library_live) = inspect(&mut cx, library, IsolateWidgets::Library);
        assert_eq!(
            base_keys, library_keys,
            "Button uses the same definition in both sets"
        );
        assert!(
            base_live < library_live,
            "base live objects ({base_live}) are fewer than library ({library_live})"
        );
        cx.with_script_vm_id(base, |vm| {
            vm.capture_errors();
            let value = vm.eval(script! { mod.widgets.Slider });
            let errors = vm.take_errors();
            assert!(
                value.is_err() && errors.iter().any(|e| e.contains("not found")),
                "an excluded widget is an ordinary unknown name: {errors:?}"
            );
            let widgets = vm.module(id!(widgets));
            assert!(
                !vm.heap()
                    .map_entries(widgets)
                    .iter()
                    .any(|e| e.key == id!(Slider).into()),
                "unknown names never expand the library"
            );
        });
        cx.free_splash_vm(base);
        cx.free_splash_vm(library);
    }

    #[test]
    fn host_io_blocks_native_http_sockets_listeners_and_resource_aliases() {
        let (mut cx, id) = context();
        // With a network runtime present, only the host I/O guards can refuse these calls.
        with_isolate(&mut *cx, id, |cx| {
            let net = cx.net.clone();
            cx.script_data.std.net = Some(net.into());
        });
        let attempts = [
            script! { mod.net.http_request(mod.net.HttpRequest{url: "http://127.0.0.1:9/private"}, mod.net.HttpEvents{}) },
            script! { mod.net.socket_stream(mod.net.SocketStreamOptions{host: "127.0.0.1", port: "9"}) },
            script! { mod.net.web_socket(mod.net.HttpRequest{url: "ws://127.0.0.1:9/private"}, mod.net.WebSocketEvents{}) },
            script! { mod.net.http_server(mod.net.HttpServerOptions{listen: "127.0.0.1:0"}, mod.net.HttpServerEvents{}) },
            script! { mod.prelude.widgets.http_resource("http://127.0.0.1:9/private") },
            script! { mod.prelude.widgets.file_resource("/etc/passwd") },
            script! { mod.prelude.widgets.crate_resource("self:../../private") },
            script! { mod.prelude.widgets.load_all_resources() },
        ];
        for attempt in attempts {
            cx.with_script_vm_id(id, |vm| {
                vm.capture_errors();
                let value = vm.eval(attempt);
                let errors = vm.take_errors();
                assert!(value.is_err(), "native I/O unexpectedly succeeded");
                assert!(errors.iter().any(|error| error.contains("host request")), "unexpected errors: {errors:?}");
            });
        }
        with_isolate(&mut *cx, id, |cx| {
            assert!(cx.script_data.std.data.http_requests.is_empty());
            assert!(cx.script_data.std.data.http_servers.is_empty());
            assert!(cx.script_data.std.data.socket_streams.borrow().is_empty());
        });
        cx.free_splash_vm(id);
    }

    #[test]
    fn host_io_restriction_is_inherited_and_restored_after_unwind() {
        let (mut cx, id) = context();
        assert!(!cx.script_data.std.host_io_only());
        let child = with_isolate(&mut *cx, id, |cx| cx.alloc_splash_vm_with_network(true));
        with_isolate(&mut *cx, child, |cx| {
            assert!(cx.script_data.std.host_io_only());
            assert!(cx.script_data.std.net.is_none());
        });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_isolate(&mut *cx, id, |cx| {
                assert!(cx.script_data.std.host_io_only());
                panic!("test guest panic");
            });
        }));
        assert!(result.is_err());
        assert!(!cx.script_data.std.host_io_only());
        cx.free_splash_vm(child);
        cx.free_splash_vm(id);
    }

    #[test]
    fn host_io_does_not_register_widgets_with_ambient_io() {
        let (mut cx, id) = context();
        cx.with_script_vm_id(id, |vm| {
            let widgets = vm.module(id!(widgets));
            // WebView and MapView are families of their own, which an isolate
            // never gets (widgets_mod_with_host_io registers the core only).
            let keys = [
                id!(Window), id!(WindowBase), id!(ScreenCap), id!(ScreenCapBase),
                id!(WindowMenu), id!(WindowMenuBase), id!(CachedWidget),
                id!(WebView), id!(WebViewBase), id!(MapView), id!(MapViewBase),
            ];
            for key in keys {
                let value = vm.value(widgets, key);
                assert!(value.is_nil() || value.is_err(), "unsafe widget {key:?} was registered");
            }
        });
        cx.free_splash_vm(id);
    }

    #[test]
    fn installed_isolates_run_without_an_implicit_budget() {
        let (mut cx, id) = context();
        // An isolate's work is never cut off by a wall clock nobody asked for.
        let budgeted = with_isolate(&mut *cx, id, |cx| {
            cx.with_script_vm_id(id, |vm| vm.run_budget().is_some())
        });
        assert!(!budgeted);
        cx.free_splash_vm(id);
    }

    #[test]
    fn host_io_keeps_host_requests_and_exact_isolate_completion() {
        let (mut cx, id) = context();
        let heap = cx.with_script_vm_id(id, |vm| vm.heap_key());
        crate::splash_host::set_tag_for_heap(heap, Some("trusted-host-tag".into()));
        cx.with_script_vm_id(id, |vm| {
            vm.eval(script! {
                mod.observed = false
                mod.host.request("network.http", {url: "https://example.invalid/"}, fn(result) {
                    mod.observed = result.is_ok
                })
            });
            assert!(vm.take_errors().is_empty());
        });
        let requests = take_splash_host_requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.app_tag, "trusted-host-tag");
        assert_eq!(request.heap_key, heap);
        assert_eq!(request.service, "network.http");
        assert_eq!(splash_host_respond(&mut cx, heap, request.req_id, Ok("{}")), SplashRespondOutcome::Delivered);
        cx.with_script_vm_id(id, |vm| {
            assert_eq!(vm.eval(script! { mod.observed }), true.into());
        });
        assert_eq!(splash_host_respond(&mut cx, heap, request.req_id, Ok("{}")), SplashRespondOutcome::NoCallback);
        cx.free_splash_vm(id);
    }
}

#[cfg(test)]
mod async_rooting_tests {
    use super::*;
    use crate::view::View;
    use crate::widget_tree::set_ui_root;

    /// A render's `me` and its result wait in the async queues for the next
    /// pump; a collection in between (the paint loop's, after the draw that
    /// rendered) must leave them alive. The counter app's `on_startup` render
    /// hit exactly this: delivery panicked on a freed `me`.
    #[test]
    fn a_collection_between_render_and_delivery_keeps_me_and_result() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let view = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                View{ height: Fit, on_render: || {
                    Label{text: "one"}
                    Label{text: "two"}
                } }
            });
            WidgetRef::script_from_value(vm, v)
        });
        set_ui_root(&mut cx, &view);
        for round in 0..3 {
            let rendered = cx.with_vm(|vm| {
                let args = vm.new_object();
                view.script_call(vm, live_id!(render), args.into())
            });
            assert!(matches!(rendered, ScriptAsyncResult::Return(_)), "round {round}: {rendered:?}");
            // The paint loop's collection, before the pump delivers.
            cx.with_vm(|vm| vm.gc());
            // Churn so the freed slots are taken again.
            cx.with_vm(|vm| {
                for _ in 0..4096 {
                    vm.new_object();
                }
            });
            assert!(pump_widget_async(&mut cx), "round {round}: nothing delivered");
            let children = view.borrow::<View>().map(|v| v.children.len()).unwrap_or(usize::MAX);
            assert_eq!(children, 2, "round {round}: the render did not commit");
        }
    }
}

#[cfg(test)]
mod isolate_shader_tests {
    use super::*;

    /// A draw shader compiled while an isolate is installed keeps its source
    /// object in the isolate's heap. The frame builder and the damage tracker
    /// asked about that source later, with the app VM installed: the app heap
    /// answered for its own slot of that index (CUT panicked every frame on
    /// "use-after-free index=8996 ref_gen=0 slot_gen=4"). The answers are read
    /// at registration now, where the owning VM is the installed one.
    #[test]
    fn an_isolate_shaders_source_is_answered_from_its_own_heap() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let main_heap = cx.with_vm(|vm| vm.heap_key());
        let vm_id = cx.alloc_splash_vm();
        let shader = with_isolate(&mut cx, vm_id, |cx| {
            cx.with_vm(|vm| {
                let v = vm.eval(crate::makepad_script::script! {
                    use mod.prelude.widgets.*
                    use mod.draw.*
                    DrawQuad{ pixel: fn() { return vec4(0.25, 0.5, 0.75, 1.0) } }
                });
                vm.capture_errors();
                let quad = DrawQuad::script_from_value(vm, v);
                let errors = vm.take_errors();
                quad.draw_vars.draw_shader_id.unwrap_or_else(|| panic!("the isolate's quad does not compile: {errors:?}"))
            })
        });
        let mapping = &cx.draw_shaders[shader.index].mapping;
        assert_ne!(mapping.source.heap_key(), main_heap, "the source lives in the isolate");
        // Answered at registration, in the isolate: the frame needs no VM to ask.
        if cfg!(target_os = "macos") {
            let loc = mapping.source_loc.clone().expect("the HAL's trace location");
            // the first of pixel, fragment, vertex (draw_shader.rs read_source): this quad's own
            // pixel fn, written above
            assert!(loc.contains("widget_async.rs"), "the quad's pixel fn, not {loc}");
        }
        cx.free_splash_vm(vm_id);
    }
}

#[cfg(test)]
mod isolate_teardown_tests {
    use super::*;

    /// A dropped Splash's isolate (heap, std, timers) is gone after the same cycle's pump, the
    /// one that follows every event dispatch: nothing else has to allocate first.
    #[test]
    fn a_dropped_splash_frees_its_isolate_at_the_next_pump() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let splash = cx.with_vm(|vm| {
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                Splash{}
            });
            WidgetRef::script_from_value(vm, v)
        });
        splash.set_text(&mut cx, "Label{text: \"hi\"}");
        let isolates = cx.global::<CxWidgetAsync>().isolated_vms.vms.len();
        assert_eq!(isolates, 1);
        drop(splash);
        pump_widget_async(&mut cx);
        // (the heap count is process-wide and other tests run alongside: the table is this Cx's)
        assert_eq!(cx.global::<CxWidgetAsync>().isolated_vms.vms.len(), 0, "the isolate (its heap with it) is gone");
        assert!(cx.global::<CxWidgetAsync>().heap_to_vm.is_empty(), "and its heap routes nowhere");
    }
}

#[cfg(test)]
mod lazy_isolate_type_tests {
    use super::*;

    /// A second isolate's widget library is lazy; host Rust code that turns one of its types
    /// into a script value before any script touched `mod` (a second sandbox game, a split
    /// screen's second world) finds the type registered: the registry miss expands the library.
    #[test]
    fn a_second_isolate_converts_a_library_enum_before_any_script_ran() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let first = cx.alloc_splash_vm();
        let second = cx.alloc_splash_vm();
        for id in [first, second] {
            let value = cx.with_script_vm_id(id, |vm| crate::makepad_draw::shader::draw_text::TextOverflow::Ellipsis.script_to_value(vm));
            assert!(!value.is_nil() && !value.is_err(), "{value:?}");
            let flow = cx.with_script_vm_id(id, |vm| crate::makepad_draw::Flow::Down.script_to_value(vm));
            assert!(!flow.is_err(), "{flow:?}");
        }
        cx.free_splash_vm(first);
        cx.free_splash_vm(second);
    }
}

#[cfg(test)]
mod isolate_ui_step_tests {
    //! A host that registers an isolate's UI a unit at a time (`step_isolate_ui`) ends with the UI
    //! a whole registration makes, however the steps are interrupted. Base is the game host's set;
    //! Library is what Splash mini-apps and Stage slides allocate.

    use super::*;
    use crate::makepad_script::makepad_error_log::{set_log_tap, LogLevel};
    use crate::makepad_script::vm_ctl::{ScriptCtlReq, CTL_GC};

    thread_local! {
        static ERRORS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    fn count_errors(_: &str, level: LogLevel) {
        if matches!(level, LogLevel::Error | LogLevel::Panic) {
            ERRORS.with(|errors| errors.set(errors.get() + 1));
        }
    }

    /// What a registered isolate UI is made of.
    #[derive(Debug, PartialEq)]
    struct Ui {
        mod_keys: Vec<LiveId>,
        widgets: Vec<LiveId>,
        prelude_widgets: Vec<LiveId>,
        widgets_internal: Vec<LiveId>,
        theme_keys: Vec<LiveId>,
        /// the theme's colours and numbers
        theme_values: Vec<(LiveId, Option<u32>, Option<u64>)>,
        types: usize,
    }

    fn keys(vm: &mut ScriptVm, object: ScriptObject) -> Vec<LiveId> {
        let mut keys: Vec<LiveId> = vm.heap().map_entries(object).iter().filter_map(|e| e.key.as_id()).collect();
        keys.sort_unstable();
        keys
    }

    /// The isolate's UI, read in it (an entry, so a lazy one registers on the read).
    fn ui_of(cx: &mut Cx, vm_id: SplashVmId) -> Ui {
        cx.with_script_vm_id(vm_id, |vm| {
            let widgets = vm.module(id!(widgets));
            let prelude = vm.module(id!(prelude));
            let theme = vm.module(id!(theme));
            let modules = vm.modules();
            let prelude_widgets = vm.value(prelude, id!(widgets)).as_object().expect("mod.prelude.widgets");
            let widgets_internal = vm.value(prelude, id!(widgets_internal)).as_object().expect("mod.prelude.widgets_internal");
            let theme_values = vm.heap().map_entries(theme).iter().filter_map(|e| {
                let (colour, number) = (e.value.as_color(), e.value.as_f64().map(f64::to_bits));
                (colour.is_some() || number.is_some()).then(|| (e.key.as_id().unwrap_or_default(), colour, number))
            }).collect();
            assert!(!vm.has_lazy_modules(), "the UI is registered once read");
            Ui {
                mod_keys: keys(vm, modules),
                widgets: keys(vm, widgets),
                prelude_widgets: keys(vm, prelude_widgets),
                widgets_internal: keys(vm, widgets_internal),
                theme_keys: keys(vm, theme),
                theme_values,
                types: vm.heap().registered_type_count(),
            }
        })
    }

    /// A collection of the parked isolate, as the pump's round-robin makes between frames.
    fn collect(cx: &mut Cx, vm_id: SplashVmId) {
        let bx = cx.global::<CxWidgetAsync>().isolated_vms.vms.get_mut(&vm_id).and_then(|isolated| isolated.vm.as_mut()).expect("a parked isolate");
        bx.vm_ctl(&mut ScriptCtlReq::new(CTL_GC));
    }

    fn unit_names(cx: &mut Cx, vm_id: SplashVmId) -> Vec<&'static str> {
        cx.with_script_vm_id(vm_id, |vm| isolate_ui_units_of(vm).expect("an isolate's record").iter().map(|unit| unit.name).collect())
    }

    fn step(cx: &mut Cx, vm_id: SplashVmId, units: usize) {
        for _ in 0..units {
            assert!(cx.step_isolate_ui(vm_id), "units remain");
            collect(cx, vm_id);
        }
    }

    fn restyle(cx: &mut Cx, vm_id: SplashVmId, sheet: &crate::desktop_style::StyleSheet) {
        // As a Splash restyles its isolate (`Splash::apply_source`).
        if !restyle_lazy_isolate(cx, vm_id, sheet) {
            let loader = isolate_loader(cx, vm_id);
            cx.with_script_vm_id(vm_id, |vm| {
                crate::desktop_style::install(vm, sheet.clone());
                restyle_isolate(vm, splash_theme(), splash_theme_script(), loader.as_ref());
            });
        }
    }

    fn stepping_registers_the_whole_ui(set: IsolateWidgets) -> Vec<&'static str> {
        set_log_tap(Some(count_errors));
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        // The first isolate of a shape registers eagerly and teaches the keys; the rest are lazy.
        let first = cx.alloc_splash_vm_with_widgets(false, set);
        let [whole, stepped, entered, type_miss, restyled] = std::array::from_fn(|_| cx.alloc_splash_vm_with_widgets(false, set));
        let names = unit_names(&mut cx, stepped);
        let errors_before = ERRORS.with(|errors| errors.get());

        // Stepped to the end, with a collection after every unit, equals whole.
        let ui = ui_of(&mut cx, whole);
        let mut steps = 1;
        while cx.step_isolate_ui(stepped) {
            steps += 1;
            collect(&mut cx, stepped);
        }
        assert_eq!(steps, names.len(), "one unit per step");
        assert!(!cx.step_isolate_ui(stepped), "nothing left to step");
        assert_eq!(ui_of(&mut cx, stepped), ui, "stepped equals whole");

        // Interrupted: any other entry runs the rest first.
        step(&mut cx, entered, 3);
        let button = cx.with_script_vm_id(entered, |vm| {
            let lazy = vm.has_lazy_modules();
            let widgets = vm.module(id!(widgets));
            (lazy, vm.value(widgets, id!(Button)).as_object().is_some())
        });
        assert_eq!(button, (false, true), "an entry finds the UI registered");
        assert!(!cx.step_isolate_ui(entered), "nothing left to step after an entry");
        assert_eq!(ui_of(&mut cx, entered), ui, "an entry completes it as whole");

        // A Rust type miss in an entry that does not finish it (taken out of the set by hand)
        // runs the rest from where the steps were.
        step(&mut cx, type_miss, 3);
        cx.global::<CxWidgetAsync>().ui_in_progress.remove(&type_miss);
        assert!(cx.with_script_vm_id(type_miss, |vm| crate::makepad_script::expand_on_type_miss(vm)), "the miss finds the group");
        assert!(!cx.step_isolate_ui(type_miss), "nothing left to step after a type miss");
        assert_eq!(ui_of(&mut cx, type_miss), ui, "a type miss completes it as whole");

        // A restyle half way equals the same restyle after a whole registration.
        let sheet = crate::desktop_style::StyleSheet::named("macos").expect("the catalogue's sheet");
        step(&mut cx, restyled, 3);
        restyle(&mut cx, restyled, &sheet);
        restyle(&mut cx, whole, &sheet);
        let restyled_ui = ui_of(&mut cx, restyled);
        assert_eq!(restyled_ui, ui_of(&mut cx, whole), "a restyle half way equals one after whole");
        if let Some(colour) = sheet.theme_color("color_bg_app") {
            let bg = restyled_ui.theme_values.iter().find(|(key, _, _)| *key == id!(color_bg_app)).and_then(|(_, colour, _)| *colour);
            assert_eq!(bg, Some(colour), "the restyle wears the sheet");
        }

        assert_eq!(ERRORS.with(|errors| errors.get()), errors_before, "no errors");
        for vm_id in [first, whole, stepped, entered, type_miss, restyled] {
            cx.free_splash_vm(vm_id);
        }
        assert!(cx.global::<CxWidgetAsync>().ui_in_progress.is_empty());
        names
    }

    /// The game host's set. Its unit list is a unit per base theme and per base widget module.
    #[test]
    fn a_stepped_base_isolate_ui_registers_as_whole_however_interrupted() {
        let names = stepping_registers_the_whole_ui(IsolateWidgets::Base);
        let widget_modules = names.iter().filter(|name| name.starts_with("widgets: ") && !matches!(**name, "widgets: head" | "widgets: insets and prelude")).count();
        assert_eq!(widget_modules, 17, "a unit per base widget module: {names:?}");
        for theme in ["theme: dark", "theme: light", "theme: skeleton"] {
            assert_eq!(names.iter().filter(|name| **name == theme).count(), 1, "a unit for {theme}: {names:?}");
        }
        let mut once: Vec<&str> = names.iter().copied().filter(|name| *name != "ui: host mod").collect();
        once.sort_unstable();
        once.dedup();
        assert_eq!(once.len(), names.iter().filter(|name| **name != "ui: host mod").count(), "no unit runs twice: {names:?}");
    }

    /// What Splash mini-apps and Stage slides allocate.
    #[test]
    fn a_stepped_library_isolate_ui_registers_as_whole_however_interrupted() {
        stepping_registers_the_whole_ui(IsolateWidgets::Library);
    }
}
