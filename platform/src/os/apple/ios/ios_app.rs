use {
    std::{
        cell::Cell,
        time::Instant,
    },
    crate::{
        makepad_math::*,
        makepad_objc_sys::runtime::{ObjcId, nil},
        os::{
            apple::apple_sys::*,
            ios::{
                ios_delegates::*,
                ios_event::*,
            },
            cx_native::EventFlow,
        },
        area::Area,
        event::{
            KeyModifiers,
            TouchUpdateEvent,
            TouchState,
            TouchPoint,
            WindowGeomChangeEvent,
            WindowGeom,
            TimerEvent,
        },
        window::CxWindowPool
    }
};

// this value will be fetched from multiple threads (post signal uses it)
pub static mut IOS_CLASSES: *const IosClasses = 0 as *const _;
// this value should not. Todo: guard this somehow proper
pub static mut IOS_APP: *mut IosApp = 0 as *mut _;

pub fn init_ios_app_global(metal_device: ObjcId, event_callback: Box<dyn FnMut(&mut IosApp, IosEvent) -> EventFlow>) {
    unsafe {
        IOS_CLASSES = Box::into_raw(Box::new(IosClasses::new()));
        IOS_APP = Box::into_raw(Box::new(IosApp::new(metal_device, event_callback)));
    }
}

pub fn get_ios_app_global() -> &'static mut IosApp {
    unsafe {
        &mut *(IOS_APP)
    }
}

pub fn get_ios_class_global() -> &'static IosClasses {
    unsafe {
        &*(IOS_CLASSES)
    }
}

#[derive(Clone)]
pub struct IosTimer {
    timer_id: u64,
    nstimer: ObjcId,
    repeats: bool
}

pub struct IosClasses {
    pub app_delegate: *const Class,
    pub mtk_view: *const Class,
    pub mtk_view_dlg: *const Class,
    pub timer_delegate: *const Class,
}
impl IosClasses {
    pub fn new() -> Self {
        Self {
            app_delegate: define_ios_app_delegate(),
            mtk_view: define_mtk_view(),
            mtk_view_dlg: define_mtk_view_dlg(),
            timer_delegate: define_ios_timer_delegate()
        }
    }
}

pub struct IosApp {
    pub time_start: Instant,
    pub timer_delegate_instance: ObjcId,
    timers: Vec<IosTimer>,
    touches: Vec<TouchPoint>,
    pub last_window_geom: WindowGeom,
    metal_device: ObjcId,
    first_draw: bool,
    pub mtk_view: Option<ObjcId>,
    event_callback: Option<Box<dyn FnMut(&mut IosApp, IosEvent) -> EventFlow >>,
    event_flow: EventFlow,
}

impl IosApp {
    pub fn new(metal_device: ObjcId, event_callback: Box<dyn FnMut(&mut IosApp, IosEvent) -> EventFlow>) -> IosApp {
        unsafe {
            
            // Construct the bits that are shared between windows
            IosApp {
                touches: Vec::new(),
                last_window_geom: WindowGeom::default(),
                metal_device,
                first_draw: true,
                mtk_view: None,
                time_start: Instant::now(),
                timer_delegate_instance: msg_send![get_ios_class_global().timer_delegate, new],
                timers: Vec::new(),
                event_flow: EventFlow::Poll,
                event_callback: Some(event_callback),
            }
        }
    }
    
    pub fn did_finish_launching_with_options(&mut self) {
        unsafe {
            let main_screen: ObjcId = msg_send![class!(UIScreen), mainScreen];
            let screen_rect: NSRect = msg_send![main_screen, bounds];
            
            let window_obj: ObjcId = msg_send![class!(UIWindow), alloc];
            let window_obj: ObjcId = msg_send![window_obj, initWithFrame: screen_rect];
            
            let mtk_view_obj: ObjcId = msg_send![get_ios_class_global().mtk_view, alloc];
            let mtk_view_obj: ObjcId = msg_send![mtk_view_obj, initWithFrame: screen_rect];
            
            let mtk_view_dlg_obj: ObjcId = msg_send![get_ios_class_global().mtk_view_dlg, alloc];
            let mtk_view_dlg_obj: ObjcId = msg_send![mtk_view_dlg_obj, init];
            
            let view_ctrl_obj: ObjcId = msg_send![class!(UIViewController), alloc];
            let view_ctrl_obj: ObjcId = msg_send![view_ctrl_obj, init];
            
            let () = msg_send![view_ctrl_obj, setView: mtk_view_obj];
            
            let () = msg_send![mtk_view_obj, setPreferredFramesPerSecond: 120];
            let () = msg_send![mtk_view_obj, setDelegate: mtk_view_dlg_obj];
            let () = msg_send![mtk_view_obj, setDevice: self.metal_device];
            let () = msg_send![mtk_view_obj, setUserInteractionEnabled: YES];
            
            let () = msg_send![window_obj, addSubview: mtk_view_obj];
            
            let () = msg_send![window_obj, setRootViewController: view_ctrl_obj];

            let () = msg_send![view_ctrl_obj, beginAppearanceTransition: true animated: false];
            let () = msg_send![view_ctrl_obj, endAppearanceTransition];
            
            let () = msg_send![window_obj, makeKeyAndVisible];
            
            
            self.mtk_view = Some(mtk_view_obj);
        }
    }
    
    
    pub fn draw_in_rect(&mut self) {
        let main_screen: ObjcId = unsafe {msg_send![class!(UIScreen), mainScreen]};
        let screen_rect: NSRect = unsafe {msg_send![main_screen, bounds]};
        let dpi_factor: f64 = unsafe {msg_send![main_screen, scale]};
        let new_size = dvec2(screen_rect.size.width as f64, screen_rect.size.height as f64);
        let new_geom = WindowGeom {
            xr_is_presenting: false,
            is_topmost: false,
            is_fullscreen: true,
            can_fullscreen: false,
            inner_size: new_size,
            outer_size: new_size,
            dpi_factor,
            position: dvec2(0.0, 0.0)
        };
        if self.first_draw {
            self.last_window_geom = new_geom.clone();
            self.do_callback(
                IosEvent::Init,
            );
        }
        if self.first_draw || new_geom != self.last_window_geom {
            let old_geom = self.last_window_geom.clone();
            self.last_window_geom = new_geom.clone();
            self.do_callback(
                IosEvent::WindowGeomChange(WindowGeomChangeEvent {
                    window_id: CxWindowPool::id_zero(),
                    old_geom,
                    new_geom
                }),
            );
        }
        self.first_draw = false;
        self.do_callback(IosEvent::Paint);
    }
    
    pub fn update_touch(&mut self, uid: u64, abs:DVec2, state:TouchState){
        if let Some(touch) = self.touches.iter_mut().find(|v| v.uid == uid){
            touch.state = state;
            touch.abs = abs;
        }
        else{
            self.touches.push(TouchPoint{
                state,
                abs,
                uid,
                rotation_angle:0.0,
                force:0.0,
                radius:dvec2(0.0,0.0),
                handled: Cell::new(Area::Empty),
                sweep_lock: Cell::new(Area::Empty)
            })
        }
    }
    
    pub fn send_touch_update(&mut self){
        self.do_callback(IosEvent::TouchUpdate(TouchUpdateEvent{
            time: self.time_now(),
            window_id: CxWindowPool::id_zero(),
            modifiers: KeyModifiers::default(),
            touches: self.touches.clone()
        }));
        // remove the stopped touches
        self.touches.retain(|v| if let TouchState::Stop = v.state{false}else{true});
    }
    
    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_micros() as f64 / 1_000_000.0
    }
    
    pub fn event_loop(&mut self) {
        unsafe {
            let class: ObjcId = msg_send!(get_ios_class_global().app_delegate, class);
            let class_string = NSStringFromClass(class as _);
            let argc = 1;
            let mut argv = b"Makepad\0" as *const u8 as *mut i8;
            
            UIApplicationMain(argc, &mut argv, nil, class_string);
        }
    }
    
    pub fn do_callback(&mut self, event: IosEvent) {
        if let Some(mut callback) = self.event_callback.take() {
            self.event_flow = callback(self, event);
            self.event_callback = Some(callback);
        }
    }
    
    pub fn start_timer(&mut self, timer_id: u64, interval: f64, repeats: bool) {
        unsafe {
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
            
            let nstimer: ObjcId = msg_send![
                class!(NSTimer),
                timerWithTimeInterval: interval
                target: self.timer_delegate_instance
                selector: sel!(receivedTimer:)
                userInfo: nil
                repeats: repeats
            ];
            let nsrunloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
            let () = msg_send![nsrunloop, addTimer: nstimer forMode: NSRunLoopCommonModes];
            
            self.timers.push(IosTimer {
                timer_id: timer_id,
                nstimer: nstimer,
                repeats: repeats
            });
            let () = msg_send![pool, release];
        }
    }
    
    pub fn stop_timer(&mut self, timer_id: u64) {
        for i in 0..self.timers.len() {
            if self.timers[i].timer_id == timer_id {
                unsafe {
                    let () = msg_send![self.timers[i].nstimer, invalidate];
                }
                self.timers.remove(i);
                return;
            }
        }
    }
    
    pub fn send_timer_received(&mut self, nstimer: ObjcId) {
        for i in 0..self.timers.len() {
            if self.timers[i].nstimer == nstimer {
                let timer_id = self.timers[i].timer_id;
                if !self.timers[i].repeats {
                    self.timers.remove(i);
                }
                self.do_callback(IosEvent::Timer(TimerEvent {timer_id: timer_id}));
            }
        }
    }
    
    pub fn send_paint_event(&mut self) {
        self.do_callback(IosEvent::Paint);
    }
}
