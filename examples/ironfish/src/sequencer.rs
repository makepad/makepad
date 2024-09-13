
use {
    crate::{
        makepad_draw::*,
        makepad_widgets::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    
    DrawButton = {{DrawButton}} {
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.box(1, 1, self.rect_size.x - 5, self.rect_size.y - 5, 1.5);
            sdf.stroke_keep(mix(#xFFFFFF80, #x00000040, pow(self.pos.y, 0.2)), 1.0);
            sdf.fill(
                mix(
                    mix(
                    mix(
                        mix(#xFFFFFF18, #xFFFDDD30, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 1.5)), // 1st value = center, 2nd value = outer edges
                        mix(#xFFFFFF40, #xFFFFFF20, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 1.25)),
                        self.hover
                    ),
                    mix(#xFFFDDDFF, #xFFFFFF08, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 1.25)),
                    // mix(#xFFFFFFFF, #xFFFFFF08, pow(length((self.pos - vec2(0.5, 0.5)) * 1.2), 1.25)),
                    self.active
                ),#ffffff,
                self.active_step)
            );
            return sdf.result
        }
    }
    
    SeqButton = {{SeqButton}} {
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {draw_button: {hover: 0.0}}
                }
                
                on = {
                    from: {all: Snap}
                    apply: {draw_button: {hover: 1.0}}
                }
            }
            
            active = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {draw_button: {active: 0.0}}
                }
                
                on = {
                    from: {all: Snap}
                    apply: {draw_button: {active: 1.0}}
                }
            }
        }
    }
    
    Sequencer = {{Sequencer}} {
        current_step: 0,
        button: <SeqButton> {}
        button_size: vec2(25.0, 25.0),
        grid_x: 16,
        grid_y: 16,
        
        margin: {top: 3, right: 10, bottom: 3, left: 10}
        width: Fit,
        height: Fit
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook, LiveRegister)]#[repr(C)]
struct DrawButton {
    #[deref] draw_super: DrawQuad,
    #[live] active: f32,
    #[live] active_step: f32,
    #[live] hover: f32,
}

#[derive(Live, LiveHook, LiveRegister)]
pub struct SeqButton {
    #[live] draw_button: DrawButton,
    #[animator] animator: Animator,
    #[live] x: usize,
    #[live] y: usize,
   // #[live] activestep: f32
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct SeqButtonId(pub LiveId);

#[derive(Live, Widget)]
pub struct Sequencer {
    #[redraw] #[rust] area: Area,
    #[walk] walk: Walk,
    #[live] button: Option<LivePtr>,
    #[live] current_step: usize,
    #[live] grid_x: usize,
    #[live] grid_y: usize,
    
    #[live] button_size: DVec2,
    
    #[rust] buttons: ComponentMap<SeqButtonId, SeqButton>,
}

impl LiveHook for Sequencer {
fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        for button in self.buttons.values_mut() {
            if let Some(index) = nodes.child_by_name(index, live_id!(button).as_field()) {
                button.apply(cx, apply, index, nodes);
            }
        }
        self.area.redraw(cx);
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum SequencerAction {
    Change,
    None
}

impl SeqButton {
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_button.draw_abs(cx, rect);
    }
    
    fn set_is_active(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.animator_toggle(cx, is, animate, id!(active.on), id!(active.off))
    }
    
    fn is_active(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, id!(active.on))
    }
    
    pub fn handle_event_changed(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
    )->bool {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_button.area().redraw(cx);
        }
        match event.hits_with_options(
            cx,
            self.draw_button.area(),
            HitOptions::new().with_sweep_area(sweep_area)
        ) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerDown(_) => {
                self.animator_play(cx, id!(hover.on));
                if self.animator_in_state(cx, id!(active.on)) {
                    self.animator_play(cx, id!(active.off));
                    return true
                }
                else {
                    self.animator_play(cx, id!(active.on));
                    return true
                }
            }
            Hit::FingerUp(se) => {
                if !se.is_sweep && se.is_over && se.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else {
                    self.animator_play(cx, id!(hover.off));
                }
            }
            _ => {}
        }
        false
    }
}


impl Sequencer {
    
    pub fn _set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.area);
    }
    
    pub fn get_steps(&self, cx: &Cx) -> Vec<u32> {
        let mut steps = Vec::new();
        steps.resize(self.grid_y, 0u32);
        for (btn_id, button) in self.buttons.iter() {
            let active = button.is_active(cx);
            let i = btn_id.0.0 as usize;
            let x = i % self.grid_x;
            let y = i / self.grid_x;
            if active {steps[x] |= 1 << y};
        }
        steps
    }
    
    pub fn set_steps(&mut self, cx: &mut Cx, steps: &[u32]) {
        if steps.len() != self.grid_x {
            panic!("Steps not correct for sequencer got {} expected {}", steps.len(), self.grid_x);
        }
        for (btn_id, button) in self.buttons.iter_mut() {
            let i = btn_id.0.0 as usize;
            let x = i % self.grid_x;
            let y = i / self.grid_x;
            let bit = 1 << y;
            let active = steps[x] & bit == bit;
            button.set_is_active(cx, active, Animate::Yes);
        }
    }
    
    pub fn write_state_to_data(&self, cx: &mut Cx, nodes: &mut LiveNodeVec, path: &[LiveId]) {
        let steps = self.get_steps(cx);
        let mut array = LiveNodeVec::new();
        array.open_array(LiveId(0));
        for step in steps {
            array.push(LiveNode::from_value(LiveValue::Int64(step as i64)));
        }
        array.close();
        nodes.write_field_nodes(path, &array);
    }
}


impl Widget for Sequencer {

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        for button in self.buttons.values_mut() {
            if button.handle_event_changed(cx, event, self.area){
                cx.widget_action(uid, &scope.path, SequencerAction::Change);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
                
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
                if x == self.current_step{
                    btn.draw_button.active_step = 1.0;
                }
                else
                {
                    btn.draw_button.active_step = 0.0;
                }
                btn.draw_abs(cx, Rect {pos: pos, size: sz});
            }
        }
        let used = dvec2(self.grid_x as f64 * self.button_size.x, self.grid_y as f64 * self.button_size.y);
                
        cx.turtle_mut().set_used(used.x, used.y);
                
        cx.end_turtle_with_area(&mut self.area);
        self.buttons.retain_visible();
        DrawStep::done()
    }
    
    fn widget_to_data(&self, cx: &mut Cx, actions: &Actions, nodes: &mut LiveNodeVec, path: &[LiveId]) -> bool {
        let uid = self.widget_uid();
        if actions.find_widget_action(uid).is_some() {
            self.write_state_to_data(cx, nodes, path);
            true
        }
        else {
            false
        }
    }

    fn data_to_widget(&mut self, cx: &mut Cx, nodes:&[LiveNode], path: &[LiveId]){
        if let Some(mut index) = nodes.child_by_field_path(0, path) {
            let mut steps = Vec::new();
            if nodes[index].is_array() {
                index += 1;
                while !nodes[index].is_close() {
                    steps.push(nodes[index].value.as_int().unwrap_or(0) as u32);
                    index += 1;
                }
            }
            self.set_steps(cx, &steps);
        }
    }
}

impl SequencerRef {

    /*pub fn set_step(&self, step: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.current_step = step;
        }
    }*/
    
    pub fn clear_grid(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            let mut steps = inner.get_steps(cx);
            for step in &mut steps {*step = 0};
            inner.set_steps(cx, &steps);
            cx.widget_action(inner.widget_uid(), &HeapLiveIdPath::default(), SequencerAction::Change);
        }
    }
    
    pub fn grid_down(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            let mut steps = inner.get_steps(cx);
            for step in &mut steps {
                let mut modstep = *step << 1;
                if (modstep & 1 << 16) == 1 << 16 {modstep += 1; modstep -= 1 << 16};
                *step = modstep;
            }
            inner.set_steps(cx, &steps);
            cx.widget_action(inner.widget_uid(), &HeapLiveIdPath::default(), SequencerAction::Change);
        }
    }
    
    pub fn grid_up(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            let mut steps = inner.get_steps(cx);
            for step in &mut steps {
                let mut modstep = *step >> 1;
                if (*step & 1) == 1 {modstep += 1 << 15;}
                *step = modstep;
            }
            inner.set_steps(cx, &steps);
            cx.widget_action(inner.widget_uid(), &HeapLiveIdPath::default(), SequencerAction::Change);
        }
    }
}
