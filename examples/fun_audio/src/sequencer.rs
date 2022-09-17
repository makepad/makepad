
use {
    crate::{
        makepad_draw_2d::*,
        makepad_component::*,
        makepad_component::imgui::*
    }
};

live_register!{
    import makepad_draw_2d::shader::std::*;
    import makepad_component::theme::*;
    
    DrawButton: {{DrawButton}} {
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.box(1, 1, self.rect_size.x - 2, self.rect_size.y - 2, 2);
            sdf.fill(mix(#2,#9, self.active));
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
                    from: {all: Play::Forward {duration: 0.05}}
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
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct SeqButtonId(pub LiveId);

#[derive(Live, FrameComponent)]
#[live_register(frame_component!(Sequencer))]
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

#[derive(Clone, FrameAction)]
pub enum SeqButtonAction {
    Change(bool),
    None
}

#[derive(Clone, FrameAction)]
pub enum SequencerAction {
    Change(usize, usize, bool),
    None
}

impl SeqButton {
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.button.draw_abs(cx, rect);
    }
    
    fn _set_is_active(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.toggle_state(cx, is, animate, ids!(pressed.on), ids!(pressed.off))
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


pub struct SequencerImGUI(ImGUIRef);

impl SequencerImGUI {
    pub fn on_buttons(&self) -> Vec<(usize, usize, bool)> {
        let mut btns = Vec::new();
        for item in self.0.actions.0.iter() {
            if item.uid() == self.0.uid {
                if let SequencerAction::Change(x,y,on) = item.action() {
                    btns.push((x,y,on))
                }
            }
        }
        btns
    }
    
    pub fn _inner(&self) -> Option<std::cell::RefMut<'_, Sequencer >> {
        self.0.inner()
    }
}

pub trait SequencerImGUIExt {
    fn sequencer(&mut self, path: &[LiveId]) -> SequencerImGUI;
}

impl<'a> SequencerImGUIExt for ImGUIRun<'a> {
    fn sequencer(&mut self, path: &[LiveId]) -> SequencerImGUI {
        let mut frame = self.imgui.root_frame();
        SequencerImGUI(self.safe_ref::<Sequencer>(frame.component_by_path(path)))
    }
}
