use crate::makepad_draw::event::TouchUpdateEvent;

use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    view::*,
    label::*,
    widget::*
};

script_mod!{
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    
    mod.widgets.TooltipBase = #(Tooltip::register_widget(vm))
    
    mod.widgets.Tooltip = mod.widgets.TooltipBase{
        width: Fill
        height: Fill
        
        flow: Overlay
        align: Align{x: 0.0 y: 0.0}
        
        draw_bg +: {
            pixel: fn() {
                return vec4(0. 0. 0. 0.0)
            }
        }
        
        flow: Overlay
        width: Fit
        height: Fit
            
        RoundedView{
            width: Fit
            height: Fit
                
            padding: 16
                
            draw_bg +: {
                color: #fff
                border_size: 1.0
                border_color: #D0D5DD
                radius: 2.
            }
                
            tooltip_label := Label{
                width: 270
                draw_text +: {
                    text_style: theme.font_regular{font_size: 9}
                    //text_wrap: TextWrap.Word
                    color: #000
                }
            }
        }
    }
}

#[derive(Script, Widget)]
pub struct Tooltip {
    #[source] source: ScriptObjectRef,
    
    #[deref]
    view: View,

    #[rust] draw_list: Option<DrawList2d>,

    #[live] draw_bg: DrawQuad,

    #[rust] opened: bool,
    
    /// The position where the tooltip should be displayed
    #[rust] tooltip_pos: Vec2d,
}

impl ScriptHook for Tooltip {
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

impl Widget for Tooltip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.opened {
            return;
        }

        let content = self.view.widget(ids!(content));
        content.handle_event(cx, event, scope);

        // Hide the tooltip if any kind of user interaction occurs (taps/clicks, drags, scrolls, etc).
        //
        // Typically you don't handle raw events, but we do so here because:
        // 1. We don't want to impact the way that hit handling occurs for other views.
        // 2. We don't care about the details of the hit, only the fact that it happened.
        match event {
            Event::BackPressed { .. }
            | Event::MouseDown(_)
            | Event::MouseUp(_)
            | Event::Scroll(_) => {
               // self.hide(cx);
            }
            Event::TouchUpdate(TouchUpdateEvent { touches, .. }) => {
                if touches.iter().any(|tp| matches!(tp.state, event::TouchState::Start)) {
                   //self.hide(cx);
                }
            }
            _ => { }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let draw_list = self.draw_list.as_mut().unwrap();
        draw_list.begin_overlay_reuse(cx);
        
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, self.view.layout);
        self.draw_bg.begin(cx, self.view.walk, self.view.layout);

        if self.opened {
            let content_walk = self.view.walk(cx).with_abs_pos(self.tooltip_pos);
            self.view.draw_walk_all(cx, scope, content_walk);
        }

        self.draw_bg.end(cx);

        cx.end_pass_sized_turtle();
        self.draw_list.as_mut().unwrap().end(cx);

        DrawStep::done()
    }

    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        self.label(ids!(tooltip_label)).set_text(cx, text);
    }
}

impl Tooltip {
    pub fn set_pos(&mut self, _cx: &mut Cx, pos: Vec2d) {
        self.tooltip_pos = pos;
    }

    pub fn show(&mut self, cx: &mut Cx) {
        self.opened = true;
        self.redraw(cx);
    }

    pub fn show_with_options(&mut self, cx: &mut Cx, pos: Vec2d, text: &str) {
        self.set_text(cx, text);
        self.set_pos(cx, pos);
        self.show(cx);
    }

    pub fn hide(&mut self, cx: &mut Cx) {
        self.opened = false;
        self.redraw(cx);
    }
}

impl TooltipRef {
    pub fn set_text(&mut self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, text);
        }
    }

    pub fn set_pos(&self, cx: &mut Cx, pos: Vec2d) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_pos(cx, pos);
        }
    }

    pub fn show(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show(cx);
        }
    }

    pub fn show_with_options(&self, cx: &mut Cx, pos: Vec2d, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_with_options(cx, pos, text);
        }
    }

    pub fn hide(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.hide(cx);
        }
    }
}
