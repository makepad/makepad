use {
    std::{
        fmt::Write,
        time::Instant,
    },
    makepad_math::{
        Vec2
    },
    crate::{
        cx::Cx,
        event::{
            DraggedItem,
            Timer,
            Signal,
            NextFrame,
        },
        cursor::{
            MouseCursor
        },
        area::{
            Area,
            ViewArea
        },
        window::{
            CxWindowState
        },
        menu::{
            Menu,
        },
        pass::{
            CxPassDepOf
        },
    }
};

pub use crate::log;

pub trait CxPlatformApi{
    fn show_text_ime(&mut self, x: f32, y: f32);
    fn hide_text_ime(&mut self);
    fn set_window_outer_size(&mut self, size: Vec2);
    fn set_window_position(&mut self, pos: Vec2);
    fn start_timer(&mut self, interval: f64, repeats: bool) -> Timer;
    fn stop_timer(&mut self, timer: Timer); 
    fn post_signal(signal: Signal, status: u64); 
    fn update_menu(&mut self, menu: &Menu);
    fn start_dragging(&mut self, dragged_item: DraggedItem);
}

impl Cx {
    
    pub fn get_dpi_factor_of(&mut self, area: &Area) -> f32 {
        match area {
            Area::Instance(ia) => {
                let pass_id = self.views[ia.view_id].pass_id;
                return self.get_delegated_dpi_factor(pass_id)
            },
            Area::View(va) => {
                let pass_id = self.views[va.view_id].pass_id;
                return self.get_delegated_dpi_factor(pass_id)
            },
            _ => ()
        }
        return 1.0;
    }
    
    pub fn get_delegated_dpi_factor(&mut self, pass_id: usize) -> f32 {
        let mut dpi_factor = 1.0;
        let mut pass_id_walk = pass_id;
        for _ in 0..25 {
            match self.passes[pass_id_walk].dep_of {
                CxPassDepOf::Window(window_id) => {
                    dpi_factor = match self.windows[window_id].window_state {
                        CxWindowState::Create {..} => {
                            self.default_dpi_factor
                        },
                        CxWindowState::Created => {
                            self.windows[window_id].window_geom.dpi_factor
                        },
                        _ => 1.0
                    };
                    break;
                },
                CxPassDepOf::Pass(next_pass_id) => {
                    pass_id_walk = next_pass_id;
                },
                _ => {break;}
            }
        }
        dpi_factor
    }

    
    pub fn get_scroll_pos(&self) -> Vec2 {
        let cxview = &self.views[*self.view_stack.last().unwrap()];
        cxview.unsnapped_scroll
    }
    
    pub fn redraw_pass_of(&mut self, area: Area) {
        // we walk up the stack of area
        match area {
            Area::Empty => (),
            Area::Instance(instance) => {
                self.redraw_pass_and_parent_passes(self.views[instance.view_id].pass_id);
            },
            Area::View(viewarea) => {
                self.redraw_pass_and_parent_passes(self.views[viewarea.view_id].pass_id);
            }
        }
    }
    
    pub fn redraw_pass_and_parent_passes(&mut self, pass_id: usize) {
        let mut walk_pass_id = pass_id;
        loop {
            if let Some(main_view_id) = self.passes[walk_pass_id].main_view_id {
                self.redraw_view_and_children_of(Area::View(ViewArea {redraw_id: 0, view_id: main_view_id}));
            }
            match self.passes[walk_pass_id].dep_of.clone() {
                CxPassDepOf::Pass(next_pass_id) => {
                    walk_pass_id = next_pass_id;
                },
                _ => {
                    break;
                }
            }
        }
    }
    
    pub fn redraw_pass_and_child_passes(&mut self, pass_id: usize) {
        let cxpass = &self.passes[pass_id];
        if let Some(main_view_id) = cxpass.main_view_id {
            self.redraw_view_and_children_of(Area::View(ViewArea {redraw_id: 0, view_id: main_view_id}));
        }
        // lets redraw all subpasses as well
        for sub_pass_id in 0..self.passes.len() {
            if let CxPassDepOf::Pass(dep_pass_id) = self.passes[sub_pass_id].dep_of.clone() {
                if dep_pass_id == pass_id {
                    self.redraw_pass_and_child_passes(sub_pass_id);
                }
            }
        }
    }
    
    pub fn redraw_all(&mut self) {
        self.new_redraw_all_views = true;
    }
    
    pub fn redraw_view_of(&mut self, area: Area) {
        if let Some(view_id) = area.view_id() {
            if self.new_redraw_views.iter().position( | v | *v == view_id).is_some() {
                return;
            }
            self.new_redraw_views.push(view_id);
        }
    }
    
    pub fn redraw_view_and_children_of(&mut self, area: Area) {
        if let Some(view_id) = area.view_id() {
            if self.new_redraw_views_and_children.iter().position( | v | *v == view_id).is_some() {
                return;
            }
            self.new_redraw_views_and_children.push(view_id);
        }
    }
    
    pub fn is_xr_presenting(&mut self) -> bool {
        if !self.in_redraw_cycle {
            panic!("Cannot call is_xr_presenting outside of redraw flow");
        }
        if self.window_stack.len() == 0 {
            panic!("Can only call is_xr_presenting inside of a window");
        }
        self.windows[*self.window_stack.last().unwrap()].window_geom.xr_is_presenting
    }
    
    pub fn view_will_redraw(&self, view_id: usize) -> bool {
        
        if self.redraw_all_views {
            return true;
        }
        // figure out if areas are in some way a child of view_id, then we need to redraw
        for check_view_id in &self.redraw_views {
            let mut next_vw = Some(*check_view_id);
            while let Some(vw) = next_vw{
                if vw == view_id {
                    return true
                }
                next_vw = self.views[vw].codeflow_parent_id;
            }
        }
        // figure out if areas are in some way a parent of view_id, then redraw
        for check_view_id in &self.redraw_views_and_children {
            let mut next_vw = Some(view_id);
            while let Some(vw) = next_vw{
                if vw == *check_view_id {
                    return true
                }
                next_vw = self.views[vw].codeflow_parent_id;
            }
        }
        false
    }
    
    
    pub fn update_area_refs(&mut self, old_area: Area, new_area: Area) -> Area {
        if old_area == Area::Empty {
            return new_area
        }
        
        for finger in &mut self.fingers {
            if finger.captured == old_area {
                finger.captured = new_area.clone();
            }
            if finger._over_last == old_area {
                finger._over_last = new_area.clone();
            }
        }
        
        if self.drag_area == old_area {
            self.drag_area = new_area.clone();
        }
        
        // update capture keyboard
        if self.key_focus == old_area {
            self.key_focus = new_area.clone()
        }
        
        // update capture keyboard
        if self.prev_key_focus == old_area {
            self.prev_key_focus = new_area.clone()
        }
        if self.next_key_focus == old_area {
            self.next_key_focus = new_area.clone()
        }
        
        new_area
    }
    
    pub fn set_key_focus(&mut self, focus_area: Area) {
        self.next_key_focus = focus_area;
    }
    
    pub fn revert_key_focus(&mut self) {
        self.next_key_focus = self.prev_key_focus;
    }
    
    pub fn has_key_focus(&self, focus_area: Area) -> bool {
        self.key_focus == focus_area
    }
    
    pub fn new_next_frame(&mut self) -> NextFrame {
        let res = NextFrame(self.next_frame_id);
        self.next_frame_id += 1;
        self.new_next_frames.insert(res);
        res
    }
    
    pub fn new_signal(&mut self) -> Signal {
        self.signal_id += 1;
        return Signal {signal_id: self.signal_id};
    }
    
    pub fn send_signal(&mut self, signal: Signal, status: Option<u64>) {
        if signal.signal_id == 0 {
            return
        }
        if let Some(set) = self.signals.get_mut(&signal) {
            if let Some(status) = status{
                set.push(status);
            }
        }
        else {
            let mut set = Vec::new();
            if let Some(status) = status{
                set.push(status);
            }
            self.signals.insert(signal, set);
        }
    }

    pub fn send_trigger(&mut self, area:Area, trigger_id:Option<u64>){
         if let Some(triggers) = self.triggers.get_mut(&area) {
            if let Some(trigger_id) = trigger_id{
                triggers.push(trigger_id);
            }
        }
        else {
            let mut new_set = Vec::new();
            if let Some(trigger_id) = trigger_id{
                new_set.push(trigger_id);
            }
            self.triggers.insert(area, new_set);
        }
    }
    
    pub fn set_down_mouse_cursor(&mut self, mouse_cursor: MouseCursor) {
        // ok so lets set the down mouse cursor
        self.down_mouse_cursor = Some(mouse_cursor);
    }
    pub fn set_hover_mouse_cursor(&mut self, mouse_cursor: MouseCursor) {
        // the down mouse cursor gets removed when there are no captured fingers
        self.hover_mouse_cursor = Some(mouse_cursor);
    }

    pub fn profile_start(&mut self, id: u64) {
        self.profiles.insert(id, Instant::now());
    }
    
    pub fn profile_end(&self, id: u64) {
        if let Some(inst) = self.profiles.get(&id) {
            log!("Profile {} time {}", id, (inst.elapsed().as_nanos() as f64) / 1000000f64);
        }
    }
    
    pub fn debug_draw_tree(&self, dump_instances: bool, view_id: usize) {
        fn debug_draw_tree_recur(cx:&Cx, dump_instances: bool, s: &mut String, view_id: usize, depth: usize) {
            if view_id >= cx.views.len() {
                writeln!(s, "---------- Drawlist still empty ---------").unwrap();
                return
            }
            let mut indent = String::new();
            for _i in 0..depth {
                indent.push_str("|   ");
            }
            let draw_items_len = cx.views[view_id].draw_items_len;
            if view_id == 0 {
                writeln!(s, "---------- Begin Debug draw tree for redraw_id: {} ---------", cx.redraw_id).unwrap();
            }
            let rect = cx.views[view_id].rect;
            let scroll = cx.views[view_id].get_local_scroll();
            writeln!(
                s,
                "{}{} {}: len:{} rect:({}, {}, {}, {}) scroll:({}, {})",
                indent,
                cx.views[view_id].debug_id,
                view_id,
                draw_items_len,
                rect.pos.x,
                rect.pos.y,
                rect.size.x,
                rect.size.y,
                scroll.x,
                scroll.y
            ).unwrap();
            indent.push_str("  ");
            let mut indent = String::new();
            for _i in 0..depth + 1 {
                indent.push_str("|   ");
            }
            for draw_item_id in 0..draw_items_len {
                if let Some(sub_view_id) = cx.views[view_id].draw_items[draw_item_id].sub_view_id {
                    debug_draw_tree_recur(cx, dump_instances, s, sub_view_id, depth + 1);
                }
                else {
                    let cxview = &cx.views[view_id];
                    let draw_call = cxview.draw_items[draw_item_id].draw_call.as_ref().unwrap();
                    let sh = &cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                    let slots = sh.mapping.instances.total_slots;
                    let instances = draw_call.instances.as_ref().unwrap().len() / slots;
                    writeln!(
                        s,
                        "{}{}({}) sid:{} inst:{} scroll:{}",
                        indent,
                        sh.field,
                        sh.type_name,
                        draw_call.draw_shader.draw_shader_id,
                        instances,
                        draw_call.get_local_scroll()
                    ).unwrap();
                    // lets dump the instance geometry
                    if dump_instances {
                        for inst in 0..instances.min(1) {
                            let mut out = String::new();
                            let mut off = 0;
                            for input in &sh.mapping.instances.inputs {
                                let buf = draw_call.instances.as_ref().unwrap();
                                match input.slots {
                                    1 => out.push_str(&format!("{}:{} ", input.id, buf[inst * slots + off])),
                                    2 => out.push_str(&format!("{}:v2({},{}) ", input.id, buf[inst * slots + off], buf[inst * slots + 1 + off])),
                                    3 => out.push_str(&format!("{}:v3({},{},{}) ", input.id, buf[inst * slots + off], buf[inst * slots + 1 + off], buf[inst * slots + 1 + off])),
                                    4 => out.push_str(&format!("{}:v4({},{},{},{}) ", input.id, buf[inst * slots + off], buf[inst * slots + 1 + off], buf[inst * slots + 2 + off], buf[inst * slots + 3 + off])),
                                    _ => {}
                                }
                                off += input.slots;
                            }
                            writeln!(s, "  {}instance {}: {}", indent, inst, out).unwrap();
                        }
                    }
                }
            }
            if view_id == 0 {
                writeln!(s, "---------- End Debug draw tree for redraw_id: {} ---------", cx.redraw_id).unwrap();
            }
        }
        
        let mut s = String::new();
        debug_draw_tree_recur(self, dump_instances, &mut s, view_id, 0);
        println!("{}", s);
    }
}

#[macro_export]
macro_rules!main_app {
    ( $ app: ident) => {
        #[cfg(not(target_arch = "wasm32"))]
        fn main() {
            //TODO do this with a macro to generate both entrypoints for App and Cx
            let mut cx = Cx::default();
            cx.live_register();
            //$ app ::live_register(&mut cx);
            live_register(&mut cx);
            cx.live_expand();
            let mut app = None;
            cx.event_loop( | cx, mut event | {
                if let Event::Construct = event {
                    app = Some( $ app::new_app(cx));
                }
                else if let Event::Draw = event {
                    app.as_mut().unwrap().draw(cx);
                    cx.after_draw();
                    return
                }
                app.as_mut().unwrap().handle_event(cx, &mut event);
            });
        }
        
        #[export_name = "create_wasm_app"]
        #[cfg(target_arch = "wasm32")]
        pub extern "C" fn create_wasm_app() -> u32 {
            let mut cx = Box::new(Cx::default());
            cx.live_register();
            //$ app ::live_register(&mut cx);
            live_register(&mut cx);
            cx.live_expand();
            Box::into_raw(Box::new((0, Box::into_raw(cx)/*, Box::into_raw(cxafterdraw)*/))) as u32
        }
        
        #[export_name = "process_to_wasm"]
        #[cfg(target_arch = "wasm32")]
        pub unsafe extern "C" fn process_to_wasm(appcx: u32, msg_bytes: u32) -> u32 {
            let appcx = &*(appcx as *mut (*mut $ app, *mut Cx/*, *mut CxAfterDraw*/));
            (*appcx.1).process_to_wasm(msg_bytes, | cx, mut event | {
                if let Event::Construct = event {
                    (*appcx.0) = Box::new( $ app::new_app(&mut cx));
                }
                else if let Event::Draw = event {
                    (*appcx.0).draw(cx);
                    cx.after_draw();
                    return;
                };
                (*appcx.0).handle_event(cx, &mut event);
            })
        }
    }
}

