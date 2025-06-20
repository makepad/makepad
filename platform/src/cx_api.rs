use crate::file_dialogs::FileDialog;

use {
    crate::{
        area::Area,
        cursor::MouseCursor,
        cx::{Cx, CxRef, OsType, XrCapabilities},
        draw_list::DrawListId,
        event::{DragItem, HttpRequest, NextFrame, Timer, Trigger, VideoSource},
        gpu_info::GpuInfo,
        macos_menu::MacosMenu,
        makepad_futures::executor::Spawner,
        makepad_live_id::*,
        event::xr::XrAnchor,
        makepad_math::{DVec2, Rect},
        pass::{CxPassParent, CxPassRect, PassId},
        texture::Texture,
        window::WindowId,
        dvec2,
    },
    std::{
        any::{Any, TypeId},
        rc::Rc,
    },
};
pub enum OpenUrlInPlace{
    Yes,
    No
}
pub trait CxOsApi {
    fn init_cx_os(&mut self);

    fn spawn_thread<F>(&mut self, f: F)
    where
        F: FnOnce() + Send + 'static;
        
    fn start_stdin_service(&mut self) {}
    fn pre_start() -> bool {
        false
    }
    
    fn open_url(&mut self, url:&str, in_place:OpenUrlInPlace);
    
    fn seconds_since_app_start(&self)->f64;
    
    fn default_window_size(&self)->DVec2{dvec2(800.,600.)}
    
    fn max_texture_width()->usize{4096}
    
    fn in_xr_mode(&self)->bool{false}
    
    fn micro_zbias_step(&self)->f32{0.00001}
    
    /*
    fn web_socket_open(&mut self, url: String, rec: WebSocketAutoReconnect) -> WebSocket;
    fn web_socket_send(&mut self, socket: WebSocket, data: Vec<u8>);*/
}

#[derive(PartialEq)]
pub enum CxOsOp {
    CreateWindow(WindowId),
    ResizeWindow(WindowId, DVec2),
    RepositionWindow(WindowId, DVec2),
    CloseWindow(WindowId),
    MinimizeWindow(WindowId),
    Deminiaturize(WindowId),
    MaximizeWindow(WindowId),
    FullscreenWindow(WindowId),
    NormalizeWindow(WindowId),
    RestoreWindow(WindowId),
    HideWindow(WindowId),
    SetTopmost(WindowId, bool),
    ShowInDock(bool),

    ShowTextIME(Area, DVec2),
    HideTextIME,
    SetCursor(MouseCursor),
    StartTimer {
        timer_id: u64,
        interval: f64,
        repeats: bool,
    },
    StopTimer(u64),
    Quit,

    StartDragging(Vec<DragItem>),
    UpdateMacosMenu(MacosMenu),
    ShowClipboardActions(String),
    CopyToClipboard(String),

    HttpRequest {
        request_id: LiveId,
        request: HttpRequest,
    },
    CancelHttpRequest{
        request_id: LiveId,
    },

    PrepareVideoPlayback(LiveId, VideoSource, u32, bool, bool),
    BeginVideoPlayback(LiveId),
    PauseVideoPlayback(LiveId),
    ResumeVideoPlayback(LiveId),
    MuteVideoPlayback(LiveId),
    UnmuteVideoPlayback(LiveId),
    CleanupVideoPlaybackResources(LiveId),
    UpdateVideoSurfaceTexture(LiveId),
    
    CreateWebView{
        id: LiveId,
        area: Area,
        texture: Texture,
        url: String
    },
    UpdateWebView{
        id: LiveId,
        area: Area
    },
    CloseWebView{
        id:LiveId
    },
    SaveFileDialog(FileDialog),
    SelectFileDialog(FileDialog),
    SaveFolderDialog(FileDialog),
    SelectFolderDialog(FileDialog),    
    
    XrStartPresenting,
    XrSetLocalAnchor(XrAnchor),
    XrAdvertiseAnchor(XrAnchor),
    XrDiscoverAnchor(u8),
    XrStopPresenting,
    
}

impl std::fmt::Debug for CxOsOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self{
            Self::CreateWindow(..)=>write!(f, "CreateWindow"),
            Self::CloseWindow(..)=>write!(f, "CloseWindow"),
            Self::MinimizeWindow(..)=>write!(f, "MinimizeWindow"),
            Self::Deminiaturize(..)=>write!(f, "Deminiaturize"),
            Self::MaximizeWindow(..)=>write!(f, "MaximizeWindow"),
            Self::FullscreenWindow(..)=>write!(f, "FullscreenWindow"),
            Self::NormalizeWindow(..)=>write!(f, "NormalizeWindow"),
            Self::RestoreWindow(..)=>write!(f, "RestoreWindow"),
            Self::HideWindow(..)=>write!(f, "HideWindow"),
            Self::SetTopmost(..)=>write!(f, "SetTopmost"),
            Self::ShowInDock(..)=>write!(f, "ShowInDock"),
            
            Self::ShowTextIME(..)=>write!(f, "ShowTextIME"),
            Self::HideTextIME=>write!(f, "HideTextIME"),
            Self::SetCursor(..)=>write!(f, "SetCursor"),
            Self::StartTimer{..}=>write!(f, "StartTimer"),
            Self::StopTimer(..)=>write!(f, "StopTimer"),
            Self::Quit=>write!(f, "Quit"),
            
            Self::StartDragging(..)=>write!(f, "StartDragging"),
            Self::UpdateMacosMenu(..)=>write!(f, "UpdateMacosMenu"),
            Self::ShowClipboardActions(..)=>write!(f, "ShowClipboardActions"),
            Self::CopyToClipboard(..)=>write!(f, "CopyToClipboard"),
            
            Self::HttpRequest{..}=>write!(f, "HttpRequest"),
            Self::CancelHttpRequest{..}=>write!(f, "CancelHttpRequest"),
            
            Self::PrepareVideoPlayback(..)=>write!(f, "PrepareVideoPlayback"),
            Self::BeginVideoPlayback(..)=>write!(f, "BeginVideoPlayback"),
            Self::PauseVideoPlayback(..)=>write!(f, "PauseVideoPlayback"),
            Self::ResumeVideoPlayback(..)=>write!(f, "ResumeVideoPlayback"),
            Self::MuteVideoPlayback(..)=>write!(f, "MuteVideoPlayback"),
            Self::UnmuteVideoPlayback(..)=>write!(f, "UnmuteVideoPlayback"),
            Self::CleanupVideoPlaybackResources(..)=>write!(f, "CleanupVideoPlaybackResources"),
            Self::UpdateVideoSurfaceTexture(..)=>write!(f, "UpdateVideoSurfaceTexture"),
            Self::CreateWebView{..}=>write!(f, "CreateWebView"),
            Self::UpdateWebView{..}=>write!(f, "UpdateWebView"),
            Self::CloseWebView{..}=>write!(f, "CloseWebView"),
            Self::SaveFileDialog(..)=>write!(f, "SaveFileDialog"),
            Self::SelectFileDialog(..)=>write!(f, "SelectFileDialog"),
            Self::SaveFolderDialog(..)=>write!(f, "SaveFolderDialog"),
            Self::SelectFolderDialog(..)=>write!(f, "SelectFolderDialog"),
            Self::ResizeWindow(..)=>write!(f, "ResizeWindow"),
            Self::RepositionWindow(..)=>write!(f, "RepositionWindow"),
            
            Self::XrStartPresenting=>write!(f, "XrStartPresenting"),
            Self::XrStopPresenting=>write!(f, "XrStopPresenting"),
            Self::XrAdvertiseAnchor(_)=>write!(f, "XrAdvertiseAnchor"),
            Self::XrSetLocalAnchor(_)=>write!(f, "XrSetLocalAnchor"),
            Self::XrDiscoverAnchor(_)=>write!(f, "XrDiscoverAnchor"),
        }
    }
}
impl Cx {
    pub fn in_draw_event(&self)->bool{
        self.in_draw_event
    }

    pub fn xr_capabilities(&self) -> &XrCapabilities {
        &self.xr_capabilities
    }

    pub fn get_ref(&self) -> CxRef {
        CxRef(self.self_ref.clone().unwrap())
    }
    
    pub fn take_dependency(&mut self, path: &str) -> Result<Rc<Vec<u8>>, String> {
        if let Some(data) = self.dependencies.get_mut(path) {
            if let Some(data) = data.data.take() {
                return match data {
                    Ok(data) => Ok(data),
                    Err(s) => Err(s.clone()),
                };
            }
        }
        Err(format!("Dependency not loaded {}", path))
    }
    
    pub fn get_dependency(&self, path: &str) -> Result<Rc<Vec<u8>>, String> {
        if let Some(data) = self.dependencies.get(path) {
            if let Some(data) = &data.data {
                return match data {
                    Ok(data) => Ok(data.clone()),
                    Err(s) => Err(s.clone()),
                };
            }
        }
        Err(format!("Dependency not loaded {}", path))
    }
    pub fn null_texture(&self) -> Texture {
        self.null_texture.clone()
    }
    pub fn redraw_id(&self) -> u64 {
        self.redraw_id
    }

    pub fn os_type(&self) -> &OsType {
        &self.os_type
    }
    
    /// Returns the app's writable data directory path.
    /// 
    /// On Android, this is the directory returned by Activity's getFilesDir().
    /// On iOS, this is the Application Support directory.
    /// Returns None on unsupported platforms (e.g. wasm).
    /// 
    /// Note that this path is not guaranteed to exist (it doesn't by default on iOS simulators),
    /// so you might need to create it.
    pub fn get_data_dir(&self) -> Option<String> {
        self.os_type.get_data_dir()
    }

    pub fn in_makepad_studio(&self) -> bool {
        self.in_makepad_studio
    }

    pub fn cpu_cores(&self) -> usize {
        self.cpu_cores
    }
    pub fn gpu_info(&self) -> &GpuInfo {
        &self.gpu_info
    }

    pub fn update_macos_menu(&mut self, menu: MacosMenu) {
        self.platform_ops.push(CxOsOp::UpdateMacosMenu(menu));
    }
    
    pub fn xr_start_presenting(&mut self) {
        self.platform_ops.push(CxOsOp::XrStartPresenting);
    }
    
    pub fn xr_advertise_anchor(&mut self, anchor:XrAnchor) {
        self.platform_ops.push(CxOsOp::XrAdvertiseAnchor(anchor));
    }
    
    pub fn xr_set_local_anchor(&mut self,  anchor:XrAnchor) {
        self.platform_ops.push(CxOsOp::XrSetLocalAnchor(anchor));
    }
            
    pub fn xr_discover_anchor(&mut self, id: u8) {
        self.platform_ops.push(CxOsOp::XrDiscoverAnchor(id));
    }
        
        
    pub fn quit(&mut self) {
        self.platform_ops.push(CxOsOp::Quit);
    }
    // Determines whether to show your application in the dock when it runs. The default value is true. 
    // You can remove the dock icon by setting this value to false. 
    pub fn show_in_dock(&mut self, show: bool) {
        self.platform_ops.push(CxOsOp::ShowInDock(show));
    }
    pub fn push_unique_platform_op(&mut self, op: CxOsOp) {
        if self.platform_ops.iter().find(|o| **o == op).is_none() {
            self.platform_ops.push(op);
        }
    }

    pub fn show_text_ime(&mut self, area: Area, pos: DVec2) {
        if !self.keyboard.text_ime_dismissed {
            self.ime_area = area;
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
        self.platform_ops
            .push(CxOsOp::ShowClipboardActions(selected));
    }

    /// Copies the given string to the clipboard.
    /// 
    /// Due to lack of platform clipboard support, it does not work on Web or tvOS.
    pub fn copy_to_clipboard(&mut self, content: &str) {
        self.platform_ops.push(CxOsOp::CopyToClipboard(content.to_owned()));
    }

    pub fn start_dragging(&mut self, items: Vec<DragItem>) {
        self.platform_ops.iter().for_each(|p| {
            if let CxOsOp::StartDragging { .. } = p {
                panic!("start drag twice");
            }
        });
        self.platform_ops.push(CxOsOp::StartDragging(items));
    }

    pub fn set_cursor(&mut self, cursor: MouseCursor) {
        // down cursor overrides the hover cursor
        if let Some(p) = self.platform_ops.iter_mut().find(|p| match p {
            CxOsOp::SetCursor(_) => true,
            _ => false,
        }) {
            *p = CxOsOp::SetCursor(cursor)
        } else {
            self.platform_ops.push(CxOsOp::SetCursor(cursor))
        }
    }

    pub fn sweep_lock(&mut self, value: Area) {
        self.fingers.sweep_lock(value);
    }

    pub fn sweep_unlock(&mut self, value: Area) {
        self.fingers.sweep_unlock(value);
    }

    pub fn start_timeout(&mut self, interval: f64) -> Timer {
        self.timer_id += 1;
        self.platform_ops.push(CxOsOp::StartTimer {
            timer_id: self.timer_id,
            interval,
            repeats: false,
        });
        Timer(self.timer_id)
    }

    pub fn start_interval(&mut self, interval: f64) -> Timer {
        self.timer_id += 1;
        self.platform_ops.push(CxOsOp::StartTimer {
            timer_id: self.timer_id,
            interval,
            repeats: true,
        });
        Timer(self.timer_id)
    }

    pub fn stop_timer(&mut self, timer: Timer) {
        if timer.0 != 0 {
            self.platform_ops.push(CxOsOp::StopTimer(timer.0));
        }
    }


    pub fn get_dpi_factor_of(&mut self, area: &Area) -> f64 {
        if let Some(draw_list_id) = area.draw_list_id() {
            let pass_id = self.draw_lists[draw_list_id].pass_id.unwrap();
            return self.get_delegated_dpi_factor(pass_id);
        }
        return 1.0;
    }
    
    pub fn get_pass_window_id(&self, pass_id: PassId) -> Option<WindowId> {
         let mut pass_id_walk = pass_id;
         for _ in 0..25 {
             match self.passes[pass_id_walk].parent {
                 CxPassParent::Window(window_id) => {
                     return Some(window_id)
                 }
                 CxPassParent::Pass(next_pass_id) => {
                     pass_id_walk = next_pass_id;
                 }
                 _ => {
                     break;
                 }
             }
         }
         None
     }
    
    pub fn get_delegated_dpi_factor(&mut self, pass_id: PassId) -> f64 {
        let mut pass_id_walk = pass_id;
        for _ in 0..25 {
            match self.passes[pass_id_walk].parent {
                CxPassParent::Window(window_id) => {
                    if !self.windows[window_id].is_created {
                        return 1.0;
                    }
                    return self.windows[window_id].window_geom.dpi_factor;
                }
                CxPassParent::Pass(next_pass_id) => {
                    pass_id_walk = next_pass_id;
                }
                _ => {
                    break;
                }
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
                }
                _ => {
                    break;
                }
            }
        }
    }

    pub fn get_pass_rect(&self, pass_id: PassId, dpi: f64) -> Option<Rect> {
        match self.passes[pass_id].pass_rect {
            Some(CxPassRect::Area(area)) => {
                let rect = area.rect(self);
                Some(Rect {
                    pos: (rect.pos * dpi).floor() / dpi,
                    size: (rect.size * dpi).ceil() / dpi,
                })
            }
            Some(CxPassRect::AreaOrigin(area, origin)) => {
                let rect = area.rect(self);
                Some(Rect {
                    pos: origin,
                    size: (rect.size * dpi).ceil() / dpi,
                })
            }
            /*Some(CxPassRect::ScaledArea(area, scale)) => {
                let rect = area.rect(self);
                Some(Rect {
                    pos: (rect.pos * dpi).floor() / dpi,
                    size: scale * (rect.size * dpi).ceil() / dpi,
                })
            }*/
            Some(CxPassRect::Size(size)) => Some(Rect {
                pos: DVec2::default(),
                size: (size * dpi).ceil() / dpi,
            }),
            None => None,
        }
    }

    pub fn get_pass_name(&self, pass_id: PassId) -> &str {
        &self.passes[pass_id].debug_name
    }

    pub fn repaint_pass(&mut self, pass_id: PassId) {
        let cxpass = &mut self.passes[pass_id];
        cxpass.paint_dirty = true;
    }

    pub fn repaint_pass_and_child_passes(&mut self, pass_id: PassId) {
        let cxpass = &mut self.passes[pass_id];
        cxpass.paint_dirty = true;
        for sub_pass_id in self.passes.id_iter() {
            if let CxPassParent::Pass(dep_pass_id) = self.passes[sub_pass_id].parent.clone() {
                if dep_pass_id == pass_id {
                    self.repaint_pass_and_child_passes(sub_pass_id);
                }
            }
        }
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
    
    pub fn redraw_area_in_draw(&mut self, area: Area) {
        if let Some(draw_list_id) = area.draw_list_id() {
            self.redraw_list_in_draw(draw_list_id);
        }
    }
    
    pub fn redraw_area_and_children(&mut self, area: Area) {
        if let Some(draw_list_id) = area.draw_list_id() {
            self.redraw_list_and_children(draw_list_id);
        }
    }

    pub fn redraw_list(&mut self, draw_list_id: DrawListId) {
        if self.in_draw_event{
            return
        }
        self.redraw_list_in_draw(draw_list_id);
    }
    
    pub fn redraw_list_in_draw(&mut self, draw_list_id: DrawListId) {
        if self
        .new_draw_event
        .draw_lists
        .iter()
        .position(|v| *v == draw_list_id)
        .is_some()
        {
            return;
        }
        self.new_draw_event.draw_lists.push(draw_list_id);
    }

    pub fn redraw_list_and_children(&mut self, draw_list_id: DrawListId) {
        if self.in_draw_event{
            return
        }
        if self
            .new_draw_event
            .draw_lists_and_children
            .iter()
            .position(|v| *v == draw_list_id)
            .is_some()
        {
            return;
        }
        self.new_draw_event
            .draw_lists_and_children
            .push(draw_list_id);
    }

    pub fn get_ime_area_rect(&self) -> Rect {
        self.ime_area.rect(self)
    }

    pub fn update_area_refs(&mut self, old_area: Area, new_area: Area) -> Area {
        if old_area == Area::Empty {
            return new_area;
        }
        if self.ime_area == old_area {
            self.ime_area = new_area;
        }
        self.fingers.update_area(old_area, new_area);
        self.drag_drop.update_area(old_area, new_area);
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
        } else {
            let mut new_set = Vec::new();
            new_set.push(trigger);
            self.triggers.insert(area, new_set);
        }
    }

    pub fn set_global<T: 'static + Any + Sized>(&mut self, value: T) {
        if !self.globals.iter().any(|v| v.0 == TypeId::of::<T>()) {
            self.globals.push((TypeId::of::<T>(), Box::new(value)));
        }
    }

    pub fn get_global<T: 'static + Any>(&mut self) -> &mut T {
        let item = self
            .globals
            .iter_mut()
            .find(|v| v.0 == TypeId::of::<T>())
            .unwrap();
        item.1.downcast_mut().unwrap()
    }

    pub fn has_global<T: 'static + Any>(&mut self) -> bool {
        self.globals
            .iter_mut()
            .find(|v| v.0 == TypeId::of::<T>())
            .is_some()
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

    pub fn http_request(&mut self, request_id: LiveId, request: HttpRequest) {
        self.platform_ops.push(CxOsOp::HttpRequest {
            request_id,
            request,
        });
    }
    
    pub fn cancel_http_request(&mut self, request_id: LiveId) {
        self.platform_ops.push(CxOsOp::CancelHttpRequest {
            request_id,
        });
    }
    /*
        pub fn web_socket_open(&mut self, request_id: LiveId, request: HttpRequest) {
            self.platform_ops.push(CxOsOp::WebSocketOpen{
                request,
                request_id,
            });
        }

        pub fn web_socket_send_binary(&mut self, request_id: LiveId, data: Vec<u8>) {
            self.platform_ops.push(CxOsOp::WebSocketSendBinary{
                request_id,
                data,
            });
        }
    */
    pub fn prepare_video_playback(
        &mut self,
        video_id: LiveId,
        source: VideoSource,
        external_texture_id: u32,
        autoplay: bool,
        should_loop: bool,
    ) {
        self.platform_ops.push(CxOsOp::PrepareVideoPlayback(
            video_id,
            source,
            external_texture_id,
            autoplay,
            should_loop,
        ));
    }

    pub fn begin_video_playback(&mut self, video_id: LiveId) {
        self.platform_ops.push(CxOsOp::BeginVideoPlayback(video_id));
    }

    pub fn pause_video_playback(&mut self, video_id: LiveId) {
        self.platform_ops.push(CxOsOp::PauseVideoPlayback(video_id));
    }

    pub fn resume_video_playback(&mut self, video_id: LiveId) {
        self.platform_ops
            .push(CxOsOp::ResumeVideoPlayback(video_id));
    }

    pub fn mute_video_playback(&mut self, video_id: LiveId) {
        self.platform_ops.push(CxOsOp::MuteVideoPlayback(video_id));
    }

    pub fn unmute_video_playback(&mut self, video_id: LiveId) {
        self.platform_ops
            .push(CxOsOp::UnmuteVideoPlayback(video_id));
    }

    pub fn cleanup_video_playback_resources(&mut self, video_id: LiveId) {
        self.platform_ops
            .push(CxOsOp::CleanupVideoPlaybackResources(video_id));
    }

    pub fn println_resources(&self) {
        println!("Num textures: {}", self.textures.0.pool.len());
    }

    pub fn open_system_savefile_dialog(&mut self) {
        self.platform_ops.push(CxOsOp::SaveFileDialog(FileDialog::new()));
    }

    pub fn open_system_openfile_dialog(&mut self) {
        self.platform_ops.push(CxOsOp::SelectFileDialog(FileDialog::new()));
    }

    pub fn open_system_savefolder_dialog(&mut self) {
        self.platform_ops.push(CxOsOp::SaveFolderDialog(FileDialog::new()));

    }

    pub fn open_system_openfolder_dialog(&mut self) {
        self.platform_ops.push(CxOsOp::SelectFolderDialog(FileDialog::new()));

    }

    pub fn event_id(&self) -> u64 {
        self.event_id
    }
}

#[macro_export]
macro_rules! register_component_factory {
    ( $ cx: ident, $ registry: ident, $ ty: ty, $ factory: ident) => {
        let module_id = LiveModuleId::from_str(&module_path!()).unwrap();
        if let Some((reg, _)) = $cx
            .live_registry
            .borrow()
            .components
            .get_or_create::<$registry>()
            .map
            .get(&LiveType::of::<$ty>())
        {
            if reg.module_id != module_id {
                panic!(
                    "Component already registered {} {}",
                    stringify!($ty),
                    reg.module_id
                );
            }
        }
        $cx.live_registry
            .borrow()
            .components
            .get_or_create::<$registry>()
            .map
            .insert(
                LiveType::of::<$ty>(),
                (
                    LiveComponentInfo {
                        name: LiveId::from_str_with_lut(stringify!($ty)).unwrap(),
                        module_id,
                    },
                    Box::new($factory()),
                ),
            );
    };
}
