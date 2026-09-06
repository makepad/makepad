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
            cx.with_script_vm_id_trusted(instance.vm_id, |vm| shutdown(vm));
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

#[cfg(all(test, feature="app-sheets"))]
mod style_tests {
    use super::*;
    #[test]
    fn module_restyle_updates_custom_roles_and_keeps_instance() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        cx.with_vm(makepad_widgets::script_mod);
        let mut host=ModuleHost::default();
        let module=&makepad_sheets::module::SHEETS_MODULE;
        let open=module.open_schema().validate("{}", &[]).unwrap();
        host.create(&mut cx,1,module,open,dvec2(900.0,700.0)).unwrap();
        let uid=host.get(1).unwrap().root.widget_uid();
        host.apply_style(&mut cx,&desktop_style::StyleSheet::load(desktop_style::DesktopStyle::Macos));
        let instance=host.get(1).unwrap();
        assert_eq!(instance.root.widget_uid(),uid);
        cx.with_script_vm_id_trusted(instance.vm_id,|vm| {
            let palette=makepad_wm_theme::current_for_vm(vm).unwrap();
            assert_eq!(palette.get("background"),Some("#ececec"));
            let sheets=vm.module(id!(sheets));
            assert_eq!(vm.bx.heap.value(sheets,id!(bg).into(),NoTrap).as_color(),Some(0xecececff));
            assert!(vm.take_errors().is_empty());
        });
        host.teardown(&mut cx,1);
    }
}
