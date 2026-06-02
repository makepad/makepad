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
    #[rust]
    vm_id: SplashVmId,
}

const SPLASH_PREFIX: &str = "use mod.prelude.widgets.*View{height:Fit, ";
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
            self.vm_id = cx.alloc_splash_vm();
        }

        let self_id = self.self_id();
        // Full code string: prefix + body (no closing - parser auto-closes)
        let code = format!("{}{}", SPLASH_PREFIX, body);

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
            self.unregister_view_owners(cx);
            self.view = view;
            self.register_view_owners(cx);
            cx.widget_tree_mark_dirty(self.uid);
        }
    }

    fn register_view_owners(&self, cx: &mut Cx) {
        Self::register_view_owner(cx, &self.view, self.vm_id);
        self.view.children(&mut |_, child| {
            Self::register_widget_ref_owner(cx, &child, self.vm_id);
        });
    }

    fn unregister_view_owners(&self, cx: &mut Cx) {
        Self::unregister_view_owner(cx, &self.view);
        self.view.children(&mut |_, child| {
            Self::unregister_widget_ref_owner(cx, &child);
        });
    }

    fn register_view_owner(cx: &mut Cx, view: &View, vm_id: SplashVmId) {
        cx.register_widget_vm_id(view.widget_uid(), vm_id);
    }

    fn unregister_view_owner(cx: &mut Cx, view: &View) {
        cx.unregister_widget_vm_id(view.widget_uid());
    }

    fn register_widget_ref_owner(cx: &mut Cx, widget: &WidgetRef, vm_id: SplashVmId) {
        cx.register_widget_vm_id(widget.widget_uid(), vm_id);
        widget.children(&mut |_, child| {
            Self::register_widget_ref_owner(cx, &child, vm_id);
        });
    }

    fn unregister_widget_ref_owner(cx: &mut Cx, widget: &WidgetRef) {
        cx.unregister_widget_vm_id(widget.widget_uid());
        widget.children(&mut |_, child| {
            Self::unregister_widget_ref_owner(cx, &child);
        });
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

impl Widget for Splash {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
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
