use {
    std::{
        cell::{Cell, RefCell},
        ffi::c_void,
        rc::Rc,
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
    pub edit_menu_delegate: *const Class,
    // UITextInput protocol classes for IME support
    pub text_position: *const Class,
    pub text_range: *const Class,
    pub text_input_view: *const Class,
}
impl IosClasses {
    pub fn new() -> Self {
        Self {
            app_delegate: define_ios_app_delegate(),
            mtk_view: define_mtk_view(),
            mtk_view_delegate: define_mtk_view_delegate(),
            gesture_recognizer_handler: define_gesture_recognizer_handler(),
            textfield_delegate: define_textfield_delegate(),
            timer_delegate: define_ios_timer_delegate(),
            edit_menu_delegate: define_edit_menu_interaction_delegate(),
            // All UITextInput classes enabled
            text_position: define_makepad_text_position(),
            text_range: define_makepad_text_range(),
            text_input_view: define_text_input_view(),
        }
    }
}

pub struct IosApp {
    pub time_start: Instant,
    pub virtual_keyboard_event:  Option<VirtualKeyboardEvent>,
    /// Queued text input from UITextInput
    /// (input, replace_last)
    pub queued_text_input: Option<(String, bool)>,
    /// Queued text range replace from UITextInput
    /// (start, end, text)
    pub queued_text_range_replace: Option<(usize, usize, String)>,
    /// Queued key event from UITextInput
    pub queued_key_event: Option<KeyCode>,
    pub timer_delegate_instance: ObjcId,
    timers: Vec<IosTimer>,
    touches: Vec<TouchPoint>,
    pub last_window_geom: WindowGeom,
    metal_device: ObjcId,
    first_draw: bool,
    pub mtk_view: Option<ObjcId>,
    /// UITextInput view for IME support
    pub text_input_view: Option<ObjcId>,
    /// IME candidate window position
    pub ime_position: Option<DVec2>,
    event_callback: Option<Box<dyn FnMut(IosEvent) -> EventFlow >>,
    event_flow: EventFlow,
    pasteboard: ObjcId,
    edit_menu_delegate_instance: ObjcId,
    edit_menu_interaction: Option<ObjcId>,
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
            let edit_menu_delegate_instance: ObjcId = msg_send![get_ios_class_global().edit_menu_delegate, new];
            IosApp {
                virtual_keyboard_event: None,
                queued_text_input: None,
                queued_text_range_replace: None,
                queued_key_event: None,
                touches: Vec::new(),
                last_window_geom: WindowGeom::default(),
                metal_device,
                first_draw: true,
                mtk_view: None,
                text_input_view: None,
                ime_position: None,
                time_start: Instant::now(),
                timer_delegate_instance: msg_send![get_ios_class_global().timer_delegate, new],
                timers: Vec::new(),
                event_flow: EventFlow::Poll,
                event_callback: Some(event_callback),
                pasteboard,
                edit_menu_delegate_instance,
                edit_menu_interaction: None,
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
            
            // Create UITextInput view for proper IME support
            let text_input_view: ObjcId = msg_send![get_ios_class_global().text_input_view, alloc];
            let text_input_view: ObjcId = msg_send![text_input_view, initWithFrame: NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: NSSize { width: 1.0, height: 1.0 }
            }];

            // Initialize ivars
            let marked_text: ObjcId = msg_send![class!(NSMutableAttributedString), alloc];
            let marked_text: ObjcId = msg_send![marked_text, init];
            (*text_input_view).set_ivar::<ObjcId>("markedText", marked_text);
            (*text_input_view).set_ivar::<i64>("cursorPosition", 0);
            (*text_input_view).set_ivar::<i64>("selectionStart", 0);
            (*text_input_view).set_ivar::<i64>("selectionEnd", 0);
            (*text_input_view).set_ivar::<ObjcId>("_inputDelegate", nil);
            (*text_input_view).set_ivar::<ObjcId>("_tokenizer", nil);
            (*text_input_view).set_ivar::<f64>("ime_pos_x", 0.0);
            (*text_input_view).set_ivar::<f64>("ime_pos_y", 0.0);

            let () = msg_send![text_input_view, setUserInteractionEnabled: YES];
            let () = msg_send![mtk_view_obj, addSubview: text_input_view];

            // Set up textfield delegate for keyboard notifications only
            let textfield_dlg: ObjcId = msg_send![get_ios_class_global().textfield_delegate, alloc];
            let textfield_dlg: ObjcId = msg_send![textfield_dlg, init];
            
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

            // Initialize UIEditMenuInteraction for clipboard actions
            // Store MTKView reference in the delegate for accessing menu rect
            (*self.edit_menu_delegate_instance).set_ivar("mtk_view", mtk_view_obj as *mut c_void);

            // Create UIEditMenuInteraction with our delegate
            let edit_menu_interaction: ObjcId = msg_send![class!(UIEditMenuInteraction), alloc];
            let edit_menu_interaction: ObjcId = msg_send![edit_menu_interaction, initWithDelegate: self.edit_menu_delegate_instance];

            // Add the interaction to the MTKView
            let () = msg_send![mtk_view_obj, addInteraction: edit_menu_interaction];

            self.text_input_view = Some(text_input_view);
            self.mtk_view = Some(mtk_view_obj);
            self.edit_menu_interaction = Some(edit_menu_interaction);
        }
    }
    
    pub fn draw_size_will_change() {
        // Avoid re-entrant calls by checking if we're already in a with_ios_app call
        if IOS_APP.try_with(|app| {
            if let Ok(app_ref) = app.try_borrow_mut() {
                if app_ref.is_some() {
                    Self::check_window_geom();
                }
                // Otherwise we skip the call, should be safe since draw_size_will_change is called again afterwards
            }
        }).is_err() {
            // IOS_APP is not accessible on this thread, ignore the call (this shouldn't happen)
        }
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
    
    pub fn update_touch(&mut self, uid: u64, abs: Vec2d, state: TouchState) {
        self.update_touch_with_details(uid, abs, state, dvec2(0.0, 0.0), 0.0);
    }

    pub fn update_touch_with_details(&mut self, uid: u64, abs: Vec2d, state: TouchState, radius: Vec2d, force: f64) {
        if let Some(touch) = self.touches.iter_mut().find( | v | v.uid == uid) {
            touch.state = state;
            touch.abs = abs;
            touch.radius = radius;
            touch.force = force;
        }
        else {
            self.touches.push(TouchPoint {
                state,
                abs,
                uid,
                time: self.time_now(),
                rotation_angle: 0.0,
                force,
                radius,
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
        // Use text_input_view for keyboard (UITextInput protocol)
        let _ = IOS_APP.try_with(|app| {
            if let Ok(app_ref) = app.try_borrow_mut() {
                if let Some(ref app) = *app_ref {
                    if let Some(text_input_view) = app.text_input_view {
                        let () = unsafe { msg_send![text_input_view, becomeFirstResponder] };
                    }
                }
            }
        });
    }

    pub fn hide_keyboard() {
        // Use text_input_view for keyboard
        let _ = IOS_APP.try_with(|app| {
            if let Ok(app_ref) = app.try_borrow_mut() {
                if let Some(ref app) = *app_ref {
                    if let Some(text_input_view) = app.text_input_view {
                        let () = unsafe { msg_send![text_input_view, resignFirstResponder] };
                    }
                }
            }
        });
    }

    pub fn set_ime_position(pos: DVec2) {
        // Avoid re-entrant calls by checking if we're already in a with_ios_app call
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    app.ime_position = Some(pos);
                    // Also set ivars directly on the text_input_view to avoid re-entrant borrow issues
                    // when UITextInput callbacks access the position
                    if let Some(text_input_view) = app.text_input_view {
                        unsafe {
                            (*text_input_view).set_ivar::<f64>("ime_pos_x", pos.x);
                            (*text_input_view).set_ivar::<f64>("ime_pos_y", pos.y);
                        }
                    }
                }
            }
        });
    }

    pub fn set_ime_text(text: String, cursor_byte_pos: usize) {
        // Convert byte position to UTF-16 code unit position for iOS
        // cursor_byte_pos is a UTF-8 byte index, iOS NSString uses UTF-16 internally
        let cursor_char_pos = text[..cursor_byte_pos.min(text.len())]
            .encode_utf16()
            .count();

        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    if let Some(text_input_view) = app.text_input_view {
                        unsafe {
                            // Get or create text buffer
                            let buffer: ObjcId = *(*text_input_view).get_ivar("textBuffer");
                            let buffer = if buffer != nil {
                                buffer
                            } else {
                                let new_buffer: ObjcId = msg_send![class!(NSMutableString), alloc];
                                let new_buffer: ObjcId = msg_send![new_buffer, init];
                                (*text_input_view).set_ivar("textBuffer", new_buffer);
                                new_buffer
                            };

                            // Clear existing content
                            let len: u64 = msg_send![buffer, length];
                            if len > 0 {
                                let range = NSRange { location: 0, length: len };
                                let () = msg_send![buffer, deleteCharactersInRange: range];
                            }

                            // Set new content
                            let ns_text = str_to_nsstring(&text);
                            let () = msg_send![buffer, appendString: ns_text];

                            // Set cursor position (in characters, not bytes)
                            (*text_input_view).set_ivar("cursorPosition", cursor_char_pos as i64);
                        }
                    }
                }
            }
        });
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
        // Always queue - will be processed on next timer tick
        // This avoids re-entrancy issues from UITextField delegate callbacks
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    app.queued_text_input = Some((input, replace_last));
                }
            }
        });
    }

    pub fn send_text_range_replace(start: usize, end: usize, text: String) {
        // Queue range replacement for iOS autocorrect
        // This avoids re-entrancy issues from UITextInput delegate callbacks
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    app.queued_text_range_replace = Some((start, end, text));
                }
            }
        });
    }

    pub fn send_backspace() {
        // Always queue - will be processed on next timer tick
        // This avoids re-entrancy issues from UITextField delegate callbacks
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    app.queued_key_event = Some(KeyCode::Backspace);
                }
            }
        });
    }

    pub fn send_return_key() {
        // Queue Return key event
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    app.queued_key_event = Some(KeyCode::ReturnKey);
                }
            }
        });
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

    pub fn paste_from_clipboard(&self) -> String {
        unsafe {
            let pasteboard: ObjcId = self.pasteboard;
            let nsstring: ObjcId = msg_send![pasteboard, string];
            if nsstring != nil {
                nsstring_to_string(nsstring)
            } else {
                String::new()
            }
        }
    }

    pub fn show_clipboard_actions(has_selection: bool, rect: Rect, _keyboard_shift: f64) {
        // Extract what we need from IosApp first, then do ObjC calls AFTER the borrow ends
        // This avoids re-entrant borrow panics when UIKit triggers keyboard notifications
        let views = IOS_APP.try_with(|app| {
            if let Ok(app_ref) = app.try_borrow_mut() {
                if let Some(ref app) = *app_ref {
                    if let (Some(mtk_view), Some(edit_menu_interaction)) = (app.mtk_view, app.edit_menu_interaction) {
                        return Some((mtk_view, edit_menu_interaction));
                    }
                }
            }
            None
        }).ok().flatten();

        let Some((mtk_view, edit_menu_interaction)) = views else { return };

        unsafe {
            // Store selection state in the view for canPerformAction filtering
            let has_sel: BOOL = if has_selection { YES } else { NO };
            (*mtk_view).set_ivar::<BOOL>("has_selection", has_sel);

            // Store the menu rect in the view for the delegate's targetRectForConfiguration
            (*mtk_view).set_ivar::<f64>("menu_rect_x", rect.pos.x);
            (*mtk_view).set_ivar::<f64>("menu_rect_y", rect.pos.y);
            (*mtk_view).set_ivar::<f64>("menu_rect_width", rect.size.x.max(1.0));
            (*mtk_view).set_ivar::<f64>("menu_rect_height", rect.size.y.max(1.0));

            // Create configuration with source point at center of the rect
            let source_point = NSPoint {
                x: rect.pos.x + rect.size.x / 2.0,
                y: rect.pos.y + rect.size.y / 2.0,
            };
            let config: ObjcId = msg_send![
                class!(UIEditMenuConfiguration),
                configurationWithIdentifier: nil
                sourcePoint: source_point
            ];

            // Present the edit menu - this may trigger keyboard notifications,
            // but now we're not holding any borrow so it's safe
            let () = msg_send![edit_menu_interaction, presentEditMenuWithConfiguration: config];
        }
    }

    pub fn hide_clipboard_actions() {
        // Extract what we need first, then do ObjC calls after borrow ends
        let interaction = IOS_APP.try_with(|app| {
            if let Ok(app_ref) = app.try_borrow_mut() {
                if let Some(ref app) = *app_ref {
                    return app.edit_menu_interaction;
                }
            }
            None
        }).ok().flatten();

        if let Some(edit_menu_interaction) = interaction {
            unsafe {
                let () = msg_send![edit_menu_interaction, dismissMenu];
            }
        }
    }

    // Action dispatch methods called from MakepadView's action handlers
    pub fn send_clipboard_action(action: &str) {
        match action {
            "copy" => {
                let response = Rc::new(RefCell::new(None));
                IosApp::do_callback(IosEvent::TextCopy(TextClipboardEvent {
                    response: response.clone()
                }));
                // After the event handler fills in the response, copy to clipboard
                let text_to_copy = response.borrow().clone();
                if let Some(text) = text_to_copy {
                    with_ios_app(|app| app.copy_to_clipboard(&text));
                }
            },
            "cut" => {
                let response = Rc::new(RefCell::new(None));
                IosApp::do_callback(IosEvent::TextCut(TextClipboardEvent {
                    response: response.clone()
                }));
                // After the event handler fills in the response, copy to clipboard
                let text_to_copy = response.borrow().clone();
                if let Some(text) = text_to_copy {
                    with_ios_app(|app| app.copy_to_clipboard(&text));
                }
            },
            "select_all" => {
                // Send Cmd+A keypress to trigger select all in widgets
                // On Apple platforms, is_primary() checks for logo (Command)
                let time = with_ios_app(|app| app.time_now());
                IosApp::do_callback(IosEvent::KeyDown(KeyEvent {
                    key_code: KeyCode::KeyA,
                    is_repeat: false,
                    modifiers: KeyModifiers {
                        shift: false,
                        control: false,
                        alt: false,
                        logo: true,
                    },
                    time,
                }));
            },
            _ => {
                crate::log!("iOS: Unknown clipboard action: {}", action);
            }
        }
    }

    pub fn send_clipboard_paste() {
        let content = with_ios_app(|app| app.paste_from_clipboard());
        if !content.is_empty() {
            IosApp::do_callback(IosEvent::TextInput(TextInputEvent {
                input: content,
                replace_last: false,
                was_paste: true,
            }));
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
