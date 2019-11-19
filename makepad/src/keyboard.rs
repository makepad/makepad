use render::*; 
use widget::*; 
use crate::appstorage::*;
use editor::*;


#[derive(Clone)]
pub struct Keyboard {
    pub view: ScrollView,
    pub modifiers: KeyModifiers,
    pub key_down: Option<KeyCode>,
    pub key_up: Option<KeyCode>,
    pub buttons: Elements<KeyType, NormalButton, NormalButton>,
}

#[derive(Clone)]
pub enum KeyboardEvent {
    None,
}

#[derive(Clone, PartialEq, PartialOrd, Hash, Ord)]
pub enum KeyType {
    Control,
    Alt,
    Shift
}
impl Eq for KeyType {}

impl KeyType {
    fn name(&self) -> String {
        match self {
            KeyType::Control => "Control".to_string(),
            KeyType::Alt => "Alternate".to_string(),
            KeyType::Shift => "Shift".to_string(),
        }
    }
}

impl Keyboard {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            view: ScrollView::proto(cx),
            buttons: Elements::new(NormalButton {
                ..NormalButton::proto(cx)
            }),
            modifiers: KeyModifiers::default(),
            key_down: None,
            key_up: None,
            
        }
    }
    
    fn send_textbuffers_update(&mut self, cx: &mut Cx, app_storage: &mut AppStorage) {
        // clear all files we missed
        for (_, atb) in &mut app_storage.text_buffers {
            atb.text_buffer.keyboard.modifiers = self.modifiers.clone();
            atb.text_buffer.keyboard.key_down = self.key_down.clone();
            atb.text_buffer.keyboard.key_up = self.key_up.clone();
            cx.send_signal(atb.text_buffer.signal, SIGNAL_TEXTBUFFER_KEYBOARD_UPDATE);
        }
    }
    
    pub fn handle_keyboard(&mut self, cx: &mut Cx, event: &mut Event, app_storage: &mut AppStorage) -> KeyboardEvent {
        // do shit here
        if self.view.handle_scroll_bars(cx, event) {
        }
        let mut update_textbuffers = false;
        for (key_type, btn) in self.buttons.enumerate() {
            match btn.handle_button(cx, event) {
                ButtonEvent::Down => {
                    match key_type {
                        KeyType::Control => {
                            self.modifiers.control = true;
                            self.key_up = None;
                            self.key_down = Some(KeyCode::Control);
                        },
                        KeyType::Alt => {
                            self.modifiers.alt = true;
                            self.key_up = None;
                            self.key_down = Some(KeyCode::Alt);
                        },
                        KeyType::Shift => {
                            self.modifiers.shift = true;
                            self.key_up = None;
                            self.key_down = Some(KeyCode::Shift);
                        }
                    }
                    update_textbuffers = true;
                },
                ButtonEvent::Up | ButtonEvent::Clicked => {
                    match key_type {
                        KeyType::Control => {
                            self.modifiers.control = false;
                            self.key_down = None;
                            self.key_up = Some(KeyCode::Control);
                        },
                        KeyType::Alt => {
                            self.modifiers.alt = false;
                            self.key_down = None;
                            self.key_up = Some(KeyCode::Alt);
                        },
                        KeyType::Shift => {
                            self.modifiers.shift = false;
                            self.key_down = None;
                            self.key_up = Some(KeyCode::Shift);
                        }
                    }
                    update_textbuffers = true;
                },
                _ => ()
            }
        }
        if update_textbuffers {
            self.send_textbuffers_update(cx, app_storage);
        }
        
        KeyboardEvent::None
    }
    
    pub fn draw_keyboard(&mut self, cx: &mut Cx) {
        if self.view.begin_view(cx, Layout::default()).is_err() {return}
        
        let keys = vec![KeyType::Alt, KeyType::Control, KeyType::Shift];
        
        for key in keys {
            self.buttons.get_draw(cx, key.clone(), | _cx, templ | {
                templ.clone()
            }).draw_button(cx, &key.name());
        }
        
        self.view.end_view(cx);
    }
}