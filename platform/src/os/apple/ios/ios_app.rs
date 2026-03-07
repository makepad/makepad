use {
    crate::{
        area::Area,
        event::*,
        makepad_math::*,
        os::{
            apple::apple_sys::*,
            cx_native::EventFlow,
            ios::{ios_delegates::*, ios_event::*, ios_text_input::*},
        },
        window::CxWindowPool,
    },
    std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        ffi::c_void,
        rc::Rc,
        time::Instant,
    },
};

// UIKeyboardType
pub const UI_KEYBOARD_TYPE_DEFAULT: i64 = 0;
pub const UI_KEYBOARD_TYPE_ASCII_CAPABLE: i64 = 1;
pub const UI_KEYBOARD_TYPE_URL: i64 = 3;
pub const UI_KEYBOARD_TYPE_NUMBER_PAD: i64 = 4;
pub const UI_KEYBOARD_TYPE_PHONE_PAD: i64 = 5;
pub const UI_KEYBOARD_TYPE_EMAIL_ADDRESS: i64 = 7;
pub const UI_KEYBOARD_TYPE_DECIMAL_PAD: i64 = 8;
pub const UI_KEYBOARD_TYPE_WEB_SEARCH: i64 = 10;

// UITextAutocapitalizationType
pub const UI_TEXT_AUTOCAPITALIZATION_NONE: i64 = 0;
pub const UI_TEXT_AUTOCAPITALIZATION_WORDS: i64 = 1;
pub const UI_TEXT_AUTOCAPITALIZATION_SENTENCES: i64 = 2;
pub const UI_TEXT_AUTOCAPITALIZATION_ALL: i64 = 3;

// UITextAutocorrectionType
pub const UI_TEXT_AUTOCORRECTION_DEFAULT: i64 = 0;
pub const UI_TEXT_AUTOCORRECTION_NO: i64 = 1;
pub const UI_TEXT_AUTOCORRECTION_YES: i64 = 2;

// UIReturnKeyType
pub const UI_RETURN_KEY_DEFAULT: i64 = 0;
pub const UI_RETURN_KEY_GO: i64 = 1;
pub const UI_RETURN_KEY_SEARCH: i64 = 6;
pub const UI_RETURN_KEY_SEND: i64 = 7;
pub const UI_RETURN_KEY_DONE: i64 = 9;

// this value will be fetched from multiple threads (post signal uses it)
pub static mut IOS_CLASSES: *const IosClasses = 0 as *const _;
// this value should not. Todo: guard this somehow proper

thread_local! {
    pub static IOS_APP: RefCell<Option<IosApp>> = RefCell::new(None);
}

pub fn with_ios_app<R>(f: impl FnOnce(&mut IosApp) -> R) -> R {
    IOS_APP.with_borrow_mut(|app| f(app.as_mut().unwrap()))
}

pub fn init_ios_app_global(
    metal_device: ObjcId,
    event_callback: Box<dyn FnMut(IosEvent) -> EventFlow>,
) {
    unsafe {
        IOS_CLASSES = Box::into_raw(Box::new(IosClasses::new()));
        IOS_APP.with(|app| {
            *app.borrow_mut() = Some(IosApp::new(metal_device, event_callback));
        })
    }
}

pub fn get_ios_class_global() -> &'static IosClasses {
    unsafe { &*(IOS_CLASSES) }
}

#[derive(Clone)]
pub struct IosTimer {
    timer_id: u64,
    nstimer: ObjcId,
    repeats: bool,
}

pub struct IosClasses {
    pub app_delegate: *const Class,
    pub view_controller: *const Class,
    pub mtk_view: *const Class,
    pub mtk_view_delegate: *const Class,
    pub gesture_recognizer_handler: *const Class,
    pub selection_handle_gesture_handler: *const Class,
    pub textfield_delegate: *const Class,
    pub timer_delegate: *const Class,
    pub edit_menu_delegate: *const Class,
    // UITextInput protocol classes for IME support
    pub text_position: *const Class,
    pub text_range: *const Class,
    pub text_selection_rect: *const Class,
    pub text_input_view: *const Class,
}
impl IosClasses {
    pub fn new() -> Self {
        Self {
            app_delegate: define_ios_app_delegate(),
            view_controller: define_makepad_view_controller(),
            mtk_view: define_mtk_view(),
            mtk_view_delegate: define_mtk_view_delegate(),
            gesture_recognizer_handler: define_gesture_recognizer_handler(),
            selection_handle_gesture_handler: define_selection_handle_gesture_handler(),
            textfield_delegate: define_textfield_delegate(),
            timer_delegate: define_ios_timer_delegate(),
            edit_menu_delegate: define_edit_menu_interaction_delegate(),
            // All UITextInput classes enabled
            text_position: define_makepad_text_position(),
            text_range: define_makepad_text_range(),
            text_selection_rect: define_makepad_selection_rect(),
            text_input_view: define_text_input_view(),
        }
    }
}

/// Text input events from iOS UITextInput, queued to avoid re-entrancy
#[derive(Debug, Clone)]
pub enum IosTextInputEvent {
    /// Regular text input (input, replace_last)
    TextInput(String, bool),
    /// Range replacement for autocorrect (start, end, text)
    RangeReplace(usize, usize, String),
    /// Key event (e.g., Backspace, Return)
    KeyEvent(KeyCode),
}

pub struct IosApp {
    pub time_start: Instant,
    pub virtual_keyboard_event: Option<VirtualKeyboardEvent>,
    /// Queue of text input events from UITextInput
    /// Using a Vec allows batching multiple events (e.g., replaceRange + insertText)
    /// to be processed atomically before SyncImeState can interfere
    pub queued_text_events: Vec<IosTextInputEvent>,
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
    event_callback: Option<Box<dyn FnMut(IosEvent) -> EventFlow>>,
    event_flow: EventFlow,
    pasteboard: ObjcId,
    edit_menu_delegate_instance: ObjcId,
    edit_menu_interaction: Option<ObjcId>,
    /// Keyboard notification observer delegate - stored for cleanup
    keyboard_observer_delegate: Option<ObjcId>,
    /// Cached keyboard config to avoid redundant reloadInputViews calls
    last_keyboard_config: Option<crate::ime::TextInputConfig>,
    /// Root view controller for status bar / home indicator control
    pub view_controller: Option<ObjcId>,
    /// Native camera preview layers keyed by video_id.
    pub camera_preview_layers: HashMap<u64, ObjcId>,
    /// Selection handles overlayed over the MTK view (iOS 15+ custom implementation).
    selection_handle_start_view: Option<ObjcId>,
    selection_handle_end_view: Option<ObjcId>,
    selection_handle_start_handler: Option<ObjcId>,
    selection_handle_end_handler: Option<ObjcId>,
    /// iOS 16+ runtime-selected native selection display interaction.
    native_selection_display_interaction: Option<ObjcId>,
    has_native_selection_display_api: bool,
}

impl IosApp {
    pub fn new(
        metal_device: ObjcId,
        event_callback: Box<dyn FnMut(IosEvent) -> EventFlow>,
    ) -> IosApp {
        unsafe {
            let pasteboard: ObjcId = msg_send![class!(UIPasteboard), generalPasteboard];
            let edit_menu_delegate_instance: ObjcId =
                msg_send![get_ios_class_global().edit_menu_delegate, new];
            IosApp {
                virtual_keyboard_event: None,
                queued_text_events: Vec::new(),
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
                keyboard_observer_delegate: None,
                last_keyboard_config: None,
                view_controller: None,
                camera_preview_layers: HashMap::new(),
                selection_handle_start_view: None,
                selection_handle_end_view: None,
                selection_handle_start_handler: None,
                selection_handle_end_handler: None,
                native_selection_display_interaction: None,
                has_native_selection_display_api: false,
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

            let mtk_view_dlg_obj: ObjcId =
                msg_send![get_ios_class_global().mtk_view_delegate, alloc];
            let mtk_view_dlg_obj: ObjcId = msg_send![mtk_view_dlg_obj, init];

            // Instantiate a long-press gesture recognizer and our delegate,
            // set that delegate to be the target of the "gesture recognized" action,
            // and add the gesture recognizer to our MTKView subclass.
            let gesture_recognizer_handler_obj: ObjcId =
                msg_send![get_ios_class_global().gesture_recognizer_handler, alloc];
            let gesture_recognizer_handler_obj: ObjcId =
                msg_send![gesture_recognizer_handler_obj, init];
            let gesture_recognizer_obj: ObjcId =
                msg_send![class!(UILongPressGestureRecognizer), alloc];
            let gesture_recognizer_obj: ObjcId = msg_send![
                gesture_recognizer_obj,
                initWithTarget: gesture_recognizer_handler_obj
                action: sel!(handleLongPressGesture: gestureRecognizer:)
            ];
            // Set `cancelsTouchesInView` to NO so that the gesture recognizer doesn't prevent
            // later touch events from being sent to the MTKView *after* it has recognized its gesture.
            let () = msg_send!(gesture_recognizer_obj, setCancelsTouchesInView: NO);
            let () = msg_send![mtk_view_obj, addGestureRecognizer: gesture_recognizer_obj];

            let view_ctrl_obj: ObjcId = msg_send![get_ios_class_global().view_controller, alloc];
            let view_ctrl_obj: ObjcId = msg_send![view_ctrl_obj, init];
            (*view_ctrl_obj).set_ivar::<BOOL>("_prefersStatusBarHidden", NO);
            (*view_ctrl_obj).set_ivar::<BOOL>("_prefersHomeIndicatorAutoHidden", NO);

            let () = msg_send![view_ctrl_obj, setView: mtk_view_obj];

            let () = msg_send![mtk_view_obj, setPreferredFramesPerSecond: 120];
            let () = msg_send![mtk_view_obj, setDelegate: mtk_view_dlg_obj];
            let () = msg_send![mtk_view_obj, setDevice: self.metal_device];
            let () = msg_send![mtk_view_obj, setUserInteractionEnabled: YES];
            let () = msg_send![mtk_view_obj, setAutoResizeDrawable: YES];
            let () = msg_send![mtk_view_obj, setMultipleTouchEnabled: YES];

            let text_input_view: ObjcId = msg_send![get_ios_class_global().text_input_view, alloc];
            let text_input_view: ObjcId = msg_send![text_input_view, initWithFrame: NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: NSSize { width: 1.0, height: 1.0 }
            }];

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
            // Initialize keyboard config ivars with defaults
            (*text_input_view).set_ivar::<i64>("_keyboard_type", UI_KEYBOARD_TYPE_DEFAULT);
            (*text_input_view).set_ivar::<i64>(
                "_autocapitalization_type",
                UI_TEXT_AUTOCAPITALIZATION_SENTENCES,
            );
            (*text_input_view).set_ivar::<i64>("_autocorrection_type", -1); // Use CJK detection logic
            (*text_input_view).set_ivar::<i64>("_return_key_type", UI_RETURN_KEY_DEFAULT);
            (*text_input_view).set_ivar::<bool>("_secure_text_entry", false);
            // Floating cursor (keyboard trackpad) state
            (*text_input_view).set_ivar::<BOOL>("floating_cursor_active", NO);
            (*text_input_view).set_ivar::<f64>("floating_cursor_last_x", 0.0);
            (*text_input_view).set_ivar::<f64>("floating_cursor_last_y", 0.0);
            (*text_input_view).set_ivar::<f64>("selection_handle_start_x", 0.0);
            (*text_input_view).set_ivar::<f64>("selection_handle_start_y", 0.0);
            (*text_input_view).set_ivar::<f64>("selection_handle_end_x", 0.0);
            (*text_input_view).set_ivar::<f64>("selection_handle_end_y", 0.0);
            (*text_input_view).set_ivar::<BOOL>("selection_handles_visible", NO);

            let () = msg_send![text_input_view, setUserInteractionEnabled: YES];
            let () = msg_send![mtk_view_obj, addSubview: text_input_view];

            // iOS 16+: use UITextSelectionDisplayInteraction when available.
            // We keep the custom handle overlay as a fallback and event source.
            let selection_display_cls: ObjcId = makepad_objc_sys::runtime::objc_getClass(
                b"UITextSelectionDisplayInteraction\0".as_ptr() as *const _,
            ) as ObjcId;
            if !selection_display_cls.is_null() {
                let interaction: ObjcId = msg_send![selection_display_cls, alloc];
                if interaction != nil {
                    let can_init: BOOL = msg_send![interaction, respondsToSelector: sel!(initWithTextInput:)];
                    if can_init == YES {
                        let interaction: ObjcId =
                            msg_send![interaction, initWithTextInput: text_input_view];
                        if interaction != nil {
                            let () = msg_send![mtk_view_obj, addInteraction: interaction];
                            self.native_selection_display_interaction = Some(interaction);
                            self.has_native_selection_display_api = true;
                        }
                    }
                }
            }

            // iOS 15+ custom selection handles (fallback and explicit drag surface).
            let selection_handle_start = self.create_selection_handle_view();
            let selection_handle_end = self.create_selection_handle_view();

            let start_handler: ObjcId =
                msg_send![get_ios_class_global().selection_handle_gesture_handler, alloc];
            let start_handler: ObjcId = msg_send![start_handler, init];
            (*start_handler).set_ivar::<i64>("handle_kind", 0);
            let start_pan: ObjcId = msg_send![class!(UIPanGestureRecognizer), alloc];
            let start_pan: ObjcId = msg_send![
                start_pan,
                initWithTarget: start_handler
                action: sel!(handleSelectionHandlePan:)
            ];
            let () = msg_send![selection_handle_start, addGestureRecognizer: start_pan];

            let end_handler: ObjcId =
                msg_send![get_ios_class_global().selection_handle_gesture_handler, alloc];
            let end_handler: ObjcId = msg_send![end_handler, init];
            (*end_handler).set_ivar::<i64>("handle_kind", 1);
            let end_pan: ObjcId = msg_send![class!(UIPanGestureRecognizer), alloc];
            let end_pan: ObjcId = msg_send![
                end_pan,
                initWithTarget: end_handler
                action: sel!(handleSelectionHandlePan:)
            ];
            let () = msg_send![selection_handle_end, addGestureRecognizer: end_pan];

            let () = msg_send![mtk_view_obj, addSubview: selection_handle_start];
            let () = msg_send![mtk_view_obj, addSubview: selection_handle_end];

            self.selection_handle_start_view = Some(selection_handle_start);
            self.selection_handle_end_view = Some(selection_handle_end);
            self.selection_handle_start_handler = Some(start_handler);
            self.selection_handle_end_handler = Some(end_handler);

            // Set up textfield delegate for keyboard notifications only
            let textfield_dlg: ObjcId = msg_send![get_ios_class_global().textfield_delegate, alloc];
            let textfield_dlg: ObjcId = msg_send![textfield_dlg, init];

            let notification_center: ObjcId =
                msg_send![class!(NSNotificationCenter), defaultCenter];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardDidChangeFrame:) name: UIKeyboardDidChangeFrameNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardWillChangeFrame:) name: UIKeyboardWillChangeFrameNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardDidShow:) name: UIKeyboardDidShowNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardWillShow:) name: UIKeyboardWillShowNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardDidHide:) name: UIKeyboardDidHideNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(keyboardWillHide:) name: UIKeyboardWillHideNotification object: nil];
            let () = msg_send![notification_center, addObserver: textfield_dlg selector: sel!(inputModeDidChange:) name: UITextInputCurrentInputModeDidChangeNotification object: nil];

            // Store the delegate for cleanup
            self.keyboard_observer_delegate = Some(textfield_dlg);

            let () = msg_send![window_obj, addSubview: mtk_view_obj];

            let () = msg_send![window_obj, setRootViewController: view_ctrl_obj];
            self.view_controller = Some(view_ctrl_obj);
            let () = msg_send![window_obj, makeKeyAndVisible];

            // Initialize UIEditMenuInteraction for clipboard actions (iOS 16+)
            let edit_menu_cls: ObjcId = makepad_objc_sys::runtime::objc_getClass(
                b"UIEditMenuInteraction\0".as_ptr() as *const _,
            ) as ObjcId;
            if !edit_menu_cls.is_null() {
                // Store MTKView reference in the delegate for accessing menu rect
                (*self.edit_menu_delegate_instance)
                    .set_ivar("mtk_view", mtk_view_obj as *mut c_void);

                // Create UIEditMenuInteraction with our delegate
                let edit_menu_interaction: ObjcId = msg_send![edit_menu_cls, alloc];
                let edit_menu_interaction: ObjcId = msg_send![edit_menu_interaction, initWithDelegate: self.edit_menu_delegate_instance];

                // Add the interaction to the MTKView
                let () = msg_send![mtk_view_obj, addInteraction: edit_menu_interaction];

                self.edit_menu_interaction = Some(edit_menu_interaction);
            }

            self.text_input_view = Some(text_input_view);
            self.mtk_view = Some(mtk_view_obj);
        }
    }

    fn create_selection_handle_view(&self) -> ObjcId {
        unsafe {
            let handle_size = 24.0;
            let handle: ObjcId = msg_send![class!(UIView), alloc];
            let handle: ObjcId = msg_send![handle, initWithFrame: NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: NSSize {
                    width: handle_size,
                    height: handle_size,
                },
            }];
            let color: ObjcId = msg_send![class!(UIColor), systemBlueColor];
            let () = msg_send![handle, setBackgroundColor: color];
            let layer: ObjcId = msg_send![handle, layer];
            let () = msg_send![layer, setCornerRadius: handle_size * 0.5];
            let () = msg_send![handle, setUserInteractionEnabled: YES];
            let () = msg_send![handle, setHidden: YES];
            handle
        }
    }

    pub fn draw_size_will_change(_view: ObjcId, _size: NSSize) {
        // Avoid re-entrant calls by checking if we're already in a with_ios_app call.
        // We must drop the borrow *before* calling check_window_geom, because
        // check_window_geom calls with_ios_app which tries to borrow_mut again.
        let should_call = IOS_APP
            .try_with(|app| {
                match app.try_borrow_mut() {
                    Ok(app_ref) => app_ref.is_some(),
                    Err(_) => false, // already borrowed (re-entrant call), skip
                }
            })
            .unwrap_or(false);
        if should_call {
            Self::check_window_geom();
        }
    }

    pub fn check_window_geom() {
        let main_screen: ObjcId = unsafe { msg_send![class!(UIScreen), mainScreen] };
        let screen_rect: NSRect = unsafe { msg_send![main_screen, bounds] };
        let dpi_factor: f64 = unsafe { msg_send![main_screen, scale] };
        let new_size = dvec2(
            screen_rect.size.width as f64,
            screen_rect.size.height as f64,
        );

        let new_geom = WindowGeom {
            xr_is_presenting: false,
            is_topmost: false,
            is_fullscreen: true,
            can_fullscreen: false,
            inner_size: new_size,
            outer_size: new_size,
            dpi_factor,
            position: dvec2(0.0, 0.0),
        };

        let first_draw = with_ios_app(|app| app.first_draw);
        if first_draw {
            with_ios_app(|app| app.update_geom(new_geom.clone()));
            IosApp::do_callback(IosEvent::Init);
        }

        let old_geom = with_ios_app(|app| app.update_geom(new_geom.clone()));
        if let Some(old_geom) = old_geom {
            IosApp::do_callback(IosEvent::WindowGeomChange(WindowGeomChangeEvent {
                window_id: CxWindowPool::id_zero(),
                old_geom,
                new_geom,
            }));
        }
    }

    fn update_geom(&mut self, new_geom: WindowGeom) -> Option<WindowGeom> {
        if self.first_draw || new_geom != self.last_window_geom {
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

    pub fn update_touch_with_details(
        &mut self,
        uid: u64,
        abs: Vec2d,
        state: TouchState,
        radius: Vec2d,
        force: f64,
    ) {
        if let Some(touch) = self.touches.iter_mut().find(|v| v.uid == uid) {
            touch.state = state;
            touch.abs = abs;
            touch.radius = radius;
            touch.force = force;
        } else {
            self.touches.push(TouchPoint {
                state,
                abs,
                uid,
                time: self.time_now(),
                rotation_angle: 0.0,
                force,
                radius,
                handled: Cell::new(Area::Empty),
                sweep_lock: Cell::new(Area::Empty),
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
            touches,
        }));
        // remove the stopped touches
        with_ios_app(|app| {
            app.touches.retain(|v| {
                if let TouchState::Stop = v.state {
                    false
                } else {
                    true
                }
            })
        });
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

    pub fn metal_device(&self) -> ObjcId {
        self.metal_device
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

    /// Configure keyboard settings (UITextInputTraits)
    /// Uses caching to avoid calling reloadInputViews every frame
    pub fn configure_keyboard(config: &crate::ime::TextInputConfig) {
        use crate::ime::{AutoCapitalize, AutoCorrect, InputMode, ReturnKeyType};

        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    if app.last_keyboard_config.as_ref() == Some(config) {
                        return;
                    }

                    if let Some(text_input_view) = app.text_input_view {
                        unsafe {
                            let kb_type: i64 = match config.soft_keyboard.input_mode {
                                InputMode::Text => UI_KEYBOARD_TYPE_DEFAULT,
                                InputMode::Ascii => UI_KEYBOARD_TYPE_ASCII_CAPABLE,
                                InputMode::Url => UI_KEYBOARD_TYPE_URL,
                                InputMode::Numeric => UI_KEYBOARD_TYPE_NUMBER_PAD,
                                InputMode::Tel => UI_KEYBOARD_TYPE_PHONE_PAD,
                                InputMode::Email => UI_KEYBOARD_TYPE_EMAIL_ADDRESS,
                                InputMode::Decimal => UI_KEYBOARD_TYPE_DECIMAL_PAD,
                                InputMode::Search => UI_KEYBOARD_TYPE_WEB_SEARCH,
                            };

                            let autocap_type: i64 = match config.soft_keyboard.autocapitalize {
                                AutoCapitalize::None => UI_TEXT_AUTOCAPITALIZATION_NONE,
                                AutoCapitalize::Words => UI_TEXT_AUTOCAPITALIZATION_WORDS,
                                AutoCapitalize::Sentences => UI_TEXT_AUTOCAPITALIZATION_SENTENCES,
                                AutoCapitalize::AllCharacters => UI_TEXT_AUTOCAPITALIZATION_ALL,
                            };

                            let autocorrect_type: i64 = match config.soft_keyboard.autocorrect {
                                AutoCorrect::Default => -1,
                                AutoCorrect::Disabled => UI_TEXT_AUTOCORRECTION_NO,
                                AutoCorrect::Enabled => UI_TEXT_AUTOCORRECTION_YES,
                            };

                            let return_type: i64 = match config.soft_keyboard.return_key_type {
                                ReturnKeyType::Default => UI_RETURN_KEY_DEFAULT,
                                ReturnKeyType::Go => UI_RETURN_KEY_GO,
                                ReturnKeyType::Search => UI_RETURN_KEY_SEARCH,
                                ReturnKeyType::Send => UI_RETURN_KEY_SEND,
                                ReturnKeyType::Done => UI_RETURN_KEY_DONE,
                            };

                            (*text_input_view).set_ivar::<i64>("_keyboard_type", kb_type);
                            (*text_input_view)
                                .set_ivar::<i64>("_autocapitalization_type", autocap_type);
                            (*text_input_view)
                                .set_ivar::<i64>("_autocorrection_type", autocorrect_type);
                            (*text_input_view).set_ivar::<i64>("_return_key_type", return_type);
                            (*text_input_view)
                                .set_ivar::<bool>("_secure_text_entry", config.is_secure);

                            let () = msg_send![text_input_view, reloadInputViews];
                        }
                    }

                    app.last_keyboard_config = Some(*config);
                }
            }
        });
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
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
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

    pub fn set_ime_text(text: String, cursor: usize) {
        // Convert character cursor index to UTF-16 code units for NSString indexing.
        let cursor_utf16_pos: usize = text.chars().take(cursor).map(|c| c.len_utf16()).sum();

        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    if let Some(text_input_view) = app.text_input_view {
                        unsafe {
                            // Get inputDelegate for notifications - this is critical for iOS
                            // to know the text/cursor has changed (needed for autocorrect positioning)
                            let input_delegate: ObjcId =
                                *(*text_input_view).get_ivar("_inputDelegate");

                            // Notify BEFORE changes
                            if input_delegate != nil {
                                let () = msg_send![input_delegate, textWillChange: text_input_view];
                                let () =
                                    msg_send![input_delegate, selectionWillChange: text_input_view];
                            }

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
                                let range = NSRange {
                                    location: 0,
                                    length: len,
                                };
                                let () = msg_send![buffer, deleteCharactersInRange: range];
                            }

                            // Set new content
                            let ns_text = str_to_nsstring(&text);
                            let () = msg_send![buffer, appendString: ns_text];

                            // Set cursor position and selection (UTF-16 index)
                            (*text_input_view).set_ivar("cursorPosition", cursor_utf16_pos as i64);
                            (*text_input_view).set_ivar("selectionStart", cursor_utf16_pos as i64);
                            (*text_input_view).set_ivar("selectionEnd", cursor_utf16_pos as i64);

                            // Notify AFTER changes (CRITICAL for autocorrect positioning)
                            if input_delegate != nil {
                                let () =
                                    msg_send![input_delegate, selectionDidChange: text_input_view];
                                let () = msg_send![input_delegate, textDidChange: text_input_view];
                            }
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
                let () = unsafe { msg_send![mtk_view, setPaused: YES] };
            } else {
                let () = unsafe { msg_send![mtk_view, setPaused: NO] };
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
                repeats: repeats,
            });
            let () = msg_send![pool, release];
        }
    }

    pub fn queue_virtual_keyboard_event(&mut self, event: VirtualKeyboardEvent) {
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
        // Queue text input - will be processed on next timer tick
        // Using a Vec queue allows batching multiple events (e.g., autocorrect + space)
        // This avoids re-entrancy issues from UITextInput delegate callbacks
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    app.queued_text_events
                        .push(IosTextInputEvent::TextInput(input, replace_last));
                }
            }
        });
    }

    pub fn send_text_range_replace(start: usize, end: usize, text: String) {
        // Queue range replacement for iOS autocorrect
        // Using a Vec queue allows batching with subsequent insertText calls
        // This avoids re-entrancy issues from UITextInput delegate callbacks
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    app.queued_text_events
                        .push(IosTextInputEvent::RangeReplace(start, end, text));
                }
            }
        });
    }

    pub fn send_backspace() {
        // Queue backspace key event
        // This avoids re-entrancy issues from UITextInput delegate callbacks
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    app.queued_text_events
                        .push(IosTextInputEvent::KeyEvent(KeyCode::Backspace));
                }
            }
        });
    }

    pub fn send_return_key() {
        // Queue Return key event
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    app.queued_text_events
                        .push(IosTextInputEvent::KeyEvent(KeyCode::ReturnKey));
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
                IosApp::do_callback(IosEvent::Timer(TimerEvent {
                    timer_id: timer_id,
                    time: Some(time),
                }));
                return;
            }
        }
    }

    pub fn send_paint_event() {
        IosApp::do_callback(IosEvent::Paint);
    }

    pub fn set_fullscreen(&mut self, fullscreen: bool) {
        if let Some(vc) = self.view_controller {
            unsafe {
                let val = if fullscreen { YES } else { NO };
                (*vc).set_ivar::<BOOL>("_prefersStatusBarHidden", val);
                (*vc).set_ivar::<BOOL>("_prefersHomeIndicatorAutoHidden", val);
                let () = msg_send![vc, setNeedsStatusBarAppearanceUpdate];
                let () = msg_send![vc, setNeedsUpdateOfHomeIndicatorAutoHidden];
            }
        }
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
        let views = IOS_APP
            .try_with(|app| {
                if let Ok(app_ref) = app.try_borrow_mut() {
                    if let Some(ref app) = *app_ref {
                        return Some((app.mtk_view, app.edit_menu_interaction));
                    }
                }
                None
            })
            .ok()
            .flatten();

        let Some((Some(mtk_view), edit_menu_interaction)) = views else {
            return;
        };

        unsafe {
            // Store selection state in the view for canPerformAction filtering
            let has_sel: BOOL = if has_selection { YES } else { NO };
            (*mtk_view).set_ivar::<BOOL>("has_selection", has_sel);

            // Store the menu rect in the view for the delegate's targetRectForConfiguration
            (*mtk_view).set_ivar::<f64>("menu_rect_x", rect.pos.x);
            (*mtk_view).set_ivar::<f64>("menu_rect_y", rect.pos.y);
            (*mtk_view).set_ivar::<f64>("menu_rect_width", rect.size.x.max(1.0));
            (*mtk_view).set_ivar::<f64>("menu_rect_height", rect.size.y.max(1.0));

            if let Some(edit_menu_interaction) = edit_menu_interaction {
                // iOS 16+: UIEditMenuInteraction
                let source_point = NSPoint {
                    x: rect.pos.x + rect.size.x / 2.0,
                    y: rect.pos.y + rect.size.y / 2.0,
                };
                let config: ObjcId = msg_send![
                    class!(UIEditMenuConfiguration),
                    configurationWithIdentifier: nil
                    sourcePoint: source_point
                ];
                let () = msg_send![edit_menu_interaction, presentEditMenuWithConfiguration: config];
            } else {
                // iOS 15: UIMenuController fallback
                let menu_controller: ObjcId =
                    msg_send![class!(UIMenuController), sharedMenuController];
                let target_rect = NSRect {
                    origin: NSPoint {
                        x: rect.pos.x,
                        y: rect.pos.y,
                    },
                    size: NSSize {
                        width: rect.size.x.max(1.0),
                        height: rect.size.y.max(1.0),
                    },
                };
                let () = msg_send![mtk_view, becomeFirstResponder];
                let () = msg_send![menu_controller, setTargetRect: target_rect inView: mtk_view];
                let () = msg_send![menu_controller, setMenuVisible: YES animated: YES];
            }
        }
    }

    pub fn hide_clipboard_actions() {
        // Extract what we need first, then do ObjC calls after borrow ends
        let state = IOS_APP
            .try_with(|app| {
                if let Ok(app_ref) = app.try_borrow_mut() {
                    if let Some(ref app) = *app_ref {
                        return Some(app.edit_menu_interaction);
                    }
                }
                None
            })
            .ok()
            .flatten();

        let Some(edit_menu_interaction) = state else {
            return;
        };

        unsafe {
            if let Some(edit_menu_interaction) = edit_menu_interaction {
                // iOS 16+
                let () = msg_send![edit_menu_interaction, dismissMenu];
            } else {
                // iOS 15: UIMenuController fallback
                let menu_controller: ObjcId =
                    msg_send![class!(UIMenuController), sharedMenuController];
                let () = msg_send![menu_controller, setMenuVisible: NO animated: YES];
            }
        }
    }

    fn update_native_selection_display(start: DVec2, end: DVec2, visible: bool) {
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    if !app.has_native_selection_display_api {
                        return;
                    }
                    let Some(text_input_view) = app.text_input_view else {
                        return;
                    };
                    unsafe {
                        (*text_input_view).set_ivar::<f64>("selection_handle_start_x", start.x);
                        (*text_input_view).set_ivar::<f64>("selection_handle_start_y", start.y);
                        (*text_input_view).set_ivar::<f64>("selection_handle_end_x", end.x);
                        (*text_input_view).set_ivar::<f64>("selection_handle_end_y", end.y);
                        (*text_input_view).set_ivar::<BOOL>(
                            "selection_handles_visible",
                            if visible { YES } else { NO },
                        );

                        // UITextSelectionDisplayInteraction listens via the input delegate.
                        let input_delegate: ObjcId = *(*text_input_view).get_ivar("_inputDelegate");
                        if input_delegate != nil {
                            let () = msg_send![input_delegate, selectionWillChange: text_input_view];
                            let () = msg_send![input_delegate, selectionDidChange: text_input_view];
                        }

                        if let Some(interaction) = app.native_selection_display_interaction {
                            let has_refresh: BOOL =
                                msg_send![interaction, respondsToSelector: sel!(setNeedsSelectionUpdate)];
                            if has_refresh == YES {
                                let () = msg_send![interaction, setNeedsSelectionUpdate];
                            }
                        }
                    }
                }
            }
        });
    }

    fn set_selection_handle_center(handle: ObjcId, center: DVec2) {
        unsafe {
            let () = msg_send![
                handle,
                setCenter: NSPoint {
                    x: center.x,
                    y: center.y,
                }
            ];
        }
    }

    pub fn show_selection_handles(start: DVec2, end: DVec2) {
        Self::update_native_selection_display(start, end, true);
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    if let Some(start_view) = app.selection_handle_start_view {
                        Self::set_selection_handle_center(start_view, start);
                        unsafe {
                            let () = msg_send![start_view, setHidden: NO];
                            let parent: ObjcId = msg_send![start_view, superview];
                            if parent != nil {
                                let () = msg_send![parent, bringSubviewToFront: start_view];
                            }
                        }
                    }
                    if let Some(end_view) = app.selection_handle_end_view {
                        Self::set_selection_handle_center(end_view, end);
                        unsafe {
                            let () = msg_send![end_view, setHidden: NO];
                            let parent: ObjcId = msg_send![end_view, superview];
                            if parent != nil {
                                let () = msg_send![parent, bringSubviewToFront: end_view];
                            }
                        }
                    }
                }
            }
        });
    }

    pub fn update_selection_handles(start: DVec2, end: DVec2) {
        Self::update_native_selection_display(start, end, true);
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    if let Some(start_view) = app.selection_handle_start_view {
                        Self::set_selection_handle_center(start_view, start);
                    }
                    if let Some(end_view) = app.selection_handle_end_view {
                        Self::set_selection_handle_center(end_view, end);
                    }
                }
            }
        });
    }

    pub fn hide_selection_handles() {
        Self::update_native_selection_display(dvec2(0.0, 0.0), dvec2(0.0, 0.0), false);
        let _ = IOS_APP.try_with(|app| {
            if let Ok(mut app_ref) = app.try_borrow_mut() {
                if let Some(ref mut app) = *app_ref {
                    if let Some(start_view) = app.selection_handle_start_view {
                        unsafe {
                            let () = msg_send![start_view, setHidden: YES];
                        }
                    }
                    if let Some(end_view) = app.selection_handle_end_view {
                        unsafe {
                            let () = msg_send![end_view, setHidden: YES];
                        }
                    }
                }
            }
        });
    }

    pub fn send_selection_handle_drag(
        handle: SelectionHandleKind,
        phase: SelectionHandlePhase,
        abs: DVec2,
    ) {
        let time = IOS_APP
            .try_with(|app| {
                if let Ok(mut app_ref) = app.try_borrow_mut() {
                    if let Some(ref mut app) = *app_ref {
                        return Some(app.time_now());
                    }
                }
                None
            })
            .ok()
            .flatten();

        let Some(time) = time else {
            return;
        };

        IosApp::do_callback(IosEvent::SelectionHandleDrag(SelectionHandleDragEvent {
            handle,
            phase,
            abs,
            time,
        }));
    }

    // Action dispatch methods called from MakepadView's action handlers
    pub fn send_clipboard_action(action: &str) {
        match action {
            "copy" => {
                let response = Rc::new(RefCell::new(None));
                IosApp::do_callback(IosEvent::TextCopy(TextClipboardEvent {
                    response: response.clone(),
                }));
                // After the event handler fills in the response, copy to clipboard
                let text_to_copy = response.borrow().clone();
                if let Some(text) = text_to_copy {
                    with_ios_app(|app| app.copy_to_clipboard(&text));
                }
            }
            "cut" => {
                let response = Rc::new(RefCell::new(None));
                IosApp::do_callback(IosEvent::TextCut(TextClipboardEvent {
                    response: response.clone(),
                }));
                // After the event handler fills in the response, copy to clipboard
                let text_to_copy = response.borrow().clone();
                if let Some(text) = text_to_copy {
                    with_ios_app(|app| app.copy_to_clipboard(&text));
                }
            }
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
            }
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
                ..Default::default()
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
