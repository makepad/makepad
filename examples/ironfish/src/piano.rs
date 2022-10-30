
use {
    crate::{
        makepad_draw_2d::*,
        makepad_widgets::*,
    }
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawKey= {{DrawKey}} {
        
        fn height_map(self, pos: vec2) -> float {
            let fx = 1 - pow(1.2 - sin(pos.x * PI), 10.8);
            let fy = 1 - pow(1.2 - self.pressed * 0.2 - cos(pos.y * 0.5 * PI), 25.8)
            return fx + fy
        }
        
        fn black_key(self) -> vec4 {
            let delta = 0.001;
            // differentiate heightmap to get the surface normal
            let d = self.height_map(self.pos)
            let dy = self.height_map(self.pos + vec2(0, delta))
            let dx = self.height_map(self.pos + vec2(delta, 0))
            let normal = normalize(cross(vec3(delta, 0, dx - d), vec3(0, delta, dy - d)))
            //let light = normalize(vec3(1.5, 0.5, 1.1))
            let light = normalize(vec3(0.75, 0.5, 0.5))
            let light_hover = normalize(vec3(0.75, 0.5, 1.5))
            let diff = pow(max(dot(mix(light, light_hover, self.hover * (1 - self.pressed)), normal), 0), 3)
            return mix(#181818, #bc, diff)
        }
        
        fn white_key(self) -> vec4 {
            return mix(
                #DEDAD3FF,
                mix(
                    mix(
                        #EAE7E2FF,
                        #ff,
                        self.hover
                    ),
                    mix(#96989CFF, #131820FF, pow(1.0 - sin(self.pos.x * PI), 1.8)),
                    self.pressed
                ),
                self.pos.y
            )
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            if self.is_black > 0.5 {
                sdf.box(0., -4, self.rect_size.x, self.rect_size.y + 4, 1);
                sdf.fill_keep(self.black_key())
            }
            else {
                sdf.box(0., -4.0, self.rect_size.x, self.rect_size.y + 4.0, 2.0);
                sdf.fill_keep(self.white_key())
            }
            return sdf.result
        }
    }
    
    PianoKey= {{PianoKey}} {
        
        state: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {draw_key: {hover: 0.0}}
                }
                
                on = {
                    from: {all: Snap}
                    apply: {draw_key: {hover: 1.0}}
                }
            }
            
            focus = {
                default: off
                
                off = {
                    from: {all: Snap}
                    apply: {draw_key: {focussed: 1.0}}
                }
                
                on = {
                    from: {all: Forward {duration: 0.05}}
                    apply: {draw_key: {focussed: 0.0}}
                }
            }
            pressed = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.05}}
                    apply: {draw_key: {pressed: 0.0}}
                }
                
                on = {
                    from: {all: Snap}
                    apply: {draw_key: {pressed: 1.0}}
                }
            }
        }
    }
    
    Piano= {{Piano}} {
        piano_key: <PianoKey> {}
        white_size: vec2(20.0, 75.0),
        black_size: vec2(15.0, 50.0),
        walk: {
            width: Fit,
            height: Fit
        }
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawKey {
    draw_super: DrawQuad,
    is_black: f32,
    pressed: f32,
    focussed: f32,
    hover: f32,
}

#[derive(Live, LiveHook)]
pub struct PianoKey {
    draw_key: DrawKey,
    state: State,
}

#[derive(Live, Widget)]
#[live_design_fn(widget_factory!(Piano))]
pub struct Piano {
    #[rust] area: Area,
    walk: Walk,
    piano_key: Option<LivePtr>,
    
    #[rust([0; 20])]
    keyboard_keys_down: [u8; 20],
    
    #[rust(4)]
    keyboard_octave: u8,
    
    #[rust(100)]
    keyboard_velocity: u8,
    
    black_size: Vec2,
    white_size: Vec2,
    
    #[rust] white_keys: ComponentMap<PianoKeyId, PianoKey>,
    #[rust] black_keys: ComponentMap<PianoKeyId, PianoKey>,
}

impl LiveHook for Piano {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        for piano_key in self.white_keys.values_mut().chain(self.black_keys.values_mut()) {
            if let Some(index) = nodes.child_by_name(index, live_id!(piano_key).as_field()) {
                piano_key.apply(cx, from, index, nodes);
            }
        }
        self.area.redraw(cx);
    }
}

#[derive(Clone)]
pub struct PianoNote {
    pub is_on: bool,
    pub note_number: u8,
    pub velocity: u8
}

#[derive(Clone, WidgetAction)]
pub enum PianoAction {
    Note(PianoNote),
    None
}

pub enum PianoKeyAction {
    Pressed(u8),
    Up,
}

impl PianoKey {
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, is_black: f32, rect: Rect) {
        self.draw_key.is_black = is_black;
        self.draw_key.draw_abs(cx, rect);
    }
    
    fn set_is_pressed(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.toggle_state(cx, is, animate, id!(pressed.on), id!(pressed.off))
    }
    
    fn set_is_focussed(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.toggle_state(cx, is, animate, id!(focus.on), id!(focus.off))
    }
    
    pub fn handle_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PianoKeyAction),
    ) {
        if self.state_handle_event(cx, event).must_redraw() {
            self.draw_key.area().redraw(cx);
        }
        match event.hits_with_options(
            cx,
            self.draw_key.area(),
            HitOptions::with_sweep_area(sweep_area)
        ) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            }
            Hit::FingerSweepIn(_) => {
                self.animate_state(cx, id!(hover.on));
                self.animate_state(cx, id!(pressed.on));
                dispatch_action(cx, PianoKeyAction::Pressed(127));
            }
            Hit::FingerSweepOut(se) => {
                if se.is_finger_up() && se.digit.has_hovers(){
                    self.animate_state(cx, id!(hover.on));
                }
                else{
                    self.animate_state(cx, id!(hover.off));
                }
                self.animate_state(cx, id!(pressed.off));
                dispatch_action(cx, PianoKeyAction::Up);
            }
            _ => {}
        }
    }
}

impl Piano {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(walk, Layout::default());

        let start_pos = cx.turtle().pos(); //+ vec2(10., 10.);
        let mut pos = start_pos;
        
        let midi_a0 = 21;
        let midi_c8 = 108+24;
        
        fn black_key_shift(key: u32) -> Option<f64> {
            match key % 12 {
                0 => None, // C
                1 => Some(0.6), // C#
                2 => None, // D
                3 => Some(0.4), // D#
                4 => None, // E
                5 => None, // F
                6 => Some(0.7), // F#
                7 => None, // G
                8 => Some(0.5), // G#
                9 => None, // A
                10 => Some(0.3), // A#
                11 => None, // B
                _ => None
            }
        }
        
        let white_size:DVec2 = self.white_size.into();//dvec2(20.0, 100.0);
        let black_size:DVec2 = self.black_size.into();//vec2(15.0, 62.0);
        let piano_key = self.piano_key;
        // draw the white keys first because they go below the black ones
        for i in midi_a0..midi_c8 {
            if black_key_shift(i).is_some() {
                continue;
            }
            let key_id = LiveId(i as u64).into();
            let key = self.white_keys.get_or_insert(cx, key_id, | cx | {
                PianoKey::new_from_ptr(cx, piano_key)
            });
            key.draw_abs(cx, 0.0, Rect {pos: pos, size: white_size});
            pos.x += white_size.x;
        }
        // draw the black keys
        let mut pos = start_pos;
        for i in midi_a0..midi_c8 {
            if let Some(shift) = black_key_shift(i) {
                let key_id = LiveId(i as u64).into();
                let key = self.black_keys.get_or_insert(cx, key_id, | cx | {
                    PianoKey::new_from_ptr(cx, piano_key)
                });
                key.draw_abs(cx, 1.0, Rect {
                    pos: pos - dvec2(black_size.x * shift, 0.),
                    size: black_size
                });
            }
            else {
                pos.x += white_size.x;
            }
        }
        cx.turtle_mut().set_used(white_size.x * (midi_c8 - midi_a0) as f64, white_size.y);
        cx.end_turtle_with_area(&mut self.area);
        self.white_keys.retain_visible();
        self.black_keys.retain_visible();
    }
    
    pub fn area(&mut self)->Area{
        self.area
    }
    
    pub fn set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.area);
    }
    
    pub fn set_note(&mut self, cx: &mut Cx, is_on: bool, note_number: u8) {
        let id = LiveId(note_number as u64).into();
        if let Some(key) = self.black_keys.get_mut(&id) {
            key.set_is_pressed(cx, is_on, Animate::No)
        }
        if let Some(key) = self.white_keys.get_mut(&id) {
            key.set_is_pressed(cx, is_on, Animate::No)
        }
    }
    
    pub fn handle_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, PianoAction),
    ) {
        let mut actions = Vec::new();
        for (key_id, piano_key) in self.black_keys.iter_mut().chain(self.white_keys.iter_mut()) {
            piano_key.handle_event_fn(cx, event, self.area, &mut | _, action | {
                actions.push((*key_id, action))
            });
        }
        
        for (node_id, action) in actions {
            match action {
                PianoKeyAction::Pressed(velocity) => {
                    self.set_key_focus(cx);
                    dispatch_action(cx, PianoAction::Note(PianoNote {
                        is_on: true,
                        note_number: node_id.0.0 as u8,
                        velocity
                    }));
                }
                PianoKeyAction::Up => {
                    dispatch_action(cx, PianoAction::Note(PianoNote {
                        is_on: false,
                        note_number: node_id.0.0 as u8,
                        velocity: 127
                    }));
                }
            }
        }
        
        // handle sweeping the notes
        
        fn key_map(kk: KeyCode) -> Option<u8> {
            match kk {
                KeyCode::KeyA => Some(0),
                KeyCode::KeyW => Some(1),
                KeyCode::KeyS => Some(2),
                KeyCode::KeyE => Some(3),
                KeyCode::KeyD => Some(4),
                KeyCode::KeyF => Some(5),
                KeyCode::KeyT => Some(6),
                KeyCode::KeyG => Some(7),
                KeyCode::KeyY => Some(8),
                KeyCode::KeyH => Some(9),
                KeyCode::KeyU => Some(10),
                KeyCode::KeyJ => Some(11),
                KeyCode::KeyK => Some(12),
                KeyCode::KeyO => Some(13),
                KeyCode::KeyL => Some(14),
                KeyCode::KeyP => Some(15),
                KeyCode::Semicolon => Some(16),
                KeyCode::Quote => Some(17),
                _ => None
            }
        }
        
        match event {
            Event::KeyDown(ke) => if !ke.is_repeat {
                if let Some(nn) = key_map(ke.key_code) {
                    let note_number = nn + self.keyboard_octave * 12;
                    self.keyboard_keys_down[nn as usize] = note_number;
                    self.set_note(cx, true, note_number);
                    dispatch_action(cx, PianoAction::Note(PianoNote {
                        is_on: true,
                        note_number,
                        velocity: self.keyboard_velocity
                    }));
                }
                else {match ke.key_code {
                    KeyCode::KeyZ => {
                        self.keyboard_octave -= 1;
                        self.keyboard_octave = self.keyboard_octave.max(1);
                    }
                    KeyCode::KeyX => {
                        self.keyboard_octave += 1;
                        self.keyboard_octave = self.keyboard_octave.min(7);
                    }
                    KeyCode::KeyC => {
                        self.keyboard_velocity -= 16;
                        self.keyboard_velocity = self.keyboard_velocity.max(16);
                    }
                    KeyCode::KeyV => {
                        self.keyboard_velocity += 16;
                        self.keyboard_velocity = self.keyboard_velocity.min(127);
                    }
                    _ => ()
                }}
            },
            Event::KeyUp(ke) => if let Some(nn) = key_map(ke.key_code) {
                let note_number = self.keyboard_keys_down[nn as usize];
                self.keyboard_keys_down[nn as usize] = 0;
                self.set_note(cx, false, note_number);
                dispatch_action(cx, PianoAction::Note(PianoNote {
                    is_on: false,
                    note_number,
                    velocity: self.keyboard_velocity
                }));
            },
            _ => ()
        }
        
        match event.hits(cx, self.area) {
            Hit::KeyFocus(_) => {
                for piano_key in self.white_keys.values_mut().chain(self.black_keys.values_mut()) {
                    piano_key.set_is_focussed(cx, true, Animate::Yes)
                }
            }
            Hit::KeyFocusLost(_) => {
                for piano_key in self.white_keys.values_mut().chain(self.black_keys.values_mut()) {
                    piano_key.set_is_focussed(cx, true, Animate::No)
                }
            }
            _ => ()
        }
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct PianoKeyId(pub LiveId);

#[derive(Clone, PartialEq, WidgetRef)]
pub struct PianoRef(WidgetRef);

impl PianoRef {
    pub fn notes_played(&self, actions:&WidgetActions) -> Vec<PianoNote> {
        let mut notes = Vec::new();
        for item in actions {
            if item.widget_uid == self.widget_uid() {
                if let PianoAction::Note(note) = item.action() {
                    notes.push(note)
                }
            }
        }
        notes
    }
    
    pub fn set_note(&self, cx: &mut Cx, is_on: bool, note_number: u8) {
        if let Some(mut inner) = self.inner_mut() {
            inner.set_note(cx, is_on, note_number)
        }
    }
    
    pub fn set_key_focus(&self, cx: &mut Cx) {
        if let Some(inner) = self.inner_mut() {
            inner.set_key_focus(cx)
        }
    }
}
