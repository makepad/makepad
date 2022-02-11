use {
    crate::{
        makepad_platform::*,
        makepad_component::{
            component_map::ComponentMap,
            scroll_view::ScrollView
        },
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    use makepad_component::theme::*;
    
    DrawKeyQuad: {{DrawKeyQuad}} {
        
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
            let light = normalize(vec3(1.5, 0.5, 1.1))
            let light = normalize(vec3(0.75, 0.5, 0.5))
            let light_hover = normalize(vec3(0.75, 0.5, 1.5))
            let diff = pow(max(dot(mix(light, light_hover, self.hover * (1 - self.pressed)), normal), 0), 3)
            return mix(#00, #ff, diff)
        }
        
        fn white_key(self) -> vec4 {
            return mix(
                #ff,
                mix(
                    mix(
                        #df,
                        #ff,
                        self.hover
                    ),
                    mix(#99, #39, pow(1.0 - sin(self.pos.x * PI), 1.8)),
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
    
    PianoKey: {{PianoKey}} {
        
        default_state: {
            duration: 0.2
            apply: {key_quad: {hover: 0.0}}
        }
        
        hover_state: {
            duration: 0.
            apply: {key_quad: {hover: 1.0}}
        }
        
        focussed_state: {
            track: focus,
            duration: 0.05,
            apply: {key_quad: {focussed: 0.0}}
        }
        
        unfocussed_state: {
            track: focus,
            duration: 0.,
            apply: {key_quad: {focussed: 1.0}}
        }
        
        up_state: {
            track: pressed,
            duration: 0.05,
            apply: {key_quad: {pressed: 0.0}}
        }
        
        pressed_state: {
            track: pressed,
            duration: 0.,
            apply: {key_quad: {pressed: 1.0}}
        }
    }
    
    Piano: {{Piano}} {
        piano_key: PianoKey {}
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawKeyQuad {
    deref_target: DrawQuad,
    is_black: f32,
    pressed: f32,
    focussed: f32,
    hover: f32,
}

#[derive(Live, LiveHook)]
pub struct PianoKey {
    key_quad: DrawKeyQuad,
    
    #[state(default_state, up_state, unfocussed_state)]
    animator: Animator,
    
    focussed_state: LiveRef,
    unfocussed_state: LiveRef,
    default_state: LiveRef,
    hover_state: LiveRef,
    pressed_state: LiveRef,
    up_state: LiveRef,
}

#[derive(Live)]
pub struct Piano {
    scroll_view: ScrollView,
    piano_key: Option<LivePtr>,
    
    #[rust([0; 20])]
    keyboard_keys_down: [u8; 20],
    
    #[rust(5)]
    keyboard_octave: u8,
    
    #[rust(100)]
    keyboard_velocity: u8,
    
    #[rust] white_keys: ComponentMap<PianoKeyId, PianoKey>,
    #[rust] black_keys: ComponentMap<PianoKeyId, PianoKey>,
}

impl LiveHook for Piano {
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        for piano_key in self.white_keys.values_mut().chain(self.black_keys.values_mut()) {
            if let Some(index) = nodes.child_by_name(index, id!(piano_key)) {
                piano_key.apply(cx, apply_from, index, nodes);
            }
        }
        self.scroll_view.redraw(cx);
    }
}

pub enum PianoAction {
    Note {is_on: bool, note_number: u8, velocity: u8},
}

pub enum PianoKeyAction {
    Pressed(u8),
    Up,
}

impl PianoKey {
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, is_black: f32, rect: Rect) {
        self.key_quad.is_black = is_black;
        self.key_quad.draw_abs(cx, rect);
    }
    
    fn set_is_pressed(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.toggle_animator(cx, is, animate, self.pressed_state, self.up_state)
    }
    
    fn set_is_focussed(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.toggle_animator(cx, is, animate, self.focussed_state, self.unfocussed_state)
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, PianoKeyAction),
    ) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.key_quad.draw_vars.redraw(cx);
        }
        match event.hits(cx, self.key_quad.draw_vars.area) {
            HitEvent::FingerHover(f) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match f.hover_state {
                    HoverState::In => {
                        self.animate_to(cx, self.hover_state);
                    }
                    HoverState::Out => {
                        self.animate_to(cx, self.default_state);
                    }
                    _ => {}
                }
            }
            HitEvent::FingerMove(_) => {
            }
            HitEvent::FingerDown(fd) => {
                self.animate_to(cx, self.pressed_state);
                dispatch_action(cx, PianoKeyAction::Pressed(((fd.rel.y / fd.rect.size.y) * 127.0) as u8));
            }
            HitEvent::FingerUp(_) => {
                self.animate_to(cx, self.up_state);
                dispatch_action(cx, PianoKeyAction::Up);
            }
            _ => {}
        }
    }
}


impl Piano {
    pub fn draw(&mut self, cx: &mut Cx2d) {
        // alright lets draw em fuckers
        if self.scroll_view.begin(cx).is_err() {
            return
        };
        let start_pos = cx.get_turtle_pos() + vec2(10., 10.);
        let mut pos = start_pos;
        
        let midi_a0 = 21;
        let midi_c8 = 108;
        
        fn black_key_shift(key: u32) -> Option<f32> {
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
        
        let white_size = vec2(20.0, 100.0);
        let black_size = vec2(15.0, 62.0);
        let piano_key = self.piano_key;
        // draw the white keys first because they go below the black ones
        for i in midi_a0..midi_c8 {
            if black_key_shift(i).is_some() {
                continue;
            }
            let key_id = LiveId(i as u64).into();
            let key = self.white_keys.get_or_insert(cx, key_id, | cx | {
                PianoKey::new_from_option_ptr(cx, piano_key)
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
                    PianoKey::new_from_option_ptr(cx, piano_key)
                });
                key.draw_abs(cx, 1.0, Rect {
                    pos: pos - vec2(black_size.x * shift, 0.),
                    size: black_size
                });
            }
            else {
                pos.x += white_size.x;
            }
        }
        self.scroll_view.end(cx);
        self.white_keys.retain_visible();
        self.black_keys.retain_visible();
    }
    
    pub fn set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.scroll_view.area());
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
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> Vec<PianoAction> {
        let mut a = Vec::new();
        self.handle_event_with_fn(cx, event, &mut | _, v | a.push(v));
        a
    }
    
    pub fn handle_event_with_fn(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, PianoAction),
    ) {
        if self.scroll_view.handle_event(cx, event) {
            self.scroll_view.redraw(cx);
        }
        
        let mut actions = Vec::new();
        for (key_id, piano_key) in self.black_keys.iter_mut().chain(self.white_keys.iter_mut()) {
            piano_key.handle_event(cx, event, &mut | _, e | actions.push((*key_id, e)));
        }
        
        
        for (node_id, action) in actions {
            match action {
                PianoKeyAction::Pressed(velocity) => {
                    self.set_key_focus(cx);
                    dispatch_action(cx, PianoAction::Note {is_on: true, note_number: node_id.0.0 as u8, velocity});
                }
                PianoKeyAction::Up => {
                    dispatch_action(cx, PianoAction::Note {is_on: false, note_number: node_id.0.0 as u8, velocity: 127});
                }
            }
        }
        
        
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
        
        match event.hits(cx, self.scroll_view.area()) {
            HitEvent::KeyDown(ke) => if !ke.is_repeat {
                if let Some(nn) = key_map(ke.key_code) {
                    let note_number = nn + self.keyboard_octave * 12;
                    self.keyboard_keys_down[nn as usize] = note_number;
                    self.set_note(cx, true, note_number);
                    dispatch_action(cx, PianoAction::Note {is_on: true, note_number, velocity: self.keyboard_velocity});
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
            }
            HitEvent::KeyUp(ke) => if let Some(nn) = key_map(ke.key_code) {
                let note_number = self.keyboard_keys_down[nn as usize];
                self.keyboard_keys_down[nn as usize] = 0;
                self.set_note(cx, false, note_number);
                dispatch_action(cx, PianoAction::Note {is_on: false, note_number, velocity: self.keyboard_velocity});
            },
            HitEvent::KeyFocus(_) => {
                for piano_key in self.white_keys.values_mut().chain(self.black_keys.values_mut()) {
                    piano_key.set_is_focussed(cx, true, Animate::Yes)
                }
            }
            HitEvent::KeyFocusLost(_) => {
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

