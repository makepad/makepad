use crate::makepad_draw::event::TouchUpdateEvent;

use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    view::*,
    label::*,
    widget::*
};

live_design!{
    link widgets;
    use link::widgets::*;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    pub TooltipBase = {{Tooltip}} {}
    pub Tooltip = <TooltipBase> {
        width: Fill,
        height: Fill,
        
        flow: Overlay
        align: {x: 0.0, y: 0.0}
        
        draw_bg: {
            fn pixel(self) -> vec4 {
                return vec4(0., 0., 0., 0.0)
            }
        }
        
        content: <View> {
            flow: Overlay
            width: Fit
            height: Fit
            
            <RoundedView> {
                width: Fit,
                height: Fit,
                
                padding: 16,
                
                draw_bg: {
                    color: #fff,
                    border_size: 1.0,
                    border_color: #D0D5DD,
                    radius: 2.
                }
                
                tooltip_label = <Label> {
                    width: 270,
                    draw_text: {
                        text_style: <THEME_FONT_REGULAR>{font_size: 9},
                        text_wrap: Word,
                        color: #000
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct Tooltip {
    #[rust]
    opened: bool,

    #[live]
    #[find]
    content: View,

    #[rust(DrawList2d::new(cx))]
    draw_list: DrawList2d,

    #[redraw]
    #[area]
    #[live]
    draw_bg: DrawQuad,
    #[layout]
    layout: Layout,
    #[walk]
    walk: Walk,
}

impl Widget for Tooltip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.opened {
            return;
        }

        self.content.handle_event(cx, event, scope);

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
                self.hide(cx);
            }
            Event::TouchUpdate(TouchUpdateEvent { touches, .. }) => {
                if touches.iter().any(|tp| matches!(tp.state, event::TouchState::Start)) {
                    self.hide(cx);
                }
            }
            _ => { }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        self.draw_list.begin_overlay_reuse(cx);
        
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, self.layout);
        self.draw_bg.begin(cx, self.walk, self.layout);

        if self.opened {
            let _ = self.content.draw_all(cx, scope);
        }

        self.draw_bg.end(cx);

        cx.end_pass_sized_turtle();
        self.draw_list.end(cx);

        DrawStep::done()
    }

    fn set_text(&mut self, cx:&mut Cx, text: &str) {
        self.label(ids!(tooltip_label)).set_text(cx, text);
    }
}

impl Tooltip {
    pub fn set_pos(&mut self, cx: &mut Cx, pos: Vec2d) {
        self.apply_over(
            cx,
            live! {
                content: { margin: { left: (pos.x), top: (pos.y) } }
            },
        );
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
    pub fn set_text(&mut self, cx:&mut Cx, text: &str) {
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