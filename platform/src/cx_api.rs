use {
    std::{
        fmt::Write,
        time::Instant,
    },
    crate::{
        makepad_math::Vec2,
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
            DrawListArea
        },
        window::{
            CxWindowState
        },
        menu::{
            Menu,
        },
        pass::{
            CxPassParent
        },
    }
};

pub use crate::log;

pub fn profile_start()->Instant {
   Instant::now()
}

pub fn profile_end(instant: Instant) {
    log!("Profile time {} ms", (instant.elapsed().as_nanos() as f64) / 1000000f64);
}

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
                let pass_id = self.draw_lists[ia.draw_list_id].pass_id;
                return self.get_delegated_dpi_factor(pass_id)
            },
            Area::DrawList(va) => {
                let pass_id = self.draw_lists[va.draw_list_id].pass_id;
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
            match self.passes[pass_id_walk].parent {
                CxPassParent::Window(window_id) => {
                    dpi_factor = match self.windows[window_id].window_state {
                        CxWindowState::Create {..} => {
                            panic!();
                        },
                        CxWindowState::Created => {
                            self.windows[window_id].window_geom.dpi_factor
                        },
                        _ => 1.0
                    };
                    break;
                },
                CxPassParent::Pass(next_pass_id) => {
                    pass_id_walk = next_pass_id;
                },
                _ => {break;}
            }
        }
        dpi_factor
    }

    
    
    pub fn redraw_pass_of(&mut self, area: Area) {
        // we walk up the stack of area
        match area {
            Area::Empty => (),
            Area::Instance(instance) => {
                self.redraw_pass_and_parent_passes(self.draw_lists[instance.draw_list_id].pass_id);
            },
            Area::DrawList(listarea) => {
                self.redraw_pass_and_parent_passes(self.draw_lists[listarea.draw_list_id].pass_id);
            }
        }
    }
    
    pub fn redraw_pass_and_parent_passes(&mut self, pass_id: usize) {
        let mut walk_pass_id = pass_id;
        loop {
            if let Some(main_view_id) = self.passes[walk_pass_id].main_draw_list_id {
                self.redraw_area_and_children(Area::DrawList(DrawListArea {redraw_id: 0, draw_list_id: main_view_id}));
            }
            match self.passes[walk_pass_id].parent.clone() {
                CxPassParent::Pass(next_pass_id) => {
                    walk_pass_id = next_pass_id;
                },
                _ => { 
                    break;
                }
            }
        }
    }
    
    pub fn repaint_pass(&mut self, pass_id: usize) {
        let cxpass = &mut self.passes[pass_id];
        cxpass.paint_dirty = true;
    }
    
    pub fn redraw_pass_and_child_passes(&mut self, pass_id: usize) {
        let cxpass = &self.passes[pass_id];
        if let Some(main_list) = cxpass.main_draw_list_id {
            self.redraw_area_and_children(Area::DrawList(DrawListArea {redraw_id: 0, draw_list_id: main_list}));
        }
        // lets redraw all subpasses as well
        for sub_pass_id in 0..self.passes.len() {
            if let CxPassParent::Pass(dep_pass_id) = self.passes[sub_pass_id].parent.clone() {
                if dep_pass_id == pass_id {
                    self.redraw_pass_and_child_passes(sub_pass_id);
                }
            }
        }
    }
    
    pub fn redraw_all(&mut self) {
        self.new_draw_event.redraw_all = true;
    }
    
    pub fn redraw_area(&mut self, area: Area) {
        if let Some(draw_list_id) = area.draw_list_id() {
            if self.new_draw_event.draw_lists.iter().position( | v | *v == draw_list_id).is_some() {
                return;
            }
            self.new_draw_event.draw_lists.push(draw_list_id);
        }
    }
    
    pub fn redraw_area_and_children(&mut self, area: Area) {
        if let Some(draw_list_id) = area.draw_list_id() {
            if self.new_draw_event.draw_lists_and_children.iter().position( | v | *v == draw_list_id).is_some() {
                return;
            }
            self.new_draw_event.draw_lists_and_children.push(draw_list_id);
        }
    }


    pub fn set_view_scroll_x(&mut self, draw_list_id: usize, scroll_pos: f32) {
        let fac = self.get_delegated_dpi_factor(self.draw_lists[draw_list_id].pass_id);
        let cxview = &mut self.draw_lists[draw_list_id];
        cxview.unsnapped_scroll.x = scroll_pos;
        let snapped = scroll_pos - scroll_pos % (1.0 / fac);
        if cxview.snapped_scroll.x != snapped {
            cxview.snapped_scroll.x = snapped;
            self.passes[cxview.pass_id].paint_dirty = true;
        }
    }
    
    
    pub fn set_view_scroll_y(&mut self, draw_list_id: usize, scroll_pos: f32) {
        let fac = self.get_delegated_dpi_factor(self.draw_lists[draw_list_id].pass_id);
        let cxview = &mut self.draw_lists[draw_list_id];
        cxview.unsnapped_scroll.y = scroll_pos;
        let snapped = scroll_pos - scroll_pos % (1.0 / fac);
        if cxview.snapped_scroll.y != snapped {
            cxview.snapped_scroll.y = snapped;
            self.passes[cxview.pass_id].paint_dirty = true;
        }
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

   
    pub fn debug_draw_tree(&self, dump_instances: bool, draw_list_id: usize) {
        fn debug_draw_tree_recur(cx:&Cx, dump_instances: bool, s: &mut String, draw_list_id: usize, depth: usize) {
            if draw_list_id >= cx.draw_lists.len() {
                writeln!(s, "---------- Drawlist still empty ---------").unwrap();
                return
            }
            let mut indent = String::new();
            for _i in 0..depth {
                indent.push_str("|   ");
            }
            let draw_items_len = cx.draw_lists[draw_list_id].draw_items_len;
            if draw_list_id == 0 {
                writeln!(s, "---------- Begin Debug draw tree for redraw_id: {} ---------", cx.redraw_id).unwrap();
            }
            let rect = cx.draw_lists[draw_list_id].rect;
            let scroll = cx.draw_lists[draw_list_id].get_local_scroll();
            writeln!(
                s,
                "{}{} {}: len:{} rect:({}, {}, {}, {}) scroll:({}, {})",
                indent,
                cx.draw_lists[draw_list_id].debug_id,
                draw_list_id,
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
                if let Some(sub_view_id) = cx.draw_lists[draw_list_id].draw_items[draw_item_id].sub_view_id {
                    debug_draw_tree_recur(cx, dump_instances, s, sub_view_id, depth + 1);
                }
                else {
                    let cxview = &cx.draw_lists[draw_list_id];
                    let draw_call = cxview.draw_items[draw_item_id].draw_call.as_ref().unwrap();
                    let sh = &cx.draw_shaders.shaders[draw_call.draw_shader.draw_shader_id];
                    let slots = sh.mapping.instances.total_slots;
                    let instances = draw_call.instances.as_ref().unwrap().len() / slots;
                    writeln!(
                        s,
                        "{}{}({}) sid:{} inst:{} scroll:{}",
                        indent,
                        draw_call.options.debug_id.unwrap_or(sh.class_prop),
                        sh.type_name,
                        draw_call.draw_shader.draw_shader_id,
                        instances,
                        draw_call.draw_uniforms.get_local_scroll()
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
            if draw_list_id == 0 {
                writeln!(s, "---------- End Debug draw tree for redraw_id: {} ---------", cx.redraw_id).unwrap();
            }
        }
        
        let mut s = String::new();
        debug_draw_tree_recur(self, dump_instances, &mut s, draw_list_id, 0);
        println!("{}", s);
    }
}

#[macro_export]
macro_rules!main_app {
    ( $ app: ident) => {
        #[cfg(not(target_arch = "wasm32"))]
        fn main() {
            let mut cx = Cx::default();
            live_register(&mut cx);
            cx.live_expand();
            let mut app = None;
            cx.event_loop( | cx, mut event | {
                if let Event::Construct = event {
                    app = Some( $ app::new_app(cx));
                }
                app.as_mut().unwrap().handle_event(cx, &mut event);
                cx.after_handle_event(&mut event);
            });
        }
        
        #[export_name = "create_wasm_app"]
        #[cfg(target_arch = "wasm32")]
        pub extern "C" fn create_wasm_app() -> u32 {
            let mut cx = Box::new(Cx::default());
            cx.live_register();
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
                (*appcx.0).handle_event(cx, &mut event);
                cx.after_handle_event(&mut event);
            })
        }
    }
}

