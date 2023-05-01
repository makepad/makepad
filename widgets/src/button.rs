use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*
    }
};
live_design!{
    import makepad_draw::shader::std::*;
    
    
    Button= {{Button}} {
        draw_label: {
            instance hover: 0.0
            instance pressed: 0.0
            text_style: {
                font: {
                    //path: d"resources/IBMPlexSans-SemiBold.ttf"
                }
                font_size: 11.0
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #c,
                        self.hover
                    ),
                    #9,
                    self.pressed
                )
            }
        }
        
        draw_icon:{
            instance hover: 0.0
            instance pressed: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #c,
                        self.hover
                    ),
                    #9,
                    self.pressed
                )
            }
        }
        
        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: 3.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 1.0;
                let body = mix(mix(#53, #5c, self.hover), #33, self.pressed);
                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(body_transp, mix(#6d, #1f, self.pressed), max(0.0, grad_top - sdf.pos.y) / grad_top);
                let bot_gradient = mix(
                    mix(body_transp, #5c, self.pressed),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );
                
                // the little drop shadow at the bottom
                let shift_inward = self.border_radius + 4.0;
                sdf.move_to(shift_inward, self.rect_size.y - self.border_radius);
                sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y - self.border_radius);
                sdf.stroke(
                    mix(mix(#2f, #1f, self.hover), #0000, self.pressed),
                    self.border_radius
                )
                
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )
                sdf.fill_keep(body)
                
                sdf.stroke(
                    bot_gradient,
                    1.0
                )
                
                return sdf.result
            }
        }
        
        walk: {
            width: Fit,
            height: Fit,
            margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0},
        }
        
        layout: {
            align: {x: 0.5, y: 0.5},
            padding: {left: 14.0, top: 10.0, right: 14.0, bottom: 10.0}
        }
        
        state: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0, hover: 0.0}
                        draw_icon: {pressed: 0.0, hover: 0.0}
                        draw_label: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_icon: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_label: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }
                
                pressed = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_icon: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_label: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
        }
    }
}

#[derive(Clone, WidgetAction)]
pub enum ButtonAction {
    None,
    Click,
    Press,
    Release
}

#[derive(Live)]
pub struct Button {
    #[live] state: State,
    
    #[live] draw_bg: DrawQuad,
    #[live] draw_label: DrawText,
    #[live] draw_icon: DrawIcon,
    #[live] icon_walk: Walk,
    #[live] walk: Walk,
    
    #[live] layout: Layout,
    #[live] label: String
}

impl LiveHook for Button{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, Button)
    }
}

impl Widget for Button{
   fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(),uid));
        });
    }

    fn get_walk(&self)->Walk{
        self.walk
    }
    
    fn redraw(&mut self, cx:&mut Cx){
        self.draw_bg.redraw(cx)
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        let _ = self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

impl Button {
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, ButtonAction)) {
        self.state_handle_event(cx, event);
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                dispatch_action(cx, ButtonAction::Press);
                self.animate_state(cx, id!(hover.pressed));
            },
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                 self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            }
            Hit::FingerUp(fe) => if fe.is_over {
                dispatch_action(cx, ButtonAction::Click);
                if fe.device.has_hovers() {
                    self.animate_state(cx, id!(hover.on));
                }
                else{
                    self.animate_state(cx, id!(hover.off));
                }
            }
            else {
                dispatch_action(cx, ButtonAction::Release);
                self.animate_state(cx, id!(hover.off));
            }
            _ => ()
        };
    }
    
    pub fn draw_label(&mut self, cx: &mut Cx2d, label: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_label.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_label.draw_walk(cx, Walk::fit(), Align::default(), &self.label);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_bg.end(cx);
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct ButtonRef(WidgetRef); 

impl ButtonRef {
    pub fn set_label(&self, text:&str){
        if let Some(mut inner) = self.borrow_mut(){
            inner.label.clear();
            inner.label.push_str(text);
        }
    }
    
    pub fn clicked(&self, actions:&WidgetActions) -> bool {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let ButtonAction::Click = item.action() {
                return true
            }
        }
        false
    }
}

#[derive(Clone, WidgetSet)]
pub struct ButtonSet(WidgetSet);
impl ButtonSet{
    pub fn clicked(&self, actions: &WidgetActions)->bool{
        for button in self.iter(){
            if button.clicked(actions){
                return true
            }
        }
        false
    }
}

