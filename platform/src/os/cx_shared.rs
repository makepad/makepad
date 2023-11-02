
use {
    std::collections::{HashSet, HashMap},
    crate::{
        makepad_error_log::*,
        cx::Cx,
        pass::{
            PassId,
            CxPassParent
        },
        event::{
            DrawEvent,
            TriggerEvent,
            Event,
            KeyFocusEvent,
            NextFrameEvent,
        },
    }
};

impl Cx {
    #[allow(dead_code)]
    pub (crate) fn repaint_windows(&mut self) {
        for pass_id in self.passes.id_iter() {
            match self.passes[pass_id].parent {
                CxPassParent::Window(_) => {
                    self.passes[pass_id].paint_dirty = true;
                },
                _ => ()
            }
        }
    }
    
    pub (crate) fn any_passes_dirty(&self) -> bool {
        for pass_id in self.passes.id_iter() {
            if self.passes[pass_id].paint_dirty {
                return true
            }
        }
        false
    }
    
    pub (crate) fn compute_pass_repaint_order(&mut self, passes_todo: &mut Vec<PassId>) {
        passes_todo.clear();
        
        // we need this because we don't mark the entire deptree of passes dirty every small paint
        loop { // loop untill we don't propagate anymore
            let mut altered = false;
            for pass_id in self.passes.id_iter(){
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
        
        for pass_id in self.passes.id_iter(){
            if self.passes[pass_id].paint_dirty {
                let mut inserted = false;
                match self.passes[pass_id].parent {
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
    
    
    pub (crate) fn inner_call_event_handler(&mut self, event: &Event) {
        self.event_id += 1;
        let mut event_handler = self.event_handler.take().unwrap();
        event_handler(self, event);
        self.event_handler = Some(event_handler);
    }
    
    fn inner_key_focus_change(&mut self) {
        if let Some((prev, focus)) = self.keyboard.cycle_key_focus_changed(){
            self.inner_call_event_handler(&Event::KeyFocus(KeyFocusEvent {
                prev,
                focus
            }));
        }
    }
    
    pub fn handle_triggers(&mut self) {
        // post op events like signals, triggers and key-focus
        let mut counter = 0;
        while self.triggers.len() != 0 {
            counter += 1;
            let mut triggers = HashMap::new();
            std::mem::swap(&mut self.triggers, &mut triggers);
            self.inner_call_event_handler(&Event::Trigger(TriggerEvent {
                triggers: triggers,
            }));
            self.inner_key_focus_change();
            if counter > 100 {
                error!("Trigger feedback loop detected");
                break
            }
        }
    }
    
    pub (crate) fn call_event_handler(&mut self, event: &Event) {
        self.inner_call_event_handler(event);
        self.inner_key_focus_change();
        self.handle_triggers();
    }

    // helpers
    
    /*
    pub (crate) fn call_all_keys_up(&mut self) {
        let keys_down = self.keyboard.all_keys_up();
        for key_event in keys_down {
            self.call_event_handler(&Event::KeyUp(key_event))
        }
    }*/ 
    
    pub (crate) fn call_draw_event(&mut self) {
        let mut draw_event = DrawEvent::default();
        std::mem::swap(&mut draw_event, &mut self.new_draw_event);
        self.call_event_handler(&Event::Draw(draw_event));
    }

    pub (crate) fn call_next_frame_event(&mut self, time: f64) {
        let mut set = HashSet::default();
        std::mem::swap(&mut set, &mut self.new_next_frames);

        self.performance_stats.process_frame_data(time);

        self.call_event_handler(&Event::NextFrame(NextFrameEvent {set, time: time, frame: self.repaint_id}));
    }
}
