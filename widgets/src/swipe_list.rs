
use {
    crate::{
        widget::*,
        makepad_derive_widget::*,
        makepad_draw::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    
    SwipeItem = {{SwipeItem}} {
        
    }
    
    SwipeList = {{SwipeList}} {
        item_height: 50
        walk: {
            margin: {top: 3, right: 10, bottom: 3, left: 10},
            width: Fit,
            height: Fit
        }
        Variant1 =? <SwipeItem> {}
    }
}

#[derive(Live, LiveHook)]
pub struct SwipeItem {
    left_drawer: WidgetRef,
    center: WidgetRef,
    right_drawer: WidgetRef,
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct SwipeItemRef(WidgetRef);

#[derive(Clone, Default, WidgetSet)]
pub struct SwipeItemSet(WidgetSet);

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct SwipeItemId(pub LiveId);

impl Widget for SwipeItem {
    fn redraw(&mut self, _cx: &mut Cx) {}
    fn get_walk(&self) -> Walk {Walk::default()}
    fn draw_walk_widget(&mut self, _cx: &mut Cx2d, _walk: Walk) -> WidgetDraw {
        WidgetDraw::done()
    }
}

#[derive(Live)]
#[live_design_with {widget_factory!(cx, SwipeList)}]
pub struct SwipeList {
    #[rust] area: Area,
    walk: Walk,
    item: Option<LivePtr>,
    
    item_height: f64,
    
    #[rust] items: ComponentMap<LiveId, SwipeItemRef>,
}

impl LiveHook for SwipeList {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        for item in self.items.values_mut() {
            if let Some(index) = nodes.child_by_name(index, live_id!(item).as_field()) {
                item.apply(cx, from, index, nodes);
            }
        }
        self.area.redraw(cx);
    }
}

#[derive(Clone, WidgetAction)]
pub enum SwipeListAction {
    None
}

impl SwipeItem {
   
    pub fn handle_event_with(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _sweep_area: Area,
        _dispatch_action: &mut dyn FnMut(&mut Cx, SwipeListAction),
    ) {
        /*
        if self.state_handle_event(cx, event).must_redraw() {
            self.draw_button.area().redraw(cx);
        }
        match event.hits_with_options(
            cx,
            self.draw_button.area(),
            HitOptions::new().with_sweep_area(sweep_area)
        ) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            }
            Hit::FingerDown(_) => {
                if self.state.is_in_state(cx, id!(active.on)) {
                    self.animate_state(cx, id!(active.off));
                    dispatch_action(cx, SequencerAction::Change);
                }
                else {
                    self.animate_state(cx, id!(active.on));
                    dispatch_action(cx, SequencerAction::Change);
                    
                }
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerUp(se) => {
                if !se.is_sweep && se.is_over && se.device.has_hovers() {
                    self.animate_state(cx, id!(hover.on));
                }
                else {
                    self.animate_state(cx, id!(hover.off));
                }
            }
            _ => {}
        }*/
    }
}


impl SwipeList {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(walk, Layout::default());
        /*
        let start_pos = cx.turtle().pos(); //+ vec2(10., 10.);
        let rect = cx.turtle().rect();
        let sz = rect.size / dvec2(self.grid_x as f64, self.grid_y as f64);
        let button = self.button;
        for y in 0..self.grid_y {
            for x in 0..self.grid_x {
                let i = x + y * self.grid_x;
                let pos = start_pos + dvec2(x as f64 * sz.x, y as f64 * sz.y);
                let btn_id = LiveId(i as u64).into();
                let btn = self.buttons.get_or_insert(cx, btn_id, | cx | {
                    SeqButton::new_from_ptr(cx, button)
                });
                btn.x = x;
                btn.y = y;
                btn.draw_abs(cx, Rect {pos: pos, size: sz});
            }
        }
        let used = dvec2(self.grid_x as f64 * self.button_size.x, self.grid_y as f64 * self.button_size.y);
        
        cx.turtle_mut().set_used(used.x, used.y);
        
        cx.end_turtle_with_area(&mut self.area);*/
        self.items.retain_visible();
    }
    
    pub fn _set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.area);
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, SwipeListAction),
    ) {
        /*
        for button in self.buttons.values_mut() {
            button.handle_event_with(cx, event, self.area, dispatch_action);
        }
        */
        match event.hits(cx, self.area) {
            Hit::KeyFocus(_) => {
            }
            Hit::KeyFocusLost(_) => {
            }
            _ => ()
        }
    }
    
    pub fn get_drawable(&mut self, _cx:&mut Cx, _id:SwipeItemId, _templ:&[LiveId])->Option<&mut SwipeItem>{
        None
    }
    
    pub fn begin(&mut self, _cx:&mut Cx){
    }
    
    pub fn end(&mut self, _cx:&mut Cx){
    }
}


impl Widget for SwipeList {
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
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

#[derive(Clone, Default, PartialEq, WidgetRef)]
pub struct SwipeListRef(WidgetRef);

impl SwipeListRef {
    pub fn items_with_actions(&self, actions:&WidgetActions)->SwipeItemSet{
        // find items with container set to our uid
        // and return those
        Default::default()
    }
}

#[derive(Clone, Default, WidgetSet)]
pub struct SwipeListSet(WidgetSet);

