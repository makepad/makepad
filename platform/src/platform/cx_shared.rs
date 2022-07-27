#![allow(dead_code)]
use {
    std::collections::{HashSet, HashMap},
    crate::{
        makepad_error_log::*,
        cx::Cx,
        pass::{
            CxPassParent
        },
        event::{
            DrawEvent,
            SignalEvent,
            TriggerEvent,
            Event,
            KeyFocusEvent,
            NextFrameEvent,
        },
    }
};

impl Cx {
    
    
    pub (crate) fn repaint_windows(&mut self) {
        for cxpass in self.passes.iter_mut() {
            match cxpass.parent {
                CxPassParent::Window(_) => {
                    cxpass.paint_dirty = true;
                },
                _ => ()
            }
        }
    }
    
    pub (crate) fn any_passes_dirty(&self) -> bool {
        for pass_id in 0..self.passes.len() {
            if self.passes[pass_id].paint_dirty {
                return true
            }
        }
        false
    }
    
    pub (crate) fn compute_pass_repaint_order(&mut self, passes_todo: &mut Vec<usize>) {
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
    
    
    
    
    // event handler wrappers
    
    
    pub (crate) fn inner_call_event_handler(&mut self, event: &mut Event) {
        self.event_id += 1;
        let event_handler = self.event_handler.unwrap();
        unsafe {(*event_handler)(self, event);}
    }
    
    fn inner_key_focus_change(&mut self) {
        if let Some((prev, focus)) = self.keyboard.cycle_key_focus_changed(){
            self.inner_call_event_handler(&mut Event::KeyFocus(KeyFocusEvent {
                prev,
                focus
            }));
        }
    }
    
    fn inner_triggers_and_signals(&mut self) {
        // post op events like signals, triggers and key-focus
        let mut counter = 0;
        while self.signals.len() != 0 {
            counter += 1;
            let mut signals = HashSet::new();
            std::mem::swap(&mut self.signals, &mut signals);
            self.inner_call_event_handler(&mut Event::Signal(SignalEvent {
                signals: signals,
            }));
            self.inner_key_focus_change();
            if counter > 100 {
                error!("Signal feedback loop detected");
                break
            }
        }
        
        let mut counter = 0;
        while self.triggers.len() != 0 {
            counter += 1;
            let mut triggers = HashMap::new();
            std::mem::swap(&mut self.triggers, &mut triggers);
            self.inner_call_event_handler(&mut Event::Trigger(TriggerEvent {
                triggers: triggers,
            }));
            self.inner_key_focus_change();
            if counter > 100 {
                error!("Trigger feedback loop detected");
                break
            }
        }
    }
    
    pub (crate) fn call_event_handler(&mut self, event: &mut Event) {
        self.inner_call_event_handler(event);
        self.inner_key_focus_change();
        self.inner_triggers_and_signals();
    }
    
    
    // helpers
    
    
    pub (crate) fn call_all_keys_up(&mut self) {
        let keys_down = self.keyboard.all_keys_up();
        for key_event in keys_down {
            self.call_event_handler(&mut Event::KeyUp(key_event))
        }
    }
    
    pub (crate) fn call_draw_event(&mut self) {
        let mut draw_event = DrawEvent::default();
        std::mem::swap(&mut draw_event, &mut self.new_draw_event);
        self.call_event_handler(&mut Event::Draw(draw_event));
    }
    
    pub (crate) fn call_next_frame_event(&mut self, time: f64) {
        let mut set = HashSet::default();
        std::mem::swap(&mut set, &mut self.new_next_frames);
        self.call_event_handler(&mut Event::NextFrame(NextFrameEvent {set, time: time, frame: self.repaint_id}));
    }
    
    pub fn terminate_thread_pools(&mut self) {
        for pool in &self.thread_pool_senders {
            pool.borrow_mut().take();
        }
    }
}
