use {
    std::rc::Rc,
    std::cell::RefCell,
    crate::{
        makepad_derive_frame::*,
        popup_menu::{PopupMenu, PopupMenuAction},
        makepad_draw_2d::*,
        frame::*
    }
};
pub use crate::button_logic::ButtonAction;

live_register!{
    import makepad_draw_2d::shader::std::*;
    import makepad_component::popup_menu::PopupMenu;
    
    DrawLabelText: {{DrawLabelText}} {
        fn get_color(self) -> vec4 {
            return mix(
                mix(
                    mix(
                        #9,
                        #b,
                        self.focus
                    ),
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
            instance focus: 0.0,
            const BORDER_RADIUS: 0.5
            
            fn get_bg(self, inout sdf:Sdf2d){
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    BORDER_RADIUS
                )
                sdf.fill(mix(#2, #3, self.hover));
            }
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                self.get_bg(sdf);
                // lets draw a little triangle in the corner
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 2.5;
                
                sdf.move_to(c.x - sz, c.y - sz);
                sdf.line_to(c.x + sz, c.y - sz);
                sdf.line_to(c.x, c.y + sz * 0.75);
                sdf.close_path();
                
                sdf.fill(mix(#8, #c, self.hover));
                
                return sdf.result
            }
        }
        
        walk: {
            width: Size::Fill,
            height: Size::Fit,
            margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0},
        }
        
        layout: {
            clip_x: true,
            align: {x: 0., y: 0.},
            padding: {left: 5.0, top: 5.0, right: 4.0, bottom: 5.0}
        }
        
        popup_menu: PopupMenu {}
        selected_item: 0
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
            focus = {
                default: off
                off = {
                    from: {all: Play::Snap}
                    apply: {
                        bg: {focus: 0.0},
                        label: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {
                        bg: {focus: 1.0},
                        label: {focus: 1.0}
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
    
    bind: String,
    bind_enum: String,
    
    popup_menu: Option<LivePtr>,
    
    items: Vec<String>,
    display: Vec<String>,
    
    #[rust] last_rect: Option<Rect>,
    #[rust] is_open: bool,
    selected_item: usize,
    
    layout: Layout,
}

#[derive(Default, Clone)]
struct PopupMenuGlobal {
    map: Rc<RefCell<ComponentMap<LivePtr, PopupMenu >> >
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawLabelText {
    draw_super: DrawText,
    focus: f32,
    hover: f32,
    pressed: f32,
}

impl LiveHook for DropDown {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if self.popup_menu.is_none() || !from.is_from_doc() {
            return
        }
        let global = cx.global::<PopupMenuGlobal>().clone();
        let mut map = global.map.borrow_mut();
        
        // when live styling clean up old style references
        map.retain( | k, _ | cx.live_registry.borrow().generation_valid(*k));
        
        let list_box = self.popup_menu.unwrap();
        map.get_or_insert(cx, list_box, | cx | {
            PopupMenu::new_from_ptr(cx, Some(list_box))
        });
    }
}
#[derive(Clone, FrameAction)]
pub enum DropDownAction {
    Select(usize),
    None
}


impl DropDown {
    
    pub fn set_open(&mut self, cx: &mut Cx) {
        self.is_open = true;
        self.bg.redraw(cx);
        let global = cx.global::<PopupMenuGlobal>().clone();
        let mut map = global.map.borrow_mut();
        let lb = map.get_mut(&self.popup_menu.unwrap()).unwrap();
        let node_id = LiveId(self.selected_item as u64).into();
        lb.init_select_item(node_id);
        self.last_rect = Some(self.bg.area().get_rect(cx));
    }
    
    pub fn set_closed(&mut self, cx: &mut Cx) {
        self.is_open = false;
        self.bg.redraw(cx);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, &DropDown, DropDownAction)) {
        self.state_handle_event(cx, event);
        
        if self.is_open && self.popup_menu.is_some() {
            // ok so how will we solve this one
            let global = cx.global::<PopupMenuGlobal>().clone();
            let mut map = global.map.borrow_mut();
            let menu = map.get_mut(&self.popup_menu.unwrap()).unwrap();
            let mut close = false;
            menu.handle_event(cx, event, self.bg.area(), &mut | cx, action | {
                match action {
                    PopupMenuAction::WasSweeped(_node_id) => {
                        //dispatch_action(cx, PopupMenuAction::WasSweeped(node_id));
                    }
                    PopupMenuAction::WasSelected(node_id) => {
                        //dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                        self.selected_item = node_id.0.0 as usize;
                        dispatch_action(cx, self, DropDownAction::Select(self.selected_item));
                        self.bg.redraw(cx);
                        close = true;
                    }
                    _ => ()
                }
            });
            if close {
                self.set_closed(cx);
            }
            // check if we clicked outside of the popup menu
            if let Event::FingerDown(fd) = event {
                if !menu.menu_contains_pos(cx, fd.abs) {
                    self.set_closed(cx);
                    self.animate_state(cx, ids!(hover.off));
                }
            }
            if let Event::FingerUp(fd) = event {
                if !menu.menu_contains_pos(cx, fd.abs) {
                    self.set_closed(cx);
                    self.animate_state(cx, ids!(hover.off));
                }
            }
        }
        
        match event.hits(cx, self.bg.area()) {
            Hit::KeyFocusLost(_) => {
                self.animate_state(cx, ids!(focus.off));
                self.set_closed(cx);
                self.animate_state(cx, ids!(hover.off));
                self.bg.redraw(cx);
            }
            Hit::KeyFocus(_) => {
                self.animate_state(cx, ids!(focus.on));
            }
            Hit::KeyDown(ke) => match ke.key_code {
                KeyCode::ArrowUp => {
                    if self.selected_item > 0 {
                        self.selected_item -= 1;
                        dispatch_action(cx, self, DropDownAction::Select(self.selected_item));
                        self.set_closed(cx);
                        self.bg.redraw(cx);
                    }
                }
                KeyCode::ArrowDown => {
                    if self.items.len() > 0 && self.selected_item < self.items.len() - 1 {
                        self.selected_item += 1;
                        dispatch_action(cx, self, DropDownAction::Select(self.selected_item));
                        self.set_closed(cx);
                        self.bg.redraw(cx);
                    }
                },
                _ => ()
            }
            Hit::FingerDown(_fe) => {
                cx.set_key_focus(self.bg.area());
                self.set_open(cx);
                self.animate_state(cx, ids!(hover.pressed));
            },
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animate_state(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, ids!(hover.off));
            }
            Hit::FingerUp(fe) => {
                if fe.is_over {
                    if fe.digit.has_hovers() {
                        self.animate_state(cx, ids!(hover.on));
                    }
                }
                else {
                    self.animate_state(cx, ids!(hover.off));
                }
            }
            _ => ()
        };
    }
    
    pub fn draw_label(&mut self, cx: &mut Cx2d, label: &str) {
        self.bg.begin(cx, self.walk, self.layout);
        self.label.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.bg.end(cx);
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.clear_sweep_lock(self.bg.area());
        
        self.bg.begin(cx, walk, self.layout);
        //let start_pos = cx.turtle().rect().pos;
        if let Some(val) = self.display.get(self.selected_item) {
            self.label.draw_walk(cx, Walk::fit(), Align::default(), val);
        }
        else if let Some(val) = self.items.get(self.selected_item) {
            self.label.draw_walk(cx, Walk::fit(), Align::default(), val);
        }
        else{
            self.label.draw_walk(cx, Walk::fit(), Align::default(), " ");
        }
        self.bg.end(cx);
        
        cx.add_nav_stop(self.bg.area(), NavRole::DropDown, Margin::default());
        
        if self.is_open && self.popup_menu.is_some() {
            let last_rect = self.last_rect.unwrap_or(Rect::default());
            cx.set_sweep_lock(self.bg.area());
            // ok so if self was not open, we need to
            // ok so how will we solve this one
            let global = cx.global::<PopupMenuGlobal>().clone();
            let mut map = global.map.borrow_mut();
            let lb = map.get_mut(&self.popup_menu.unwrap()).unwrap();
            let mut item_pos = None;
            
            lb.begin(cx, last_rect.size.x);
            for (i, item) in self.items.iter().enumerate() {
                let node_id = LiveId(i as u64).into();
                if i == self.selected_item {
                    item_pos = Some(cx.turtle().pos());
                }
                if i < self.display.len(){
                    lb.draw_item(cx, node_id, &self.display[i]);
                }
                else{
                    lb.draw_item(cx, node_id, item);
                }
            }
            // ok we shift the entire menu. however we shouldnt go outside the screen area
            lb.end(cx, last_rect.pos - item_pos.unwrap());
        }
    }
}

impl FrameComponent for DropDown {
    fn bind_read(&mut self, _cx: &mut Cx, nodes: &[LiveNode]) {
        if let Some(LiveValue::BareEnum {variant, ..}) = nodes.read_path(&self.bind) {
            // it should be a BareEnum
            for (index, item) in self.items.iter().enumerate() {
                if LiveId::from_str(item).unwrap() == *variant {
                    self.selected_item = index;
                    break;
                }
            }
        }
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.bg.redraw(cx);
    }
    
    fn handle_component_event(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, FrameActionItem)) {
        self.handle_event(cx, event, &mut | cx, drop_down, action | {
            let mut delta = Vec::new();
            match &action {
                DropDownAction::Select(v) => {
                    if drop_down.bind.len()>0 {
                        let base = LiveId::from_str(&drop_down.bind_enum).unwrap();
                        let variant = LiveId::from_str(&drop_down.items[*v]).unwrap();
                        delta.write_path(&drop_down.bind, LiveValue::BareEnum {base, variant});
                    }
                },
                _ => ()
            };
            dispatch_action(cx, FrameActionItem::new(action.into()).bind_delta(delta))
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk, _self_uid: FrameUid) -> FrameDraw {
        self.draw_walk(cx, walk);
        FrameDraw::done()
    }
}