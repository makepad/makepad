use {
    std::collections::{HashSet, HashMap},
    crate::{
        cx_api::CxOsApi,
        cx::Cx,
        draw_pass::{
            DrawPassId,
            CxDrawPassParent
        },
        event::{
            TimerEvent,
            DrawEvent,
            TriggerEvent,
            Event,
            KeyFocusEvent,
            NextFrameEvent,
        },
        studio::{AppToStudio,EventSample},
    }
};

impl Cx {
    #[allow(dead_code)]
    pub (crate) fn repaint_windows(&mut self) {
        for draw_pass_id in self.passes.id_iter() {
            match self.passes[draw_pass_id].parent {
                CxDrawPassParent::Window(_) => {
                    self.passes[draw_pass_id].paint_dirty = true;
                },
                _ => ()
            }
        }
    }
    
    #[allow(unused)]
    pub (crate) fn any_passes_dirty(&self) -> bool {
        for draw_pass_id in self.passes.id_iter() {
            if self.passes[draw_pass_id].paint_dirty {
                return true
            }
        }
        false
    }
    
    pub (crate) fn compute_pass_repaint_order(&mut self, passes_todo: &mut Vec<DrawPassId>) {
        passes_todo.clear();
        
        // we need this because we don't mark the entire deptree of passes dirty every small paint
        loop { // loop untill we don't propagate anymore
            let mut altered = false;
            for draw_pass_id in self.passes.id_iter(){
                if self.demo_time_repaint {
                    if self.passes[draw_pass_id].main_draw_list_id.is_some(){
                        self.passes[draw_pass_id].paint_dirty = true;
                    }
                }
                if self.passes[draw_pass_id].paint_dirty {
                    let other = match self.passes[draw_pass_id].parent {
                        CxDrawPassParent::DrawPass(parent_pass_id) => {
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
        
        for draw_pass_id in self.passes.id_iter(){
            if self.passes[draw_pass_id].paint_dirty {
                let mut inserted = false;
                match self.passes[draw_pass_id].parent {
                    CxDrawPassParent::Window(_) | CxDrawPassParent::Xr=> {
                    },
                    CxDrawPassParent::DrawPass(dep_of_pass_id) => {
                        if draw_pass_id == dep_of_pass_id {
                            panic!()
                        }
                        for insert_before in 0..passes_todo.len() {
                            if passes_todo[insert_before] == dep_of_pass_id {
                                passes_todo.insert(insert_before, draw_pass_id);
                                inserted = true;
                                break;
                            }
                        }
                    },
                    CxDrawPassParent::None => { // we need to be first
                        passes_todo.insert(0, draw_pass_id);
                        inserted = true;
                    },
                }
                if !inserted {
                    passes_todo.push(draw_pass_id);
                }
            }
        }
        self.demo_time_repaint = false;
    }
    
    pub (crate) fn need_redrawing(&self) -> bool {
        self.new_draw_event.will_redraw() 
    }
    
    
    
    
    // event handler wrappers
    
    
    pub (crate) fn inner_call_event_handler(&mut self, event: &Event) {
        self.event_id += 1;
        if Cx::has_studio_web_socket(){
            let start = self.seconds_since_app_start();
            let mut event_handler = self.event_handler.take().unwrap();
            event_handler(self, event);
            self.event_handler = Some(event_handler);
            let end = self.seconds_since_app_start();
            Cx::send_studio_message(AppToStudio::EventSample(EventSample{
                event_u32: event.to_u32(),
                start: start,
                event_meta: if let Event::Timer(TimerEvent{timer_id,..}) = event{*timer_id}else{0},
                end: end
            }))
        }
        else{
            let mut event_handler = self.event_handler.take().unwrap();
            event_handler(self, event);
            self.event_handler = Some(event_handler);
        }

        // Reset widget query invalidation after all views have processed it.
        // We wait until event_id is at least 1 events past the invalidation event
        // to ensure the cache clear has propagated through the widget hierarchy
        // during the previous event cycle.
        if let Some(event_id) = self.widget_query_invalidation_event {
            if self.event_id > event_id + 1 {
                self.widget_query_invalidation_event = None;
            }
        }
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
                crate::error!("Trigger feedback loop detected");
                break
            }
        }
    }
    
    pub fn handle_actions(&mut self) {
        // post op events like signals, triggers and key-focus
        let mut counter = 0;
        while self.new_actions.len() != 0 {
            counter += 1;
            let mut actions = Vec::new();
            std::mem::swap(&mut self.new_actions, &mut actions);
            self.inner_call_event_handler(&Event::Actions(actions));
            self.inner_key_focus_change();
            if counter > 100 {
                crate::error!("Action feedback loop detected");
                crate::error!("New actions {:#?}", self.new_actions);
                break
            }
        }
    }
    
    pub (crate) fn call_event_handler(&mut self, event: &Event) {
        self.inner_call_event_handler(event);
        self.inner_key_focus_change();
        self.handle_triggers();
        self.handle_actions();
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
        self.in_draw_event = true;

        self.call_event_handler(&Event::Draw(draw_event));
        self.in_draw_event = false;
    }

    pub (crate) fn call_next_frame_event(&mut self, time: f64) {
        let mut set = HashSet::default();
        std::mem::swap(&mut set, &mut self.new_next_frames);

        self.performance_stats.process_frame_data(time);

        self.call_event_handler(&Event::NextFrame(NextFrameEvent {set, time: time, frame: self.repaint_id}));
    }
}
