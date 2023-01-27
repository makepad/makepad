use {
    self::super::super::{
        libc_sys,
    },
    self::super::{
        direct_event::*,
    },
    crate::{
        event::*,
        makepad_math::*,
        area::Area,
        window::{WindowId},
    },
    std::{
        fs::File,
        io::Read,
        sync::mpsc,
        cell::Cell
    }
};

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
struct InputEvent {
    time: libc_sys::timeval,
    ty: u16,
    code: u16,
    value: i32,
}

pub struct RawKeyboard {
    pub key_modifiers: KeyModifiers,
    receiver: mpsc::Receiver<InputEvent>,
    old: InputEvent,
}


impl RawKeyboard {
    pub fn new() -> Self {
        let (send, receiver) = mpsc::channel();
        
        std::thread::spawn(move || {
            
            let mut kb = File::open("/dev/input/event0").expect("cannot open /dev/input/event0, make sure we are in the 'input' group");
            loop { 
                let mut buf = [0u8;std::mem::size_of::<InputEvent>()];
                if let Ok(len) = kb.read(&mut buf){
                    if len == std::mem::size_of::<InputEvent>(){
                        let buf = unsafe{std::mem::transmute(buf)};
                        send.send(buf).unwrap();
                    }
                }
            }
        });
        
        Self { 
            receiver,
            key_modifiers: Default::default(),
            old: Default::default(),
        }
    }
    
    pub fn poll_keyboard(&mut self, time: f64) -> Vec<DirectEvent> {
        let mut evts = Vec::new(); 
        while let Ok(new) = self.receiver.try_recv() {
            let old = self.old;
            if new.ty == 1{ // key press
                println!("{} {} {}", new.ty, new.code, new.value);
            }
            self.old = new;
        }
        evts
    }
}
