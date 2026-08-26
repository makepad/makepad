use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    view::View,
    widget::*,
    widget_async::{
        CxSplashVmExt, SplashVmId, MAIN_SPLASH_VM_ID, WIDGET_SCRIPT_INSTRUCTION_LIMIT,
    },
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SplashBase = #(Splash::register_widget(vm))

    mod.widgets.Splash = set_type_default() do mod.widgets.SplashBase{
        width: Fill height: Fit
    }
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct Splash {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[deref]
    pub view: View,
    #[live]
    body: ArcStringMut,
    #[live]
    allow_net: bool,
    /// The app's private storage directory — the root of its jailed `fs`
    /// module (see splash_storage.rs). None (the default) = every storage
    /// call errors, which is right for previews/validation-less contexts.
    #[rust]
    sandbox_dir: Option<std::path::PathBuf>,
    #[rust]
    vm_id: SplashVmId,
    /// Index of this Splash's script body in its isolate's bodies list, cached
    /// at eval time so host->script calls don't depend on re-deriving the
    /// pointer-based ScriptMod identity (the struct could in principle move
    /// between eval and a later call).
    #[rust]
    body_id: Option<u16>,
    /// Host-trusted identity carried on every `host.request` this isolate
    /// makes (see splash_host.rs). None = requests arrive untagged, which a
    /// policy-enforcing host treats as deniable — right for previews.
    #[rust]
    host_tag: Option<String>,
    /// Granted-capability names `host.capabilities()` reports. Informational
    /// for the script's UI; enforcement is the host's per-request decision.
    #[rust]
    host_caps: Vec<String>,
    /// Whether this isolate's surface may raise user prompts (true for a
    /// foreground app host, false for background surfaces like home-screen
    /// widget tiles). Rides on every host.request as `may_prompt`.
    #[rust(true)]
    host_prompts: bool,
    /// Whole-jail byte cap when the host has granted this app extra room.
    /// None leaves the storage default.
    #[rust]
    storage_quota: Option<u64>,
    /// How much CPU, memory, timers and concurrency this isolate may use
    /// (see splash_limits.rs). None = the built-in defaults.
    ///
    /// `#[rust]`, like every other host-assigned field here: a script that
    /// could raise its own limits from its own DSL would not have limits.
    #[rust]
    limits: Option<crate::splash_limits::SplashLimits>,
    /// What to call this script in error messages. Set by the host to the
    /// mini-app's id; empty for previews and one-off evals.
    ///
    /// Script errors are logged as `{file}:{line}:{col} - {message}` with
    /// nothing else to go on, and every Splash app used to report an empty
    /// file — so an error from a generated app named neither the app nor a
    /// usable line, and finding the culprit meant grepping every installed
    /// script by hand.
    #[rust]
    debug_name: String,
}

// `let fs = mod.fs` puts the jailed storage module (splash_storage.rs) in
// scope as a bare name — app scripts say `fs.read("/x")`, not `mod.fs.read`.
// A script reassigning `fs` only sabotages its own binding; the jail itself
// lives host-side.
/// Lines the no-net prefix occupies, which is the amount every reported
/// script line is ahead of the app's own file. Subtract it to get the line in
/// the `.splash` source; net-enabled apps carry one extra line
/// ([`SPLASH_NET_PREFIX_LINES`]).
///
/// It cannot be zero: the prefix would then have to share line 1 with the
/// app's first line, and a generated app's first line is its `// name:`
/// header — a comment, which would swallow the rest of the prefix.
pub const SPLASH_PREFIX_LINES: u32 = 3;
/// Line offset for net-enabled apps (their prefix adds `use mod.net`).
pub const SPLASH_NET_PREFIX_LINES: u32 = 4;

const SPLASH_PREFIX: &str =
    "use mod.prelude.widgets.*\nlet fs = mod.fs\nlet host = mod.host\nView{height:Fit, ";
const SPLASH_NET_PREFIX: &str =
    "use mod.prelude.widgets.*\nuse mod.net\nlet fs = mod.fs\nlet host = mod.host\nView{height:Fit, ";
const SPLASH_EVAL_INSTRUCTION_LIMIT: usize = 200_000;

impl Splash {
    /// Stable identity for the streaming script body, based on pointer address.
    fn self_id(&self) -> usize {
        self as *const Self as usize
    }

    /// Names this script for error reporting — see [`Self::debug_name`].
    pub fn set_debug_name(&mut self, name: &str) {
        self.debug_name = name.to_string();
    }

    /// The body's identity within its isolate, carried in `ScriptMod`'s
    /// `module_path`.
    ///
    /// It used to live in `line`, which the VM adds to a script's real line
    /// when it reports an error (`ScriptCode::ip_to_loc`) — so every location
    /// came out as `real_line + a_pointer_address`, i.e. numbers like
    /// 1804943384. `module_path` is a free-form string nobody else reads for
    /// these bodies, so identity and line no longer fight over one field.
    fn body_key(&self) -> String {
        format!("splash#{}", self.self_id())
    }

    /// The name a script error reports. Falls back to something searchable
    /// rather than the empty string that made these untraceable.
    ///
    /// Errors read `splash:<name>:<line>:<col>`, where `<line>` is ahead of
    /// the app's own file by [`SPLASH_PREFIX_LINES`].
    fn source_label(&self) -> String {
        if self.debug_name.is_empty() {
            format!("splash:{}", self.self_id())
        } else {
            format!("splash:{}", self.debug_name)
        }
    }

    fn eval_body(&mut self, cx: &mut Cx) {
        let body = self.body.as_ref();
        if body.is_empty() {
            return;
        }

        if self.vm_id == MAIN_SPLASH_VM_ID {
            // Privilege only ever narrows when Splashes nest. `allow_net` is a
            // `#[live]` field, so a script CAN write `Splash{allow_net: true}`
            // in its own body — harmless while nothing evaluates such a widget,
            // and a network grant it was never given the moment something does.
            // A nested isolate gets at most what the isolate creating it has.
            let allow_net = self.allow_net && cx.current_vm_allows_net();
            self.vm_id = cx.alloc_splash_vm_with_network(allow_net);
        }
        // (Re)bind this isolate's storage jail and host-bridge identity.
        // Keyed by heap so the script can neither read nor retarget them.
        let heap_key = cx.with_script_vm_id(self.vm_id, |vm| vm.bx.heap.heap_key());
        crate::splash_storage::set_root_for_heap(heap_key, self.sandbox_dir.clone());
        crate::splash_host::set_tag_for_heap(heap_key, self.host_tag.clone());
        crate::splash_host::set_caps_for_heap(heap_key, self.host_caps.clone());
        crate::splash_host::set_prompts_for_heap(heap_key, self.host_prompts);
        crate::splash_storage::set_quota_for_heap(heap_key, self.storage_quota);
        crate::splash_limits::set_limits_for_heap(heap_key, self.limits);
        cx.sync_splash_http_cap(self.vm_id);

        let body_key = self.body_key();
        // Full code string: prefix + body (no closing - parser auto-closes)
        let prefix = if self.allow_net {
            SPLASH_NET_PREFIX
        } else {
            SPLASH_PREFIX
        };
        let code = format!("{}{}", prefix, body);

        // Identity is stable (same module_path each call) AND the location
        // fields are left alone, so `ip_to_loc` reports the script's own line
        // instead of one offset by a pointer address.
        let script_mod = ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: self.body_key(),
            file: self.source_label(),
            line: 0,
            column: 0,
            code: String::new(),
            values: vec![],
        };

        let vm_id = self.vm_id;
        let new_view = cx.with_script_vm_id(vm_id, |vm| {
            // Setup, not ongoing work: a body that cannot finish evaluating is
            // an app that does not exist, and trimming it gives nobody a frame.
            let value = crate::splash_limits::with_setup_slice(|| {
                vm.with_instruction_limit(SPLASH_EVAL_INSTRUCTION_LIMIT, |vm| {
                    vm.eval_with_append_source(script_mod, &code, NIL.into())
                })
            });
            if !value.is_err() && !value.is_nil() {
                Some(View::script_from_value(vm, value))
            } else {
                None
            }
        });

        if let Some(view) = new_view {
            // Cache the body index for host->script calls (call_script_fn etc).
            self.body_id = cx.with_script_vm_id(vm_id, |vm| {
                let bodies = vm.bx.code.bodies.borrow();
                bodies.iter().position(|body| match &body.source {
                    ScriptSource::Mod(m) => m.module_path == body_key,
                    _ => false,
                })
            })
            .map(|i| i as u16);
            self.view = view;
            // Make `ui` a global in this splash's VM so helper `fn`s inside the block can use
            // `ui.<id>.set_text(...)`, not just inline handlers. It points at the Splash widget
            // itself (not the wrapper view, which never becomes a widget-tree node since
            // `children()` forwards through it), so confined subtree lookups resolve.
            crate::widget_async::inject_splash_ui_handle(cx, self.vm_id, self.uid);
            cx.widget_tree_mark_dirty(self.uid);
        }
    }

    /// Tears down this Splash's isolate (if any), returning it to the empty
    /// state a freshly-created Splash has. The isolate-minted view holds refs
    /// into the isolate heap, so it is REPLACED with a fresh empty view built
    /// in the main VM BEFORE the isolate is reclaimed; the reclamation then
    /// stops the isolate's timers and drops its storage-jail binding. A later
    /// non-empty `set_text` allocates a fresh isolate as usual.
    fn stop(&mut self, cx: &mut Cx) {
        if self.vm_id == MAIN_SPLASH_VM_ID {
            return; // nothing running
        }
        self.view = cx.with_vm(|vm| View::script_from_value(vm, NIL.into()));
        self.body_id = None;
        crate::widget_async::mark_splash_isolate_dead(self.vm_id);
        self.vm_id = MAIN_SPLASH_VM_ID;
        // Reclaim now (Cx is in hand and nothing runs in the isolate) so the
        // timers stop immediately rather than lingering to the next pump.
        crate::widget_async::gc_dead_splash_isolates(cx);
        cx.widget_tree_mark_dirty(self.uid);
    }
}

/// Evaluates a Splash body in a throwaway isolate — the same prelude prefix,
/// instruction limit, and network gating the `Splash` widget itself uses — and
/// returns the formatted script errors, freeing the isolate afterwards. An
/// empty result means the body parses and its root expression evaluates to a
/// widget tree; runtime errors inside handlers can of course still occur later.
///
/// This is the widget's `eval_body` as a checked dry run: hosts that install
/// script source from outside (downloads, AI generation, user input) can
/// validate it — with real errors to show or feed back — before committing it
/// to a live `Splash`, whose own eval silently keeps the old view on failure.
///
/// Caveats (identical to installing the same source in a real `Splash`, so
/// validation adds no NEW exposure): the instruction limit bounds compute but
/// not heap growth, so a hostile script can still allocate aggressively within
/// its budget; and top-level side effects (e.g. `start_interval`) run and live
/// until the marked-dead isolate is reclaimed by `gc_dead_splash_isolates`.
pub fn validate_splash_body(cx: &mut Cx, body: &str, allow_net: bool) -> Vec<String> {
    let vm_id = cx.alloc_splash_vm_with_network(allow_net);
    // Give the dry run a throwaway storage jail so top-level `fs.read` boot
    // loads validate instead of erroring "storage not available". The path is
    // unpredictable and created with an EXCLUSIVE mkdir (fails EEXIST on any
    // pre-existing entry incl. a planted symlink, so it never follows one out
    // of temp); on failure the jail is simply left unset (fs calls error, same
    // as a preview). Reclaimed below; per-vm so concurrent validations differ.
    let scratch = std::env::temp_dir().join(format!(
        "splash_validate_{}_{}",
        std::process::id(),
        vm_id.0,
    ));
    let heap_key = cx.with_script_vm_id(vm_id, |vm| vm.bx.heap.heap_key());
    // Clear a leftover from a crashed run (we own this exact name), then take
    // it exclusively.
    let _ = std::fs::remove_dir_all(&scratch);
    if std::fs::create_dir(&scratch).is_ok() {
        crate::splash_storage::set_root_for_heap(heap_key, Some(scratch.clone()));
    }
    let prefix = if allow_net {
        SPLASH_NET_PREFIX
    } else {
        SPLASH_PREFIX
    };
    let code = format!("{}{}", prefix, body);
    let script_mod = ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: format!("splash-validate#{}", vm_id.0),
        // Named, and NOT via `line` — the validator's errors are shown to the
        // user (and fed back to the agent as repair input), so a location
        // offset by a vm id would be actively misleading there.
        file: "splash:validating".to_string(),
        line: 0,
        column: 0,
        code: String::new(),
        values: vec![],
    };
    let errors = cx.with_script_vm_id(vm_id, |vm| {
        // Capture instead of logging: mid-eval errors otherwise go straight to
        // the error log (see `ScriptVm::take_errors`) and can't be returned.
        vm.bx.captured_errors = Some(Vec::new());
        // Same exemption as eval_body: validation is setup, and a validator
        // that fails only because the machine was busy validates nothing.
        let value = crate::splash_limits::with_setup_slice(|| {
            vm.with_instruction_limit(SPLASH_EVAL_INSTRUCTION_LIMIT, |vm| {
                vm.eval_with_append_source(script_mod, &code, NIL.into())
            })
        });
        let mut errors = vm.take_errors();
        if errors.is_empty() {
            if value.is_err() {
                errors.push("script evaluated to an error".to_string());
            } else if value.as_object().is_none() {
                errors.push(
                    "script has no root widget (e.g. View{...}) as its final expression"
                        .to_string(),
                );
            }
        }
        errors
    });
    crate::widget_async::mark_splash_isolate_dead(vm_id);
    // Reclaim NOW (stops the isolate's top-level timers and drops its sandbox
    // root binding) so nothing can re-create the scratch dir after we remove
    // it; then delete last, and it stays deleted.
    crate::widget_async::gc_dead_splash_isolates(cx);
    let _ = std::fs::remove_dir_all(&scratch);
    errors
}

impl WidgetNode for Splash {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, cx: &mut Cx) -> Walk {
        self.view.walk(cx)
    }

    fn area(&self) -> Area {
        self.view.area()
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        self.view.children(visit);
    }
}

impl Drop for Splash {
    fn drop(&mut self) {
        // A Splash owns an isolate script VM. `Drop` has no `Cx`, so it can't free
        // the VM here; it just marks the id for reclamation. The isolate is torn
        // down later by `gc_dead_splash_isolates` (on the next isolate alloc, async
        // pump, or Splash event) while a `Cx` is available and nothing runs in it.
        crate::widget_async::mark_splash_isolate_dead(self.vm_id);
    }
}

impl Widget for Splash {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.allow_net {
            if let Event::NetworkResponses(responses) = event {
                crate::widget_async::handle_splash_network_responses(cx, self.vm_id, responses);
            }
        }
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        //let tree = self.view.widget_tree();
        //cx.with_vm(|vm| {
        //    log!("{}", tree.display(vm.heap()));
        //});
        self.view.draw_walk(cx, scope, walk)
    }

    fn text(&self) -> String {
        self.body.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.body.as_ref() != v {
            self.body.set(v);
            // Empty body = tear down the app: reclaim its isolate (stopping its
            // timers and dropping its storage jail) rather than leaving it
            // running behind a blank view. A reused Splash (e.g. a live-preview
            // widget) that goes back to empty must not leak its old isolate.
            if v.is_empty() {
                self.stop(cx);
            } else {
                self.eval_body(cx);
            }
            self.redraw(cx);
        }
    }
}

impl Splash {
    /// Calls a top-level `fn` defined in this Splash's script body, if the script
    /// defines one by that name. Runs inside the isolate under the standard entry
    /// budget and instruction limit. Returns whether the fn was found: hosts
    /// broadcasting optional hooks (e.g. size changes) can ignore it, while
    /// callers expecting the fn to exist can log a missing-hook diagnostic
    /// (distinguishing "script has no hook" from a typo'd name).
    pub fn call_script_fn(&mut self, cx: &mut Cx, name: LiveId, args: &[ScriptValue]) -> bool {
        let Some(scope) = self.body_scope(cx) else {
            return false;
        };
        cx.with_script_vm_id(self.vm_id, |vm| {
            // NoTrap: this is an existence probe for an OPTIONAL hook. A
            // trapping lookup queues a NotFound into the error log even though
            // the miss is handled right here — every host broadcast (e.g.
            // on_app_resize) then spams "variable <raw id> not found" for
            // every script that simply doesn't define the hook.
            let fnval = vm.bx.heap.scope_value(scope, name, NoTrap);
            if fnval.is_nil() || fnval.is_err() {
                return false;
            }
            vm.with_instruction_limit(WIDGET_SCRIPT_INSTRUCTION_LIMIT, |vm| {
                vm.call(fnval, args);
            });
            true
        })
    }

    /// Like [`Self::call_script_fn`], but with string arguments — those are
    /// heap values, so they must be minted inside this isolate's own heap
    /// right before the call (a cross-heap ScriptValue would resolve in the
    /// wrong arena). Used for host->script deliveries like IPC messages.
    pub fn call_script_fn_with_strings(&mut self, cx: &mut Cx, name: LiveId, args: &[&str]) -> bool {
        let Some(scope) = self.body_scope(cx) else {
            return false;
        };
        cx.with_script_vm_id(self.vm_id, |vm| {
            let fnval = vm.bx.heap.scope_value(scope, name, NoTrap);
            if fnval.is_nil() || fnval.is_err() {
                return false;
            }
            let vals: Vec<ScriptValue> = args
                .iter()
                .map(|s| vm.new_string_with(|_vm, out| out.push_str(s)))
                .collect();
            vm.with_instruction_limit(WIDGET_SCRIPT_INSTRUCTION_LIMIT, |vm| {
                vm.call(fnval, &vals);
            });
            true
        })
    }

    /// Sets whether this Splash's isolate gets the networking runtime. Must be
    /// called before the body is first evaluated (i.e. before `set_text`) — the
    /// VM is allocated with or without network on that first eval and isn't
    /// re-allocated afterwards.
    pub fn set_allow_net(&mut self, allow: bool) {
        self.allow_net = allow;
    }

    /// Assigns this app's private storage directory — the root its jailed
    /// `fs` module resolves against. Takes effect immediately when the
    /// isolate is live, and on the next eval otherwise; call BEFORE set_text
    /// so top-level `fs.read` boot loads see it.
    pub fn set_sandbox_dir(&mut self, cx: &mut Cx, dir: Option<std::path::PathBuf>) {
        self.sandbox_dir = dir.clone();
        if self.vm_id != MAIN_SPLASH_VM_ID {
            let heap_key = cx.with_script_vm_id(self.vm_id, |vm| vm.bx.heap.heap_key());
            crate::splash_storage::set_root_for_heap(heap_key, dir);
        }
    }

    /// Assigns the host-trusted identity for this app's `host.request` calls.
    /// Call BEFORE set_text so boot-time requests already carry it; takes
    /// effect immediately when the isolate is live.
    pub fn set_host_tag(&mut self, cx: &mut Cx, tag: Option<String>) {
        self.host_tag = tag.clone();
        if self.vm_id != MAIN_SPLASH_VM_ID {
            let heap_key = cx.with_script_vm_id(self.vm_id, |vm| vm.bx.heap.heap_key());
            crate::splash_host::set_tag_for_heap(heap_key, tag);
        }
    }

    /// Replaces the capability list `host.capabilities()` reports to this
    /// app. Informational (the host still decides every request); push it
    /// again whenever grants change so the script's UI can adapt.
    pub fn set_host_caps(&mut self, cx: &mut Cx, caps: Vec<String>) {
        self.host_caps = caps.clone();
        if self.vm_id != MAIN_SPLASH_VM_ID {
            let heap_key = cx.with_script_vm_id(self.vm_id, |vm| vm.bx.heap.heap_key());
            crate::splash_host::set_caps_for_heap(heap_key, caps);
        }
    }

    /// Raises this app's whole-jail storage cap (None = the default). Takes
    /// effect immediately; a lowered cap refuses further growth rather than
    /// deleting anything the app already wrote.
    pub fn set_storage_quota(&mut self, cx: &mut Cx, total_bytes: Option<u64>) {
        self.storage_quota = total_bytes;
        if self.vm_id != MAIN_SPLASH_VM_ID {
            let heap_key = cx.with_script_vm_id(self.vm_id, |vm| vm.bx.heap.heap_key());
            crate::splash_storage::set_quota_for_heap(heap_key, total_bytes);
        }
    }

    /// Sets this isolate's resource limits (CPU share, timers, heap ceiling,
    /// concurrent requests). `None` restores the defaults. Like the other
    /// host-assigned state, call BEFORE `set_text` so the isolate is created
    /// with them; setting them later applies from that moment.
    pub fn set_limits(&mut self, cx: &mut Cx, limits: Option<crate::splash_limits::SplashLimits>) {
        self.limits = limits;
        if self.vm_id != MAIN_SPLASH_VM_ID {
            let heap_key = cx.with_script_vm_id(self.vm_id, |vm| vm.bx.heap.heap_key());
            crate::splash_limits::set_limits_for_heap(heap_key, limits);
            cx.sync_splash_http_cap(self.vm_id);
        }
    }

    /// Marks whether this isolate's surface may raise user prompts; see
    /// `SplashHostRequest::may_prompt`. Defaults true.
    pub fn set_host_prompts(&mut self, cx: &mut Cx, may_prompt: bool) {
        self.host_prompts = may_prompt;
        if self.vm_id != MAIN_SPLASH_VM_ID {
            let heap_key = cx.with_script_vm_id(self.vm_id, |vm| vm.bx.heap.heap_key());
            crate::splash_host::set_prompts_for_heap(heap_key, may_prompt);
        }
    }

    /// This Splash's live isolate heap identity (None while stopped) — the
    /// same key `SplashHostRequest::heap_key` carries, so hosts can relate a
    /// request to a specific widget (e.g. to skip an IPC sender's own isolate
    /// when fanning a message out).
    pub fn isolate_heap_key(&mut self, cx: &mut Cx) -> Option<usize> {
        if self.vm_id == MAIN_SPLASH_VM_ID {
            return None;
        }
        Some(cx.with_script_vm_id(self.vm_id, |vm| vm.bx.heap.heap_key()))
    }

    /// Sets (or replaces) a global visible to this Splash's script, like the
    /// injected `ui` handle. Useful for handing scripts host-provided context
    /// (configuration, sizes, capabilities) without re-evaluating the body.
    pub fn set_script_global(&mut self, cx: &mut Cx, key: LiveId, value: ScriptValue) {
        if self.vm_id == MAIN_SPLASH_VM_ID {
            return;
        }
        cx.with_script_vm_id(self.vm_id, |vm| {
            vm.set_injected_global(key, value);
        });
    }

    /// The scope object holding this Splash body's top-level definitions, via
    /// the body id cached at eval time (with a pointer-identity fallback for
    /// robustness).
    fn body_scope(&mut self, cx: &mut Cx) -> Option<ScriptObject> {
        if self.vm_id == MAIN_SPLASH_VM_ID {
            return None;
        }
        let body_key = self.body_key();
        let body_id = self.body_id;
        cx.with_script_vm_id(self.vm_id, |vm| {
            let bodies = vm.bx.code.bodies.borrow();
            if let Some(body) = body_id.and_then(|i| bodies.get(i as usize)) {
                return Some(body.scope.as_object());
            }
            bodies.iter().find_map(|body| match &body.source {
                ScriptSource::Mod(m) if m.module_path == body_key => {
                    Some(body.scope.as_object())
                }
                _ => None,
            })
        })
    }
}

impl SplashRef {
    pub fn set_text(&self, cx: &mut Cx, v: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, v);
        }
    }

    /// See [`Splash::set_sandbox_dir`].
    pub fn set_sandbox_dir(&self, cx: &mut Cx, dir: Option<std::path::PathBuf>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_sandbox_dir(cx, dir);
        }
    }

    /// See [`Splash::set_host_tag`].
    pub fn set_host_tag(&self, cx: &mut Cx, tag: Option<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_host_tag(cx, tag);
        }
    }

    /// See [`Splash::set_host_caps`].
    pub fn set_host_caps(&self, cx: &mut Cx, caps: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_host_caps(cx, caps);
        }
    }

    /// See [`Splash::set_storage_quota`].
    pub fn set_storage_quota(&self, cx: &mut Cx, total_bytes: Option<u64>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_storage_quota(cx, total_bytes);
        }
    }

    /// See [`Splash::set_limits`].
    pub fn set_limits(&self, cx: &mut Cx, limits: Option<crate::splash_limits::SplashLimits>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_limits(cx, limits);
        }
    }

    /// See [`Splash::set_host_prompts`].
    pub fn set_host_prompts(&self, cx: &mut Cx, may_prompt: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_host_prompts(cx, may_prompt);
        }
    }

    /// See [`Splash::isolate_heap_key`].
    pub fn isolate_heap_key(&self, cx: &mut Cx) -> Option<usize> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.isolate_heap_key(cx)
        } else {
            None
        }
    }

    /// See [`Splash::call_script_fn`].
    pub fn call_script_fn(&self, cx: &mut Cx, name: LiveId, args: &[ScriptValue]) -> bool {
        if let Some(mut inner) = self.borrow_mut() {
            inner.call_script_fn(cx, name, args)
        } else {
            false
        }
    }

    /// See [`Splash::call_script_fn_with_strings`].
    pub fn call_script_fn_with_strings(&self, cx: &mut Cx, name: LiveId, args: &[&str]) -> bool {
        if let Some(mut inner) = self.borrow_mut() {
            inner.call_script_fn_with_strings(cx, name, args)
        } else {
            false
        }
    }

    /// See [`Splash::set_script_global`].
    pub fn set_script_global(&self, cx: &mut Cx, key: LiveId, value: ScriptValue) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_script_global(cx, key, value);
        }
    }
}
