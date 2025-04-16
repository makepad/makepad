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
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    use crate::popup_menu::PopupMenu;
    use crate::popup_menu::PopupMenuFlat;
    use crate::popup_menu::PopupMenuFlatter;
    use crate::popup_menu::PopupMenuGradientX;
    use crate::popup_menu::PopupMenuGradientY;
    
    pub DrawLabelText = {{DrawLabelText}} {}
    pub DropDownBase = {{DropDown}} {}
    
    pub DropDown = <DropDownBase> {
        width: Fit, height: Fit,
        margin: 0.,
        padding: <THEME_MSPACE_2> { right: 22.5 }
        align: {x: 0., y: 0.}
        
        draw_text: {
            uniform color: (THEME_COLOR_TEXT)
            uniform color_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_focus: (THEME_COLOR_TEXT_FOCUS)

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            
            fn get_color(self) -> vec4 {
                return mix(
                    mix(self.color, self.color_hover, self.hover),
                    mix(self.color_focus, self.color_hover, self.hover),
                    self.focus
                )
            }
        }
        
        draw_bg: {
            instance hover: 0.0
            instance focus: 0.0
            instance active: 0.0
                        
            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color: (THEME_COLOR_OUTSET)
            uniform color_hover: (THEME_COLOR_OUTSET_HOVER)
            uniform color_focus: (THEME_COLOR_OUTSET * 1.2)

            uniform border_color_1: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_LIGHT * 1.3)

            uniform border_color_2: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_SHADOW * 1.3)

            uniform arrow_color: (THEME_COLOR_TEXT)
            uniform arrow_color_focus: (THEME_COLOR_TEXT_FOCUS)
            uniform arrow_color_hover: (THEME_COLOR_TEXT_HOVER)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                // lets draw a little triangle in the corner
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 2.5;
                let offset = 1.;
                let offset_x = 2.;
                
                sdf.move_to(c.x - sz - offset_x, c.y - sz + offset);
                sdf.line_to(c.x + sz - offset_x, c.y - sz + offset);
                sdf.line_to(c.x - offset_x, c.y + sz * 0.25 + offset);
                sdf.close_path();
                
                sdf.fill_keep(
                    mix(
                        mix(
                            self.arrow_color,
                            self.arrow_color_focus,
                            self.focus
                        ),
                        self.arrow_color_hover,
                        self.hover
                    )
                );

                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                            mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                        self.focus
                    ), self.border_size)

                sdf.fill(
                    mix(
                        mix(
                            self.color,
                            self.color_hover,
                            self.hover
                        ),
                        self.color_focus,
                        self.focus
                    )
                )
                return sdf.result
            }
        }
        
        popup_menu: <PopupMenu> {}
        
        selected_item: 0,
        
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {hover: 0.0}
                        draw_text: {hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                    }
                    apply: {
                        draw_bg: { hover: [{time: 0.0, value: 1.0}],}
                        draw_text: { hover: [{time: 0.0, value: 1.0}],}
                    }
                }
                
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
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
    
    pub DropDownFlat = <DropDown> {
        draw_bg: {
            border_size: 1.,

            color: (THEME_COLOR_U_HIDDEN)
            color_hover: (THEME_COLOR_OUTSET_HOVER)
            color_focus: (THEME_COLOR_OUTSET * 1.2)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_1_focus: (THEME_COLOR_BEVEL_FOCUS)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_focus: (THEME_COLOR_BEVEL_FOCUS)
        }

        popup_menu: <PopupMenuFlat> {}
    }

    pub DropDownFlatter = <DropDown> {
        draw_bg: {
            border_size: 0.,

            color: (THEME_COLOR_U_HIDDEN)
            color_hover: (THEME_COLOR_OUTSET_HOVER)
            color_focus: (THEME_COLOR_OUTSET * 1.2)

            border_color_1: (THEME_COLOR_U_HIDDEN)
            border_color_1_hover: (THEME_COLOR_U_HIDDEN)
            border_color_1_focus: (THEME_COLOR_U_HIDDEN)

            border_color_2: (THEME_COLOR_U_HIDDEN)
            border_color_2_hover: (THEME_COLOR_U_HIDDEN)
            border_color_2_focus: (THEME_COLOR_U_HIDDEN)
        }

        popup_menu: <PopupMenuFlatter> {}
    }


    pub DropDownGradientX = <DropDown> {
        
        draw_bg: {
            instance hover: 0.0
            instance focus: 0.0
                        
            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_OUTSET * 1.75)
            uniform color_1_hover: (THEME_COLOR_OUTSET_HOVER * 1.5)
            uniform color_1_focus: (THEME_COLOR_OUTSET * 2.5)

            uniform color_2: (THEME_COLOR_OUTSET)
            uniform color_2_hover: (THEME_COLOR_OUTSET_HOVER)
            uniform color_2_focus: (THEME_COLOR_OUTSET * 1.25)

            uniform border_color_1: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_LIGHT * 1.3)

            uniform border_color_2: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_SHADOW * 1.3)

            uniform arrow_color: (THEME_COLOR_TEXT)
            uniform arrow_color_focus: (THEME_COLOR_TEXT_FOCUS)
            uniform arrow_color_hover: (THEME_COLOR_TEXT_HOVER)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                // lets draw a little triangle in the corner
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 2.5;
                let offset = 1.;
                let offset_x = 2.;
                
                sdf.move_to(c.x - sz - offset_x, c.y - sz + offset);
                sdf.line_to(c.x + sz - offset_x, c.y - sz + offset);
                sdf.line_to(c.x - offset_x, c.y + sz * 0.25 + offset);
                sdf.close_path();
                
                sdf.fill_keep(
                    mix(
                        mix(
                            self.arrow_color,
                            self.arrow_color_focus,
                            self.focus
                        ),
                        self.arrow_color_hover,
                        self.hover
                    )
                )

                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                            mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                        self.focus
                    ), self.border_size)

                sdf.fill(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.x),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.x),
                            self.hover
                        ),
                        mix(self.color_1_focus, self.color_2_focus, self.pos.x),
                        self.focus
                    )
                )
                
                return sdf.result
            }
        }
        
        popup_menu: <PopupMenuGradientX> {}
    }


    pub DropDownGradientY = <DropDownGradientX> {
        
        draw_bg: {
            instance hover: 0.0
            instance focus: 0.0
                        
            border_size: (THEME_BEVELING)
            border_radius: (THEME_CORNER_RADIUS)

            color_dither: 1.0

            color_1: (THEME_COLOR_OUTSET * 1.75)
            color_1_hover: (THEME_COLOR_OUTSET_HOVER * 1.5)
            color_1_focus: (THEME_COLOR_OUTSET * 2.5)

            color_2: (THEME_COLOR_OUTSET)
            color_2_hover: (THEME_COLOR_OUTSET_HOVER)
            color_2_focus: (THEME_COLOR_OUTSET * 1.25)

            border_color_1: (THEME_COLOR_BEVEL_LIGHT)
            border_color_1_hover: (THEME_COLOR_BEVEL_LIGHT)
            border_color_1_focus: (THEME_COLOR_BEVEL_LIGHT * 1.3)

            border_color_2: (THEME_COLOR_BEVEL_SHADOW)
            border_color_2_hover: (THEME_COLOR_BEVEL_SHADOW)
            border_color_2_focus: (THEME_COLOR_BEVEL_SHADOW * 1.3)
            
            arrow_color: (THEME_COLOR_TEXT)
            arrow_color_focus: (THEME_COLOR_TEXT_FOCUS)
            arrow_color_hover: (THEME_COLOR_TEXT_HOVER)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                // lets draw a little triangle in the corner
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 2.5;
                let offset = 1.;
                let offset_x = 2.;
                
                sdf.move_to(c.x - sz - offset_x, c.y - sz + offset);
                sdf.line_to(c.x + sz - offset_x, c.y - sz + offset);
                sdf.line_to(c.x - offset_x, c.y + sz * 0.25 + offset);
                sdf.close_path();
                
                sdf.fill_keep(
                    mix(
                        mix(
                            self.arrow_color,
                            self.arrow_color_focus,
                            self.focus
                        ),
                        self.arrow_color_hover,
                        self.hover
                    )
                )

                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                            mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                        self.focus
                    ), self.border_size)

                sdf.fill(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.y),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.y),
                            self.hover
                        ),
                        mix(self.color_1_focus, self.color_2_focus, self.pos.y),
                        self.focus
                    )
                )
                
                return sdf.result
            }
        }
        
        popup_menu: <PopupMenuGradientY> {}
    }

}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub enum PopupMenuPosition {
    #[pick] OnSelected,
    BelowInput,
}

#[derive(Live, Widget)]
pub struct DropDown {
    #[animator] animator: Animator,
    
    #[redraw] #[live] draw_bg: DrawQuad,
    #[live] draw_text: DrawLabelText,
    
    #[walk] walk: Walk,
    
    #[live] bind: String,
    #[live] bind_enum: String,
    
    #[live] popup_menu: Option<LivePtr>,
    
    #[live] labels: Vec<String>,
    #[live] values: Vec<LiveValue>,
    
    #[live] popup_menu_position: PopupMenuPosition,
    
    #[rust] is_active: bool,
    
    #[live] selected_item: usize,
    
    #[layout] layout: Layout,
}

#[derive(Default, Clone)]
struct PopupMenuGlobal {
    map: Rc<RefCell<ComponentMap<LivePtr, PopupMenu >> >
}

#[derive(Live, LiveHook, LiveRegister)]#[repr(C)]
struct DrawLabelText {
    #[deref] draw_super: DrawText2,
    #[live] focus: f32,
    #[live] hover: f32,
}

impl LiveHook for DropDown {
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        if self.popup_menu.is_none() || !apply.from.is_from_doc() {
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
#[derive(Clone, Debug, DefaultNone)]
pub enum DropDownAction {
    Select(usize, LiveValue),
    None
}

impl DropDown {
    
    pub fn set_active(&mut self, cx: &mut Cx) {
        self.is_active = true;
        self.draw_bg.apply_over(cx, live!{active: 1.0});
        self.draw_bg.redraw(cx);
        let global = cx.global::<PopupMenuGlobal>().clone();
        let mut map = global.map.borrow_mut();
        let lb = map.get_mut(&self.popup_menu.unwrap()).unwrap();
        let node_id = LiveId(self.selected_item as u64).into();
        lb.init_select_item(node_id);
        cx.sweep_lock(self.draw_bg.area());
    }
    
    pub fn set_closed(&mut self, cx: &mut Cx) {
        self.is_active = false;
        self.draw_bg.apply_over(cx, live!{active: 0.0});
        self.draw_bg.redraw(cx);
        cx.sweep_unlock(self.draw_bg.area());
    }
    
    pub fn draw_text(&mut self, cx: &mut Cx2d, label: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_text.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        //cx.clear_sweep_lock(self.draw_bg.area());
        // ok so what if. what do we have
        // we have actions
        // and we have applying states/values in response
       
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
        
        if self.is_active && self.popup_menu.is_some() {
            //cx.set_sweep_lock(self.draw_bg.area());
            // ok so if self was not active, we need to
            // ok so how will we solve this one
            let global = cx.global::<PopupMenuGlobal>().clone();
            let mut map = global.map.borrow_mut();
            let popup_menu = map.get_mut(&self.popup_menu.unwrap()).unwrap();

            // we kinda need to draw it twice.
            popup_menu.begin(cx);

            match self.popup_menu_position {
                PopupMenuPosition::OnSelected => {
                    let mut item_pos = None;
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
                PopupMenuPosition::BelowInput => {
                    for (i, item) in self.labels.iter().enumerate() {
                        let node_id = LiveId(i as u64).into();
                        popup_menu.draw_item(cx, node_id, &item);
                    }

                    let area = self.draw_bg.area().rect(cx);
                    let shift = DVec2 {
                        x: 0.0,
                        y: area.size.y,
                    };
                    
                    popup_menu.end(cx, self.draw_bg.area(), shift);
                }
            }
        }
    }
}

impl Widget for DropDown {
    fn set_disabled(&mut self, cx:&mut Cx, disabled:bool){
        self.animator_toggle(cx, disabled, Animate::Yes, id!(disabled.on), id!(disabled.off));
    }
                
    fn disabled(&self, cx:&Cx) -> bool {
        self.animator_in_state(cx, id!(disabled.on))
    }
    
    fn widget_to_data(&self, _cx: &mut Cx, actions: &Actions, nodes: &mut LiveNodeVec, path: &[LiveId]) -> bool {
        match actions.find_widget_action_cast(self.widget_uid()) {
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
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope)  {
        self.animator_handle_event(cx, event);
        let uid = self.widget_uid();
                
        if self.is_active && self.popup_menu.is_some() {
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
                        cx.widget_action(uid, &scope.path, DropDownAction::Select(self.selected_item, self.values.get(self.selected_item).cloned().unwrap_or(LiveValue::None)));
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
                    return;
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
                        cx.widget_action(uid, &scope.path, DropDownAction::Select(self.selected_item, self.values.get(self.selected_item).cloned().unwrap_or(LiveValue::None)));
                        self.set_closed(cx);
                        self.draw_bg.redraw(cx);
                    }
                }
                KeyCode::ArrowDown => {
                    if self.values.len() > 0 && self.selected_item < self.values.len() - 1 {
                        self.selected_item += 1;
                        cx.widget_action(uid, &scope.path, DropDownAction::Select(self.selected_item, self.values.get(self.selected_item).cloned().unwrap_or(LiveValue::None)));
                        self.set_closed(cx);
                        self.draw_bg.redraw(cx);
                    }
                },
                _ => ()
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                self.set_active(cx);
                // self.animator_play(cx, id!(hover.down));
            },
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
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
        
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk);
        DrawStep::done()
    }
}

impl DropDownRef {
    
    pub fn set_labels_with<F:FnMut(&mut String)>(&self, cx: &mut Cx, mut f:F) {
        if let Some(mut inner) = self.borrow_mut() {
            let mut i = 0;
            loop {
                if i>=inner.labels.len(){
                    inner.labels.push(String::new());
                }
                let s = &mut inner.labels[i];
                s.clear();
                f(s);
                if s.len()==0{
                    break;
                }
                i+=1;
            }
            inner.labels.truncate(i);
            inner.draw_bg.redraw(cx);
        }
    }
    
    pub fn set_labels(&self, cx: &mut Cx, labels: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.labels = labels;
            inner.draw_bg.redraw(cx);
        }
    }
    
    //DEPRICATED
    pub fn selected(&self, actions: &Actions) -> Option<usize> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let DropDownAction::Select(id, _) = item.cast() {
                return Some(id)
            }
        }
        None
    }
    
    pub fn changed(&self, actions: &Actions) -> Option<usize> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let DropDownAction::Select(id, _) = item.cast() {
                return Some(id)
            }
        }
        None
    }
        
    pub fn changed_label(&self, actions: &Actions) -> Option<String> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let DropDownAction::Select(id, _) = item.cast() {
                 if let Some(inner) = self.borrow() {
                    return Some(inner.labels[id].clone())
                }
            }
        }
        None
    }
    
    pub fn set_selected_item(&self, cx: &mut Cx, item: usize) {
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
    
    pub fn set_selected_by_label(&self, label: &str, cx: &mut Cx) {
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
