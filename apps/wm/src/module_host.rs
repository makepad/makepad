//! The window manager as a module host (aicontrol.md §3): app instances
//! that run IN-PROCESS, one splash isolate each, instead of as child
//! processes.
//!
//! Creating one: allocate the isolate (the widget universe is installed
//! by the allocation itself), retint its stock theme from the WM palette,
//! let the module register its own families, and call `create` — all
//! inside ONE trusted entry into the isolate, so the module never holds a
//! second `&mut Cx` beside the VM. The root comes back minted in that
//! heap; the tile (`module_view.rs`) draws it; the executor answers the
//! assistant's calls through the bus's in-process leg (`ai_bus.rs`).
//!
//! Tearing one down, in order: the tile drops the root FIRST (so nothing
//! draws a widget whose heap is about to go), the instance's `shutdown`
//! runs in the isolate, the executor and the host's own root ref are
//! dropped, the isolate is freed — its script timers stop with it. What
//! the scope token does NOT yet reach — native timers, audio lanes,
//! native layers, HTTP requests the instance opened through the platform
//! — is the InstanceScope gap the next phase closes.

use crate::hub::ClientId;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;
use std::collections::HashMap;
use std::sync::mpsc::Receiver;

pub struct AppInstance {
    pub client: ClientId,
    pub module: &'static dyn AppModule,
    pub vm_id: SplashVmId,
    pub scope: InstanceScope,
    /// The n-th instance of this app in this session: `sheets.2`.
    pub instance_no: u64,
    pub root: WidgetRef,
    executor: Box<dyn ServiceExecutor>,
    shutdown: Option<Box<dyn FnOnce(&mut ScriptVm)>>,
    /// Results and publications the executor sent later.
    upstream: Receiver<ModuleUpstream>,
}

impl AppInstance {
    pub fn manifest(&self) -> ServiceManifest {
        self.executor.manifest()
    }
}

#[derive(Default)]
pub struct ModuleHost {
    instances: HashMap<ClientId, AppInstance>,
    next_scope: u64,
    per_app: HashMap<String, u64>,
    style: Option<desktop_style::StyleSheet>,
}

impl ModuleHost {
    /// Build one instance of `module` for the client id the WM gave it.
    /// `viewport` is the tile size the layout will give it.
    pub fn create(
        &mut self,
        cx: &mut Cx,
        client: ClientId,
        module: &'static dyn AppModule,
        open: ValidatedOpen,
        viewport: DVec2,
    ) -> Result<(), String> {
        if self.instances.contains_key(&client) {
            return Err(format!("client {client} already hosts an instance"));
        }
        self.next_scope += 1;
        let scope = InstanceScope::new(client, self.next_scope);
        let instance_no = {
            let n = self.per_app.entry(module.id().to_string()).or_insert(0);
            *n += 1;
            *n
        };
        // The storage jail: a namespace of the Cx storage API, one per
        // instance (§3b's mount and the web's IndexedDB sit under it).
        let storage = cx.storage(&format!("{}.{}", module.id(), instance_no));
        let (replies, upstream) = ReplySink::pair();
        let handles = InstanceHandles { scope, storage, viewport: Viewport { size: viewport }, replies };
        let vm_id = cx.alloc_splash_vm_with_network(false);
        let parts = cx.with_script_vm_id_trusted(vm_id, |vm| {
            // The isolate allocation strips `mod.res` — right for an
            // untrusted mini-app, wrong for a trusted native module whose
            // own widget families, and the desktop styles' fonts and icons
            // (`crate_resource("self:...")` in the iOS and Android
            // themes), load through it. Back in before anything evaluates.
            makepad_widgets::makepad_platform::script::res::script_mod(vm);
            // The isolate came up with the stock theme; the WM's palette
            // retints it exactly as it retints a child process's.
            if let Some(sheet)=&self.style {
                desktop_style::install(vm,sheet.clone());
                vm.with_reload(|vm| { makepad_widgets::widgets_mod(vm); desktop_style::apply_widgets(vm); });
            }
            makepad_wm_theme::apply(vm);
            module.register(vm);
            module.create(vm, open, handles)
        });
        log!(
            "wm: module instance {}.{} for client {} in isolate {:?} (scope {})",
            module.id(),
            instance_no,
            client,
            vm_id,
            scope
        );
        self.instances.insert(
            client,
            AppInstance {
                client,
                module,
                vm_id,
                scope,
                instance_no,
                root: parts.root,
                executor: parts.executor,
                shutdown: Some(parts.shutdown),
                upstream,
            },
        );
        Ok(())
    }

    pub fn apply_style(&mut self,cx:&mut Cx,sheet:&desktop_style::StyleSheet) {
        self.style=Some(sheet.clone());
        for instance in self.instances.values_mut() {
            cx.with_script_vm_id_trusted(instance.vm_id,|vm| {
                desktop_style::install(vm,sheet.clone());
                vm.with_reload(|vm| {
                    makepad_widgets::widgets_mod(vm);
                    desktop_style::apply_widgets(vm);
                    makepad_wm_theme::apply(vm);
                    instance.module.register(vm);
                });
                let source=instance.root.widget_type_id().and_then(|ty|vm.bx.heap.type_default_for_id(ty)).unwrap_or_else(||instance.root.script_source());
                instance.root.script_apply(vm,&Apply::ScriptReapply,&mut Scope::empty(),source.into());
            });
            instance.root.redraw(cx);
        }
    }

    pub fn is_module(&self, client: ClientId) -> bool {
        self.instances.contains_key(&client)
    }

    /// Every hosted instance.
    pub fn instances(&self) -> impl Iterator<Item = &AppInstance> {
        self.instances.values()
    }
    pub fn get(&self, client: ClientId) -> Option<&AppInstance> {
        self.instances.get(&client)
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// One of the assistant's calls, to the instance's executor.
    pub fn execute(&mut self, cx: &mut Cx, client: ClientId, call: &ServiceCall) -> Option<ExecOutcome> {
        let instance = self.instances.get_mut(&client)?;
        Some(instance.executor.execute(cx, call))
    }

    pub fn cancel(&mut self, cx: &mut Cx, client: ClientId, call_id: &str) {
        if let Some(instance) = self.instances.get_mut(&client) {
            instance.executor.cancel(cx, call_id);
        }
    }

    pub fn subscribe(
        &mut self,
        cx: &mut Cx,
        client: ClientId,
        sub_id: &str,
        topic: &str,
        filter: Option<&str>,
    ) {
        if let Some(instance) = self.instances.get_mut(&client) {
            instance.executor.subscribe(cx, sub_id, topic, filter);
        }
    }

    pub fn unsubscribe(&mut self, cx: &mut Cx, client: ClientId, sub_id: &str) {
        if let Some(instance) = self.instances.get_mut(&client) {
            instance.executor.unsubscribe(cx, sub_id);
        }
    }

    pub fn chat_open(&mut self, cx: &mut Cx, open: bool) {
        for instance in self.instances.values_mut() {
            instance.executor.chat_open(cx, open);
        }
    }

    /// Every result or publication an executor sent later, with its client.
    pub fn drain_upstream(&mut self) -> Vec<(ClientId, ModuleUpstream)> {
        let mut out = Vec::new();
        for (client, instance) in &self.instances {
            while let Ok(message) = instance.upstream.try_recv() {
                out.push((*client, message));
            }
        }
        out
    }

    /// End the instance: its shutdown runs in its isolate, then the isolate
    /// is freed. The caller has already cleared the tile's root.
    pub fn teardown(&mut self, cx: &mut Cx, client: ClientId) -> bool {
        let Some(mut instance) = self.instances.remove(&client) else {
            return false;
        };
        if let Some(shutdown) = instance.shutdown.take() {
            // A shutdown that panics (an instance already broken — the
            // reason it is being torn down) must not stop the teardown:
            // the isolate is freed either way.
            let vm_id = instance.vm_id;
            let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cx.with_script_vm_id_trusted(vm_id, |vm| shutdown(vm))
            }));
            if ended.is_err() {
                error!("wm: module instance {}.{} panicked in shutdown; freeing its isolate anyway", instance.module.id(), instance.instance_no);
            }
        }
        let vm_id = instance.vm_id;
        let label = format!("{}.{}", instance.module.id(), instance.instance_no);
        // The last refs into the isolate's heap go before the heap does.
        drop(instance);
        cx.free_splash_vm(vm_id);
        log!("wm: module instance {label} torn down; isolate {vm_id:?} freed");
        true
    }
}

#[cfg(test)]
mod tests {
    use makepad_widgets::*;

    /// `self:` inside a script module names the crate that module was
    /// written in, wherever it evaluates: the shell's icons stay the WM's
    /// in the main heap, and a widgets theme evaluated in a fresh isolate
    /// registers the widgets crate's files — never apps/wm's — under that
    /// isolate's own heap, resolvable by (heap, handle) alone.
    #[test]
    fn self_resources_name_their_own_crate_in_the_host_and_in_an_isolate() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::shell::ui::script_mod(vm);
        });
        let main = cx.with_vm(|vm| vm.bx.heap.heap_key());
        let wm_icon = "apps/wm/resources/icons/menu.svg".replace('/', std::path::MAIN_SEPARATOR_STR);
        let (path, handle) = {
            let resources = cx.script_data.resources.resources.borrow();
            let res = resources.iter().find(|r| r.abs_path.ends_with(&wm_icon)).expect("the shell's menu glyph is registered");
            let (_, handle) = res.handles.iter().copied().find(|(heap, _)| *heap == main).expect("by the main heap");
            (res.abs_path.clone(), handle)
        };
        assert!(std::path::Path::new(&path).is_file(), "{path}");
        assert_eq!(cx.get_resource_abs_path(main, handle).as_deref(), Some(path.as_str()));
        // A fresh isolate: the widgets' iOS theme registers the widgets
        // crate's own fonts and icons, none of them under apps/wm.
        let vm_id = cx.alloc_splash_vm_with_network(false);
        let isolate = cx.with_script_vm_id_trusted(vm_id, |vm| {
            makepad_widgets::makepad_platform::script::res::script_mod(vm);
            desktop_style::install(vm, desktop_style::StyleSheet::load(desktop_style::DesktopStyle::Ios));
            vm.with_reload(makepad_widgets::widgets_mod);
            vm.bx.heap.heap_key()
        });
        assert_ne!(isolate, main);
        let resources = cx.script_data.resources.resources.borrow();
        let isolate_paths: Vec<&str> = resources.iter().filter(|r| r.handles.iter().any(|(heap, _)| *heap == isolate)).map(|r| r.abs_path.as_str()).collect();
        assert!(!isolate_paths.is_empty(), "the theme registered resources in the isolate");
        assert!(isolate_paths.iter().all(|p| !p.contains(&"apps/wm/".replace('/', std::path::MAIN_SEPARATOR_STR))), "{isolate_paths:?}");
        assert!(isolate_paths.iter().any(|p| p.contains(&"widgets/".replace('/', std::path::MAIN_SEPARATOR_STR))), "{isolate_paths:?}");
        // The isolate's handle values overlap the main heap's; the pair keeps them apart.
        for res in resources.iter() {
            for (heap, handle) in &res.handles {
                if *heap == isolate {
                    if let Some(other) = cx.get_resource_abs_path(main, *handle) {
                        assert!(other != res.abs_path || res.handles.contains(&(main, *handle)));
                    }
                }
            }
        }
    }
}
