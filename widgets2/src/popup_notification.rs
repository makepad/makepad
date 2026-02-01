use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    view::*,
    widget::*
};

script_mod!{
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    
    mod.widgets.PopupNotificationBase = #(PopupNotification::register_widget(vm))
    
    mod.widgets.PopupNotification = mod.widgets.PopupNotificationBase{
        width: Fill
        height: Fill
        flow: Overlay
        align: Align{x: 1.0 y: 0.0}
        
        draw_bg +: {
            pixel: fn() {
                return vec4(0. 0. 0. 0.0)
            }
        }
        
        $content: View{
            flow: Overlay
            width: Fit
            height: Fit
            
            cursor: MouseCursor.Default
            capture_overload: true
        }
    }
}

#[derive(Script, Widget)]
pub struct PopupNotification {
    #[source] source: ScriptObjectRef,
    
    #[deref]
    view: View,

    #[rust] draw_list: Option<DrawList2d>,

    #[live] draw_bg: DrawQuad,

    #[rust] opened: bool,
}

impl ScriptHook for PopupNotification {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }
    
    fn on_after_apply(&mut self, vm: &mut ScriptVm, _apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        vm.with_cx_mut(|cx| {
            if let Some(draw_list) = &self.draw_list {
                draw_list.redraw(cx);
            }
        });
    }
}

impl Widget for PopupNotification {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.opened {
            return;
        }

        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let draw_list = self.draw_list.as_mut().unwrap();
        draw_list.begin_overlay_reuse(cx);
        
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, self.view.layout);
        self.draw_bg.begin(cx, self.view.walk, self.view.layout);

        if self.opened {
            let _ = self.view.draw_all(cx, scope);
        }

        self.draw_bg.end(cx);

        cx.end_pass_sized_turtle();
        self.draw_list.as_mut().unwrap().end(cx);

        DrawStep::done()
    }
}

impl PopupNotification {
    pub fn open(&mut self, cx: &mut Cx) {
        self.opened = true;
        self.redraw(cx);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        self.opened = false;
        self.draw_bg.redraw(cx);
    }
}

impl PopupNotificationRef {
    pub fn is_open(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.opened
        } else {
            false
        }
    }

    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }
}
