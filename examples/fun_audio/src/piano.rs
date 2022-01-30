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
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.box(
                0.,
                -4.0,
                self.rect_size.x,
                self.rect_size.y + 4.0,
                2.0
            );
            if self.is_black > 0.5 {
                let hor_shape = pow(1.0 - sin(self.pos.x * PI), 2.8);
                let x = self.pos.y;
                let front_shape_up = mix(0, pow(1.0 - x, 3.0) / 0.0056, smoothstep(0.76, 0.83, x));
                let front_shape_pressed = mix(0, (1.0 - x) / 0.1, smoothstep(0.87, 0.92, x));
                sdf.fill_keep(
                    // this is the funky gradient on the black key
                    vec4((mix(
                        mix(
                            mix(#5c, #11, hor_shape),
                            #4c,
                            self.pressed
                        ),
                        #00,
                        pow(x, 1.5)
                    ) + mix(
                        #00,
                        mix(#44, #11, hor_shape),
                        mix(front_shape_up, front_shape_pressed, self.pressed)
                    ) + #33 * self.hover * (1.0 - self.pressed)).xyz, 1.0)
                );
            }
            else {
                sdf.fill_keep(mix(
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
                ));
            }
            return sdf.result
        }
    }
    
    PianoKey: {{PianoKey}} {
        
        default_state: {
            duration: 0.1
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
    
    focussed_state: Option<LivePtr>,
    unfocussed_state: Option<LivePtr>,
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    pressed_state: Option<LivePtr>,
    up_state: Option<LivePtr>,
}

#[derive(Live)]
pub struct Piano {
    scroll_view: ScrollView,
    piano_key: Option<LivePtr>,
    
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
    Pressed(PianoKeyId),
    Up(PianoKeyId),
}

pub enum PianoKeyAction {
    Pressed,
    Up,
    None,
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
            HitEvent::FingerMove(f) => {
            }
            HitEvent::FingerDown(_) => {
                self.animate_to(cx, self.pressed_state);
                dispatch_action(cx, PianoKeyAction::Pressed);
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
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_view.redraw(cx);
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
                PianoKeyAction::Pressed => {
                    dispatch_action(cx, PianoAction::Pressed(node_id));
                }
                PianoKeyAction::Up => {
                    dispatch_action(cx, PianoAction::Up(node_id));
                }
                _ => ()
            }
        }
        
        match event.hits(cx, self.scroll_view.area()) {
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

