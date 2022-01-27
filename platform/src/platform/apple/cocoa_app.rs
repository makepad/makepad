
use {
    std::{
        ptr,
        time::Instant,
        collections::HashMap,
        os::raw::{c_void}
    },
    crate::{
        makepad_math::{
            Vec2,
        },
        platform::{
            apple::frameworks::*,
            cocoa_delegate::*,
            cocoa_window::CocoaWindow,
            apple_util::{
                nsstring_to_string,
                str_to_nsstring,
                keycode_to_menu_key,
                get_event_keycode,
                get_event_key_modifier
            },
        },
        menu::{
            CxCommandSetting
        },
        //turtle::{
        //    Rect
        //},
        event::{
            Event,
            KeyCode,
            KeyEvent,
            TextInputEvent,
            TextCopyEvent,
            FingerInputType,
            FingerScrollEvent,
            TimerEvent,
            Signal,
            SignalEvent,
            DraggedItem,
            KeyModifiers
        },
        cursor::MouseCursor,
        menu::{
            Menu,
            CommandId
        }
    }
};

static mut GLOBAL_COCOA_APP: *mut CocoaApp = 0 as *mut _;

#[derive(Clone)]
pub struct CocoaTimer {
    timer_id: u64,
    nstimer: ObjcId,
    repeats: bool
}

pub struct CocoaApp {
    pub window_class: *const Class,
    pub window_delegate_class: *const Class,
    pub post_delegate_class: *const Class,
    pub timer_delegate_class: *const Class,
    pub menu_delegate_class: *const Class,
    pub app_delegate_class: *const Class,
    pub menu_target_class: *const Class,
    pub view_class: *const Class,
    pub menu_delegate_instance: ObjcId,
    pub app_delegate_instance: ObjcId,
    pub const_attributes_for_marked_text: ObjcId,
    pub const_empty_string: RcObjcId,
    pub time_start: Instant,
    pub timer_delegate_instance: ObjcId,
    pub timers: Vec<CocoaTimer>,
    pub cocoa_windows: Vec<(ObjcId, ObjcId)>,
    pub last_key_mod: KeyModifiers,
    pub pasteboard: ObjcId,
    pub startup_focus_hack_ran: bool,
    pub event_callback: Option<*mut dyn FnMut(&mut CocoaApp, &mut Vec<Event>) -> bool>,
    pub event_recur_block: bool,
    pub event_loop_running: bool,
    pub loop_block: bool,
    pub cursors: HashMap<MouseCursor, ObjcId>,
    pub current_cursor: MouseCursor,
    //pub status_map: Mutex<CocoaStatusMap>,
    pub ns_event: ObjcId,
}
/*
#[derive(Default)]
pub struct CocoaStatusMap {
    pub status_to_usize: HashMap<StatusId, usize>,
    pub usize_to_status: HashMap<usize, StatusId>,
    pub command_to_usize: HashMap<CommandId, usize>,
    pub usize_to_command: HashMap<usize, CommandId>,
}
*/
impl CocoaApp {
    pub fn new() -> CocoaApp {
        unsafe {
            
            let timer_delegate_class = define_cocoa_timer_delegate();
            let timer_delegate_instance: ObjcId = msg_send![timer_delegate_class, new];
            let menu_delegate_class = define_menu_delegate();
            let menu_delegate_instance: ObjcId = msg_send![menu_delegate_class, new];
            let app_delegate_class = define_app_delegate();
            let app_delegate_instance: ObjcId = msg_send![app_delegate_class, new];
            
            let const_attributes = vec![
                RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSMarkedClauseSegment")).unwrap()).forget(),
                RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSGlyphInfo")).unwrap()).forget(),
            ];
            
            // Construct the bits that are shared between windows
            CocoaApp {
                const_attributes_for_marked_text: msg_send![
                    class!(NSArray),
                    arrayWithObjects: const_attributes.as_ptr()
                    count: const_attributes.len()
                ],
                startup_focus_hack_ran: false,
                const_empty_string: RcObjcId::from_unowned(NonNull::new(str_to_nsstring("")).unwrap()),
                pasteboard: msg_send![class!(NSPasteboard), generalPasteboard],
                time_start: Instant::now(),
                timer_delegate_instance: timer_delegate_instance,
                timer_delegate_class: timer_delegate_class,
                post_delegate_class: define_cocoa_post_delegate(),
                window_class: define_cocoa_window_class(),
                window_delegate_class: define_cocoa_window_delegate(),
                view_class: define_cocoa_view_class(),
                menu_target_class: define_menu_target_class(),
                menu_delegate_class,
                menu_delegate_instance,
                app_delegate_class,
                app_delegate_instance,
                timers: Vec::new(),
                cocoa_windows: Vec::new(),
                loop_block: false,
                last_key_mod: KeyModifiers {..Default::default()},
                event_callback: None,
                event_recur_block: false,
                event_loop_running: true,
                cursors: HashMap::new(),
                //status_map: Mutex::new(CocoaStatusMap::default()),
                current_cursor: MouseCursor::Default,
                ns_event: ptr::null_mut(),
            }
        }
    }
    
    pub fn update_app_menu(&mut self, menu: &Menu, command_settings: &HashMap<CommandId, CxCommandSetting>,) {
        unsafe fn make_menu(
            parent_menu: ObjcId,
            delegate: ObjcId,
            menu_target_class: *const Class,
            menu: &Menu,
            //status_map: &Mutex<CocoaStatusMap>,
            command_settings: &HashMap<CommandId, CxCommandSetting>
        ) {
            match menu {
                Menu::Main {items} => {
                    let main_menu: ObjcId = msg_send![class!(NSMenu), new];
                    let () = msg_send![main_menu, setTitle: str_to_nsstring("MainMenu")];
                    let () = msg_send![main_menu, setAutoenablesItems: NO];
                    let () = msg_send![main_menu, setDelegate: delegate];
                    
                    for item in items {
                        make_menu(main_menu, delegate, menu_target_class, item, command_settings);
                    }
                    let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                    let () = msg_send![
                        ns_app,
                        setMainMenu: main_menu
                    ];
                },
                Menu::Sub {name, items} => {
                    let sub_menu: ObjcId = msg_send![class!(NSMenu), new];
                    let () = msg_send![sub_menu, setTitle: str_to_nsstring(name)];
                    let () = msg_send![sub_menu, setAutoenablesItems: NO];
                    let () = msg_send![sub_menu, setDelegate: delegate];
                    // append item to parebt
                    let sub_item: ObjcId = msg_send![
                        parent_menu,
                        addItemWithTitle: str_to_nsstring(name)
                        action: nil
                        keyEquivalent: str_to_nsstring("")
                    ];
                    // connect submenu
                    let () = msg_send![parent_menu, setSubmenu: sub_menu forItem: sub_item];
                    for item in items {
                        make_menu(sub_menu, delegate, menu_target_class, item, command_settings);
                    }
                },
                Menu::Item {name, command} => {
                    let settings = if let Some(settings) = command_settings.get(command) {
                        *settings
                    }
                    else {
                        CxCommandSetting::default()
                    };
                    let sub_item: ObjcId = msg_send![
                        parent_menu,
                        addItemWithTitle: str_to_nsstring(name)
                        action: sel!(menuAction:)
                        keyEquivalent: str_to_nsstring(keycode_to_menu_key(settings.key_code, settings.shift))
                    ];
                    let target: ObjcId = msg_send![menu_target_class, new];
                    let () = msg_send![sub_item, setTarget: target];
                    let () = msg_send![sub_item, setEnabled: if settings.enabled {YES}else {NO}];
                    /*
                    let command_usize = if let Ok(mut status_map) = status_map.lock() {
                        if let Some(id) = status_map.command_to_usize.get(&command) {
                            *id
                        }
                        else {
                            let id = status_map.status_to_usize.len();
                            status_map.command_to_usize.insert(*command, id);
                            status_map.usize_to_command.insert(id, *command);
                            id
                        }
                    }
                    else {
                        panic!("cannot lock cmd_map");
                    };*/
                    
                    (*target).set_ivar("cocoa_app_ptr", GLOBAL_COCOA_APP as *mut _ as *mut c_void);
                    (*target).set_ivar("command_usize", command.0);
                },
                Menu::Line => {
                    let sep_item: ObjcId = msg_send![class!(NSMenuItem), separatorItem];
                    let () = msg_send![
                        parent_menu,
                        addItem: sep_item
                    ];
                }
            }
        }
        unsafe {
            make_menu(nil, self.menu_delegate_instance, self.menu_target_class, menu, command_settings);
        }
    }
    
    pub fn startup_focus_hack(&mut self) {
                /*
        unsafe { 
            if !self.startup_focus_hack_ran {
                self.startup_focus_hack_ran = true;
                let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                let active: bool = msg_send![ns_app, isActive];
                if !active {
                    let dock_bundle_id = str_to_ns_string("com.apple.dock");
                    let dock_array: ObjcId = msg_send![
                        class!(NSRunningApplication),
                        runningApplicationsWithBundleIdentifier: dock_bundle_id
                    ];
                    let dock_array_len: u64 = msg_send![dock_array, count];
                    if dock_array_len == 0 {
                        panic!("Dock not running");
                    } else {
                        let dock: ObjcId = msg_send![dock_array, objectAtIndex: 0];
                        let _status: BOOL = msg_send![
                            dock,
                            activateWithOptions: NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps
                        ];
                        let ns_running_app: ObjcId = msg_send![class!(NSRunningApplication), currentApplication];
                        let () = msg_send![
                            ns_running_app,
                            activateWithOptions: NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps
                        ];
                    }
                }
            }
        }*/
    }
    
    /*    pub fn init_app_after_first_window(&mut self) {
        if self.init_application {
            return
        }
        self.init_app_after_first_window = true;
        unsafe {
            let ns_app: id = msg_send![class!(NSApplication), sharedApplication];
            let () = msg_send![ns_app, setActivationPolicy: NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular as i64];
            let () = msg_send![ns_app, finishLaunching];
            let () = msg_send![ns_app, activateIgnoringOtherApps:true];
            
            //let current_app: id = msg_send![class!(NSRunningApplication), currentApplication];
            //let () = msg_send![ns_app, activateWithOptions: NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps as u64];
            
        }
    }*/
    
    pub fn init(&mut self) {
        unsafe {
            GLOBAL_COCOA_APP = self;
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            (*self.timer_delegate_instance).set_ivar("cocoa_app_ptr", self as *mut _ as *mut c_void);
            (*self.menu_delegate_instance).set_ivar("cocoa_app_ptr", self as *mut _ as *mut c_void);
            (*self.app_delegate_instance).set_ivar("cocoa_app_ptr", self as *mut _ as *mut c_void);
            let () = msg_send![ns_app, setDelegate: self.app_delegate_instance];
            let () = msg_send![ns_app, setActivationPolicy: NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular as i64];
            //let () = msg_send![ns_app, finishLaunching];
            //let () = msg_send![ns_app, run];
            //let () = msg_send![ns_app, activateIgnoringOtherApps:true];
        }
    }
    
    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_micros() as f64 / 1_000_000.0
    }
    
    unsafe fn process_ns_event(&mut self, ns_event: ObjcId) {
        let ev_type: NSEventType = msg_send![ns_event, type];
        
        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![ns_app, sendEvent: ns_event];
        
        if ev_type as u64 == 21 { // some missing event from cocoa-rs crate
            return;
        }
        
        match ev_type {
            NSEventType::NSApplicationDefined => { // event loop unblocker
            },
            NSEventType::NSKeyUp => {
                if let Some(key_code) = get_event_keycode(ns_event) {
                    let modifiers = get_event_key_modifier(ns_event);
                    //let key_char = get_event_char(ns_event);
                    let is_repeat: bool = msg_send![ns_event, isARepeat];
                    self.do_callback(&mut vec![
                        Event::KeyUp(KeyEvent {
                            key_code: key_code,
                            //key_char: key_char,
                            is_repeat: is_repeat,
                            modifiers: modifiers,
                            time: self.time_now()
                        })
                    ]);
                }
            },
            NSEventType::NSKeyDown => {
                if let Some(key_code) = get_event_keycode(ns_event) {
                    let modifiers = get_event_key_modifier(ns_event);
                    //let key_char = get_event_char(ns_event);
                    let is_repeat: bool = msg_send![ns_event, isARepeat];
                    //let is_return = if let KeyCode::Return = key_code{true} else{false};
                    
                    
                    match key_code {
                        KeyCode::KeyV => if modifiers.logo || modifiers.control {
                            // was a paste
                            let nsstring: ObjcId = msg_send![self.pasteboard, stringForType: NSStringPboardType];
                            let string = nsstring_to_string(nsstring);
                            self.do_callback(&mut vec![
                                Event::TextInput(TextInputEvent {
                                    input: string,
                                    was_paste: true,
                                    replace_last: false
                                })
                            ]);
                        },
                        KeyCode::KeyX | KeyCode::KeyC => if modifiers.logo || modifiers.control {
                            // cut or copy.
                            let mut events = vec![
                                Event::TextCopy(TextCopyEvent {
                                    response: None
                                })
                            ];
                            self.do_callback(&mut events);
                            match &events[0] {
                                Event::TextCopy(req) => if let Some(response) = &req.response {
                                    // plug it into the apple clipboard
                                    let nsstring = str_to_nsstring(&response);
                                    let array: ObjcId = msg_send![class!(NSArray), arrayWithObject: NSStringPboardType];
                                    let () = msg_send![self.pasteboard, declareTypes: array owner: nil];
                                    let () = msg_send![self.pasteboard, setString: nsstring forType: NSStringPboardType];
                                },
                                _ => ()
                            };
                        },
                        _ => {}
                    }
                    
                    self.do_callback(&mut vec![
                        Event::KeyDown(KeyEvent {
                            key_code: key_code,
                            //key_char: key_char,
                            is_repeat: is_repeat,
                            modifiers: modifiers,
                            time: self.time_now()
                        })
                    ]);
                    /*
                    if is_return{
                        self.do_callback(&mut vec![
                            Event::TextInput(TextInputEvent{
                                input:"\n".to_string(),
                                was_paste:false,
                                replace_last:false
                            })
                        ]);
                    }*/
                    
                    
                    
                }
            },
            NSEventType::NSFlagsChanged => {
                let modifiers = get_event_key_modifier(ns_event);
                let last_key_mod = self.last_key_mod.clone();
                self.last_key_mod = modifiers.clone();
                let mut events = Vec::new();
                fn add_event(time: f64, old: bool, new: bool, modifiers: KeyModifiers, events: &mut Vec<Event>, key_code: KeyCode) {
                    if old != new {
                        let event = KeyEvent {
                            key_code: key_code,
                            //key_char: '\0',
                            is_repeat: false,
                            modifiers: modifiers,
                            time: time
                        };
                        if new {
                            events.push(Event::KeyDown(event));
                        }
                        else {
                            events.push(Event::KeyUp(event));
                        }
                    }
                }
                let time = self.time_now();
                add_event(time, last_key_mod.shift, modifiers.shift, modifiers.clone(), &mut events, KeyCode::Shift);
                add_event(time, last_key_mod.alt, modifiers.alt, modifiers.clone(), &mut events, KeyCode::Alt);
                add_event(time, last_key_mod.logo, modifiers.logo, modifiers.clone(), &mut events, KeyCode::Logo);
                add_event(time, last_key_mod.control, modifiers.control, modifiers.clone(), &mut events, KeyCode::Control);
                if events.len() >0 {
                    self.do_callback(&mut events);
                }
            },
            NSEventType::NSMouseEntered => {},
            NSEventType::NSMouseExited => {},
            /*
            appkit::NSMouseMoved |
            appkit::NSLeftMouseDragged |
            appkit::NSOtherMouseDragged |
            appkit::NSRightMouseDragged => {
                let window: id = ns_event.window();
                if window == nil {
                    return
                }
                let window_delegate = NSWindow::delegate(window);
                if window_delegate == nil {
                    return
                }
                let ptr: *mut c_void = *(*window_delegate).get_ivar("cocoa_window_ptr");
                let cocoa_window = &mut *(ptr as *mut CocoaWindow);
                
                let window_point = ns_event.locationInWindow();
                let view_point = cocoa_window.view.convertPoint_fromView_(window_point, nil);
                let view_rect = NSView::frame(cocoa_window.view);
                let mouse_pos = Vec2 {x: view_point.x as f32, y: view_rect.size.height as f32 - view_point.y as f32};
                
                cocoa_window.send_finger_hover_and_move(mouse_pos, get_event_key_modifier(ns_event));
            },*/
            NSEventType::NSScrollWheel => {
                let window: ObjcId = msg_send![ns_event, window];
                if window == nil {
                    return
                }
                let window_delegate: ObjcId = msg_send![window, delegate];
                if window_delegate == nil {
                    return
                }
                let ptr: *mut c_void = *(*window_delegate).get_ivar("cocoa_window_ptr");
                let cocoa_window = &mut *(ptr as *mut CocoaWindow);
                let dx: f64 = msg_send![ns_event, scrollingDeltaX];
                let dy: f64 = msg_send![ns_event, scrollingDeltaY];
                let has_prec: BOOL = msg_send![ns_event, hasPreciseScrollingDeltas];
                return if has_prec == YES {
                    self.do_callback(&mut vec![
                        Event::FingerScroll(FingerScrollEvent {
                            digit: 0,
                            window_id: cocoa_window.window_id,
                            scroll: Vec2 {x: -dx as f32, y: -dy as f32},
                            abs: cocoa_window.last_mouse_pos,
                            input_type: FingerInputType::Touch,
                            modifiers: get_event_key_modifier(ns_event),
                            handled_x: false,
                            handled_y: false,
                            time: self.time_now()
                        })
                    ]);
                } else {
                    self.do_callback(&mut vec![
                        Event::FingerScroll(FingerScrollEvent {
                            digit: 0,
                            window_id: cocoa_window.window_id,
                            scroll: Vec2 {x: -dx as f32 * 32., y: -dy as f32 * 32.},
                            abs: cocoa_window.last_mouse_pos,
                            input_type: FingerInputType::Mouse,
                            modifiers: get_event_key_modifier(ns_event),
                            handled_x: false,
                            handled_y: false,
                            time: self.time_now()
                        })
                    ]);
                }
            },
            NSEventType::NSEventTypePressure => {},
            _ => (),
        }
    }
    
    pub fn terminate_event_loop(&mut self) {
        self.event_loop_running = false;
    }
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut CocoaApp, &mut Vec<Event>) -> bool,
    {
        unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let () = msg_send![ns_app, finishLaunching];
            
            self.event_callback = Some(&mut event_handler as *const dyn FnMut(&mut CocoaApp, &mut Vec<Event>) -> bool as *mut dyn FnMut(&mut CocoaApp, &mut Vec<Event>) -> bool);
            
            while self.event_loop_running {
                let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
                
                let ns_until: ObjcId = if self.loop_block {
                    msg_send![class!(NSDate), distantFuture]
                }else {
                    msg_send![class!(NSDate), distantPast]
                };
                let ns_event: ObjcId = msg_send![
                    ns_app,
                    nextEventMatchingMask: NSEventMask::NSAnyEventMask as u64 | NSEventMask::NSEventMaskPressure as u64
                    untilDate: ns_until
                    inMode: NSDefaultRunLoopMode
                    dequeue: YES
                ];
                
                if ns_event != nil {
                    self.process_ns_event(ns_event);
                }
                
                if ns_event == nil || self.loop_block {
                    self.do_callback(&mut vec![Event::Paint]);
                }
                
                let () = msg_send![pool, release];
            }
            self.event_callback = None;
        }
    }
    
    pub fn do_callback(&mut self, events: &mut Vec<Event>) {
        unsafe {
            if self.event_callback.is_none() || self.event_recur_block {
                return
            };
            self.event_recur_block = true;
            let callback = self.event_callback.unwrap();
            self.loop_block = (*callback)(self, events);
            self.event_recur_block = false;
        }
    }
    
    pub fn post_signal(signal_id: usize, status: u64) {
        unsafe {
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
            
            let cocoa_app = &mut (*GLOBAL_COCOA_APP);
            let post_delegate_instance: ObjcId = msg_send![cocoa_app.post_delegate_class, new];
            /*
            // lock it
            let status_id = if let Ok(mut status_map) = cocoa_app.status_map.lock() {
                if let Some(id) = status_map.status_to_usize.get(&status) {
                    *id
                }
                else {
                    let id = status_map.status_to_usize.len();
                    status_map.status_to_usize.insert(status, id);
                    status_map.usize_to_status.insert(id, status);
                    id
                }
            }
            else {
                panic!("Cannot lock cmd_map");
            };*/
            
            (*post_delegate_instance).set_ivar("cocoa_app_ptr", GLOBAL_COCOA_APP as *mut _ as *mut c_void);
            (*post_delegate_instance).set_ivar("signal_id", signal_id);
            (*post_delegate_instance).set_ivar("status", status);
            let nstimer: ObjcId = msg_send![
                class!(NSTimer),
                timerWithTimeInterval: 0.
                target: post_delegate_instance
                selector: sel!(receivedPost:)
                userInfo: nil
                repeats: false
            ];
            let nsrunloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
            let () = msg_send![nsrunloop, addTimer: nstimer forMode: NSRunLoopCommonModes];
            
            let () = msg_send![pool, release];
        }
    }
    
    pub fn set_mouse_cursor(&mut self, cursor: MouseCursor) {
        if self.current_cursor != cursor {
            self.current_cursor = cursor;
            // todo set it on all windows
            unsafe {
                for (window, view) in &self.cocoa_windows {
                    let _: () = msg_send![
                        *window,
                        invalidateCursorRectsForView: *view
                    ];
                }
            }
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
            
            self.timers.push(CocoaTimer {
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
                self.do_callback(&mut vec![Event::Timer(TimerEvent {timer_id: timer_id})]);
                // break the eventloop if its in blocked mode
                unsafe {
                    let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
                    let nsevent: ObjcId = msg_send![
                        class!(NSEvent),
                        otherEventWithType: NSEventType::NSApplicationDefined
                        location: NSPoint {x: 0., y: 0.}
                        modifierFlags: 0u64
                        timestamp: 0f64
                        windowNumber: 1u64
                        context: nil
                        subtype: 0i16
                        data1: 0u64
                        data2: 0u64
                    ];
                    let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                    let () = msg_send![ns_app, postEvent: nsevent atStart: 0];
                    let () = msg_send![pool, release];
                }
                return;
            }
        }
    }
    
    pub fn send_signal_event(&mut self, signal: Signal, status: u64) {
        let mut signals = HashMap::new();
        let mut new_set = Vec::new();
        new_set.push(status);
        signals.insert(signal, new_set);
        self.do_callback(&mut vec![
            Event::Signal(SignalEvent {
                signals: signals,
            })
        ]);
        self.do_callback(&mut vec![Event::Paint]);
    }
    
    pub fn send_command_event(&mut self, command: CommandId) {
        self.do_callback(&mut vec![
            Event::Command(command)
        ]);
        self.do_callback(&mut vec![Event::Paint]);
    }
    
    pub fn send_paint_event(&mut self) {
        self.do_callback(&mut vec![Event::Paint]);
    }
    
    pub fn start_dragging(&mut self, dragged_item: DraggedItem) {
        let cocoa_window = unsafe {
            let window: ObjcId = msg_send![self.ns_event, window];
            let window_delegate: ObjcId = msg_send![window, delegate];
            let cocoa_window: *mut c_void = *(*window_delegate).get_ivar("cocoa_window_ptr");
            &mut *(cocoa_window as *mut CocoaWindow)
        };
        
        cocoa_window.start_dragging(self.ns_event, dragged_item);
    }
}

pub fn get_cocoa_app(this: &Object) -> &mut CocoaApp {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar("cocoa_app_ptr");
        &mut *(ptr as *mut CocoaApp)
    }
}

pub fn get_cocoa_app_global() -> &'static mut CocoaApp {
    unsafe {
        &mut *(GLOBAL_COCOA_APP)
    }
}
