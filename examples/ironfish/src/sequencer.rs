
use {
    crate::{
        makepad_draw_2d::*,
        makepad_widgets::*,
    }
};

live_register!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawButton: {{DrawButton}} {
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.box(1, 1, self.rect_size.x - 5, self.rect_size.y - 5, 2);
            sdf.stroke_keep(mix(#xFFFFFF80, #x00000040, pow(self.pos.y, 0.2)), 1.0);
            sdf.fill(
                mix(
                mix(
                mix(#xFFFFFF10, #xFFFFFF10, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 0.75)), // 1st value = outer edges, 2nd value = center
                mix(#xFFFFFF40, #xFFFFFF08, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 1.25)),
                self.hover),
                mix(#xFFFDDDFF, #xFFFFFF08, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 1.25)),
                // mix(#xFFFFFFFF, #xFFFFFF08, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 1.25)),
                self.active
                )
            );
            return sdf.result
        }
    }
    
    SeqButton: {{SeqButton}} {
        state: {
            hover = {
                default: off,
                off = {
                    from: {all: Play::Forward {duration: 0.2}}
                    apply: {button: {hover: 0.0}}
                }
                
                on = {
                    from: {all: Play::Snap}
                    apply: {button: {hover: 1.0}}
                }
            }
            
            active = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.15}}
                    apply: {button: {active: 0.0}}
                }
                
                on = {
                    from: {all: Play::Snap}
                    apply: {button: {active: 1.0}}
                }
            }
        }
    }
    
    Sequencer: {{Sequencer}} {
        button: SeqButton {}
        button_size: vec2(25.0, 25.0),
        grid_x: 16,
        grid_y: 16,
        walk: {
            margin: 3,
            width: Size::Fit,
            height: Size::Fit
        }
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawButton {
    draw_super: DrawQuad,
    active: f32,
    hover: f32,
}

#[derive(Live, LiveHook)]
pub struct SeqButton {
    button: DrawButton,
    state: State,
    x: usize,
    y: usize
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct SeqButtonId(pub LiveId);

#[derive(Live, Widget)]
#[live_register(widget!(Sequencer))]
pub struct Sequencer {
    #[rust] area: Area,
    walk: Walk,
    button: Option<LivePtr>,
    
    grid_x: usize,
    grid_y: usize,
    
    button_size: DVec2,
    
    #[rust] buttons: ComponentMap<SeqButtonId, SeqButton>,
}

impl LiveHook for Sequencer {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        for button in self.buttons.values_mut() {
            if let Some(index) = nodes.child_by_name(index, id!(button).as_field()) {
                button.apply(cx, from, index, nodes);
            }
        }
        self.area.redraw(cx);
    }
}

#[derive(Clone, WidgetAction)]
pub enum SeqButtonAction {
    Change(bool),
    None
}

#[derive(Clone, WidgetAction)]
pub enum SequencerAction {
    Change(usize, usize, bool),
    None
}

impl SeqButton {
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.button.draw_abs(cx, rect);
    }
    
    fn set_is_active(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.toggle_state(cx, is, animate, ids!(active.on), ids!(active.off))
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, SeqButtonAction),
    ) {
        if self.state_handle_event(cx, event).must_redraw() {
            self.button.area().redraw(cx);
        }
        match event.hits_with_options(
            cx,
            self.button.area(),
            HitOptions::with_sweep_area(sweep_area)
        ) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animate_state(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, ids!(hover.off));
            }
            Hit::FingerSweepIn(_) => {
                if self.state.is_in_state(cx, ids!(active.on)){
                    self.animate_state(cx, ids!(active.off));
                    dispatch_action(cx, SeqButtonAction::Change(false));
                }
                else{
                    self.animate_state(cx, ids!(active.on));
                    dispatch_action(cx, SeqButtonAction::Change(true));
                    
                }
                self.animate_state(cx, ids!(hover.on));
            }
            Hit::FingerSweepOut(se) => {
                if se.is_finger_up() && se.digit.has_hovers(){
                    self.animate_state(cx, ids!(hover.on));
                }
                else{
                    self.animate_state(cx, ids!(hover.off));
                }
            }
            _ => {}
        }
    }
}


impl Sequencer {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(walk, Layout::default());

        let start_pos = cx.turtle().pos(); //+ vec2(10., 10.);
        
        let rect = cx.turtle().rect();
        let sz = rect.size / dvec2(self.grid_x as f64, self.grid_y as f64);
        let button = self.button;
        for y in 0..self.grid_y{
            for x in 0..self.grid_x{
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

        cx.end_turtle_with_area(&mut self.area);
        self.buttons.retain_visible();
    }
    
    pub fn _set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.area);
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event, 
        dispatch_action: &mut dyn FnMut(&mut Cx, SequencerAction),
    ) {
        let mut actions = Vec::new();
        for (btn_id, button) in self.buttons.iter_mut() {
            button.handle_event(cx, event, self.area, &mut | _, action | {
                actions.push((*btn_id, action))
            });
        }
        
        for (btn_id, action) in actions {
            let i = btn_id.0.0 as usize;
            let x = i % self.grid_x;
            let y = i / self.grid_x;
            match action {
                SeqButtonAction::Change(active) => {
                    dispatch_action(cx, SequencerAction::Change(x, y, active));
                }
                _=>()
            }
        }
        
        match event {
            _ => ()
        }
        
        match event.hits(cx, self.area) {
            Hit::KeyFocus(_) => {
            }
            Hit::KeyFocusLost(_) => {
            }
            _ => ()
        }
    }
}


#[derive(Clone, PartialEq, WidgetRef)]
pub struct SequencerRef(WidgetRef);

impl SequencerRef {
    pub fn buttons_clicked(&self, actions:&WidgetActions) -> Vec<(usize, usize, bool)> {
        let mut btns = Vec::new();
        for item in actions {
            if item.widget == self.0{
                if let SequencerAction::Change(x,y,on) = item.action() {
                    btns.push((x,y,on))
                }
            }
        }
        btns
    }
    
    pub fn clear_buttons(&self, cx:&mut Cx){
        if let Some(mut inner) = self.inner_mut(){
            for (_, button) in inner.buttons.iter_mut() {                
                button.set_is_active(cx, false, Animate::Yes);
            }
        }
    }
    
    pub fn update_button(&self, cx:&mut Cx, x:usize, y:usize, state: bool){
        
        if let Some(mut inner) = self.inner_mut(){
            for (_, button) in inner.buttons.iter_mut() {
                if button.x == x && button.y  == y {
                button.set_is_active(cx, state, Animate::Yes);
                }
            }
        }
    }
}
