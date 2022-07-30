use {
    std::{
        fmt::Write,
        collections::HashSet,
    },
    crate::{
        makepad_error_log::*,
        makepad_math::Vec2,
        gpu_info::GpuInfo,
        cx::{Cx, PlatformType},
        event::{
            DraggedItem,
            Timer,
            Trigger,
            Signal,
            WebSocketAutoReconnect,
            WebSocket,
            NextFrame,
        },
        draw_list::{
            DrawListId
        },
        window::{
            WindowId
        },
        audio::{
            AudioTime,
            AudioOutputBuffer
        },
        cursor::{
            MouseCursor
        },
        area::{
            Area,
            DrawListArea
        },
        menu::{
            Menu,
        },
        pass::{
            PassId,
            CxPassParent
        },
    }
};


pub trait CxPlatformApi {
    fn post_signal(signal: Signal);
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static;
    
    fn web_socket_open(&mut self, url: String, rec: WebSocketAutoReconnect) -> WebSocket;
    fn web_socket_send(&mut self, socket: WebSocket, data: Vec<u8>);
    
    fn start_midi_input(&mut self);
    fn spawn_audio_output<F>(&mut self, f: F) where F: FnMut(AudioTime, &mut dyn AudioOutputBuffer) + Send + 'static;
}

#[derive(PartialEq)]
pub enum CxPlatformOp {
    CreateWindow(WindowId),
    CloseWindow(WindowId),
    MinimizeWindow(WindowId),
    MaximizeWindow(WindowId),
    FullscreenWindow(WindowId),
    NormalizeWindow(WindowId),
    RestoreWindow(WindowId),
    SetTopmost(WindowId, bool),
    XrStartPresenting(WindowId),
    XrStopPresenting(WindowId),
    
    ShowTextIME(Vec2),
    HideTextIME,
    SetCursor(MouseCursor),
    StartTimer {timer_id: u64, interval: f64, repeats: bool},
    StopTimer(u64),
    StartDragging(DraggedItem),
    UpdateMenu(Menu)
}

impl Cx {
    pub fn redraw_id(&self) -> u64 {self.redraw_id}
    
    pub fn platform_type(&self) -> &PlatformType {&self.platform_type}
    
    pub fn gpu_info(&self) -> &GpuInfo {&self.gpu_info}
    
    pub fn update_menu(&mut self, menu: Menu) {
        self.platform_ops.push(CxPlatformOp::UpdateMenu(menu));
    }
    
    pub fn push_unique_platform_op(&mut self, op: CxPlatformOp) {
        if self.platform_ops.iter().find( | o | **o == op).is_none() {
            self.platform_ops.push(op);
        }
    }
    
    pub fn show_text_ime(&mut self, pos: Vec2) {
        self.platform_ops.push(CxPlatformOp::ShowTextIME(pos));
    }
    
    pub fn hide_text_ime(&mut self) {
        self.platform_ops.push(CxPlatformOp::HideTextIME);
    }
    
    pub fn start_dragging(&mut self, dragged_item: DraggedItem) {
        self.platform_ops.iter().for_each( | p | {
            if let CxPlatformOp::StartDragging(_) = p {
                panic!("start drag twice");
            }
        });
        self.platform_ops.push(CxPlatformOp::StartDragging(dragged_item));
    }
    
    pub fn set_cursor(&mut self, cursor: MouseCursor) {
        // down cursor overrides the hover cursor
        if let Some(p) = self.platform_ops.iter_mut().find( | p | match p {
            CxPlatformOp::SetCursor(_) => true,
            _ => false
        }) {
            *p = CxPlatformOp::SetCursor(cursor)
        }
        else {
            self.platform_ops.push(CxPlatformOp::SetCursor(cursor))
        }
    }
    
    pub fn start_timer(&mut self, interval: f64, repeats: bool) -> Timer {
        self.timer_id += 1;
        self.platform_ops.push(CxPlatformOp::StartTimer {
            timer_id: self.timer_id,
            interval,
            repeats
        });
        Timer(self.timer_id)
    }
    
    pub fn stop_timer(&mut self, timer: Timer) {
        if timer.0 != 0 {
            self.platform_ops.push(CxPlatformOp::StopTimer(timer.0));
        }
    }
    
    pub fn get_dpi_factor_of(&mut self, area: &Area) -> f32 {
        match area {
            Area::Instance(ia) => {
                let pass_id = self.draw_lists[ia.draw_list_id].pass_id.unwrap();
                return self.get_delegated_dpi_factor(pass_id)
            },
            Area::DrawList(va) => {
                let pass_id = self.draw_lists[va.draw_list_id].pass_id.unwrap();
                return self.get_delegated_dpi_factor(pass_id)
            },
            _ => ()
        }
        return 1.0;
    }
    
    pub fn get_delegated_dpi_factor(&mut self, pass_id: PassId) -> f32 {
        let mut pass_id_walk = pass_id;
        for _ in 0..25 {
            match self.passes[pass_id_walk].parent {
                CxPassParent::Window(window_id) => {
                    if !self.windows[window_id].is_created {
                        panic!();
                    }
                    return self.windows[window_id].window_geom.dpi_factor;
                },
                CxPassParent::Pass(next_pass_id) => {
                    pass_id_walk = next_pass_id;
                },
                _ => {break;}
            }
        }
        1.0
    }
    
    pub fn redraw_pass_of(&mut self, area: Area) {
        // we walk up the stack of area
        match area {
            Area::Empty => (),
            Area::Instance(instance) => {
                self.redraw_pass_and_parent_passes(self.draw_lists[instance.draw_list_id].pass_id.unwrap());
            },
            Area::DrawList(listarea) => {
                self.redraw_pass_and_parent_passes(self.draw_lists[listarea.draw_list_id].pass_id.unwrap());
            }
        }
    }
    
    pub fn redraw_pass_and_parent_passes(&mut self, pass_id: PassId) {
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
    
    pub fn repaint_pass(&mut self, pass_id: PassId) {
        let cxpass = &mut self.passes[pass_id];
        cxpass.paint_dirty = true;
    }
    
    pub fn redraw_pass_and_child_passes(&mut self, pass_id: PassId) {
        let cxpass = &self.passes[pass_id];
        if let Some(main_list) = cxpass.main_draw_list_id {
            self.redraw_area_and_children(Area::DrawList(DrawListArea {redraw_id: 0, draw_list_id: main_list}));
        }
        // lets redraw all subpasses as well
        for sub_pass_id in self.passes.id_iter() {
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
    
    
    pub fn set_scroll_x(&mut self, draw_list_id: DrawListId, scroll_pos: f32) {
        if let Some(pass_id) = self.draw_lists[draw_list_id].pass_id {
            let fac = self.get_delegated_dpi_factor(pass_id);
            let cxview = &mut self.draw_lists[draw_list_id];
            cxview.unsnapped_scroll.x = scroll_pos;
            let snapped = scroll_pos - scroll_pos % (1.0 / fac);
            if cxview.snapped_scroll.x != snapped {
                cxview.snapped_scroll.x = snapped;
                self.passes[cxview.pass_id.unwrap()].paint_dirty = true;
            }
        }
    }
    
    
    pub fn set_scroll_y(&mut self, draw_list_id: DrawListId, scroll_pos: f32) {
        if let Some(pass_id) = self.draw_lists[draw_list_id].pass_id {
            let fac = self.get_delegated_dpi_factor(pass_id);
            let cxview = &mut self.draw_lists[draw_list_id];
            cxview.unsnapped_scroll.y = scroll_pos;
            let snapped = scroll_pos - scroll_pos % (1.0 / fac);
            if cxview.snapped_scroll.y != snapped {
                cxview.snapped_scroll.y = snapped;
                self.passes[cxview.pass_id.unwrap()].paint_dirty = true;
            }
        }
    }
    
    pub fn update_area_refs(&mut self, old_area: Area, new_area: Area) -> Area {
        if old_area == Area::Empty {
            return new_area
        }
        
        self.fingers.update_area(old_area, new_area);
        self.finger_drag.update_area(old_area, new_area);
        self.keyboard.update_area(old_area, new_area);
        
        new_area
    }
    
    pub fn set_key_focus(&mut self, focus_area: Area) {
        self.keyboard.set_key_focus(focus_area);
    }
    
    pub fn revert_key_focus(&mut self) {
        self.keyboard.revert_key_focus();
    }
    
    pub fn has_key_focus(&self, focus_area: Area) -> bool {
        self.keyboard.has_key_focus(focus_area)
    }
    
    pub fn new_next_frame(&mut self) -> NextFrame {
        let res = NextFrame(self.next_frame_id);
        self.next_frame_id += 1;
        self.new_next_frames.insert(res);
        res
    }
    
    pub fn send_signal(&mut self, signal: Signal) {
        self.signals.insert(signal);
    }
    
    pub fn send_trigger(&mut self, area: Area, trigger: Trigger) {
        if let Some(triggers) = self.triggers.get_mut(&area) {
            triggers.insert(trigger);
        }
        else {
            let mut new_set = HashSet::new();
            new_set.insert(trigger);
            self.triggers.insert(area, new_set);
        }
    }
    
    pub fn debug_draw_tree(&self, dump_instances: bool, draw_list_id: DrawListId) {
        fn debug_draw_tree_recur(cx: &Cx, dump_instances: bool, s: &mut String, draw_list_id: DrawListId, depth: usize) {
            //if draw_list_id >= cx.draw_lists.len() {
            //    writeln!(s, "---------- Drawlist still empty ---------").unwrap();
            //    return
            //}
            let mut indent = String::new();
            for _i in 0..depth {
                indent.push_str("|   ");
            }
            let draw_items_len = cx.draw_lists[draw_list_id].draw_items_len;
            //if draw_list_id == 0 {
            //    writeln!(s, "---------- Begin Debug draw tree for redraw_id: {} ---------", cx.redraw_id).unwrap();
            // }
            let rect = cx.draw_lists[draw_list_id].rect;
            let scroll = cx.draw_lists[draw_list_id].get_local_scroll();
            writeln!(
                s,
                "{}{} {:?}: len:{} rect:({}, {}, {}, {}) scroll:({}, {})",
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
            //if draw_list_id == 0 {
            //    writeln!(s, "---------- End Debug draw tree for redraw_id: {} ---------", cx.redraw_id).unwrap();
            //}
        }
        
        let mut s = String::new();
        debug_draw_tree_recur(self, dump_instances, &mut s, draw_list_id, 0);
        log!("{}", s);
    }
}


#[macro_export]
macro_rules!main_app {
    ( $ app: ident) => {
        #[cfg(not(target_arch = "wasm32"))]
        fn main() {
            let app = std::rc::Rc::new(std::cell::RefCell::new(None));
            let mut cx = Cx::new(Box::new(move | cx, event | {
                
                if let Event::Construct = event {
                    *app.borrow_mut() = Some($app::new_main(cx));
                    log!("GOT HERE!");
                }
                
                app.borrow_mut().as_mut().unwrap().handle_event(cx, event);
                cx.after_handle_event(event);
            }));
            live_register(&mut cx);
            cx.live_expand();
            cx.live_scan_dependencies();
            cx.desktop_load_dependencies();
            cx.event_loop();
        }
        
        #[cfg(target_arch = "wasm32")]
        fn main() {}
        
        #[export_name = "wasm_create_app"]
        #[cfg(target_arch = "wasm32")]
        pub extern "C" fn create_wasm_app() -> u32 {
            
            let app = std::rc::Rc::new(std::cell::RefCell::new(None));
            let mut cx = Box::new(Cx::new(Box::new(move | cx, event | {
                if let Event::Construct = event {
                    *app.borrow_mut() = Some($app::new_main(cx));
                }
                app.borrow_mut().as_mut().unwrap().handle_event(cx, event);
                cx.after_handle_event(event);
            })));
            
            live_register(&mut cx);
            cx.live_expand();
            cx.live_scan_dependencies();
            Box::into_raw(cx) as u32
        }
        
        #[export_name = "wasm_terminate_thread_pools"]
        #[cfg(target_arch = "wasm32")]
        pub unsafe extern "C" fn wasm_terminate_thread_pools(cx_ptr: u32) {
            let cx = cx_ptr as *mut Cx;
            (*cx).terminate_thread_pools();
        }
        
        #[export_name = "wasm_process_msg"]
        #[cfg(target_arch = "wasm32")]
        pub unsafe extern "C" fn wasm_process_msg(msg_ptr: u32, cx_ptr: u32) -> u32 {
            let cx = cx_ptr as *mut Cx;
            (*cx).process_to_wasm(msg_ptr)
        }
    }
}

#[macro_export]
macro_rules!register_component_factory {
    ( $ cx: ident, $ registry: ident, $ ty: ty, $ factory: ident) => {
        let module_id = LiveModuleId::from_str(&module_path!()).unwrap();
        if let Some((reg, _)) = $ cx.live_registry.borrow().components.get_or_create::< $ registry>().map.get(&LiveType::of::< $ ty>()) {
            if reg.module_id != module_id {
                panic!("Component already registered {} {}", stringify!( $ ty), reg.module_id);
            }
        }
        $ cx.live_registry.borrow().components.get_or_create::< $ registry>().map.insert(
            LiveType::of::< $ ty>(),
            (LiveComponentInfo {
                name: LiveId::from_str(stringify!( $ ty)).unwrap(),
                module_id
            }, Box::new( $ factory()))
        );
    }
}
