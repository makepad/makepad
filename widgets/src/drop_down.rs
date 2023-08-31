use {
    std::rc::Rc,
    std::cell::RefCell,
    crate::{
        makepad_derive_widget::*,
        popup_menu::{PopupMenu, PopupMenuAction},
        makepad_draw::*,
        widget::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
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
        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            instance focus: 0.0,
            uniform border_radius: 0.5
            
            fn get_bg(self, inout sdf: Sdf2d) {
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
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
        
        width: Fill,
        height: Fit,
        margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0}
        align: {x: 0., y: 0.}
        padding: {left: 5.0, top: 5.0, right: 4.0, bottom: 5.0}
        
        popup_menu: <PopupMenu> {
        }
        
        popup_shift: vec2(-6.0, 4.0)
        
        selected_item: 0
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0, hover: 0.0}
                        draw_text: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }
                
                pressed = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 0.0},
                        draw_text: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 1.0},
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }
}

#[derive(Live)]
pub struct DropDown {
    #[animator] animator: Animator,
    
    #[live] draw_bg: DrawQuad,
    #[live] draw_text: DrawLabelText,
    
    #[walk] walk: Walk,
    
    #[live] bind: String,
    #[live] bind_enum: String,
    
    #[live] popup_menu: Option<LivePtr>,
    
    #[live] labels: Vec<String>,
    #[live] values: Vec<LiveValue>,
    
    #[live] popup_shift: DVec2,
    
    #[rust] is_open: bool,
    
    #[live] selected_item: usize,
    
    #[layout] layout: Layout,
}

#[derive(Default, Clone)]
struct PopupMenuGlobal {
    map: Rc<RefCell<ComponentMap<LivePtr, PopupMenu >> >
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawLabelText {
    #[deref] draw_super: DrawText,
    #[live] focus: f32,
    #[live] hover: f32,
    #[live] pressed: f32,
}

impl LiveHook for DropDown {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, DropDown)
    }
    
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
        self.draw_bg.redraw(cx);
        let global = cx.global::<PopupMenuGlobal>().clone();
        let mut map = global.map.borrow_mut();
        let lb = map.get_mut(&self.popup_menu.unwrap()).unwrap();
        let node_id = LiveId(self.selected_item as u64).into();
        lb.init_select_item(node_id);
        cx.sweep_lock(self.draw_bg.area());
    }
    
    pub fn set_closed(&mut self, cx: &mut Cx) {
        self.is_open = false;
        self.draw_bg.redraw(cx);
        cx.sweep_unlock(self.draw_bg.area());
    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, DropDownAction)) {
        self.animator_handle_event(cx, event);
        
        if self.is_open && self.popup_menu.is_some() {
            // ok so how will we solve this one
            let global = cx.global::<PopupMenuGlobal>().clone();
            let mut map = global.map.borrow_mut();
            let menu = map.get_mut(&self.popup_menu.unwrap()).unwrap();
            let mut close = false;
            menu.handle_event_with(cx, event, self.draw_bg.area(), &mut | cx, action | {
                match action {
                    PopupMenuAction::WasSweeped(_node_id) => {
                        //dispatch_action(cx, PopupMenuAction::WasSweeped(node_id));
                    }
                    PopupMenuAction::WasSelected(node_id) => {
                        //dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                        self.selected_item = node_id.0.0 as usize;
                        dispatch_action(cx, DropDownAction::Select(self.selected_item, self.values.get(self.selected_item).cloned().unwrap_or(LiveValue::None)));
                        self.draw_bg.redraw(cx);
                        close = true;
                    }
                    _ => ()
                }
            });
            if close {
                self.set_closed(cx);
            }
            
            // check if we clicked outside of the popup menu
            if let Event::MouseDown(e) = event {
                if !menu.menu_contains_pos(cx, e.abs) {
                    self.set_closed(cx);
                    self.animator_play(cx, id!(hover.off));
                }
            }
        }
        
        match event.hits_with_sweep_area(cx, self.draw_bg.area(), self.draw_bg.area()) {
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, id!(focus.off));
                self.set_closed(cx);
                self.animator_play(cx, id!(hover.off));
                self.draw_bg.redraw(cx);
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, id!(focus.on));
            }
            Hit::KeyDown(ke) => match ke.key_code {
                KeyCode::ArrowUp => {
                    if self.selected_item > 0 {
                        self.selected_item -= 1;
                        dispatch_action(cx, DropDownAction::Select(self.selected_item, self.values.get(self.selected_item).cloned().unwrap_or(LiveValue::None)));
                        self.set_closed(cx);
                        self.draw_bg.redraw(cx);
                    }
                }
                KeyCode::ArrowDown => {
                    if self.values.len() > 0 && self.selected_item < self.values.len() - 1 {
                        self.selected_item += 1;
                        dispatch_action(cx, DropDownAction::Select(self.selected_item, self.values.get(self.selected_item).cloned().unwrap_or(LiveValue::None)));
                        self.set_closed(cx);
                        self.draw_bg.redraw(cx);
                    }
                },
                _ => ()
            }
            Hit::FingerDown(_fe) => {
                cx.set_key_focus(self.draw_bg.area());
                self.set_open(cx);
                self.animator_play(cx, id!(hover.pressed));
            },
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerUp(fe) => {
                if fe.is_over {
                    if fe.device.has_hovers() {
                        self.animator_play(cx, id!(hover.on));
                    }
                }
                else {
                    self.animator_play(cx, id!(hover.off));
                }
            }
            _ => ()
        };
    }
    
    pub fn draw_text(&mut self, cx: &mut Cx2d, label: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_text.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        //cx.clear_sweep_lock(self.draw_bg.area());
        
        self.draw_bg.begin(cx, walk, self.layout);
        //let start_pos = cx.turtle().rect().pos;
        
        if let Some(val) = self.labels.get(self.selected_item) {
            self.draw_text.draw_walk(cx, Walk::fit(), Align::default(), val);
        }
        else {
            self.draw_text.draw_walk(cx, Walk::fit(), Align::default(), " ");
        }
        self.draw_bg.end(cx);
        
        cx.add_nav_stop(self.draw_bg.area(), NavRole::DropDown, Margin::default());
        
        if self.is_open && self.popup_menu.is_some() {
            //cx.set_sweep_lock(self.draw_bg.area());
            // ok so if self was not open, we need to
            // ok so how will we solve this one
            let global = cx.global::<PopupMenuGlobal>().clone();
            let mut map = global.map.borrow_mut();
            let popup_menu = map.get_mut(&self.popup_menu.unwrap()).unwrap();
            let mut item_pos = None;
            
            // we kinda need to draw it twice.
            
            popup_menu.begin(cx);
            
            for (i, item) in self.labels.iter().enumerate() {
                let node_id = LiveId(i as u64).into();
                if i == self.selected_item {
                    item_pos = Some(cx.turtle().pos());
                }
                popup_menu.draw_item(cx, node_id, &item);
            }
            
            // ok we shift the entire menu. however we shouldnt go outside the screen area
            popup_menu.end(cx, self.draw_bg.area(), -item_pos.unwrap_or(dvec2(0.0, 0.0)));
        }
    }
}

impl Widget for DropDown {
    
    fn widget_to_data(&self, _cx: &mut Cx, actions: &WidgetActions, nodes: &mut LiveNodeVec, path: &[LiveId]) -> bool {
        match actions.single_action(self.widget_uid()) {
            DropDownAction::Select(_, value) => {
                nodes.write_field_value(path, value.clone());
                true
            }
            _ => false
        }
    }
    
    fn data_to_widget(&mut self, cx: &mut Cx, nodes: &[LiveNode], path: &[LiveId]) {
        if let Some(value) = nodes.read_field_value(path) {
            if let Some(index) = self.values.iter().position( | v | v == value) {
                if self.selected_item != index {
                    self.selected_item = index;
                    self.redraw(cx);
                }
            }
            else {
                error!("Value not in values list {:?}", value);
            }
        }
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct DropDownRef(WidgetRef);

impl DropDownRef {
    pub fn set_labels(&self, labels: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.labels = labels
        }
    }
    
    pub fn set_labels_and_redraw(&self, cx: &mut Cx, labels: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.labels = labels;
            inner.draw_bg.redraw(cx);
        }
    }
    
    pub fn selected(&self, actions: &WidgetActions) -> Option<usize> {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let DropDownAction::Select(id, _) = item.action() {
                return Some(id)
            }
        }
        None
    }
    
    pub fn set_selected_item(&self, item: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.selected_item = item.min(inner.labels.len().max(1) - 1)
        }
    }
    
    pub fn set_selected_item_and_redraw(&self, cx: &mut Cx, item: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            let new_selected = item.min(inner.labels.len().max(1) - 1);
            if new_selected != inner.selected_item{
                inner.selected_item = new_selected;
                inner.draw_bg.redraw(cx);
            }
        }
    }
    pub fn selected_item(&self) -> usize {
        if let Some(inner) = self.borrow() {
            return inner.selected_item
        }
        0
    }
    
    pub fn selected_label(&self) -> String {
        if let Some(inner) = self.borrow() {
            return inner.labels[inner.selected_item].clone()
        }
        "".to_string()
    }
    
    pub fn set_selected_by_label(&self, label: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            if let Some(index) = inner.labels.iter().position( | v | v == label) {
                inner.selected_item = index
            }
        }
    }
    
    pub fn set_selected_by_label_and_redraw(&self, label: &str, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            if let Some(index) = inner.labels.iter().position( | v | v == label) {
                if inner.selected_item != index{
                    inner.selected_item = index;
                    inner.draw_bg.redraw(cx);
                }
            }
        }
    }
    
}
