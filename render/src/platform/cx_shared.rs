use {
    std::collections::{HashMap, HashSet},
    crate::{
        makepad_math::Vec2,
        cx::Cx,
        pass::{
            CxPassParent
        },
        event::{
            DrawEvent,
            SignalEvent,
            TriggerEvent,
            Event,
            KeyEvent,
            KeyFocusEvent,
            NextFrameEvent,
        },
    }
};

impl Cx {
    
    pub (crate) fn process_tap_count(&mut self, digit: usize, pos: Vec2, time: f64) -> u32 {
        if digit >= self.fingers.len() {
            return 0
        };
        let (last_pos, last_time, count) = self.fingers[digit].tap_count;
        
        if (time - last_time) < 0.5 && pos.distance(&last_pos) < 10. {
            self.fingers[digit].tap_count = (pos, time, count + 1);
            count + 1
        }
        else {
            self.fingers[digit].tap_count = (pos, time, 1);
            1
        }
    }
    
    pub (crate) fn repaint_windows(&mut self) {
        
        for cxpass in self.passes.iter_mut() {
            match cxpass.parent {
                CxPassParent::Window(_) => {
                    cxpass.paint_dirty = true;
                },
                _=>()
            }
        }
    }
    
    pub (crate) fn compute_passes_to_repaint(&mut self, passes_todo: &mut Vec<usize>, windows_need_repaint: &mut usize) {
        passes_todo.clear();
        
        // we need this because we don't mark the entire deptree of passes dirty every small paint
        loop { // loop untill we don't propagate anymore
            let mut altered = false;
            for pass_id in 0..self.passes.len() {
                if self.passes[pass_id].paint_dirty {
                    let other = match self.passes[pass_id].parent {
                        CxPassParent::Pass(parent_pass_id) => {
                            Some(parent_pass_id)
                        }
                        _ => None
                    };
                    if let Some(other) = other {
                        if !self.passes[other].paint_dirty {
                            self.passes[other].paint_dirty = true;
                            altered = true;
                        }
                    }
                }
            }
            if !altered {
                break
            }
        }
        
        for (pass_id, cxpass) in self.passes.iter().enumerate() {
            if cxpass.paint_dirty {
                let mut inserted = false;
                match cxpass.parent {
                    CxPassParent::Window(_) => {
                        *windows_need_repaint += 1;
                    }, 
                    CxPassParent::Pass(dep_of_pass_id) => {
                        if pass_id == dep_of_pass_id {
                            panic!()
                        }
                        for insert_before in 0..passes_todo.len() {
                            if passes_todo[insert_before] == dep_of_pass_id {
                                passes_todo.insert(insert_before, pass_id);
                                inserted = true;
                                break;
                            }
                        }
                    },
                    CxPassParent::None => { // we need to be first
                        passes_todo.insert(0, pass_id);
                        inserted = true;
                    },
                }
                if !inserted {
                    passes_todo.push(pass_id);
                }
            }
        }
        
    }
    
    pub (crate) fn need_redrawing(&self) -> bool {
        self.new_draw_event.will_redraw()
    }
    
    pub (crate) fn process_key_down(&mut self, key_event: KeyEvent) {
        if let Some(_) = self.keys_down.iter().position( | k | k.key_code == key_event.key_code) {
            return;
        }
        self.keys_down.push(key_event);
    }
    
    pub (crate) fn process_key_up(&mut self, key_event: &KeyEvent) {
        for i in 0..self.keys_down.len() {
            if self.keys_down[i].key_code == key_event.key_code {
                self.keys_down.remove(i);
                return
            }
        }
    }
    
    
    
    // event handler wrappers
    
    
    pub (crate) fn call_event_handler(&mut self, event: &mut Event)
    {
        self.event_id += 1;
        let event_handler = self.event_handler.unwrap();
        unsafe {(*event_handler)(self, event);}
        
        if self.next_key_focus != self.key_focus {
            self.prev_key_focus = self.key_focus;
            self.key_focus = self.next_key_focus;
            self.event_id += 1;
            unsafe {(*event_handler)(self, &mut Event::KeyFocus(KeyFocusEvent {
                prev: self.prev_key_focus,
                focus: self.key_focus
            }));}
        }
    }
    
    pub (crate) fn call_live_edit(&mut self) {
        if self.live_edit_event.is_some() {
            let ev = self.live_edit_event.take().unwrap();
            self.call_event_handler(&mut Event::LiveEdit(ev));
        }
    }
    
    pub (crate) fn call_signals_and_triggers(&mut self)
    {
        
        let mut counter = 0;
        while self.signals.len() != 0 {
            counter += 1;
            let mut signals = HashMap::new();
            std::mem::swap(&mut self.signals, &mut signals);
            
            self.call_event_handler(&mut Event::Signal(SignalEvent {
                signals: signals,
            }));
            
            if counter > 100 {
                println!("Signal feedback loop detected");
                break
            }
        }
        
        let mut counter = 0;
        while self.triggers.len() != 0 {
            counter += 1;
            let mut triggers = HashMap::new();
            std::mem::swap(&mut self.triggers, &mut triggers);
            
            self.call_event_handler(&mut Event::Trigger(TriggerEvent {
                triggers: triggers,
            }));
            
            if counter > 100 {
                println!("Trigger feedback loop detected");
                break
            }
        }
        
    }
    
    pub (crate) fn call_all_keys_up(&mut self)
    {
        let mut keys_down = Vec::new();
        std::mem::swap(&mut keys_down, &mut self.keys_down);
        for key_event in keys_down {
            self.call_event_handler(&mut Event::KeyUp(key_event))
        }
    }
    
    pub (crate) fn call_draw_event(&mut self)
    {
        let mut draw_event = DrawEvent::default();
        std::mem::swap(&mut draw_event, &mut self.new_draw_event);

        self.call_event_handler(&mut Event::Draw(draw_event));
    }
    
    pub (crate) fn call_next_frame_event(&mut self, time: f64)
    {
        let mut set = HashSet::default();
        std::mem::swap(&mut set, &mut self.new_next_frames);
        self.call_event_handler(&mut Event::NextFrame(NextFrameEvent {set, time: time, frame: self.repaint_id}));
    }
}
