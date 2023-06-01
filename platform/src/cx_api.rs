use {
    makepad_futures::executor::Spawner,
    std::{
        any::{TypeId, Any},
    },
    crate::{
        makepad_math::{DVec2, Rect},
        gpu_info::GpuInfo,
        cx::{Cx, CxRef, OsType, XrCapabilities},
        event::{
            DraggedItem, 
            Timer,
            Trigger,
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
        cursor::{
            MouseCursor
        },
        area::{
            Area, 
            //DrawListArea
        },
        menu::{
            Menu,
        },
        pass::{
            PassId,
            CxPassRect,
            CxPassParent
        },
        network::HttpRequest,
    }
};


pub trait CxOsApi {
    fn init_cx_os(&mut self);
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static;
    
    fn web_socket_open(&mut self, url: String, rec: WebSocketAutoReconnect) -> WebSocket;
    fn web_socket_send(&mut self, socket: WebSocket, data: Vec<u8>);
}

#[derive(PartialEq)]
pub enum CxOsOp {
    CreateWindow(WindowId),
    CloseWindow(WindowId),
    MinimizeWindow(WindowId),
    MaximizeWindow(WindowId),
    FullscreenWindow(WindowId),
    NormalizeWindow(WindowId),
    RestoreWindow(WindowId),
    SetTopmost(WindowId, bool),
    
    XrStartPresenting,
    XrStopPresenting,
    
    ShowTextIME(Area, DVec2),
    HideTextIME,
    SetCursor(MouseCursor),
    StartTimer {timer_id: u64, interval: f64, repeats: bool},
    StopTimer(u64),
    StartDragging(DraggedItem),
    UpdateMenu(Menu),
    ShowClipboardActions(String),
    HttpRequest(HttpRequest),
}

impl Cx { 
    pub fn xr_capabilities(&self) -> &XrCapabilities {
        &self.xr_capabilities 
    } 
    
    pub fn get_ref(&self)->CxRef{
        CxRef(self.self_ref.clone().unwrap())
    }
    
    pub fn get_dependency(&self, path: &str) -> Result<&Vec<u8>,
    String> { 
        if let Some(data) = self.dependencies.get(path) {
            if let Some(data) = &data.data {
                return match data {
                    Ok(data) => Ok(data),
                    Err(s) => Err(s.clone())
                }
            } 
        }
        Err(format!("Dependency not loaded {}", path))
    }
    
    pub fn redraw_id(&self) -> u64 {self.redraw_id}
    
    pub fn os_type(&self) -> &OsType {&self.os_type}
    pub fn cpu_cores(&self) -> usize {self.cpu_cores}
    pub fn gpu_info(&self) -> &GpuInfo {&self.gpu_info}
    
    pub fn update_menu(&mut self, menu: Menu) {
        self.platform_ops.push(CxOsOp::UpdateMenu(menu));
    }
    
    pub fn push_unique_platform_op(&mut self, op: CxOsOp) {
        if self.platform_ops.iter().find( | o | **o == op).is_none() {
            self.platform_ops.push(op);
        }
    }
    
    pub fn show_text_ime(&mut self, area: Area, pos: DVec2) {
        if !self.keyboard.text_ime_dismissed {
            self.platform_ops.push(CxOsOp::ShowTextIME(area, pos));
        }
    }
    
    pub fn hide_text_ime(&mut self) {
        self.keyboard.reset_text_ime_dismissed();
        self.platform_ops.push(CxOsOp::HideTextIME);
    }

    pub fn text_ime_was_dismissed(&mut self) {
        self.keyboard.set_text_ime_dismissed();
        self.platform_ops.push(CxOsOp::HideTextIME);
    }

    pub fn show_clipboard_actions(&mut self, selected: String) {
        self.platform_ops.push(CxOsOp::ShowClipboardActions(selected));
    }

    pub fn start_dragging(&mut self, dragged_item: DraggedItem) {
        self.platform_ops.iter().for_each( | p | {
            if let CxOsOp::StartDragging(_) = p {
                panic!("start drag twice");
            }
        });
        self.platform_ops.push(CxOsOp::StartDragging(dragged_item));
    }
    
    pub fn set_cursor(&mut self, cursor: MouseCursor) {
        // down cursor overrides the hover cursor
        if let Some(p) = self.platform_ops.iter_mut().find( | p | match p {
            CxOsOp::SetCursor(_) => true,
            _ => false
        }) {
            *p = CxOsOp::SetCursor(cursor)
        }
        else {
            self.platform_ops.push(CxOsOp::SetCursor(cursor))
        }
    }
    
    pub fn sweep_lock(&mut self, value:Area){
        self.fingers.sweep_lock(value);
    }
    
    pub fn sweep_unlock(&mut self, value:Area){
        self.fingers.sweep_unlock(value);
    }
    
    pub fn start_timeout(&mut self, interval: f64) -> Timer {
        self.timer_id += 1;
        self.platform_ops.push(CxOsOp::StartTimer {
            timer_id: self.timer_id,
            interval,
            repeats: false
        });
        Timer(self.timer_id)
    }
    
    pub fn start_interval(&mut self, interval: f64) -> Timer {
        self.timer_id += 1;
        self.platform_ops.push(CxOsOp::StartTimer {
            timer_id: self.timer_id,
            interval,
            repeats: true
        });
        Timer(self.timer_id)
    }
    
    pub fn stop_timer(&mut self, timer: Timer) {
        if timer.0 != 0 {
            self.platform_ops.push(CxOsOp::StopTimer(timer.0));
        }
    }
    
    pub fn xr_start_presenting(&mut self) {
        self.platform_ops.push(CxOsOp::XrStartPresenting);
    }
    
    pub fn xr_stop_presenting(&mut self) {
        self.platform_ops.push(CxOsOp::XrStopPresenting);
    }
    
    pub fn get_dpi_factor_of(&mut self, area: &Area) -> f64 {
        if let Some(draw_list_id) = area.draw_list_id() {
            let pass_id = self.draw_lists[draw_list_id].pass_id.unwrap();
            return self.get_delegated_dpi_factor(pass_id)
        }
        return 1.0;
    }
    
    pub fn get_delegated_dpi_factor(&mut self, pass_id: PassId) -> f64 {
        let mut pass_id_walk = pass_id;
        for _ in 0..25 {
            match self.passes[pass_id_walk].parent {
                CxPassParent::Window(window_id) => {
                    if !self.windows[window_id].is_created {
                        return 1.0
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
    
    pub fn redraw_pass_and_parent_passes(&mut self, pass_id: PassId) {
        let mut walk_pass_id = pass_id;
        loop {
            if let Some(main_list_id) = self.passes[walk_pass_id].main_draw_list_id {
                self.redraw_list_and_children(main_list_id);
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
    
    pub fn get_pass_rect(&self, pass_id: PassId, dpi:f64) -> Option<Rect> {
        match self.passes[pass_id].pass_rect {
            Some(CxPassRect::Area(area)) => {
                let rect = area.get_rect(self);
                Some(Rect{
                    pos: (rect.pos * dpi).floor() / dpi,
                    size: (rect.size * dpi).ceil() / dpi
                })
            }
            Some(CxPassRect::ScaledArea(area, scale)) => {
                let rect = area.get_rect(self);
                Some(Rect{
                    pos: (rect.pos * dpi).floor() / dpi,
                    size:  scale * (rect.size * dpi).ceil() / dpi
                })
            }
            Some(CxPassRect::Size(size)) => Some(Rect {pos: DVec2::default(), size: (size / dpi).floor() * dpi}),
            None => None
        } 
    }
    
    pub fn get_pass_name(&self, pass_id: PassId) -> &str {
        &self.passes[pass_id].debug_name
    }
    
    pub fn repaint_pass(&mut self, pass_id: PassId) {
        let cxpass = &mut self.passes[pass_id];
        cxpass.paint_dirty = true;
    }
    
    pub fn redraw_pass_and_child_passes(&mut self, pass_id: PassId) {
        let cxpass = &self.passes[pass_id];
        if let Some(main_list_id) = cxpass.main_draw_list_id {
            self.redraw_list_and_children(main_list_id);
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
            self.redraw_list(draw_list_id);
        }
    }
    
    pub fn redraw_area_and_children(&mut self, area: Area) {
        if let Some(draw_list_id) = area.draw_list_id() {
            self.redraw_list_and_children(draw_list_id);
        }
    }
    
    pub fn redraw_list(&mut self, draw_list_id: DrawListId) {
        if self.new_draw_event.draw_lists.iter().position( | v | *v == draw_list_id).is_some() {
            return;
        }
        self.new_draw_event.draw_lists.push(draw_list_id);
    }
    
    pub fn redraw_list_and_children(&mut self, draw_list_id: DrawListId) {
        if self.new_draw_event.draw_lists_and_children.iter().position( | v | *v == draw_list_id).is_some() {
            return;
        }
        self.new_draw_event.draw_lists_and_children.push(draw_list_id);
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
    
    pub fn send_trigger(&mut self, area: Area, trigger: Trigger) {
        if let Some(triggers) = self.triggers.get_mut(&area) {
            triggers.push(trigger);
        }
        else {
            let mut new_set = Vec::new();
            new_set.push(trigger);
            self.triggers.insert(area, new_set);
        }
    }
    
    pub fn set_global<T: 'static + Any + Sized>(&mut self, value: T) {
        if !self.globals.iter().any( | v | v.0 == TypeId::of::<T>()) {
            self.globals.push((TypeId::of::<T>(), Box::new(value)));
        }
    }
    
    pub fn get_global<T: 'static + Any>(&mut self) -> &mut T {
        let item = self.globals.iter_mut().find( | v | v.0 == TypeId::of::<T>()).unwrap();
        item.1.downcast_mut().unwrap()
    }
    
    pub fn has_global<T: 'static + Any>(&mut self) -> bool {
        self.globals.iter_mut().find( | v | v.0 == TypeId::of::<T>()).is_some()
    }
    
    pub fn global<T: 'static + Any + Default>(&mut self) -> &mut T {
        if !self.has_global::<T>() {
            self.set_global(T::default());
        }
        self.get_global::<T>()
    }

    pub fn spawner(&self) -> &Spawner {
        &self.spawner
    }

    pub fn http_request(&mut self, request: HttpRequest) {
        self.platform_ops.push(CxOsOp::HttpRequest(request));
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
