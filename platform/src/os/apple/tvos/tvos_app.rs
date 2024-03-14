use {
    std::{
        cell::{RefCell},
        time::Instant,
    },
    crate::{
        makepad_math::*,
        os::{
            apple::apple_sys::*,
            tvos::{
                tvos_delegates::*,
                tvos_event::*,
            },
            cx_native::EventFlow,
        },
        //area::Area,
        event::{
            //VirtualKeyboardEvent,
            //KeyCode,
            ////KeyEvent,
            //TextInputEvent,
            //KeyModifiers,
            //TouchUpdateEvent,
            //TouchState,
            //TouchPoint,
            WindowGeomChangeEvent,
            WindowGeom,
            TimerEvent,
        },
        window::CxWindowPool
    }
};

// this value will be fetched from multiple threads (post signal uses it)
pub static mut TVOS_CLASSES: *const TvosClasses = 0 as *const _;
// this value should not. Todo: guard this somehow proper
pub static mut TVOS_APP: Option<RefCell<TvosApp>> = None;

pub fn init_tvos_app_global(metal_device: ObjcId, event_callback: Box<dyn FnMut(TvosEvent) -> EventFlow>) {
    unsafe {
        TVOS_CLASSES = Box::into_raw(Box::new(TvosClasses::new()));
        TVOS_APP = Some(RefCell::new(TvosApp::new(metal_device, event_callback)));
    }
}

pub fn get_tvos_app_global() -> std::cell::RefMut<'static, TvosApp> {
    unsafe {
        TVOS_APP.as_mut().unwrap().borrow_mut()
    }
}

pub fn get_tvos_class_global() -> &'static TvosClasses {
    unsafe {
        &*(TVOS_CLASSES)
    }
}

#[derive(Clone)]
pub struct TvosTimer {
    timer_id: u64,
    nstimer: ObjcId,
    repeats: bool
}

pub struct TvosClasses {
    pub app_delegate: *const Class,
    pub mtk_view: *const Class,
    pub mtk_view_delegate: *const Class,
   // pub textfield_delegate: *const Class,
    pub timer_delegate: *const Class,
}
impl TvosClasses {
    pub fn new() -> Self {
        Self {
            app_delegate: define_tvos_app_delegate(),
            mtk_view: define_mtk_view(),
            mtk_view_delegate: define_mtk_view_delegate(),
            //textfield_delegate: define_textfield_delegate(),
            timer_delegate: define_ios_timer_delegate()
        }
    }
}

pub struct TvosApp {
    pub time_start: Instant,
    //pub virtual_keyboard_event:  Option<VirtualKeyboardEvent>,
    pub timer_delegate_instance: ObjcId,
    timers: Vec<TvosTimer>,
    //touches: Vec<TouchPoint>,
    pub last_window_geom: WindowGeom,
    metal_device: ObjcId,
    first_draw: bool,
    pub mtk_view: Option<ObjcId>,
   // pub textfield: Option<ObjcId>,
    event_callback: Option<Box<dyn FnMut(TvosEvent) -> EventFlow >>,
    event_flow: EventFlow,
}

impl TvosApp {
    pub fn new(metal_device: ObjcId, event_callback: Box<dyn FnMut(TvosEvent) -> EventFlow>) -> TvosApp {
        unsafe {
            
            // Construct the bits that are shared between windows
            TvosApp {
                //touches: Vec::new(),
                last_window_geom: WindowGeom::default(),
                metal_device,
                first_draw: true,
                mtk_view: None,
                time_start: Instant::now(),
                timer_delegate_instance: msg_send![get_tvos_class_global().timer_delegate, new],
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
            
            let mtk_view_obj: ObjcId = msg_send![get_tvos_class_global().mtk_view, alloc];
            let mtk_view_obj: ObjcId = msg_send![mtk_view_obj, initWithFrame: screen_rect];
            
            let mtk_view_dlg_obj: ObjcId = msg_send![get_tvos_class_global().mtk_view_delegate, alloc];
            let mtk_view_dlg_obj: ObjcId = msg_send![mtk_view_dlg_obj, init];
            
            let view_ctrl_obj: ObjcId = msg_send![class!(UIViewController), alloc];
            let view_ctrl_obj: ObjcId = msg_send![view_ctrl_obj, init];
            
            let () = msg_send![view_ctrl_obj, setView: mtk_view_obj];
            
            let () = msg_send![mtk_view_obj, setPreferredFramesPerSecond: 120];
            let () = msg_send![mtk_view_obj, setDelegate: mtk_view_dlg_obj];
            let () = msg_send![mtk_view_obj, setDevice: self.metal_device];
            let () = msg_send![mtk_view_obj, setUserInteractionEnabled: YES];
            let () = msg_send![mtk_view_obj, setAutoResizeDrawable: YES];
            let () = msg_send![mtk_view_obj, setMultipleTouchEnabled: YES];
            /*
            let textfield_dlg: ObjcId = msg_send![get_tvos_class_global().textfield_delegate, alloc];
            let textfield_dlg: ObjcId= msg_send![textfield_dlg, init];
             
            let textfield: ObjcId = msg_send![class!(UITextField), alloc];
            let textfield: ObjcId =  msg_send![textfield, initWithFrame: NSRect {origin: NSPoint {x: 10.0, y: 10.0}, size: NSSize {width: 100.0, height: 50.0}}];
            let () = msg_send![textfield, setAutocapitalizationType: 0]; // UITextAutocapitalizationTypeNone
            let () = msg_send![textfield, setAutocorrectionType: 1]; // UITextAutocorrectionTypeNo
            let () = msg_send![textfield, setSpellCheckingType: 1]; // UITextSpellCheckingTypeNo
            let () = msg_send![textfield, setHidden: YES];
            let () = msg_send![textfield, setDelegate: textfield_dlg];
            // to make backspce work - with empty text there is no event on text removal
            let () = msg_send![textfield, setText: str_to_nsstring("x")];
            let () = msg_send![mtk_view_obj, addSubview: textfield];
            
            let notification_center: ObjcId = msg_send![class!(NSNotificationCenter), defaultCenter];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardDidChangeFrame:) name: UIKeyboardDidChangeFrameNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardWillChangeFrame:) name: UIKeyboardWillChangeFrameNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardDidShow:) name: UIKeyboardDidShowNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardWillShow:) name: UIKeyboardWillShowNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardDidHide:) name: UIKeyboardDidHideNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardWillHide:) name: UIKeyboardWillHideNotification object: nil];
            */
            let () = msg_send![window_obj, addSubview: mtk_view_obj];
            
            let () = msg_send![window_obj, setRootViewController: view_ctrl_obj];
            
            //let () = msg_send![view_ctrl_obj, beginAppearanceTransition: true animated: false];
            //let () = msg_send![view_ctrl_obj, endAppearanceTransition];
            
            let () = msg_send![window_obj, makeKeyAndVisible];
            
            //self.textfield = Some(textfield);
            self.mtk_view = Some(mtk_view_obj);
        }
    }
    
    pub fn draw_size_will_change() {
        Self::check_window_geom();
    }
    
    pub fn check_window_geom() {
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

        if get_tvos_app_global().first_draw {
            get_tvos_app_global().update_geom(new_geom.clone());
            TvosApp::do_callback(
                TvosEvent::Init,
            );
        }
        let old_geom = get_tvos_app_global().update_geom(new_geom.clone());
        if let Some(old_geom) = old_geom {
            TvosApp::do_callback(
                TvosEvent::WindowGeomChange(WindowGeomChangeEvent {
                    window_id: CxWindowPool::id_zero(),
                    old_geom,
                    new_geom
                }), 
            );
        }
    }
    
    fn update_geom(&mut self, new_geom: WindowGeom)->Option<WindowGeom>{
        if self.first_draw || new_geom != self.last_window_geom{
            let old_geom = self.last_window_geom.clone();
            self.last_window_geom = new_geom;
            return Some(old_geom);
        }
        None
    }
    
    pub fn draw_in_rect() {
        Self::check_window_geom();
        get_tvos_app_global().first_draw = false;
        TvosApp::do_callback(TvosEvent::Paint);
    }
    
    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_micros() as f64 / 1_000_000.0
    }
    
    pub fn event_loop() {
        unsafe {
             
                        
            let app_delegate = get_tvos_class_global().app_delegate;
            let class: ObjcId = msg_send!(app_delegate, class);
            let class_string = NSStringFromClass(class as _);
            let argc = 1;
            let mut argv = b"Makepad\0" as *const u8 as *mut i8;
            
            UIApplicationMain(argc, &mut argv, nil, class_string);
        }
    }
    
    pub fn do_callback(event: TvosEvent) {
        let cb = get_tvos_app_global().event_callback.take();
        if let Some(mut callback) = cb {
            
            
            let event_flow = callback(event);
            let mtk_view = get_tvos_app_global().mtk_view.unwrap();
            get_tvos_app_global().event_flow = event_flow;
            if let EventFlow::Wait = event_flow {
                let () = unsafe {msg_send![mtk_view, setPaused: YES]};
            }
            else {
                let () = unsafe {msg_send![mtk_view, setPaused: NO]};
            }
            get_tvos_app_global().event_callback = Some(callback);
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
            
            self.timers.push(TvosTimer {
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
    
    pub fn send_timer_received(nstimer: ObjcId) {
        let len = get_tvos_app_global().timers.len();
        let time = get_tvos_app_global().time_now();
        for i in 0..len {
            if get_tvos_app_global().timers[i].nstimer == nstimer {
                let timer_id = get_tvos_app_global().timers[i].timer_id;
                if !get_tvos_app_global().timers[i].repeats {
                    get_tvos_app_global().timers.remove(i);
                }
                TvosApp::do_callback(TvosEvent::Timer(TimerEvent {timer_id: timer_id, time:Some(time)}));
            }
        }
    }
    
    pub fn send_paint_event() {
        TvosApp::do_callback(TvosEvent::Paint);
    }
}
