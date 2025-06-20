use {
    std::{
        cell::{Cell,RefCell},
        time::Instant,
    },
    crate::{
        event::*,
        os::{
            apple::{
                apple_sys::*,
                apple_util::*,
            },
            cx_native::EventFlow,
            ios::{
                ios_delegates::*,
                ios_event::*,
            }
        },
        area::Area,
        window::CxWindowPool,
        makepad_math::*,
    }
};

// this value will be fetched from multiple threads (post signal uses it)
pub static mut IOS_CLASSES: *const IosClasses = 0 as *const _;
// this value should not. Todo: guard this somehow proper

thread_local! {
    pub static IOS_APP: RefCell<Option<IosApp>> = RefCell::new(None);
}

pub fn with_ios_app<R>(f: impl FnOnce(&mut IosApp) -> R) -> R {
    IOS_APP.with_borrow_mut(|app| {
        f(app.as_mut().unwrap())
    })
}

pub fn init_ios_app_global(metal_device: ObjcId, event_callback: Box<dyn FnMut(IosEvent) -> EventFlow>) {
    unsafe {
        IOS_CLASSES = Box::into_raw(Box::new(IosClasses::new()));
        IOS_APP.with(|app| {
            *app.borrow_mut() = Some(IosApp::new(metal_device, event_callback));
        })
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
    pub mtk_view_delegate: *const Class,
    pub gesture_recognizer_handler: *const Class,
    pub textfield_delegate: *const Class,
    pub timer_delegate: *const Class,
}
impl IosClasses {
    pub fn new() -> Self {
        Self {
            app_delegate: define_ios_app_delegate(),
            mtk_view: define_mtk_view(),
            mtk_view_delegate: define_mtk_view_delegate(),
            gesture_recognizer_handler: define_gesture_recognizer_handler(),
            textfield_delegate: define_textfield_delegate(),
            timer_delegate: define_ios_timer_delegate()
        }
    }
}

pub struct IosApp {
    pub time_start: Instant,
    pub virtual_keyboard_event:  Option<VirtualKeyboardEvent>,
    pub timer_delegate_instance: ObjcId,
    timers: Vec<IosTimer>,
    touches: Vec<TouchPoint>,
    pub last_window_geom: WindowGeom,
    metal_device: ObjcId,
    first_draw: bool,
    pub mtk_view: Option<ObjcId>,
    pub textfield: Option<ObjcId>,
    event_callback: Option<Box<dyn FnMut(IosEvent) -> EventFlow >>,
    event_flow: EventFlow,
    pasteboard: ObjcId
}

impl IosApp {
    pub fn new(metal_device: ObjcId, event_callback: Box<dyn FnMut(IosEvent) -> EventFlow>) -> IosApp {
        unsafe {
            
            // Construct the bits that are shared between windows
            //let ns_app: ObjcId = msg_send![class!(UIApplication), sharedApplication];
            //let app_delegate_instance: ObjcId = msg_send![get_ios_class_global().app_delegate, new];
            //if ns_app == nil{
            //   panic!();
            //}
            //let () = msg_send![ns_app, setDelegate: app_delegate_instance];
            
            let pasteboard: ObjcId = msg_send![class!(UIPasteboard), generalPasteboard];
            IosApp {
                virtual_keyboard_event: None,
                touches: Vec::new(),
                last_window_geom: WindowGeom::default(),
                metal_device,
                first_draw: true,
                mtk_view: None,
                textfield: None,
                time_start: Instant::now(),
                timer_delegate_instance: msg_send![get_ios_class_global().timer_delegate, new],
                timers: Vec::new(),
                event_flow: EventFlow::Poll,
                event_callback: Some(event_callback),
                pasteboard,
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
            
            let mtk_view_dlg_obj: ObjcId = msg_send![get_ios_class_global().mtk_view_delegate, alloc];
            let mtk_view_dlg_obj: ObjcId = msg_send![mtk_view_dlg_obj, init];

            // Instantiate a long-press gesture recognizer and our delegate,
            // set that delegate to be the target of the "gesture recognized" action,
            // and add the gesture recognizer to our MTKView subclass.
            let gesture_recognizer_handler_obj: ObjcId = msg_send![get_ios_class_global().gesture_recognizer_handler, alloc];
            let gesture_recognizer_handler_obj: ObjcId = msg_send![gesture_recognizer_handler_obj, init];
            let gesture_recognizer_obj: ObjcId = msg_send![class!(UILongPressGestureRecognizer), alloc];
            let gesture_recognizer_obj: ObjcId = msg_send![
                gesture_recognizer_obj,
                initWithTarget: gesture_recognizer_handler_obj
                action: sel!(handleLongPressGesture: gestureRecognizer:)
            ];
            // Set `cancelsTouchesInView` to NO so that the gesture recognizer doesn't prevent
            // later touch events from being sent to the MTKView *after* it has recognized its gesture.
            let () = msg_send!(gesture_recognizer_obj, setCancelsTouchesInView: NO);
            let () = msg_send![mtk_view_obj, addGestureRecognizer: gesture_recognizer_obj];
            
            let view_ctrl_obj: ObjcId = msg_send![class!(UIViewController), alloc];
            let view_ctrl_obj: ObjcId = msg_send![view_ctrl_obj, init];
            
            let () = msg_send![view_ctrl_obj, setView: mtk_view_obj];
            
            let () = msg_send![mtk_view_obj, setPreferredFramesPerSecond: 120];
            let () = msg_send![mtk_view_obj, setDelegate: mtk_view_dlg_obj];
            let () = msg_send![mtk_view_obj, setDevice: self.metal_device];
            let () = msg_send![mtk_view_obj, setUserInteractionEnabled: YES];
            let () = msg_send![mtk_view_obj, setAutoResizeDrawable: YES];
            let () = msg_send![mtk_view_obj, setMultipleTouchEnabled: YES];
            
            let textfield_dlg: ObjcId = msg_send![get_ios_class_global().textfield_delegate, alloc];
            let textfield_dlg: ObjcId = msg_send![textfield_dlg, init];
             
            let textfield: ObjcId = msg_send![class!(UITextField), alloc];
            let textfield: ObjcId =  msg_send![textfield, initWithFrame: NSRect {origin: NSPoint {x: 10.0, y: 10.0}, size: NSSize {width: 100.0, height: 50.0}}];
            let () = msg_send![textfield, setAutocapitalizationType: 0]; // UITextAutocapitalizationTypeNone
            let () = msg_send![textfield, setAutocorrectionType: 1]; // UITextAutocorrectionTypeNo
            let () = msg_send![textfield, setSpellCheckingType: 1]; // UITextSpellCheckingTypeNo
            let () = msg_send![textfield, setHidden: YES];
            let () = msg_send![textfield, setDelegate: textfield_dlg];
            // to make backspace work - with empty text there is no event on text removal
            let () = msg_send![textfield, setText: str_to_nsstring("x")];
            let () = msg_send![mtk_view_obj, addSubview: textfield];
            
            let notification_center: ObjcId = msg_send![class!(NSNotificationCenter), defaultCenter];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardDidChangeFrame:) name: UIKeyboardDidChangeFrameNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardWillChangeFrame:) name: UIKeyboardWillChangeFrameNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardDidShow:) name: UIKeyboardDidShowNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardWillShow:) name: UIKeyboardWillShowNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardDidHide:) name: UIKeyboardDidHideNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardWillHide:) name: UIKeyboardWillHideNotification object: nil];
            
            let () = msg_send![window_obj, addSubview: mtk_view_obj];
            
            let () = msg_send![window_obj, setRootViewController: view_ctrl_obj];
            
            //let () = msg_send![view_ctrl_obj, beginAppearanceTransition: true animated: false];
            //let () = msg_send![view_ctrl_obj, endAppearanceTransition];
            
            let () = msg_send![window_obj, makeKeyAndVisible];
            
            self.textfield = Some(textfield);
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
        
        let first_draw = with_ios_app(|app| app.first_draw);
        if first_draw {
            with_ios_app(|app| app.update_geom(new_geom.clone()));
            IosApp::do_callback(
                IosEvent::Init,
            );
        }
        
        let old_geom = with_ios_app(|app| app.update_geom(new_geom.clone()));
        if let Some(old_geom) = old_geom {
            IosApp::do_callback(
                IosEvent::WindowGeomChange(WindowGeomChangeEvent {
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
        with_ios_app(|app| app.first_draw = false);
        IosApp::do_callback(IosEvent::Paint);
    }
    
    pub fn update_touch(&mut self, uid: u64, abs: DVec2, state: TouchState) {
        if let Some(touch) = self.touches.iter_mut().find( | v | v.uid == uid) {
            touch.state = state;
            touch.abs = abs;
        }
        else {
            self.touches.push(TouchPoint {
                state,
                abs,
                uid,
                time: self.time_now(),
                rotation_angle: 0.0,
                force: 0.0,
                radius: dvec2(0.0, 0.0),
                handled: Cell::new(Area::Empty),
                sweep_lock: Cell::new(Area::Empty)
            })
        }
    }
    
    pub fn send_touch_update() {
        let time_now = with_ios_app(|app| app.time_now());
        let touches = with_ios_app(|app| app.touches.clone());
        IosApp::do_callback(IosEvent::TouchUpdate(TouchUpdateEvent {
            time: time_now,
            window_id: CxWindowPool::id_zero(),
            modifiers: KeyModifiers::default(),
            touches
        }));
        // remove the stopped touches
        with_ios_app(|app| app.touches.retain( | v | if let TouchState::Stop = v.state {false}else {true}));
    }

    pub fn send_long_press(abs: NSPoint, uid: u64) {
        let time_now = with_ios_app(|app| app.time_now());
        IosApp::do_callback(IosEvent::LongPress(LongPressEvent {
            abs: dvec2(abs.x, abs.y),
            time: time_now,
            window_id: CxWindowPool::id_zero(),
            uid,
        }));
    }

    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_micros() as f64 / 1_000_000.0
    }
    
    pub fn event_loop() {
        unsafe {
            let app_delegate = get_ios_class_global().app_delegate;
            let class: ObjcId = msg_send!(app_delegate, class);
            let class_string = NSStringFromClass(class as _);
            let argc = 1;
            let mut argv = b"Makepad\0" as *const u8 as *mut i8;
            
            UIApplicationMain(argc, &mut argv, nil, class_string);
        }
    }
    
    pub fn show_keyboard() {
        let textfield = with_ios_app(|app| app.textfield.unwrap());
        let () = unsafe {msg_send![textfield, becomeFirstResponder]};
    }

    pub fn hide_keyboard(){
        let textfield = with_ios_app(|app| app.textfield.unwrap());
        let () = unsafe {msg_send![textfield, resignFirstResponder]};
    }
    
    pub fn do_callback(event: IosEvent) {
        let cb = with_ios_app(|app| app.event_callback.take());
        if let Some(mut callback) = cb {
            let event_flow = callback(event);
            let mtk_view = with_ios_app(|app| app.mtk_view.unwrap());
            with_ios_app(|app| app.event_flow = event_flow);
            
            if let EventFlow::Wait = event_flow {
                let () = unsafe {msg_send![mtk_view, setPaused: YES]};
            }
            else {
                let () = unsafe {msg_send![mtk_view, setPaused: NO]};
            }
            
            with_ios_app(|app| app.event_callback = Some(callback));
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
    
    pub fn send_virtual_keyboard_event(event:VirtualKeyboardEvent){
        IosApp::do_callback(IosEvent::VirtualKeyboard(event));
    }
    
    pub fn queue_virtual_keyboard_event(&mut self, event:VirtualKeyboardEvent){
        self.virtual_keyboard_event = Some(event);
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
    
    pub fn send_text_input(input: String, replace_last: bool) {
        IosApp::do_callback(IosEvent::TextInput(TextInputEvent {
            input: input,
            was_paste: false,
            replace_last: replace_last
        }))
    }
    
    pub fn send_backspace() {
        let time = with_ios_app(|app| app.time_now());
        IosApp::do_callback(IosEvent::KeyDown(KeyEvent {
            key_code: KeyCode::Backspace,
            is_repeat: false,
            modifiers: Default::default(),
            time,
        }));
        IosApp::do_callback(IosEvent::KeyUp(KeyEvent {
            key_code: KeyCode::Backspace,
            is_repeat: false,
            modifiers: Default::default(),
            time,
        }));
    }
    
    pub fn send_timer_received(nstimer: ObjcId) {
        let len = with_ios_app(|app| app.timers.len());
        let time = with_ios_app(|app| app.time_now());
        for i in 0..len {
            if with_ios_app(|app| app.timers[i].nstimer == nstimer) {
                let timer_id = with_ios_app(|app| app.timers[i].timer_id);
                if !with_ios_app(|app| app.timers[i].repeats) {
                    with_ios_app(|app| app.timers.remove(i));
                }
                IosApp::do_callback(IosEvent::Timer(TimerEvent {timer_id: timer_id, time:Some(time)}));
                return
            }
        }
    }
    
    pub fn send_paint_event() {
        IosApp::do_callback(IosEvent::Paint);
    }

    pub fn copy_to_clipboard(&self, content: &str) {
        unsafe {
            let nsstring = str_to_nsstring(content);
            let pasteboard: ObjcId = self.pasteboard;
            let _: () = msg_send![pasteboard, setString: nsstring];
        }
    }

    pub fn get_ios_directory_paths() -> String {
        unsafe {
            let file_manager: ObjcId = msg_send![class!(NSFileManager), defaultManager];
            
            // Get application support directory
            let app_support_dir: ObjcId = msg_send![
                file_manager,
                URLsForDirectory: NSApplicationSupportDirectory
                inDomains: NSUserDomainMask
            ];
            let app_support_url: ObjcId = msg_send![app_support_dir, firstObject];
            let app_support_path: ObjcId = msg_send![app_support_url, path];
            let data_path = nsstring_to_string(app_support_path);
        
            data_path
        }
    }
}
