use {
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

pub struct RawMouse {
    receiver: mpsc::Receiver<[u8; 3]>,
    old: [u8; 3],
    width: f64,
    height: f64,
    abs: DVec2,
    scale: f64
}

const LEFT_BTN: u8 = 1 << 0;
const RIGHT_BTN: u8 = 1 << 1;
const MID_BTN: u8 = 1 << 2;
//const X_SIGN: u8 = 1 << 4;
//const Y_SIGN: u8 = 1 << 5;
const X_OVERFLOW: u8 = 1 << 6;
const Y_OVERFLOW: u8 = 1 << 7;

impl RawMouse {
    pub fn new(width: f64, height: f64, scale: f64) -> Self {
        let (send, receiver) = mpsc::channel();
        
        std::thread::spawn(move || {
            let mut mice = File::open("/dev/input/mice").expect("cannot open /dev/input/mice, make sure we are in the 'input' group");
            loop {
                let mut buf = [0u8; 3];
                mice.read(&mut buf).expect("error reading");
                send.send(buf).unwrap();
            }
        });
        
        Self {
            old: Default::default(),
            width,
            height,
            abs: dvec2(width / 2.0,height / 2.0),
            scale,
            receiver
        }
    }
    
    pub fn poll_mouse(&mut self, time: f64, modifiers: KeyModifiers, window_id: WindowId) -> Vec<DirectEvent> {
        let mut evts = Vec::new();
        while let Ok(new) = self.receiver.try_recv() {
            let old = self.old;
            // process movements
            if new[1] != 0 && new[0] & X_OVERFLOW == 0{ 
                self.abs.x += new[1] as i8 as f64 * self.scale;
                if self.abs.x < 0.0{ self.abs.x = 0.0}
                if self.abs.x > self.width{ self.abs.x = self.width}
            }
            if new[2] != 0 && new[0] & Y_OVERFLOW == 0{
                self.abs.y -= new[2] as i8 as f64 * self.scale;
                if self.abs.y < 0.0{ self.abs.y = 0.0}
                if self.abs.y > self.height{ self.abs.y = self.height}
            }
            let abs = self.abs;
            if new[1] != 0 || new[2] != 0 {
                evts.push(DirectEvent::MouseMove(MouseMoveEvent {
                    abs,
                    window_id,
                    modifiers,
                    time,
                    handled: Cell::new(Area::Empty),
                }))
            } 
            
            if new[0] & LEFT_BTN != 0 && old[0] & LEFT_BTN == 0 {
                evts.push(DirectEvent::MouseDown(MouseDownEvent {button: 0, abs, window_id, modifiers, time, handled: Cell::new(Area::Empty),}));
            }
            if new[0] & LEFT_BTN == 0 && old[0] & LEFT_BTN != 0 {
                evts.push(DirectEvent::MouseUp(MouseUpEvent {button: 0, abs, window_id, modifiers, time,}));
            }
            if new[0] & RIGHT_BTN != 0 && old[0] & RIGHT_BTN == 0 {
                evts.push(DirectEvent::MouseDown(MouseDownEvent {button: 1, abs, window_id, modifiers, time, handled: Cell::new(Area::Empty),}));
            } 
            if new[0] & RIGHT_BTN == 0 && old[0] & RIGHT_BTN != 0 {
                evts.push(DirectEvent::MouseUp(MouseUpEvent {button: 1, abs, window_id, modifiers, time,}));
            }
            if new[0] & MID_BTN != 0 && old[0] & MID_BTN == 0 {
                evts.push(DirectEvent::MouseDown(MouseDownEvent {button: 2, abs, window_id, modifiers, time, handled: Cell::new(Area::Empty),}));
            }
            if new[0] & MID_BTN == 0 && old[0] & MID_BTN != 0 {
                evts.push(DirectEvent::MouseUp(MouseUpEvent {button: 2, abs, window_id, modifiers, time,}));
            }
            self.old = new;
        }
        evts
    }
}
