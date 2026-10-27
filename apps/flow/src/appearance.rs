//! The appearance the main VM shows, carried into every isolate.
//!
//! The main VM restyles itself on `Event::LiveEdit` and takes a saved
//! theme's tokens on `Event::ScriptReapply`; an isolate allocated by
//! `alloc_splash_vm` only ever saw the launch-time theme. This module keeps
//! one snapshot of the main appearance (the desktop-style sheet, the base
//! theme and every colour and number in `mod.theme`) and brings an isolate
//! to it: at mount, before anything the package wrote is evaluated, and on
//! a change, by rebuilding the isolate's theme and stock prototypes under
//! reload and cloning the presentation objects a package authored onto the
//! fresh prototypes. Nothing here evaluates package script. The allocator's
//! strip stays in force: `fs`, `run`, `res` and `cx.quit` are `nil` again
//! before any sheet or package source runs.

use makepad_widgets::desktop_style::{self, StyleSheet};
use makepad_widgets::makepad_script::trap::NoTrap;
use makepad_widgets::makepad_script::*;
use makepad_widgets::widget_async::{CxSplashVmExt, MAIN_SPLASH_VM_ID};
use makepad_widgets::*;
use std::collections::HashMap;
use std::rc::Rc;

/// What the main VM shows. `epoch` changes whenever any of it does; a host
/// compares it with the epoch it applied last.
pub struct Appearance {
    pub epoch: u64,
    pub base: BaseTheme,
    pub sheet: Option<StyleSheet>,
    /// Every colour, number and flag of the main VM's `mod.theme`, resolved
    /// through its prototype chain: the base, the sheet's theme phase, the
    /// window manager's palette and a saved theme's pins, as one list.
    pub tokens: Vec<(LiveId, ScriptValue)>,
}

#[derive(Default)]
struct AppearanceGlobal {
    current: Option<Rc<Appearance>>,
    epoch: u64,
}

/// Read the main VM and remember what it shows. Returns whether the
/// appearance changed since the last call. Once per `Startup`, `LiveEdit`
/// and `ScriptReapply`, never per frame: `desktop_style::current` clones the
/// sheet. The main VM is named outright, so a call while an isolate is
/// installed is reported by the VM layer rather than snapshotting the
/// isolate as if it were the app.
pub fn sync(cx: &mut Cx) -> bool {
    let (sheet, tokens) = cx.with_script_vm_id(MAIN_SPLASH_VM_ID, |vm| (desktop_style::current(vm), theme_tokens(vm)));
    let base = base_theme(cx);
    let global = cx.global::<AppearanceGlobal>();
    if global
        .current
        .as_ref()
        .is_some_and(|current| current.base == base && current.sheet == sheet && current.tokens == tokens)
    {
        return false;
    }
    global.epoch += 1;
    global.current = Some(Rc::new(Appearance { epoch: global.epoch, base, sheet, tokens }));
    true
}

/// The current snapshot, taken now if `sync` has not run yet.
pub fn current(cx: &mut Cx) -> Rc<Appearance> {
    if cx.global::<AppearanceGlobal>().current.is_none() {
        sync(cx);
    }
    cx.global::<AppearanceGlobal>()
        .current
        .clone()
        .expect("sync leaves a snapshot")
}

/// The immediate values of `mod.theme` as a script reads them: the chain
/// from its root prototype down, later levels overriding. Strings, text
/// styles and insets belong to their heap and are left to the theme phase.
fn theme_tokens(vm: &mut ScriptVm) -> Vec<(LiveId, ScriptValue)> {
    let theme = vm.module(live_id!(theme));
    let mut chain = vec![theme];
    while let Some(proto) = vm.bx.heap_ref().proto(*chain.last().unwrap()).as_object() {
        if chain.len() >= 16 || chain.contains(&proto) {
            break;
        }
        chain.push(proto);
    }
    let mut tokens: HashMap<LiveId, ScriptValue> = HashMap::new();
    for obj in chain.iter().rev() {
        for (key, value) in own_entries(vm, *obj) {
            let Some(id) = key.as_id() else { continue };
            if value.is_color() || value.is_number() || value.is_bool() {
                tokens.insert(id, value);
            }
        }
    }
    let mut tokens: Vec<(LiveId, ScriptValue)> = tokens.into_iter().collect();
    tokens.sort_by_key(|(id, _)| id.0);
    tokens
}

fn base_name(base: BaseTheme) -> &'static str {
    match base {
        BaseTheme::Dark => "dark",
        BaseTheme::Light => "light",
        BaseTheme::Skeleton => "skeleton",
    }
}

/// Bring an isolate to `appearance`. Returns whether anything was rebuilt:
/// unless `force`, an isolate that already reads the same tokens with no
/// sheet anywhere is left as the allocator made it.
///
/// 1. `theme_mod` under reload, behind a `mod.res` that holds only the
///    `crate_resource` the widget prelude already carried: pristine bases,
///    fonts, `mod.draw`, the prelude header. `mod.res` is `nil` again before
///    anything else runs.
/// 2. The sheet's theme phase, then the main VM's tokens over it.
/// 3. Under reload: `widgets_mod`, with every captured token explicitly
///    preserved in the sheet so role derivation cannot overwrite saved
///    colors. The snapshot already contains the window manager's palette.
///
/// The caller registers its own modules after this.
///
/// A fresh isolate does not go through here: [`isolate_ui`] registers its
/// library once, already in the appearance, at allocation.
pub fn prepare_isolate(vm: &mut ScriptVm, appearance: &Appearance, force: bool) -> Result<bool, String> {
    if !force && !needs_apply(vm, appearance) {
        return Ok(false);
    }
    rebuild_bases(vm)?;
    if appearance.base != base_theme(vm.cx_mut()) {
        eval_str(vm, &format!("mod.theme = mod.themes.{}", base_name(appearance.base)))?;
    }
    if let Some(sheet) = &appearance.sheet {
        desktop_style::install(vm, sheet.clone());
        vm.capture_errors();
        desktop_style::apply_theme(vm);
        let errors = vm.take_errors();
        vm.capture_errors();
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
    }
    let theme = vm.module(live_id!(theme));
    for (id, value) in &appearance.tokens {
        vm.bx.heap_mut().set_value_def(theme, (*id).into(), *value);
    }
    match &appearance.sheet {
        Some(sheet) => {
            // An empty theme phase still derives the library's semantic
            // roles. Name every captured token so that derivation leaves
            // the main VM's exact values intact while widgets are rebuilt.
            let mut pinned = format!("// Base: mod.themes.{}\n", base_name(appearance.base));
            for (id, _) in &appearance.tokens {
                if let Some(name) = id.as_string(|name| name.map(str::to_owned)) {
                    pinned.push_str(&format!("mod.theme.{name} = mod.theme.{name}\n"));
                }
            }
            pinned.push_str("nil\n");
            desktop_style::install(vm, StyleSheet { theme: pinned, ..sheet.clone() });
        }
        None => desktop_style::uninstall(vm),
    }
    vm.capture_errors();
    vm.with_reload(|vm| {
        makepad_widgets::widgets_mod(vm);
    });
    let errors = vm.take_errors();
    vm.capture_errors();
    if errors.is_empty() {
        Ok(true)
    } else {
        Err(errors.join("\n"))
    }
}

/// A fresh isolate's UI registration in `appearance` (for
/// `alloc_splash_vm_with_ui`): the theme, the base, the sheet's theme phase
/// and the main VM's tokens, then the widget library ONCE over them -- what
/// allocating with the stock library and [`prepare_isolate`] on top did with
/// two registrations. Errors land in `errors`.
pub fn isolate_ui(appearance: Rc<Appearance>, errors: Rc<std::cell::RefCell<Vec<String>>>) -> Box<dyn FnOnce(&mut ScriptVm)> {
    Box::new(move |vm: &mut ScriptVm| {
        vm.capture_errors();
        makepad_widgets::theme_mod(vm);
        let mut out = vm.take_errors();
        if appearance.base != base_theme(vm.cx_mut()) {
            if let Err(e) = eval_str(vm, &format!("mod.theme = mod.themes.{}", base_name(appearance.base))) {
                out.push(e);
            }
        }
        if let Some(sheet) = &appearance.sheet {
            desktop_style::install(vm, sheet.clone());
            vm.capture_errors();
            desktop_style::apply_theme(vm);
            out.extend(vm.take_errors());
        }
        let theme = vm.module(live_id!(theme));
        for (id, value) in &appearance.tokens {
            vm.bx.heap_mut().set_value_def(theme, (*id).into(), *value);
        }
        match &appearance.sheet {
            Some(sheet) => {
                let mut pinned = format!("// Base: mod.themes.{}\n", base_name(appearance.base));
                for (id, _) in &appearance.tokens {
                    if let Some(name) = id.as_string(|name| name.map(str::to_owned)) {
                        pinned.push_str(&format!("mod.theme.{name} = mod.theme.{name}\n"));
                    }
                }
                pinned.push_str("nil\n");
                desktop_style::install(vm, StyleSheet { theme: pinned, ..sheet.clone() });
            }
            None => desktop_style::uninstall(vm),
        }
        vm.capture_errors();
        makepad_widgets::isolate_widgets_mod(vm);
        out.extend(vm.take_errors());
        errors.borrow_mut().extend(out);
    })
}

fn needs_apply(vm: &mut ScriptVm, appearance: &Appearance) -> bool {
    if appearance.sheet.is_some() || desktop_style::current_name(vm).is_some() || appearance.base != BaseTheme::Dark {
        return true;
    }
    let theme = vm.module(live_id!(theme));
    appearance
        .tokens
        .iter()
        .any(|(id, value)| vm.bx.heap_ref().value(theme, (*id).into(), NoTrap) != *value)
}

/// The base themes as the library wrote them, again. A sheet's theme phase
/// assigns `mod.theme` one of them and writes into it, so after one sheet
/// the bases are not pristine and `uninstall` does not make them so; the
/// theme scripts only need `crate_resource`, which the widget prelude kept
/// a copy of when the allocator stripped `mod.res`.
fn rebuild_bases(vm: &mut ScriptVm) -> Result<(), String> {
    let header = resolve_path(vm, &[live_id!(prelude), live_id!(widgets_header)])
        .ok_or_else(|| "the widget prelude header is missing".to_string())?;
    let crate_resource = own_value_id(vm, header, live_id!(crate_resource))
        .ok_or_else(|| "the widget prelude has no crate_resource".to_string())?;
    let res = vm.new_module(live_id!(res));
    vm.bx.heap_mut().set_value_def(res, live_id!(crate_resource).into(), crate_resource);
    vm.capture_errors();
    vm.with_reload(|vm| makepad_widgets::theme_mod(vm));
    let errors = vm.take_errors();
    vm.capture_errors();
    // Whatever happened above, the strip is back before any sheet or
    // package source can run.
    let strip = eval_str(vm, "mod.res = nil");
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    strip
}

fn eval_str(vm: &mut ScriptVm, code: &str) -> Result<(), String> {
    vm.capture_errors();
    // A module's last statement is its result: an assignment there never
    // lands, so every script here ends in `nil`.
    vm.eval(ScriptMod::new("", "", "<flow-appearance>", 0, 0, &(format!("{code}\nnil\n")), &[]));
    let errors = vm.take_errors();
    vm.capture_errors();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// An object's own entry, by id key, from its vec or its map.
pub(crate) fn own_value_id(vm: &ScriptVm<'_>, obj: ScriptObject, id: LiveId) -> Option<ScriptValue> {
    let key: ScriptValue = id.into();
    let heap = vm.bx.heap_ref();
    heap.object_own_value(obj, &key).or_else(|| {
        heap.vec_ref(obj).iter().rev().find(|entry| entry.key == key).map(|entry| entry.value)
    })
}

fn own_vec_entries(vm: &ScriptVm<'_>, obj: ScriptObject) -> Vec<(ScriptValue, ScriptValue)> {
    vm.bx.heap_ref().vec_ref(obj).iter().map(|entry| (entry.key, entry.value)).collect()
}

fn own_map_entries(vm: &ScriptVm<'_>, obj: ScriptObject) -> Vec<(ScriptValue, ScriptValue)> {
    vm.bx.heap_ref().map_entries(obj).iter().map(|entry| (entry.key, entry.value)).collect()
}

/// Every own entry: the vec in order, then the map.
fn own_entries(vm: &ScriptVm<'_>, obj: ScriptObject) -> Vec<(ScriptValue, ScriptValue)> {
    let mut out = own_vec_entries(vm, obj);
    out.extend(own_map_entries(vm, obj));
    out
}

/// Old prototype object → its path from `mod`, for the modules a reload
/// replaces, and the objects those entries own (`widgets.Button.draw_bg`),
/// to a bounded depth.
pub(crate) type ProtoIndex = HashMap<ScriptObject, Vec<LiveId>>;

const INDEX_DEPTH: usize = 6;
const INDEX_LIMIT: usize = 50_000;
const CLONE_LIMIT: usize = 50_000;

/// Take the index BEFORE the reload; the objects it names are the ones the
/// authored objects still inherit from afterwards.
pub(crate) fn index_prototypes(vm: &mut ScriptVm) -> ProtoIndex {
    let mut index = ProtoIndex::new();
    for path in [
        vec![live_id!(widgets)],
        vec![live_id!(theme)],
        vec![live_id!(themes)],
        vec![live_id!(flow), live_id!(ui)],
        vec![live_id!(draw)],
        vec![live_id!(turtle)],
        vec![live_id!(animator)],
        vec![live_id!(sdf)],
        vec![live_id!(shader)],
        vec![live_id!(geometry)],
        vec![live_id!(text)],
    ] {
        let Some(root) = resolve_path(vm, &path) else { continue };
        index_from(vm, root, &path, 0, &mut index);
    }
    index
}

fn index_from(vm: &ScriptVm<'_>, obj: ScriptObject, path: &[LiveId], depth: usize, index: &mut ProtoIndex) {
    if depth >= INDEX_DEPTH || index.len() >= INDEX_LIMIT {
        return;
    }
    for (key, value) in own_entries(vm, obj) {
        let Some(id) = key.as_id() else { continue };
        let Some(child) = value.as_object() else { continue };
        if vm.bx.heap_ref().is_fn(child) || index.contains_key(&child) {
            continue;
        }
        let mut child_path = path.to_vec();
        child_path.push(id);
        index.insert(child, child_path.clone());
        index_from(vm, child, &child_path, depth + 1, index);
    }
}

/// The object at `path` from `mod`, as the modules stand now.
pub(crate) fn resolve_path(vm: &ScriptVm<'_>, path: &[LiveId]) -> Option<ScriptObject> {
    let mut obj = vm.modules();
    for id in path {
        obj = own_value_id(vm, obj, *id)?.as_object()?;
    }
    Some(obj)
}

/// The fresh object at the path an old prototype had, if it is a different
/// object now.
pub(crate) fn fresh_counterpart(vm: &ScriptVm<'_>, index: &ProtoIndex, obj: ScriptObject) -> Option<ScriptObject> {
    let path = index.get(&obj)?;
    resolve_path(vm, path).filter(|fresh| *fresh != obj)
}

/// One restyle's rebasing: the index taken before the reload and the clones
/// made so far, so an object two faces share is cloned once and a cycle
/// ends where it began.
pub(crate) struct Rebase {
    pub index: ProtoIndex,
    memo: HashMap<ScriptObject, ScriptObject>,
    depth: usize,
}

impl Rebase {
    pub fn new(index: ProtoIndex) -> Self {
        Self { index, memo: HashMap::new(), depth: 0 }
    }
}

/// Keys whose values are bindings and model references, never presentation:
/// copied as they are, whatever they hold.
fn is_binding_key(key: ScriptValue) -> bool {
    key.as_id().is_some_and(|id| {
        [live_id!(bind), live_id!(show), live_id!(param), live_id!(param_bind)].contains(&id)
    })
}

/// A copy of an authored presentation object on the fresh prototypes: the
/// same own vec entries in the same order, the same own map entries, the
/// construction site kept for diagnostics, the prototype the fresh object
/// at the old prototype's path (an anonymous template of the author's own
/// is copied the same way, one level up). Fn objects and binding values
/// are shared, not copied; nothing about the old object changes.
pub(crate) fn clone_presentation(vm: &mut ScriptVm, rebase: &mut Rebase, obj: ScriptObject) -> ScriptObject {
    if let Some(fresh) = fresh_counterpart(vm, &rebase.index, obj) {
        return fresh;
    }
    if let Some(clone) = rebase.memo.get(&obj) {
        return *clone;
    }
    if rebase.memo.len() >= CLONE_LIMIT || rebase.depth >= 128 || vm.bx.heap_ref().is_fn(obj) {
        return obj;
    }
    rebase.depth += 1;
    let old_proto = vm.bx.heap_ref().proto(obj);
    let proto = match old_proto.as_object() {
        Some(proto) => match fresh_counterpart(vm, &rebase.index, proto) {
            Some(fresh) => fresh.into(),
            None if rebase.index.contains_key(&proto) => old_proto,
            None => clone_presentation(vm, rebase, proto).into(),
        },
        None => old_proto,
    };
    let clone = vm.bx.heap_mut().new_with_proto_no_vec(proto);
    let made_at = vm.bx.heap_ref().made_at(obj);
    vm.bx.heap_mut().set_made_at(clone, made_at);
    rebase.memo.insert(obj, clone);
    for (key, value) in own_vec_entries(vm, obj) {
        let value = clone_field(vm, rebase, key, value);
        vm.bx.heap_mut().vec_push_unchecked(clone, key, value);
    }
    for (key, value) in own_map_entries(vm, obj) {
        let value = clone_field(vm, rebase, key, value);
        vm.bx.heap_mut().set_value_def(clone, key, value);
    }
    rebase.depth -= 1;
    clone
}

fn clone_field(vm: &mut ScriptVm, rebase: &mut Rebase, key: ScriptValue, value: ScriptValue) -> ScriptValue {
    let Some(child) = value.as_object() else { return value };
    if vm.bx.heap_ref().is_fn(child) || is_binding_key(key) {
        return value;
    }
    clone_presentation(vm, rebase, child).into()
}

/// The value to re-apply a mounted root with: the fresh object at its path
/// when the root is a stock or default-face prototype itself, otherwise a
/// clone of the authored object on the fresh prototypes.
pub(crate) fn refreshed(vm: &mut ScriptVm, rebase: &mut Rebase, source: ScriptObject) -> ScriptObject {
    match fresh_counterpart(vm, &rebase.index, source) {
        Some(fresh) => fresh,
        None => clone_presentation(vm, rebase, source),
    }
}
