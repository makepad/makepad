use crate::{
    Area,
    Cx,
    Event,
    Hit,
    MouseCursor,
    NextFrame,
};

#[derive(Clone, Copy, Debug)]
struct ScrollSample{
    abs: f64,
    time: f64,
}

#[derive(Default, Clone, Debug)]
pub enum ScrollMode {
    #[default]
    DragAndDrop,
    Swipe,
}

#[derive(Default, Clone, Debug)]
enum ScrollState {
    #[default]
    Stopped,
    Drag{samples:Vec<ScrollSample>},
    Flick {delta: f64, next_frame: NextFrame},
    Pulldown {next_frame: NextFrame},
}

#[derive(Default, PartialEq)]
pub enum TouchMotionChange {
    #[default]
    None,
    ScrollStateChanged,
    ScrolledAtChanged,
}

#[derive(Default, Clone)]
pub struct TouchGesture {
    flick_scroll_minimum: f64,
    flick_scroll_maximum: f64,
    flick_scroll_scaling: f64,
    flick_scroll_decay: f64,

    scroll_mode: ScrollMode,
    scroll_state: ScrollState,

    min_scrolled_at: f64,
    max_scrolled_at: f64,
    pulldown_maximum: f64,

    pub scrolled_at: f64,
}

impl TouchGesture {
    pub fn new() -> Self {
        Self {
            flick_scroll_minimum: 0.2,
            flick_scroll_maximum: 80.0,
            flick_scroll_scaling: 0.005,
            flick_scroll_decay: 0.98,

            scroll_state: ScrollState::Stopped,
            scroll_mode: ScrollMode::DragAndDrop,

            scrolled_at: 0.0,
            min_scrolled_at: f64::MIN,
            max_scrolled_at: f64::MAX,
            pulldown_maximum: 60.0,
        }
    }

    pub fn reset_scrolled_at(&mut self) {
        self.scrolled_at = 0.0;
    }

    pub fn set_mode(&mut self, scroll_mode: ScrollMode) {
        self.scroll_mode = scroll_mode;
    }

    pub fn set_range(&mut self, min_offset: f64, max_offset: f64) {
        self.min_scrolled_at = min_offset;
        self.max_scrolled_at = max_offset;
        self.scrolled_at = self.scrolled_at.clamp(
            self.min_scrolled_at - self.pulldown_maximum,
            self.max_scrolled_at + self.pulldown_maximum
        );
    }

    pub fn stop(&mut self) {
        self.scrolled_at = 0.0;
        self.scroll_state = ScrollState::Stopped;
    }

    pub fn is_stopped(&self) -> bool {
        match self.scroll_state {
            ScrollState::Stopped => true,
            _ => false
        }
    }

    pub fn is_dragging(&self) -> bool {
        match self.scroll_state {
            ScrollState::Drag {..} => true,
            _ => false
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, area: Area) -> TouchMotionChange {
        let needs_pulldown_when_flicking = self.needs_pulldown_when_flicking();
        let needs_pulldown = self.needs_pulldown();

        match &mut self.scroll_state {
            ScrollState::Flick {delta, next_frame} => {
                if let Some(_) = next_frame.is_event(event) {
                    *delta = *delta * self.flick_scroll_decay;
                    if needs_pulldown_when_flicking {
                        self.scroll_state = ScrollState::Pulldown {next_frame: cx.new_next_frame()};
                        return TouchMotionChange::ScrollStateChanged
                    } else if delta.abs() > self.flick_scroll_minimum {
                        *next_frame = cx.new_next_frame();
                        let delta = *delta;

                        let new_offset = self.scrolled_at - delta;
                        self.scrolled_at = new_offset.clamp(
                            self.min_scrolled_at - self.pulldown_maximum,
                            self.max_scrolled_at + self.pulldown_maximum
                        );

                        return TouchMotionChange::ScrolledAtChanged
                    } else {
                        if needs_pulldown {
                            self.scroll_state = ScrollState::Pulldown {next_frame: cx.new_next_frame()};
                        } else {
                            self.scroll_state = ScrollState::Stopped;
                        }

                        return TouchMotionChange::ScrollStateChanged
                    }
                }
            }
            ScrollState::Pulldown {next_frame} => {
                if let Some(_) = next_frame.is_event(event) {
                    if self.scrolled_at < self.min_scrolled_at {
                        self.scrolled_at += (self.min_scrolled_at - self.scrolled_at) * 0.1;
                        if self.min_scrolled_at - self.scrolled_at < 1.0 {
                            self.scrolled_at = self.min_scrolled_at + 0.5;
                        }
                        else {
                            *next_frame = cx.new_next_frame();
                        }

                        return TouchMotionChange::ScrolledAtChanged
                    }
                    else if self.scrolled_at > self.max_scrolled_at {
                        self.scrolled_at -= (self.scrolled_at - self.max_scrolled_at) * 0.1;
                        if self.scrolled_at - self.max_scrolled_at < 1.0 {
                            self.scrolled_at = self.max_scrolled_at - 0.5;

                            return TouchMotionChange::ScrolledAtChanged
                        }
                        else {
                            *next_frame = cx.new_next_frame();
                        }

                        return TouchMotionChange::ScrolledAtChanged
                    }
                    else {
                        self.scroll_state = ScrollState::Stopped;
                        return TouchMotionChange::ScrollStateChanged
                    }
                }
            }
            _=>()
        }

        match event.hits_with_capture_overload(cx, area, true) {
            Hit::FingerDown(e) => {
                self.scroll_state = ScrollState::Drag {
                    samples: vec![ScrollSample{abs: e.abs.y, time: e.time}]
                };

                return TouchMotionChange::ScrollStateChanged
            }
            Hit::FingerMove(e) => {
                cx.set_cursor(MouseCursor::Default);
                match &mut self.scroll_state {
                    ScrollState::Drag {samples}=>{
                        let new_abs = e.abs.y;
                        let old_sample = *samples.last().unwrap();
                        samples.push(ScrollSample{abs: new_abs, time: e.time});
                        if samples.len() > 4 {
                            samples.remove(0);
                        }
                        let new_offset = self.scrolled_at + old_sample.abs - new_abs;
                        self.scrolled_at = new_offset.clamp(
                            self.min_scrolled_at - self.pulldown_maximum,
                            self.max_scrolled_at + self.pulldown_maximum
                        );

                        return TouchMotionChange::ScrolledAtChanged
                    }
                    _=>()
                }
            }
            Hit::FingerUp(_e) => {
                match &mut self.scroll_state {
                    ScrollState::Drag {samples} => {
                        match self.scroll_mode {
                            ScrollMode::Swipe => {
                                let mut last = None;
                                let mut scaled_delta = 0.0;
                                let mut total_delta = 0.0;
                                for sample in samples.iter().rev() {
                                    if last.is_none() {
                                        last = Some(sample);
                                    }
                                    else {
                                        total_delta += last.unwrap().abs - sample.abs;
                                        scaled_delta += (last.unwrap().abs - sample.abs)/ (last.unwrap().time - sample.time)
                                    }
                                }
                                scaled_delta *= self.flick_scroll_scaling;

                                if self.needs_pulldown() {
                                    self.scroll_state = ScrollState::Pulldown {next_frame: cx.new_next_frame()};
                                }
                                else if total_delta.abs() > 10.0 && scaled_delta.abs() > self.flick_scroll_minimum {
                                    self.scroll_state = ScrollState::Flick {
                                        delta: scaled_delta.min(self.flick_scroll_maximum).max(-self.flick_scroll_maximum),
                                        next_frame: cx.new_next_frame()
                                    };
                                } else {
                                    self.scroll_state = ScrollState::Stopped;
                                }

                                return TouchMotionChange::ScrollStateChanged
                            }
                            ScrollMode::DragAndDrop => {
                                self.scroll_state = ScrollState::Stopped;
                                return TouchMotionChange::ScrollStateChanged
                            }
                        }
                    }
                    _=>()
                }
            }
            _ => ()
        }

        TouchMotionChange::None
    }

    fn needs_pulldown(&self) -> bool {
        self.scrolled_at < self.min_scrolled_at || self.scrolled_at > self.max_scrolled_at
    }

    fn needs_pulldown_when_flicking(&self) -> bool {
        self.scrolled_at - 0.5 < self.min_scrolled_at - self.pulldown_maximum ||
            self.scrolled_at + 0.5 > self.max_scrolled_at + self.pulldown_maximum
    }
}

impl TouchMotionChange {
    pub fn has_changed(&self) -> bool {
        match self {
            TouchMotionChange::None => false,
            _ => true
        }
    }
}