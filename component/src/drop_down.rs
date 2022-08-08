use {
    std::rc::Rc,
    std::cell::RefCell,
    crate::{
        list_box::ListBox,
        makepad_draw_2d::*,
        button_logic::*,
        frame::*
    }
};
pub use crate::button_logic::ButtonAction;

live_register!{
    import makepad_draw_2d::shader::std::*;
    import makepad_component::list_box::ListBox;
    
    DrawLabelText: {{DrawLabelText}} {
        text_style: {
            //font_size: 11.0
        }
        fn get_color(self) -> vec4 {
            return mix(
                mix(
                    #9,
                    #f,
                    self.hover
                ),
                #9,
                self.pressed
            )
        }
    }
    
    DropDown: {{DropDown}} {
        bg: {
            instance hover: 0.0
            instance pressed: 0.0
            
            const BORDER_RADIUS: 0.5
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    BORDER_RADIUS
                )
                sdf.fill(mix(#2, #3, self.hover));
                // lets draw a little triangle in the corner
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 3;
                
                sdf.move_to(c.x - sz, c.y - sz);
                sdf.line_to(c.x + sz, c.y - sz);
                sdf.line_to(c.x, c.y + sz);
                sdf.close_path();
                
                sdf.fill(mix(#a, #f, self.hover));
                
                return sdf.result
            }
        }
        
        walk: {
            width: Size::Fill,
            height: Size::Fit,
            margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0},
        }
        
        layout: {
            align: {x: 0., y: 0.},
            padding: {left: 4.0, top: 4.0, right: 4.0, bottom: 4.0}
        }
        
        list_box: ListBox {
            scroll_view: {view: {is_overlay: true, always_redraw:true}}
        }
        
        state: {
            hover = {
                default: off,
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        bg: {pressed: 0.0, hover: 0.0}
                        label: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Play::Forward {duration: 0.1}
                        pressed: Play::Forward {duration: 0.01}
                    }
                    apply: {
                        bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        label: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }
                
                pressed = {
                    from: {all: Play::Forward {duration: 0.2}}
                    apply: {
                        bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        label: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
        }
    }
}

#[derive(Live)]
#[live_register(frame_component!(DropDown))]
pub struct DropDown {
    state: State,
    
    bg: DrawQuad,
    label: DrawLabelText,
    
    walk: Walk,
    
    list_box: Option<LivePtr>,
    
    items: Vec<String>,
    
    is_open: bool,
    selected_item: usize,
    
    layout: Layout,
}

#[derive(Default, Clone)]
struct ListBoxGlobal {
    map: Rc<RefCell<ComponentMap<LivePtr, ListBox >> >
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawLabelText {
    draw_super: DrawText,
    hover: f32,
    pressed: f32,
}

impl LiveHook for DropDown {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if self.list_box.is_none() || !from.is_from_doc() {
            return
        }
        let lbg = cx.global::<ListBoxGlobal>().clone();
        let mut map = lbg.map.borrow_mut();
        
        // when live styling clean up old style references
        map.retain( | k, _ | cx.live_registry.borrow().generation_valid(*k));
        
        let list_box = self.list_box.unwrap();
        map.get_or_insert(cx, list_box, | cx | {
            ListBox::new_from_ptr(cx, Some(list_box))
        });
    }
}
impl DropDown {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, ButtonAction)) {
        self.state_handle_event(cx, event);
        let state = button_logic_handle_event(cx, event, self.bg.area(), &mut | cx, action | {
            match action {
                ButtonAction::IsPressed => {
                    self.is_open = true;
                    self.bg.redraw(cx);
                }
                ButtonAction::IsUp => {
                    self.is_open = false;
                    self.bg.redraw(cx);
                }
                _ => ()
            }
        });
        if let Some(state) = state {
            match state {
                ButtonState::Pressed => {
                    self.animate_state(cx, ids!(hover.pressed));
                }
                ButtonState::Default => self.animate_state(cx, ids!(hover.off)),
                ButtonState::Hover => self.animate_state(cx, ids!(hover.on)),
            }
        };
    }
    
    pub fn draw_label(&mut self, cx: &mut Cx2d, label: &str) {
        self.bg.begin(cx, self.walk, self.layout);
        self.label.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.bg.end(cx);
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.bg.begin(cx, walk, self.layout);
        if let Some(val) = self.items.get(self.selected_item) {
            self.label.draw_walk(cx, Walk::fit(), Align::default(), val);
        }
        self.bg.end(cx);
        if self.is_open && self.list_box.is_some(){
            // ok so how will we solve this one
            
            let lbg = cx.global::<ListBoxGlobal>().clone();
            let mut map = lbg.map.borrow_mut();
            let lb = map.get_mut(&self.list_box.unwrap()).unwrap();
            
            if lb.begin(cx, lb.get_walk()).not_redrawing(){
                return;
            };
            for (i, item) in self.items.iter().enumerate(){
                let node_id = id_num!(listbox,i as u64).into();
                lb.draw_node(cx, node_id, item);
            }
            lb.end(cx);
        }
    }
}

impl FrameComponent for DropDown {
    fn bind_read(&mut self, _cx: &mut Cx, _nodes: &[LiveNode]) {
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.bg.redraw(cx);
    }
    
    fn handle_component_event(&mut self, cx: &mut Cx, event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, FrameActionItem)) {
        self.handle_event(cx, event, &mut | _cx, _action | {
            //dispatch_action(cx, FrameActionItem::new(action.into()).bind_delta(delta))
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk, _self_uid: FrameUid) -> FrameDraw {
        self.draw_walk(cx, walk);
        FrameDraw::done()
    }
}
