use crate::cx::*;
use time::precise_time_ns;

#[derive(Clone, Default)]
pub struct WindowsWindow {
    pub last_window_geom: WindowGeom,
    
    pub time_start: u64,
    pub last_key_mod: KeyModifiers,
    pub ime_spot: Vec2,

    pub current_cursor: MouseCursor,
    pub last_mouse_pos: Vec2,
    pub fingers_down: Vec<bool>,
    pub event_callback: Option<*mut FnMut(&mut Vec<Event>)>
}

impl WindowsWindow {


    pub fn init(&mut self, _title: &str) {
        self.time_start = precise_time_ns();
        for _i in 0..10 {
            self.fingers_down.push(false);
        }
    }
    
    pub fn do_callback(&mut self, events: &mut Vec<Event>) {
        unsafe {
            if self.event_callback.is_none() {
                return
            };
            let callback = self.event_callback.unwrap();
            (*callback)(events);
        }
    }

    pub fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {
    }

    pub fn get_window_geom(&self) -> WindowGeom {
        WindowGeom {..Default::default()}
    }
    
    
    pub fn time_now(&self) -> f64 {
        let time_now = precise_time_ns();
        (time_now - self.time_start) as f64 / 1_000_000_000.0
    }
    
    pub fn set_position(&mut self, _pos: Vec2) {
    }
    
    pub fn get_position(&self) -> Vec2 {
        Vec2::zero()
    }
    
    fn get_ime_origin(&self) -> Vec2 {
        Vec2::zero()
    }
    
    pub fn get_inner_size(&self) -> Vec2 {
        Vec2::zero()
    }
    
    pub fn get_outer_size(&self) -> Vec2 {
        Vec2::zero()
    }
    
    pub fn set_outer_size(&self, _size: Vec2) {
    }
    
    pub fn get_dpi_factor(&self) -> f32 {
        1.0
    }

    pub fn poll_events<F>(&mut self, _first_block: bool, mut _event_handler: F)
    where F: FnMut(&mut Vec<Event>),
    {
    }


    pub fn start_timer(&mut self, _timer_id: u64, _interval: f64, _repeats: bool) {
    }
    
    pub fn stop_timer(&mut self, _timer_id: u64) {
    }

    pub fn post_signal(_signal_id: u64, _value: u64) {
    }
        
    pub fn send_change_event(&mut self) {
        
        let new_geom = self.get_window_geom();
        let old_geom = self.last_window_geom.clone();
        self.last_window_geom = new_geom.clone();
        
        self.do_callback(&mut vec![Event::WindowChange(WindowChangeEvent {
            old_geom: old_geom,
            new_geom: new_geom
        })]);
    }
    
    pub fn send_focus_event(&mut self) {
        self.do_callback(&mut vec![Event::AppFocus]);
    }

    pub fn send_focus_lost_event(&mut self) {
        self.do_callback(&mut vec![Event::AppFocusLost]);
    }
    
    pub fn send_finger_down(&mut self, digit: usize, modifiers: KeyModifiers) {
        self.fingers_down[digit] = true;
        self.do_callback(&mut vec![Event::FingerDown(FingerDownEvent {
            abs: self.last_mouse_pos,
            rel: self.last_mouse_pos,
            rect: Rect::zero(),
            digit: digit,
            handled: false,
            is_touch: false,
            modifiers: modifiers,
            tap_count: 0,
            time: self.time_now()
        })]);
    }
    
    pub fn send_finger_up(&mut self, digit: usize, modifiers: KeyModifiers) {
        self.fingers_down[digit] = false;
        self.do_callback(&mut vec![Event::FingerUp(FingerUpEvent {
            abs: self.last_mouse_pos,
            rel: self.last_mouse_pos,
            rect: Rect::zero(),
            abs_start: Vec2::zero(),
            rel_start: Vec2::zero(),
            digit: digit,
            is_over: false,
            is_touch: false,
            modifiers: modifiers,
            time: self.time_now()
        })]);
    }
    
    pub fn send_finger_hover_and_move(&mut self, pos: Vec2, modifiers: KeyModifiers) {
        self.last_mouse_pos = pos;
        let mut events = Vec::new();
        for (digit, down) in self.fingers_down.iter().enumerate() {
            if *down {
                events.push(Event::FingerMove(FingerMoveEvent {
                    abs: pos,
                    rel: pos,
                    rect: Rect::zero(),
                    digit: digit,
                    abs_start: Vec2::zero(),
                    rel_start: Vec2::zero(),
                    is_over: false,
                    is_touch: false,
                    modifiers: modifiers.clone(),
                    time: self.time_now()
                }));
            }
        };
        events.push(Event::FingerHover(FingerHoverEvent {
            abs: pos,
            rel: pos,
            rect: Rect::zero(),
            handled: false,
            hover_state: HoverState::Over,
            modifiers: modifiers,
            time: self.time_now()
        }));
        self.do_callback(&mut events);
    }
    
    pub fn send_close_requested_event(&mut self) {
        self.do_callback(&mut vec![Event::CloseRequested])
    }
    
    pub fn send_text_input(&mut self, input: String, replace_last: bool) {
        self.do_callback(&mut vec![Event::TextInput(TextInputEvent {
            input: input,
            was_paste: false,
            replace_last: replace_last
        })])
    }
}