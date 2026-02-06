use crate::{makepad_derive_widget::*, makepad_draw::*, view::View, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SplashBase = #(Splash::register_widget(vm))

    mod.widgets.Splash = set_type_default() do mod.widgets.SplashBase{
        width: Fill height: Fit
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Splash {
    #[deref]
    pub view: View,
    #[live]
    body: ArcStringMut,
}

impl Splash {
    fn eval_body(&mut self, cx: &mut Cx) {
        let body = self.body.as_ref();
        if !body.is_empty() {
            let self_id = self as *const Self as u64;
            let code = format!("use mod.prelude.widgets.*\n__script_source__{{~@HI; {} }};", body);
           
            let script_mod = ScriptMod {
                cargo_manifest_path: String::new(),
                module_path: String::new(),
                file: String::new(),
                line: self_id as usize,
                column: 0,
                code,
                values: vec![],
            };
            let view = &mut self.view;
            cx.with_vm(|vm| {
                let source = view.script_source();
                let value = vm.eval_with_source(script_mod, source);
                view.script_apply(vm, &Apply::Reload, &mut Scope::empty(), value);
            });
        }
    }
}

impl Widget for Splash {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let mut nodes = Vec::new();
        self.view.widget_tree_walk(&mut nodes);
        let tree = WidgetTree { nodes };
        cx.with_vm(|vm| {
            log!("{}", tree.display(vm.heap()));
        });
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
