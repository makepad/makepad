use {
    std::rc::Rc,
    std::cell::RefCell,
    crate::{
        makepad_derive_widget::*,
        popup_menu::{PopupMenu, PopupMenuAction},
        makepad_draw_2d::*,
        data_binding::DataBinding,
        widget::*,
        frame::*,
    }
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::popup_menu::PopupMenu;
    
    DrawLabelText = {{DrawLabelText}} {
        fn get_color(self) -> vec4 {
            return mix(
                mix(
                    mix(
                        #9,
                        #b,
                        self.focus
                    ),
                    #c,
                    self.hover
                ),
                #9,
                self.pressed
            )
        }
    }
    
    DropDown = {{DropDown}} {
        bg: {
            instance hover: 0.0
            instance pressed: 0.0
            instance focus: 0.0,
            const BORDER_RADIUS = 0.5
            
            fn get_bg(self, inout sdf: Sdf2d) {
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
            width: Fill,
            height: Fit,
            margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0},
        }
        
        layout: {
            clip_x: true,
            align: {x: 0., y: 0.},
            padding: {left: 5.0, top: 5.0, right: 4.0, bottom: 5.0}
        }
        
        popup_menu: <PopupMenu> {
        }
        
        selected_item: 0
        state: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        bg: {pressed: 0.0, hover: 0.0}
                        label: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        label: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }
                
                pressed = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        label: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        bg: {focus: 0.0},
                        label: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
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
#[live_design_fn(widget_factory!(DropDown))]
pub struct DropDown {
    state: State,
    
    bg: DrawQuad,
    label: DrawLabelText,
    
    walk: Walk,
    
    bind: String,
    bind_enum: String,
    
    popup_menu: Option<LivePtr>,
    
    labels: Vec<String>,
    values: Vec<LiveValue>,
    
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
#[derive(Clone, WidgetAction)]
pub enum DropDownAction {
    Select(usize, LiveValue),
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
    
    pub fn handle_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, DropDownAction)) {
        self.state_handle_event(cx, event);
        
        if self.is_open && self.popup_menu.is_some() {
            // ok so how will we solve this one
            let global = cx.global::<PopupMenuGlobal>().clone();
            let mut map = global.map.borrow_mut();
            let menu = map.get_mut(&self.popup_menu.unwrap()).unwrap();
            let mut close = false;
            menu.handle_event_fn(cx, event, self.bg.area(), &mut | cx, action | {
                match action {
                    PopupMenuAction::WasSweeped(_node_id) => {
                        //dispatch_action(cx, PopupMenuAction::WasSweeped(node_id));
                    }
                    PopupMenuAction::WasSelected(node_id) => {
                        //dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                        self.selected_item = node_id.0.0 as usize;
                        dispatch_action(cx, DropDownAction::Select(self.selected_item, self.values[self.selected_item].clone()));
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
                    self.animate_state(cx, id!(hover.off));
                }
            }
            if let Event::FingerUp(fd) = event {
                if !menu.menu_contains_pos(cx, fd.abs) {
                    self.set_closed(cx);
                    self.animate_state(cx, id!(hover.off));
                }
            }
        }
        
        match event.hits(cx, self.bg.area()) {
            Hit::KeyFocusLost(_) => {
                self.animate_state(cx, id!(focus.off));
                self.set_closed(cx);
                self.animate_state(cx, id!(hover.off));
                self.bg.redraw(cx);
            }
            Hit::KeyFocus(_) => {
                self.animate_state(cx, id!(focus.on));
            }
            Hit::KeyDown(ke) => match ke.key_code {
                KeyCode::ArrowUp => {
                    if self.selected_item > 0 {
                        self.selected_item -= 1;
                        dispatch_action(cx, DropDownAction::Select(self.selected_item, self.values[self.selected_item].clone()));
                        self.set_closed(cx);
                        self.bg.redraw(cx);
                    }
                }
                KeyCode::ArrowDown => {
                    if self.values.len() > 0 && self.selected_item < self.values.len() - 1 {
                        self.selected_item += 1;
                        dispatch_action(cx, DropDownAction::Select(self.selected_item, self.values[self.selected_item].clone()));
                        self.set_closed(cx);
                        self.bg.redraw(cx);
                    }
                },
                _ => ()
            }
            Hit::FingerDown(_fe) => {
                cx.set_key_focus(self.bg.area());
                self.set_open(cx);
                self.animate_state(cx, id!(hover.pressed));
            },
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            }
            Hit::FingerUp(fe) => {
                if fe.is_over {
                    if fe.digit.has_hovers() {
                        self.animate_state(cx, id!(hover.on));
                    }
                }
                else {
                    self.animate_state(cx, id!(hover.off));
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
        if let Some(val) = self.labels.get(self.selected_item) {
            self.label.draw_walk(cx, Walk::fit(), Align::default(), val);
        }
        else {
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
            for (i, item) in self.labels.iter().enumerate() {
                let node_id = LiveId(i as u64).into();
                if i == self.selected_item {
                    item_pos = Some(cx.turtle().pos());
                }
                lb.draw_item(cx, node_id, &item);
            }
            // ok we shift the entire menu. however we shouldnt go outside the screen area
            lb.end(cx, last_rect.pos - item_pos.unwrap());
        }
    }
}

impl Widget for DropDown {
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}
    
    fn bind_to(&mut self, cx: &mut Cx, db: &mut DataBinding, act: &WidgetActions, path: &[LiveId]) {
        match db {
            DataBinding::FromWidgets{nodes,..}=> if let Some(item) = act.find_single_action(self.widget_uid()) {
                match item.action() {
                    DropDownAction::Select(_, value) => {
                        nodes.write_by_field_path(path, &[LiveNode::from_value(value.clone())]);
                    }
                    _ => ()
                }
            }
            DataBinding::ToWidgets{nodes}=> {
                if let Some(value) = nodes.read_by_field_path(path) {
                    if let Some(index) = self.values.iter().position(|v| v == value){
                        if self.selected_item != index{
                            self.selected_item = index;
                            self.redraw(cx);
                        }
                    }
                    else{
                        error!("Value not in values list {:?}", value);
                    }
                }
            }
        }
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.bg.redraw(cx);
    }
    
    fn handle_widget_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_fn(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct DropDownRef(WidgetRef);
