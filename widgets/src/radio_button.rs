use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;


    DrawRadioButton = {{DrawRadioButton}} {

        uniform size: 7.0;

        instance color_active: #00000000
        instance color_inactive: #x99EEFF
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            match self.radio_type {
                RadioType::Round => {
                    let sz = self.size;
                    let left = sz + 1.;
                    let c = vec2(left + sz, self.rect_size.y * 0.5);
                    sdf.circle(left, c.y, sz);
                    sdf.fill(#2);
                    let isz = sz * 0.5;
                    sdf.circle(left, c.y, isz);
                    sdf.fill(mix(#fff0, #f, self.selected));
                }
                RadioType::Tab => {
                    let sz = self.size;
                    let left = 0.;
                    let c = vec2(left, self.rect_size.y);
                    sdf.rect(
                        -1., 0.,
                        self.rect_size.x + 2.0,
                        self.rect_size.y 
                    );
                    sdf.fill(mix(self.color_inactive, self.color_active, self.selected));
                }
            }
            return sdf.result
        }


    }
    
    RadioButton = {{RadioButton}} {
        draw_label: {
            instance hover: 0.0
            instance focus: 0.0
            instance selected: 0.0
            instance color_unselected: #x00000088
            instance color_unselected_hover: #x000000CC
            instance color_selected: #xFFFFFF66
            color: #9
            text_style: {
                font: {
                    //path: d"resources/ibmplexsans-semibold.ttf"
                }
                font_size: 9.5
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color_unselected,
                        self.color_unselected_hover,
                        self.hover
                    ),
                    self.color_selected,
                    self.selected
                )
            }
        }
        
        walk: {
            width: Fit,
            height: Fit
        }

        label_walk: {
            margin: {top: 4.5, bottom: 4.5, left: 8, right: 8}
            width: Fit,
            height: Fit,
        }
        
        
        label_align: {
            y: 0.0
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_radio: {hover: 0.0}
                        draw_label: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_radio: {hover: 1.0}
                        draw_label: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_radio: {focus: 0.0}
                        draw_label: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_radio: {focus: 1.0}
                        draw_label: {focus: 1.0}
                    }
                }
            }
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_radio: {selected: 0.0}
                        draw_label: {selected: 0.0}
                    }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_radio: {selected: 1.0}
                        draw_label: {selected: 1.0}
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawRadioButton {
    draw_super: DrawQuad,
    radio_type: RadioType,
    hover: f32,
    focus: f32,
    selected: f32
}


#[derive(Live, LiveHook)]
#[repr(u32)]
pub enum RadioType {
    #[pick] Round = shader_enum(1),
    Tab = shader_enum(2),
}

#[derive(Live, LiveHook)]
#[live_design_with{widget_factory!(cx, RadioButton)}]
pub struct RadioButton {
    draw_radio: DrawRadioButton,
    
    walk: Walk,
    
    value: LiveValue,
    
    layout: Layout,
    state: State,
    
    label_walk: Walk,
    label_align: Align,
    draw_label: DrawText,
    label: String,
    
    bind: String,
}

#[derive(Clone, WidgetAction)]
pub enum RadioButtonAction {
    Clicked,
    None
}


impl RadioButton {
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, RadioButtonAction)) {
        self.state_handle_event(cx, event);
        
        match event.hits(cx, self.draw_radio.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animate_state(cx, id!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                if self.state.is_in_state(cx, id!(selected.off)) {
                    self.animate_state(cx, id!(selected.on));
                    dispatch_action(cx, RadioButtonAction::Clicked);
                }
            },
            Hit::FingerUp(_fe) => {
                
            }
            Hit::FingerMove(_fe) => {
                
            }
            _ => ()
        }
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_radio.begin(cx, walk, self.layout);
        self.draw_label.draw_walk(cx, self.label_walk, self.label_align, &self.label);
        self.draw_radio.end(cx);
    }
}

impl Widget for RadioButton {
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_radio.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct RadioButtonRef(WidgetRef);

impl RadioButtonRef{
    fn unselect(&self, cx:&mut Cx){
        if let Some(mut inner) = self.borrow_mut(){
            inner.animate_state(cx, id!(selected.off));
        }
    }
}

#[derive(Clone, WidgetSet)]
pub struct RadioButtonSet(WidgetSet);

impl RadioButtonSet{
    
    pub fn selected(&self, cx: &mut Cx, actions: &WidgetActions)->Option<usize>{
        for action in actions{
            match action.action() {
                RadioButtonAction::Clicked => if let Some(index) = self.0.iter().position(|v| v.widget_uid() == action.widget_uid){
                    for (i, item) in self.0.iter().enumerate(){
                        if i != index{
                            RadioButtonRef(item).unselect(cx);
                        }
                    }
                    return Some(index);
                }
                _ => ()
            }
        }
        None
    }
    
    pub fn selected_to_visible(&self, cx: &mut Cx, ui:&WidgetRef, actions: &WidgetActions, paths:&[&[LiveId]] ) {
        // find a widget action that is in our radiogroup
        if let Some(index) = self.selected(cx, actions){
            // ok now we set visible
            for (i,path) in paths.iter().enumerate(){
                let mut widget = ui.get_widget(path);
                widget.apply_over(cx, live!{visible:(i == index)});
                widget.redraw(cx);
            }
        }
    }
}
