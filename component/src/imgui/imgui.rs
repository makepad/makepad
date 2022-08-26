use {
    std::rc::Rc,
    std::ops::Deref,
    std::cell::{RefMut, RefCell},
    crate::{
        makepad_draw_2d::*,
        frame::*,
    },
};

#[derive(Clone)]
pub struct ImGUIActions(pub Rc<Vec<FrameActionItem >>);

pub struct ImGUIRun<'a> {
    pub cx: &'a mut Cx,
    pub event: &'a Event,
    pub actions: ImGUIActions,
    pub auto_id: u64,
    pub new_items: Vec<LiveId>,
    pub imgui: ImGUI
}

impl<'a> Deref for  ImGUIRun<'a> {
    type Target = Event;
    fn deref(&self) -> &Self::Target {self.event}
}

impl<'a> ImGUIRun<'a> {
    pub fn safe_ref<T: 'static + FrameComponent>(&self, what: Option<&mut Box<dyn FrameComponent >>) -> ImGUIRef {
        let uid = if let Some(what) = what {
            if what.cast::<T>().is_none() {
                FrameUid::empty()
            }
            else {
                FrameUid::from_frame_component(what)
            }
        }
        else {
            FrameUid::empty()
        };
        ImGUIRef {
            actions: self.actions.clone(),
            imgui: self.imgui.clone(),
            uid
        }
    }
    
    pub fn frame(&self) -> RefMut<'_, Frame> {
        self.imgui.frame()
    }
    
    pub fn alloc_auto_id(&mut self) -> u64 {
        self.auto_id += 1;
        self.auto_id
    }
    
    pub fn stop(self) {}
    
    pub fn bind_read(&mut self, nodes: &[LiveNode]) {
        self.imgui.frame().bind_read(self.cx, nodes);
    }
    
    
    
    // ImGUI event forwards
    
    
    pub fn on_bind_deltas(&self) -> Vec<Vec<LiveNode >> {
        let mut ret = Vec::new();
        for item in self.actions.0.iter() {
            if let Some(delta) = &item.bind_delta {
                ret.push(delta.clone())
            }
        }
        ret
    }
    

    
    pub fn on_construct(&self) -> bool {
        if let Event::Construct = self.event {true}else {false}
    }
    
}

pub struct ImGUIRef {
    pub imgui: ImGUI,
    pub actions: ImGUIActions,
    pub uid: FrameUid,
}

impl ImGUIRef {
    pub fn find_single_action(&self) -> Option<&FrameActionItem> {
        self.actions.0.iter().find( | v | v.uid() == self.uid)
    }
    
    pub fn inner<T: 'static + FrameComponent>(&self) -> Option<std::cell::RefMut<'_, T >> {
        if self.uid.is_empty() {
            None
        }
        else {
            Some(std::cell::RefMut::map(self.imgui.frame(), | frame | {
                frame.component_by_uid(self.uid).unwrap().cast_mut::<T>().unwrap()
            }))
        }
    }
}

pub struct ImGUIInner {
    frame: Frame,
    _old_items: Option<Vec<LiveId >>,
}

#[derive(Clone)]
pub struct ImGUI {
    inner: Rc<RefCell<ImGUIInner >>,
}

impl ImGUI {
    pub fn frame(&self) -> RefMut<'_, Frame> {
        RefMut::map(self.inner.borrow_mut(), | v | &mut v.frame)
    }
    
    pub fn run<'a>(&self, cx: &'a mut Cx, event: &'a Event) -> ImGUIRun<'a> {
        // fetch actions and wrap
        let actions = ImGUIActions(Rc::new(self.frame().handle_event_vec(cx, event)));
        ImGUIRun {
            event,
            cx,
            actions,
            auto_id: 0,
            new_items: Vec::new(),
            imgui: self.clone()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) -> FrameDraw {
        self.inner.borrow_mut().frame.draw(cx)
    }
    
    pub fn by_type<T: 'static + FrameComponent>(&mut self) -> Option<std::cell::RefMut<'_, T >> {
        if self.frame().by_type::<T>().is_some() {
            Some(std::cell::RefMut::map(self.frame(), | frame | {
                frame.by_type::<T>().unwrap()
            }))
        }
        else {
            None
        }
    }
}

impl LiveNew for ImGUI {
    fn new(cx: &mut Cx) -> Self {
        Self {
            inner: Rc::new(RefCell::new(ImGUIInner {
                frame: Frame::new(cx),
                _old_items: None
            }))
        }
    }
    
    fn live_type_info(cx: &mut Cx) -> LiveTypeInfo {
        Frame::live_type_info(cx)
    }
}

impl LiveApply for ImGUI {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        self.inner.borrow_mut().frame.apply(cx, from, index, nodes)
    }
}

impl LiveHook for ImGUI {}

// ImGUI Event helpers

pub trait ImGuiEventExt{
    fn on_construct(&self) -> bool;
    fn on_destruct(&self) -> bool;
    fn on_draw(&self) -> Option<DrawEvent>;
    fn on_app_got_focus(&self) -> bool;
    fn on_app_lost_focus(&self) -> bool;
    fn on_next_frame(&self, next_frame: NextFrame) -> Option<NextFrameEvent>;
    fn on_xr_update(&self) -> Option<XRUpdateEvent>;
    fn on_window_drag_query(&self) -> Option<WindowDragQueryEvent>;
    fn on_window_close_requested(&self) -> Option<WindowCloseRequestedEvent>;
    fn on_window_closed(&self) -> Option<WindowClosedEvent>;
    fn on_window_geom_change(&self) -> Option<WindowGeomChangeEvent>;
    fn on_finger_down(&self) -> Option<FingerDownEvent>;
    fn on_finger_move(&self) -> Option<FingerMoveEvent>;
    fn on_finger_hover(&self) -> Option<FingerHoverEvent> ;
    fn on_finger_up(&self) -> Option<FingerUpEvent>;
    fn on_finger_scroll(&self) -> Option<FingerScrollEvent>;
    fn on_timer(&self, timer: Timer) -> bool;
    fn on_signal(&self, signal: Signal) -> bool;
    fn on_trigger(&self, area: Area) -> Option<Vec<Trigger>>;
    fn on_menu_command(&self) -> Option<MenuCommand>;
    fn on_key_focus(&self) -> Option<KeyFocusEvent>;
    fn on_key_focus_lost(&self) -> Option<KeyFocusEvent>;
    fn on_key_down(&self) -> Option<KeyEvent>;
    fn on_key_up(&self) -> Option<KeyEvent>;
    fn on_text_input(&self) -> Option<TextInputEvent>;
    fn on_text_copy(&self) -> Option<TextCopyEvent>;
    fn on_drag(&self) -> Option<DragEvent>;
    fn on_drop(&self) -> Option<DropEvent>;
    fn on_drag_end(&self) -> bool;
    //fn on_midi_1_input_data(&self) -> Vec<Midi1InputData>;
    //fn on_midi_1_notes(&self) -> Vec<Midi1Note> ;
    //fn on_midi_input_list(&self) -> Option<MidiInputListEvent> ;
}

// Event immediate mode API
impl ImGuiEventExt for Event {
    //Construct,
    fn on_construct(&self) -> bool {
        if let Self::Construct = self {true}else {false}
    }
    //Destruct,
    fn on_destruct(&self) -> bool {
        if let Self::Construct = self {true}else {false}
    }
    //Draw(DrawEvent),
    fn on_draw(&self) -> Option<DrawEvent> {
        if let Self::Draw(e) = self {Some(e.clone())}else {None}
    }
    
    //AppGotFocus,
    fn on_app_got_focus(&self) -> bool {
        if let Self::AppGotFocus = self {true}else {false}
    }
    //AppLostFocus,
    fn on_app_lost_focus(&self) -> bool {
        if let Self::AppLostFocus = self {true}else {false}
    }
    
    //NextFrame(NextFrameEvent),
    fn on_next_frame(&self, next_frame: NextFrame) -> Option<NextFrameEvent> {
        if let Self::NextFrame(e) = self {
            if e.set.contains(&next_frame) {
                Some(e.clone())
            }
            else {
                None
            }
        }else {
            None
        }
    }
    
    //XRUpdate(XRUpdateEvent),
    fn on_xr_update(&self) -> Option<XRUpdateEvent> {
        if let Self::XRUpdate(e) = self {Some(e.clone())}else {None}
    }
    //WindowDragQuery(WindowDragQueryEvent),
    fn on_window_drag_query(&self) -> Option<WindowDragQueryEvent> {
        if let Self::WindowDragQuery(e) = self {Some(e.clone())}else {None}
    }
    //WindowCloseRequested(WindowCloseRequestedEvent),
    fn on_window_close_requested(&self) -> Option<WindowCloseRequestedEvent> {
        if let Self::WindowCloseRequested(e) = self {Some(e.clone())}else {None}
    }
    //WindowClosed(WindowClosedEvent),
    fn on_window_closed(&self) -> Option<WindowClosedEvent> {
        if let Self::WindowClosed(e) = self {Some(e.clone())}else {None}
    }
    //WindowGeomChange(WindowGeomChangeEvent),
    fn on_window_geom_change(&self) -> Option<WindowGeomChangeEvent> {
        if let Self::WindowGeomChange(e) = self {Some(e.clone())}else {None}
    }
    
    //FingerDown(FingerDownEvent),
    fn on_finger_down(&self) -> Option<FingerDownEvent> {
        if let Self::FingerDown(e) = self {Some(e.clone())}else {None}
    }
    //FingerMove(FingerMoveEvent),
    fn on_finger_move(&self) -> Option<FingerMoveEvent> {
        if let Self::FingerMove(e) = self {Some(e.clone())}else {None}
    }
    //FingerHover(FingerHoverEvent),
    fn on_finger_hover(&self) -> Option<FingerHoverEvent> {
        if let Self::FingerHover(e) = self {Some(e.clone())}else {None}
    }
    //FingerUp(FingerUpEvent),
    fn on_finger_up(&self) -> Option<FingerUpEvent> {
        if let Self::FingerUp(e) = self {Some(e.clone())}else {None}
    }
    //FingerScroll(FingerScrollEvent),
    fn on_finger_scroll(&self) -> Option<FingerScrollEvent> {
        if let Self::FingerScroll(e) = self {Some(e.clone())}else {None}
    }
    //Timer(TimerEvent),
    fn on_timer(&self, timer: Timer) -> bool {
        if let Self::Timer(e) = self {e.timer_id == timer.0}else {false}
    }
    
    //Signal(SignalEvent),
    fn on_signal(&self, signal: Signal) -> bool {
        if let Self::Signal(e) = self {e.signals.contains(&signal)}else {false}
    }

    //Trigger(Trigger),
    fn on_trigger(&self, area: Area) -> Option<Vec<Trigger>> {
        if let Self::Trigger(e) = self {e.triggers.get(&area).cloned()}else {None}
    }
    
    //MenuCommand(MenuCommand),
    fn on_menu_command(&self) -> Option<MenuCommand> {
        if let Self::MenuCommand(e) = self {Some(e.clone())}else {None}
    }

    //KeyFocus(KeyFocusEvent),
    fn on_key_focus(&self) -> Option<KeyFocusEvent> {
        if let Self::KeyFocus(e) = self {Some(e.clone())}else {None}
    }
    
    //KeyFocusLost(KeyFocusEvent),
    fn on_key_focus_lost(&self) -> Option<KeyFocusEvent> {
        if let Self::KeyFocusLost(e) = self {Some(e.clone())}else {None}
    }

    //KeyDown(KeyEvent),
    fn on_key_down(&self) -> Option<KeyEvent> {
        if let Self::KeyDown(e) = self {Some(e.clone())}else {None}
    }

    //KeyUp(KeyEvent),
    fn on_key_up(&self) -> Option<KeyEvent> {
        if let Self::KeyDown(e) = self {Some(e.clone())}else {None}
    }
    
    //TextInput(TextInputEvent),
    fn on_text_input(&self) -> Option<TextInputEvent> {
        if let Self::TextInput(e) = self {Some(e.clone())}else {None}
    }

    //TextCopy(TextCopyEvent),
    fn on_text_copy(&self) -> Option<TextCopyEvent> {
        if let Self::TextCopy(e) = self {Some(e.clone())}else {None}
    }

    //Drag(DragEvent),
    fn on_drag(&self) -> Option<DragEvent> {
        if let Self::Drag(e) = self {Some(e.clone())}else {None}
    }
    
    //Drop(DropEvent),
    fn on_drop(&self) -> Option<DropEvent> {
        if let Self::Drop(e) = self {Some(e.clone())}else {None}
    }

    //DragEnd
    fn on_drag_end(&self) -> bool {
        if let Self::DragEnd = self {true}else {false}
    }
/*
    fn on_midi_1_input_data(&self) -> Vec<Midi1InputData> {
        if let Event::Midi1InputData(inputs) = self{
            inputs.clone()
        }
        else{
            Vec::new()
        }
    }

    fn on_midi_1_notes(&self) -> Vec<Midi1Note> {
        let mut ret = Vec::new();
        if let Event::Midi1InputData(inputs) = self{
            for input in inputs{
                if let Midi1Event::Note(note) = input.data.decode() {
                    ret.push(note);
                }
            }
        }
        ret
    }
    
    fn on_midi_input_list(&self) -> Option<MidiInputListEvent> {
        if let Event::MidiInputList(inputs) = self{
            Some(inputs.clone())
        }
        else{
            None
        }
    }*/
}

// shortcuts to check for keypresses


trait ImGUIKeyEventExt {
    fn backtick(&self) -> bool;
    fn key_0(&self) -> bool;
    fn key_1(&self) -> bool;
    fn key_2(&self) -> bool;
    fn key_3(&self) -> bool;
    fn key_4(&self) -> bool;
    fn key_5(&self) -> bool;
    fn key_6(&self) -> bool;
    fn key_7(&self) -> bool;
    fn key_8(&self) -> bool;
    fn key_9(&self) -> bool;
    fn minus(&self) -> bool;
    fn equals(&self) -> bool;
    
    fn backspace(&self) -> bool;
    fn tab(&self) -> bool;
    
    fn key_q(&self) -> bool;
    fn key_w(&self) -> bool;
    fn key_e(&self) -> bool;
    fn key_r(&self) -> bool;
    fn key_t(&self) -> bool;
    fn key_y(&self) -> bool;
    fn key_u(&self) -> bool;
    fn key_i(&self) -> bool;
    fn key_o(&self) -> bool;
    fn key_p(&self) -> bool;
    fn l_bracket(&self) -> bool;
    fn r_bracket(&self) -> bool;
    fn return_key(&self) -> bool;
    
    fn key_a(&self) -> bool;
    fn key_s(&self) -> bool;
    fn key_d(&self) -> bool;
    fn key_f(&self) -> bool;
    fn key_g(&self) -> bool;
    fn key_h(&self) -> bool;
    fn key_j(&self) -> bool;
    fn key_k(&self) -> bool;
    fn key_l(&self) -> bool;
    fn semicolon(&self) -> bool;
    fn quote(&self) -> bool;
    fn backslash(&self) -> bool;
    
    fn key_z(&self) -> bool;
    fn key_x(&self) -> bool;
    fn key_c(&self) -> bool;
    fn key_v(&self) -> bool;
    fn key_b(&self) -> bool;
    fn key_n(&self) -> bool;
    fn key_m(&self) -> bool;
    fn comma(&self) -> bool;
    fn period(&self) -> bool;
    fn slash(&self) -> bool;
    
    fn control(&self) -> bool;
    fn alt(&self) -> bool;
    fn shift(&self) -> bool;
    fn logo(&self) -> bool;
    
    fn mod_control(&self) -> bool;
    fn mod_alt(&self) -> bool;
    fn mod_shift(&self) -> bool;
    fn mod_logo(&self) -> bool;
    
    fn space(&self) -> bool;
    fn capslock(&self) -> bool;
    fn f1(&self) -> bool;
    fn f2(&self) -> bool;
    fn f3(&self) -> bool;
    fn f4(&self) -> bool;
    fn f5(&self) -> bool;
    fn f6(&self) -> bool;
    fn f7(&self) -> bool;
    fn f8(&self) -> bool;
    fn f9(&self) -> bool;
    fn f10(&self) -> bool;
    fn f11(&self) -> bool;
    fn f12(&self) -> bool;
    
    fn print_screen(&self) -> bool;
    fn scroll_lock(&self) -> bool;
    fn pause(&self) -> bool;
    
    fn insert(&self) -> bool;
    fn delete(&self) -> bool;
    fn home(&self) -> bool;
    fn end(&self) -> bool;
    fn page_up(&self) -> bool;
    fn page_down(&self) -> bool;
    
    fn numpad_0(&self) -> bool;
    fn numpad_1(&self) -> bool;
    fn numpad_2(&self) -> bool;
    fn numpad_3(&self) -> bool;
    fn numpad_4(&self) -> bool;
    fn numpad_5(&self) -> bool;
    fn numpad_6(&self) -> bool;
    fn numpad_7(&self) -> bool;
    fn numpad_8(&self) -> bool;
    fn numpad_9(&self) -> bool;
    
    fn numpad_equals(&self) -> bool;
    fn numpad_subtract(&self) -> bool;
    fn numpad_add(&self) -> bool;
    fn numpad_decimal(&self) -> bool;
    fn numpad_multiply(&self) -> bool;
    fn numpad_divide(&self) -> bool;
    fn numpad_lock(&self) -> bool;
    fn numpad_enter(&self) -> bool;
    
    fn arrow_up(&self) -> bool;
    fn arrow_down(&self) -> bool;
    fn arrow_left(&self) -> bool;
    fn arrow_right(&self) -> bool;
    
    fn unknown(&self) -> bool;
}

impl ImGUIKeyEventExt for KeyEvent{
    fn backtick(&self) -> bool {if let KeyCode::Backtick = self.key_code {true}else {false}}
    fn key_0(&self) -> bool {if let KeyCode::Key0 = self.key_code {true}else {false}}
    fn key_1(&self) -> bool {if let KeyCode::Key1 = self.key_code {true}else {false}}
    fn key_2(&self) -> bool {if let KeyCode::Key2 = self.key_code {true}else {false}}
    fn key_3(&self) -> bool {if let KeyCode::Key3 = self.key_code {true}else {false}}
    fn key_4(&self) -> bool {if let KeyCode::Key4 = self.key_code {true}else {false}}
    fn key_5(&self) -> bool {if let KeyCode::Key5 = self.key_code {true}else {false}}
    fn key_6(&self) -> bool {if let KeyCode::Key6 = self.key_code {true}else {false}}
    fn key_7(&self) -> bool {if let KeyCode::Key7 = self.key_code {true}else {false}}
    fn key_8(&self) -> bool {if let KeyCode::Key8 = self.key_code {true}else {false}}
    fn key_9(&self) -> bool {if let KeyCode::Key9 = self.key_code {true}else {false}}
    fn minus(&self) -> bool {if let KeyCode::Minus = self.key_code {true}else {false}}
    fn equals(&self) -> bool {if let KeyCode::Equals = self.key_code {true}else {false}}
    
    fn backspace(&self) -> bool {if let KeyCode::Backspace = self.key_code {true}else {false}}
    fn tab(&self) -> bool {if let KeyCode::Tab = self.key_code {true}else {false}}
    
    fn key_q(&self) -> bool {if let KeyCode::KeyQ = self.key_code {true}else {false}}
    fn key_w(&self) -> bool {if let KeyCode::KeyW = self.key_code {true}else {false}}
    fn key_e(&self) -> bool {if let KeyCode::KeyE = self.key_code {true}else {false}}
    fn key_r(&self) -> bool {if let KeyCode::KeyR = self.key_code {true}else {false}}
    fn key_t(&self) -> bool {if let KeyCode::KeyT = self.key_code {true}else {false}}
    fn key_y(&self) -> bool {if let KeyCode::KeyY = self.key_code {true}else {false}}
    fn key_u(&self) -> bool {if let KeyCode::KeyU = self.key_code {true}else {false}}
    fn key_i(&self) -> bool {if let KeyCode::KeyI = self.key_code {true}else {false}}
    fn key_o(&self) -> bool {if let KeyCode::KeyO = self.key_code {true}else {false}}
    fn key_p(&self) -> bool {if let KeyCode::KeyP = self.key_code {true}else {false}}
    fn l_bracket(&self) -> bool {if let KeyCode::LBracket = self.key_code {true}else {false}}
    fn r_bracket(&self) -> bool {if let KeyCode::RBracket = self.key_code {true}else {false}}
    fn return_key(&self) -> bool {if let KeyCode::ReturnKey = self.key_code {true}else {false}}
    
    fn key_a(&self) -> bool {if let KeyCode::KeyA = self.key_code {true}else {false}}
    fn key_s(&self) -> bool {if let KeyCode::KeyS = self.key_code {true}else {false}}
    fn key_d(&self) -> bool {if let KeyCode::KeyD = self.key_code {true}else {false}}
    fn key_f(&self) -> bool {if let KeyCode::KeyF = self.key_code {true}else {false}}
    fn key_g(&self) -> bool {if let KeyCode::KeyG = self.key_code {true}else {false}}
    fn key_h(&self) -> bool {if let KeyCode::KeyH = self.key_code {true}else {false}}
    fn key_j(&self) -> bool {if let KeyCode::KeyJ = self.key_code {true}else {false}}
    fn key_k(&self) -> bool {if let KeyCode::KeyK = self.key_code {true}else {false}}
    fn key_l(&self) -> bool {if let KeyCode::KeyL = self.key_code {true}else {false}}
    fn semicolon(&self) -> bool {if let KeyCode::Semicolon = self.key_code {true}else {false}}
    fn quote(&self) -> bool {if let KeyCode::Quote = self.key_code {true}else {false}}
    fn backslash(&self) -> bool {if let KeyCode::Backslash = self.key_code {true}else {false}}
    
    fn key_z(&self) -> bool {if let KeyCode::KeyZ = self.key_code {true}else {false}}
    fn key_x(&self) -> bool {if let KeyCode::KeyX = self.key_code {true}else {false}}
    fn key_c(&self) -> bool {if let KeyCode::KeyC = self.key_code {true}else {false}}
    fn key_v(&self) -> bool {if let KeyCode::KeyV = self.key_code {true}else {false}}
    fn key_b(&self) -> bool {if let KeyCode::KeyB = self.key_code {true}else {false}}
    fn key_n(&self) -> bool {if let KeyCode::KeyN = self.key_code {true}else {false}}
    fn key_m(&self) -> bool {if let KeyCode::KeyM = self.key_code {true}else {false}}
    fn comma(&self) -> bool {if let KeyCode::Comma = self.key_code {true}else {false}}
    fn period(&self) -> bool {if let KeyCode::Period = self.key_code {true}else {false}}
    fn slash(&self) -> bool {if let KeyCode::Slash = self.key_code {true}else {false}}
    
    fn control(&self) -> bool {if let KeyCode::Control = self.key_code {true}else {false}}
    fn alt(&self) -> bool {if let KeyCode::Alt = self.key_code {true}else {false}}
    fn shift(&self) -> bool {if let KeyCode::Shift = self.key_code {true}else {false}}
    fn logo(&self) -> bool {if let KeyCode::Logo = self.key_code {true}else {false}}
    
    fn mod_control(&self) -> bool {self.modifiers.control}
    fn mod_alt(&self) -> bool {self.modifiers.alt}
    fn mod_shift(&self) -> bool {self.modifiers.shift}
    fn mod_logo(&self) -> bool {self.modifiers.logo}
    
    fn space(&self) -> bool {if let KeyCode::Space = self.key_code {true}else {false}}
    fn capslock(&self) -> bool {if let KeyCode::Capslock = self.key_code {true}else {false}}
    fn f1(&self) -> bool {if let KeyCode::F1 = self.key_code {true}else {false}}
    fn f2(&self) -> bool {if let KeyCode::F2 = self.key_code {true}else {false}}
    fn f3(&self) -> bool {if let KeyCode::F3 = self.key_code {true}else {false}}
    fn f4(&self) -> bool {if let KeyCode::F4 = self.key_code {true}else {false}}
    fn f5(&self) -> bool {if let KeyCode::F5 = self.key_code {true}else {false}}
    fn f6(&self) -> bool {if let KeyCode::F6 = self.key_code {true}else {false}}
    fn f7(&self) -> bool {if let KeyCode::F7 = self.key_code {true}else {false}}
    fn f8(&self) -> bool {if let KeyCode::F8 = self.key_code {true}else {false}}
    fn f9(&self) -> bool {if let KeyCode::F9 = self.key_code {true}else {false}}
    fn f10(&self) -> bool {if let KeyCode::F10 = self.key_code {true}else {false}}
    fn f11(&self) -> bool {if let KeyCode::F11 = self.key_code {true}else {false}}
    fn f12(&self) -> bool {if let KeyCode::F12 = self.key_code {true}else {false}}
    
    fn print_screen(&self) -> bool {if let KeyCode::PrintScreen = self.key_code {true}else {false}}
    fn scroll_lock(&self) -> bool {if let KeyCode::ScrollLock = self.key_code {true}else {false}}
    fn pause(&self) -> bool {if let KeyCode::Pause = self.key_code {true}else {false}}
    
    fn insert(&self) -> bool {if let KeyCode::Insert = self.key_code {true}else {false}}
    fn delete(&self) -> bool {if let KeyCode::Delete = self.key_code {true}else {false}}
    fn home(&self) -> bool {if let KeyCode::Home = self.key_code {true}else {false}}
    fn end(&self) -> bool {if let KeyCode::End = self.key_code {true}else {false}}
    fn page_up(&self) -> bool {if let KeyCode::PageUp = self.key_code {true}else {false}}
    fn page_down(&self) -> bool {if let KeyCode::PageDown = self.key_code {true}else {false}}
    
    fn numpad_0(&self) -> bool {if let KeyCode::Numpad0 = self.key_code {true}else {false}}
    fn numpad_1(&self) -> bool {if let KeyCode::Numpad1 = self.key_code {true}else {false}}
    fn numpad_2(&self) -> bool {if let KeyCode::Numpad2 = self.key_code {true}else {false}}
    fn numpad_3(&self) -> bool {if let KeyCode::Numpad3 = self.key_code {true}else {false}}
    fn numpad_4(&self) -> bool {if let KeyCode::Numpad4 = self.key_code {true}else {false}}
    fn numpad_5(&self) -> bool {if let KeyCode::Numpad5 = self.key_code {true}else {false}}
    fn numpad_6(&self) -> bool {if let KeyCode::Numpad6 = self.key_code {true}else {false}}
    fn numpad_7(&self) -> bool {if let KeyCode::Numpad7 = self.key_code {true}else {false}}
    fn numpad_8(&self) -> bool {if let KeyCode::Numpad8 = self.key_code {true}else {false}}
    fn numpad_9(&self) -> bool {if let KeyCode::Numpad9 = self.key_code {true}else {false}}
    
    fn numpad_equals(&self) -> bool {if let KeyCode::NumpadEquals = self.key_code {true}else {false}}
    fn numpad_subtract(&self) -> bool {if let KeyCode::NumpadSubtract = self.key_code {true}else {false}}
    fn numpad_add(&self) -> bool {if let KeyCode::NumpadAdd = self.key_code {true}else {false}}
    fn numpad_decimal(&self) -> bool {if let KeyCode::NumpadDecimal = self.key_code {true}else {false}}
    fn numpad_multiply(&self) -> bool {if let KeyCode::NumpadMultiply = self.key_code {true}else {false}}
    fn numpad_divide(&self) -> bool {if let KeyCode::NumpadDivide = self.key_code {true}else {false}}
    fn numpad_lock(&self) -> bool {if let KeyCode::Numlock = self.key_code {true}else {false}}
    fn numpad_enter(&self) -> bool {if let KeyCode::NumpadEnter = self.key_code {true}else {false}}
    
    fn arrow_up(&self) -> bool {if let KeyCode::ArrowUp = self.key_code {true}else {false}}
    fn arrow_down(&self) -> bool {if let KeyCode::ArrowDown = self.key_code {true}else {false}}
    fn arrow_left(&self) -> bool {if let KeyCode::ArrowLeft = self.key_code {true}else {false}}
    fn arrow_right(&self) -> bool {if let KeyCode::ArrowRight = self.key_code {true}else {false}}
    
    fn unknown(&self) -> bool {if let KeyCode::Unknown = self.key_code {true}else {false}}
}

impl ImGUIKeyEventExt for Option<KeyEvent> {
    
    fn mod_control(&self) -> bool {if let Some(inner) = self { inner.modifiers.control } else {false}}    
    fn mod_alt(&self) -> bool {if let Some(inner) = self { inner.modifiers.alt } else {false}}    
    fn mod_shift(&self) -> bool {if let Some(inner) = self { inner.modifiers.shift } else {false}}    
    fn mod_logo(&self) -> bool {if let Some(inner) = self { inner.modifiers.logo } else {false}}    
    
    fn backtick(&self) -> bool {if let Some(inner) = self { if let KeyCode::Backtick = inner.key_code {true}else {false}}else{false}}
    fn key_0(&self) -> bool {if let Some(inner) = self { if let KeyCode::Key0 = inner.key_code {true}else {false}}else{false}}
    fn key_1(&self) -> bool {if let Some(inner) = self { if let KeyCode::Key1 = inner.key_code {true}else {false}}else{false}}
    fn key_2(&self) -> bool {if let Some(inner) = self { if let KeyCode::Key2 = inner.key_code {true}else {false}}else{false}}
    fn key_3(&self) -> bool {if let Some(inner) = self { if let KeyCode::Key3 = inner.key_code {true}else {false}}else{false}}
    fn key_4(&self) -> bool {if let Some(inner) = self { if let KeyCode::Key4 = inner.key_code {true}else {false}}else{false}}
    fn key_5(&self) -> bool {if let Some(inner) = self { if let KeyCode::Key5 = inner.key_code {true}else {false}}else{false}}
    fn key_6(&self) -> bool {if let Some(inner) = self { if let KeyCode::Key6 = inner.key_code {true}else {false}}else{false}}
    fn key_7(&self) -> bool {if let Some(inner) = self { if let KeyCode::Key7 = inner.key_code {true}else {false}}else{false}}
    fn key_8(&self) -> bool {if let Some(inner) = self { if let KeyCode::Key8 = inner.key_code {true}else {false}}else{false}}
    fn key_9(&self) -> bool {if let Some(inner) = self { if let KeyCode::Key9 = inner.key_code {true}else {false}}else{false}}
    fn minus(&self) -> bool {if let Some(inner) = self { if let KeyCode::Minus = inner.key_code {true}else {false}}else{false}}
    fn equals(&self) -> bool {if let Some(inner) = self { if let KeyCode::Equals = inner.key_code {true}else {false}}else{false}}
    
    fn backspace(&self) -> bool {if let Some(inner) = self { if let KeyCode::Backspace = inner.key_code {true}else {false}}else{false}}
    fn tab(&self) -> bool {if let Some(inner) = self { if let KeyCode::Tab = inner.key_code {true}else {false}}else{false}}
    
    fn key_q(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyQ = inner.key_code {true}else {false}}else{false}}
    fn key_w(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyW = inner.key_code {true}else {false}}else{false}}
    fn key_e(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyE = inner.key_code {true}else {false}}else{false}}
    fn key_r(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyR = inner.key_code {true}else {false}}else{false}}
    fn key_t(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyT = inner.key_code {true}else {false}}else{false}}
    fn key_y(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyY = inner.key_code {true}else {false}}else{false}}
    fn key_u(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyU = inner.key_code {true}else {false}}else{false}}
    fn key_i(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyI = inner.key_code {true}else {false}}else{false}}
    fn key_o(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyO = inner.key_code {true}else {false}}else{false}}
    fn key_p(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyP = inner.key_code {true}else {false}}else{false}}
    fn l_bracket(&self) -> bool {if let Some(inner) = self { if let KeyCode::LBracket = inner.key_code {true}else {false}}else{false}}
    fn r_bracket(&self) -> bool {if let Some(inner) = self { if let KeyCode::RBracket = inner.key_code {true}else {false}}else{false}}
    fn return_key(&self) -> bool {if let Some(inner) = self { if let KeyCode::ReturnKey = inner.key_code {true}else {false}}else{false}}
    
    fn key_a(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyA = inner.key_code {true}else {false}}else{false}}
    fn key_s(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyS = inner.key_code {true}else {false}}else{false}}
    fn key_d(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyD = inner.key_code {true}else {false}}else{false}}
    fn key_f(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyF = inner.key_code {true}else {false}}else{false}}
    fn key_g(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyG = inner.key_code {true}else {false}}else{false}}
    fn key_h(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyH = inner.key_code {true}else {false}}else{false}}
    fn key_j(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyJ = inner.key_code {true}else {false}}else{false}}
    fn key_k(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyK = inner.key_code {true}else {false}}else{false}}
    fn key_l(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyL = inner.key_code {true}else {false}}else{false}}
    fn semicolon(&self) -> bool {if let Some(inner) = self { if let KeyCode::Semicolon = inner.key_code {true}else {false}}else{false}}
    fn quote(&self) -> bool {if let Some(inner) = self { if let KeyCode::Quote = inner.key_code {true}else {false}}else{false}}
    fn backslash(&self) -> bool {if let Some(inner) = self { if let KeyCode::Backslash = inner.key_code {true}else {false}}else{false}}
    
    fn key_z(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyZ = inner.key_code {true}else {false}}else{false}}
    fn key_x(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyX = inner.key_code {true}else {false}}else{false}}
    fn key_c(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyC = inner.key_code {true}else {false}}else{false}}
    fn key_v(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyV = inner.key_code {true}else {false}}else{false}}
    fn key_b(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyB = inner.key_code {true}else {false}}else{false}}
    fn key_n(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyN = inner.key_code {true}else {false}}else{false}}
    fn key_m(&self) -> bool {if let Some(inner) = self { if let KeyCode::KeyM = inner.key_code {true}else {false}}else{false}}
    fn comma(&self) -> bool {if let Some(inner) = self { if let KeyCode::Comma = inner.key_code {true}else {false}}else{false}}
    fn period(&self) -> bool {if let Some(inner) = self { if let KeyCode::Period = inner.key_code {true}else {false}}else{false}}
    fn slash(&self) -> bool {if let Some(inner) = self { if let KeyCode::Slash = inner.key_code {true}else {false}}else{false}}
    
    fn control(&self) -> bool {if let Some(inner) = self { if let KeyCode::Control = inner.key_code {true}else {false}}else{false}}
    fn alt(&self) -> bool {if let Some(inner) = self { if let KeyCode::Alt = inner.key_code {true}else {false}}else{false}}
    fn shift(&self) -> bool {if let Some(inner) = self { if let KeyCode::Shift = inner.key_code {true}else {false}}else{false}}
    fn logo(&self) -> bool {if let Some(inner) = self { if let KeyCode::Logo = inner.key_code {true}else {false}}else{false}}

    fn space(&self) -> bool {if let Some(inner) = self { if let KeyCode::Space = inner.key_code {true}else {false}}else{false}}
    fn capslock(&self) -> bool {if let Some(inner) = self { if let KeyCode::Capslock = inner.key_code {true}else {false}}else{false}}
    fn f1(&self) -> bool {if let Some(inner) = self { if let KeyCode::F1 = inner.key_code {true}else {false}}else{false}}
    fn f2(&self) -> bool {if let Some(inner) = self { if let KeyCode::F2 = inner.key_code {true}else {false}}else{false}}
    fn f3(&self) -> bool {if let Some(inner) = self { if let KeyCode::F3 = inner.key_code {true}else {false}}else{false}}
    fn f4(&self) -> bool {if let Some(inner) = self { if let KeyCode::F4 = inner.key_code {true}else {false}}else{false}}
    fn f5(&self) -> bool {if let Some(inner) = self { if let KeyCode::F5 = inner.key_code {true}else {false}}else{false}}
    fn f6(&self) -> bool {if let Some(inner) = self { if let KeyCode::F6 = inner.key_code {true}else {false}}else{false}}
    fn f7(&self) -> bool {if let Some(inner) = self { if let KeyCode::F7 = inner.key_code {true}else {false}}else{false}}
    fn f8(&self) -> bool {if let Some(inner) = self { if let KeyCode::F8 = inner.key_code {true}else {false}}else{false}}
    fn f9(&self) -> bool {if let Some(inner) = self { if let KeyCode::F9 = inner.key_code {true}else {false}}else{false}}
    fn f10(&self) -> bool {if let Some(inner) = self { if let KeyCode::F10 = inner.key_code {true}else {false}}else{false}}
    fn f11(&self) -> bool {if let Some(inner) = self { if let KeyCode::F11 = inner.key_code {true}else {false}}else{false}}
    fn f12(&self) -> bool {if let Some(inner) = self { if let KeyCode::F12 = inner.key_code {true}else {false}}else{false}}
    
    fn print_screen(&self) -> bool {if let Some(inner) = self { if let KeyCode::PrintScreen = inner.key_code {true}else {false}}else{false}}
    fn scroll_lock(&self) -> bool {if let Some(inner) = self { if let KeyCode::ScrollLock = inner.key_code {true}else {false}}else{false}}
    fn pause(&self) -> bool {if let Some(inner) = self { if let KeyCode::Pause = inner.key_code {true}else {false}}else{false}}
    
    fn insert(&self) -> bool {if let Some(inner) = self { if let KeyCode::Insert = inner.key_code {true}else {false}}else{false}}
    fn delete(&self) -> bool {if let Some(inner) = self { if let KeyCode::Delete = inner.key_code {true}else {false}}else{false}}
    fn home(&self) -> bool {if let Some(inner) = self { if let KeyCode::Home = inner.key_code {true}else {false}}else{false}}
    fn end(&self) -> bool {if let Some(inner) = self { if let KeyCode::End = inner.key_code {true}else {false}}else{false}}
    fn page_up(&self) -> bool {if let Some(inner) = self { if let KeyCode::PageUp = inner.key_code {true}else {false}}else{false}}
    fn page_down(&self) -> bool {if let Some(inner) = self { if let KeyCode::PageDown = inner.key_code {true}else {false}}else{false}}
    
    fn numpad_0(&self) -> bool {if let Some(inner) = self { if let KeyCode::Numpad0 = inner.key_code {true}else {false}}else{false}}
    fn numpad_1(&self) -> bool {if let Some(inner) = self { if let KeyCode::Numpad1 = inner.key_code {true}else {false}}else{false}}
    fn numpad_2(&self) -> bool {if let Some(inner) = self { if let KeyCode::Numpad2 = inner.key_code {true}else {false}}else{false}}
    fn numpad_3(&self) -> bool {if let Some(inner) = self { if let KeyCode::Numpad3 = inner.key_code {true}else {false}}else{false}}
    fn numpad_4(&self) -> bool {if let Some(inner) = self { if let KeyCode::Numpad4 = inner.key_code {true}else {false}}else{false}}
    fn numpad_5(&self) -> bool {if let Some(inner) = self { if let KeyCode::Numpad5 = inner.key_code {true}else {false}}else{false}}
    fn numpad_6(&self) -> bool {if let Some(inner) = self { if let KeyCode::Numpad6 = inner.key_code {true}else {false}}else{false}}
    fn numpad_7(&self) -> bool {if let Some(inner) = self { if let KeyCode::Numpad7 = inner.key_code {true}else {false}}else{false}}
    fn numpad_8(&self) -> bool {if let Some(inner) = self { if let KeyCode::Numpad8 = inner.key_code {true}else {false}}else{false}}
    fn numpad_9(&self) -> bool {if let Some(inner) = self { if let KeyCode::Numpad9 = inner.key_code {true}else {false}}else{false}}
    
    fn numpad_equals(&self) -> bool {if let Some(inner) = self { if let KeyCode::NumpadEquals = inner.key_code {true}else {false}}else{false}}
    fn numpad_subtract(&self) -> bool {if let Some(inner) = self { if let KeyCode::NumpadSubtract = inner.key_code {true}else {false}}else{false}}
    fn numpad_add(&self) -> bool {if let Some(inner) = self { if let KeyCode::NumpadAdd = inner.key_code {true}else {false}}else{false}}
    fn numpad_decimal(&self) -> bool {if let Some(inner) = self { if let KeyCode::NumpadDecimal = inner.key_code {true}else {false}}else{false}}
    fn numpad_multiply(&self) -> bool {if let Some(inner) = self { if let KeyCode::NumpadMultiply = inner.key_code {true}else {false}}else{false}}
    fn numpad_divide(&self) -> bool {if let Some(inner) = self { if let KeyCode::NumpadDivide = inner.key_code {true}else {false}}else{false}}
    fn numpad_lock(&self) -> bool {if let Some(inner) = self { if let KeyCode::Numlock = inner.key_code {true}else {false}}else{false}}
    fn numpad_enter(&self) -> bool {if let Some(inner) = self { if let KeyCode::NumpadEnter = inner.key_code {true}else {false}}else{false}}
    
    fn arrow_up(&self) -> bool {if let Some(inner) = self { if let KeyCode::ArrowUp = inner.key_code {true}else {false}}else{false}}
    fn arrow_down(&self) -> bool {if let Some(inner) = self { if let KeyCode::ArrowDown = inner.key_code {true}else {false}}else{false}}
    fn arrow_left(&self) -> bool {if let Some(inner) = self { if let KeyCode::ArrowLeft = inner.key_code {true}else {false}}else{false}}
    fn arrow_right(&self) -> bool {if let Some(inner) = self { if let KeyCode::ArrowRight = inner.key_code {true}else {false}}else{false}}
    
    fn unknown(&self) -> bool {if let Some(inner) = self { if let KeyCode::Unknown = inner.key_code {true}else {false}}else{false}}
}

