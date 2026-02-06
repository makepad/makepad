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
    #[rust]
    eval_count: u64
}

impl Splash {
    fn eval_body(&mut self, cx: &mut Cx) {
        let body = self.body.as_ref();
        if !body.is_empty() {
            let self_id = self as *const Self as u64 + self.eval_count;
            self.eval_count += 1;
            let code = format!("use mod.prelude.widgets.*View{{height:Fit,  {} }};", body);
           
            let script_mod = ScriptMod {
                cargo_manifest_path: String::new(),
                module_path: String::new(),
                file: String::new(),
                line: self_id as usize,
                column: 0,
                code,
                values: vec![],
            };
            //let view = &mut self.view;
            let body_preview: String = body.chars().take(100).collect();
            log!("Splash eval_body: body_len={} preview={:?}", body.len(), body_preview);
            cx.with_vm(|vm| {
                //let source = view.script_source();
                let value = vm.eval_with_source(script_mod, NIL.into());
                // Only apply if the eval didn't produce an error
                if !value.is_err() && !value.is_nil() {
                    // Debug: log the value structure
                    if let Some(obj) = value.as_object() {
                        let mut vec_info = String::new();
                        vm.vec_with(obj, |vm, vec| {
                            vec_info = format!("vec_len={}", vec.len());
                            for (i, kv) in vec.iter().take(3).enumerate() {
                                vec_info.push_str(&format!(" [{}]={:?}", i, kv.value.value_type()));
                                // If it's an object, also show its vec len
                                if let Some(inner_obj) = kv.value.as_object() {
                                    let mut inner_vec_len = 0;
                                    vm.vec_with(inner_obj, |_, inner_vec| {
                                        inner_vec_len = inner_vec.len();
                                    });
                                    vec_info.push_str(&format!("(inner_vec={})", inner_vec_len));
                                }
                            }
                        });
                        log!("Splash eval result: obj={:?} {}", obj, vec_info);
                    } else {
                        log!("Splash eval result: value_type={:?}", value.value_type());
                    }
                    self.view = View::script_from_value(vm, value);
                    //view.script_apply(vm, &Apply::Reload, &mut Scope::empty(), value);
                } else {
                    log!("Splash eval SKIPPED: is_err={} is_nil={}", value.is_err(), value.is_nil());
                }
            });
        }
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
