use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    view::View,
    widget::*,
    widget_async::{CxSplashVmExt, SplashVmId, MAIN_SPLASH_VM_ID},
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
    #[rust]
    vm_id: SplashVmId,
}

const SPLASH_PREFIX: &str = "use mod.prelude.widgets.*\nView{height:Fit, ";
const SPLASH_NET_PREFIX: &str = "use mod.prelude.widgets.*\nuse mod.net\nView{height:Fit, ";
const SPLASH_EVAL_INSTRUCTION_LIMIT: usize = 200_000;

impl Splash {
    /// Stable identity for the streaming script body, based on pointer address.
    fn self_id(&self) -> usize {
        self as *const Self as usize
    }

    fn eval_body(&mut self, cx: &mut Cx) {
        let body = self.body.as_ref();
        if body.is_empty() {
            return;
        }

        if self.vm_id == MAIN_SPLASH_VM_ID {
            self.vm_id = cx.alloc_splash_vm_with_network(self.allow_net);
        }

        let self_id = self.self_id();
        // Full code string: prefix + body (no closing - parser auto-closes)
        let prefix = if self.allow_net {
            SPLASH_NET_PREFIX
        } else {
            SPLASH_PREFIX
        };
        let code = format!("{}{}", prefix, body);

        // ScriptMod identity is stable (same file/line/column each call)
        let script_mod = ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: String::new(),
            line: self_id,
            column: 0,
            code: String::new(),
            values: vec![],
        };

        let vm_id = self.vm_id;
        let new_view = cx.with_script_vm_id(vm_id, |vm| {
            let value = vm.with_instruction_limit(SPLASH_EVAL_INSTRUCTION_LIMIT, |vm| {
                vm.eval_with_append_source(script_mod, &code, NIL.into())
            });
            if !value.is_err() && !value.is_nil() {
                Some(View::script_from_value(vm, value))
            } else {
                None
            }
        });

        if let Some(view) = new_view {
            self.view = view;
            // Make `ui` a global in this splash's VM (pointing at the freshly-built view root) so
            // helper `fn`s inside the block can use `ui.<id>.set_text(...)`, not just inline
            // handlers. Without this, calculators/forms that route through a helper silently fail.
            crate::widget_async::inject_splash_ui_handle(cx, self.vm_id, self.view.widget_uid());
            cx.widget_tree_mark_dirty(self.uid);
        }
    }
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
            self.eval_body(cx);
            self.redraw(cx);
        }
    }
}

impl SplashRef {
    pub fn set_text(&self, cx: &mut Cx, v: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, v);
        }
    }
}
