
use {
    std::{
        rc::Rc,
        cell::RefCell,
        sync::Mutex,
        ptr,
        time::Instant,
        collections::{HashMap,HashSet},
        os::raw::{c_void}
    },
    crate::{
        makepad_math::{
            DVec2,
        },
        os::{
            apple::frameworks::*,
            cocoa_delegate::*,
            cocoa_event::{
                CocoaEvent,
                CocoaScrollEvent
            },
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
            KeyCode,
            KeyEvent,
            TextInputEvent,
            TextCopyEvent,
            TimerEvent,
            Signal,
            SignalEvent,
            DraggedItem,
            KeyModifiers
        },
        cursor::MouseCursor,
        menu::{
            Menu,
            MenuCommand
        }
    }
};

// this is unsafe, however we don't have much choice since the system calls into 
// the objective C entrypoints we need to enter our eventloop
// So wherever we put this boundary, it will be unsafe

// this value will be fetched from multiple threads (post signal uses it)
pub static mut COCOA_CLASSES: *const CocoaClasses = 0 as *const _;
// this value should not. Todo: guard this somehow proper
pub static mut COCOA_APP : *mut CocoaApp = 0 as *mut _;

pub fn init_cocoa_globals(event_callback:Box<dyn FnMut(&mut CocoaApp, Vec<CocoaEvent>) -> bool>){
    unsafe{
        COCOA_CLASSES = Box::into_raw(Box::new(CocoaClasses::new()));
        COCOA_APP = Box::into_raw(Box::new(CocoaApp::new(event_callback)));
    }
}

pub fn get_cocoa_app_global() -> &'static mut CocoaApp {
    unsafe {
        &mut *(COCOA_APP)
    }
}

pub fn get_cocoa_class_global() -> &'static CocoaClasses {
    unsafe {
        &*(COCOA_CLASSES)
    }
}

#[derive(Clone)]
pub struct CocoaTimer {
    timer_id: u64,
    nstimer: ObjcId,
    repeats: bool
}

pub struct CocoaClasses {
    pub window: *const Class,
    pub window_delegate: *const Class,
    pub post_delegate: *const Class,
    pub timer_delegate: *const Class,
    pub menu_delegate: *const Class,
    pub app_delegate: *const Class,
    pub menu_target: *const Class,
    pub view: *const Class,
    pub key_value_observing_delegate: *const Class,
    pub const_attributes_for_marked_text: ObjcId,
    pub const_empty_string: RcObjcId,
}

impl CocoaClasses{
    pub fn new()->Self{
        let const_attributes = vec![
            RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSMarkedClauseSegment")).unwrap()).forget(),
            RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSGlyphInfo")).unwrap()).forget(),
        ];
        Self{
            window: define_cocoa_window_class(),
            window_delegate: define_cocoa_window_delegate(),
            post_delegate: define_cocoa_post_delegate(),
            timer_delegate: define_cocoa_timer_delegate(),
            menu_delegate: define_menu_delegate(),
            app_delegate: define_app_delegate(),
            menu_target: define_menu_target_class(),
            view: define_cocoa_view_class(),
            key_value_observing_delegate: define_key_value_observing_delegate(),
            const_attributes_for_marked_text: unsafe{msg_send![
                class!(NSArray),
                arrayWithObjects: const_attributes.as_ptr()
                count: const_attributes.len()
            ]},
            const_empty_string: RcObjcId::from_unowned(NonNull::new(str_to_nsstring("")).unwrap()),
        }
    }
}

pub struct CocoaApp {
    menu_delegate_instance: ObjcId,
    //app_delegate_instance: ObjcId,
    pub time_start: Instant,
    pub timer_delegate_instance: ObjcId,
    timers: Vec<CocoaTimer>,
    pub signals: Mutex<RefCell<HashSet<Signal>>>,
    pub cocoa_windows: Vec<(ObjcId, ObjcId)>,
    last_key_mod: KeyModifiers,
    pasteboard: ObjcId,
    startup_focus_hack_ran: bool,
    event_callback: Option<Box<dyn FnMut(&mut CocoaApp, Vec<CocoaEvent>) -> bool>>,
    event_loop_running: bool,
    loop_block: bool,
    pub cursors: HashMap<MouseCursor, ObjcId>,
    pub current_cursor: MouseCursor,
    ns_event: ObjcId,
}

impl CocoaApp {
    pub fn new(event_callback:Box<dyn FnMut(&mut CocoaApp, Vec<CocoaEvent>) -> bool>) -> CocoaApp {
        unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let app_delegate_instance: ObjcId = msg_send![get_cocoa_class_global().app_delegate, new];
            
            let () = msg_send![ns_app, setDelegate: app_delegate_instance];
            let () = msg_send![ns_app, setActivationPolicy: NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular as i64];

            // Construct the bits that are shared between windows
            CocoaApp {
                startup_focus_hack_ran: false,
                pasteboard: msg_send![class!(NSPasteboard), generalPasteboard],
                time_start: Instant::now(),
                timer_delegate_instance:msg_send![get_cocoa_class_global().timer_delegate, new],
                menu_delegate_instance:msg_send![get_cocoa_class_global().menu_delegate, new],
                //app_delegate_instance,
                signals: Mutex::new(RefCell::new(HashSet::new())),
                timers: Vec::new(),
                cocoa_windows: Vec::new(),
                loop_block: false,
                last_key_mod: KeyModifiers {..Default::default()},
                event_callback: Some(event_callback),
                event_loop_running: true,
                cursors: HashMap::new(),
                current_cursor: MouseCursor::Default,
                ns_event: ptr::null_mut(),
            }
        }
    }
    
    pub fn update_app_menu(&mut self, menu: &Menu, command_settings: &HashMap<MenuCommand, CxCommandSetting>,) {
        unsafe fn make_menu(
            parent_menu: ObjcId,
            delegate: ObjcId,
            menu_target_class: *const Class,
            menu: &Menu,
            command_settings: &HashMap<MenuCommand, CxCommandSetting>
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
                    
                    //(*target).set_ivar("cocoa_app_ptr", GLOBAL_COCOA_APP as *mut _ as *mut c_void);
                    (*target).set_ivar("command_usize", command.0.0);
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
            make_menu(nil, self.menu_delegate_instance, get_cocoa_class_global().menu_target, menu, command_settings);
        }
    }
    /*
    pub fn startup_focus_hack(&mut self) {
        
        unsafe {
            self.startup_focus_hack_ran = true;
            if !self.startup_focus_hack_ran {
                self.startup_focus_hack_ran = true;
                
                //let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                //let active: bool = msg_send![ns_app, isActive];
                //if !active {
                let dock_bundle_id = str_to_nsstring("com.apple.dock");
                let dock_array: ObjcId = msg_send![
                    class!(NSRunningApplication),
                    runningApplicationsWithBundleIdentifier: dock_bundle_id
                ];
                let my_app: ObjcId = msg_send![
                    class!(NSRunningApplication),
                    runningApplicationWithProcessIdentifier: std::process::id()
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
                        //let ns_running_app: ObjcId = msg_send![class!(NSRunningApplication), currentApplication];
                    
                    let () = msg_send![
                        my_app,
                        activateWithOptions: NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps
                    ];
                    let () = msg_send![self.cocoa_windows[0].0, makeKeyAndOrderFront: nil];
                }
                //}
            }
        }
    }*/
    pub fn startup_focus_hack(&mut self){
        unsafe{
            if !self.startup_focus_hack_ran {
                self.startup_focus_hack_ran = true;
                let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                let active: bool = msg_send![ns_app, isActive];
                if !active {
                    let dock_bundle_id: ObjcId = str_to_nsstring("com.apple.dock");
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
                    self.do_callback(vec![
                        CocoaEvent::KeyUp(KeyEvent {
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
                            self.do_callback(vec![
                                CocoaEvent::TextInput(TextInputEvent {
                                    input: string,
                                    was_paste: true,
                                    replace_last: false
                                })
                            ]);
                        },
                        KeyCode::KeyX | KeyCode::KeyC => if modifiers.logo || modifiers.control {
                            // cut or copy.
                            let response = Rc::new(RefCell::new(None));
                            self.do_callback(vec![
                                CocoaEvent::TextCopy(TextCopyEvent {
                                    response: response.clone()
                                })
                            ]);
                            let response = response.borrow();
                            if let Some(response) = response.as_ref(){
                                let nsstring = str_to_nsstring(&response);
                                let array: ObjcId = msg_send![class!(NSArray), arrayWithObject: NSStringPboardType];
                                let () = msg_send![self.pasteboard, declareTypes: array owner: nil];
                                let () = msg_send![self.pasteboard, setString: nsstring forType: NSStringPboardType];
                            }
                        },
                        _ => {}
                    }
                    
                    self.do_callback(vec![
                        CocoaEvent::KeyDown(KeyEvent {
                            key_code: key_code,
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
                fn add_event(time: f64, old: bool, new: bool, modifiers: KeyModifiers, events: &mut Vec<CocoaEvent>, key_code: KeyCode) {
                    if old != new {
                        let event = KeyEvent {
                            key_code: key_code,
                            //key_char: '\0',
                            is_repeat: false,
                            modifiers: modifiers,
                            time: time
                        };
                        if new {
                            events.push(CocoaEvent::KeyDown(event));
                        }
                        else {
                            events.push(CocoaEvent::KeyUp(event));
                        }
                    }
                }
                let time = self.time_now();
                add_event(time, last_key_mod.shift, modifiers.shift, modifiers.clone(), &mut events, KeyCode::Shift);
                add_event(time, last_key_mod.alt, modifiers.alt, modifiers.clone(), &mut events, KeyCode::Alt);
                add_event(time, last_key_mod.logo, modifiers.logo, modifiers.clone(), &mut events, KeyCode::Logo);
                add_event(time, last_key_mod.control, modifiers.control, modifiers.clone(), &mut events, KeyCode::Control);
                if events.len() >0 {
                    self.do_callback(events);
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
                    self.do_callback(vec![
                        CocoaEvent::Scroll(CocoaScrollEvent {
                            window_id: cocoa_window.window_id,
                            scroll: DVec2 {x: -dx, y: -dy},
                            abs: cocoa_window.last_mouse_pos,
                            modifiers: get_event_key_modifier(ns_event),
                            time: self.time_now()
                        })
                    ]);
                } else {
                    self.do_callback(vec![
                        CocoaEvent::Scroll(CocoaScrollEvent {
                            window_id: cocoa_window.window_id,
                            scroll: DVec2 {x: -dx * 32., y: -dy * 32.},
                            abs: cocoa_window.last_mouse_pos,
                            modifiers: get_event_key_modifier(ns_event),
                            time: self.time_now()
                        })
                    ]);
                }
            },
            NSEventType::NSEventTypePressure => {
                
                
            },
            _ => (),
        }
    }
    
    pub fn terminate_event_loop(&mut self) {
        self.event_loop_running = false;
    }
    
    pub fn event_loop(&mut self){
        unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let () = msg_send![ns_app, finishLaunching];
            
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
                    self.do_callback(vec![CocoaEvent::Paint]);
                }
                
                let () = msg_send![pool, release];
            }
            self.event_callback = None;
        }
    }
    
    pub fn do_callback(&mut self, events: Vec<CocoaEvent>) {
        if let Some(mut callback) = self.event_callback.take(){
            self.loop_block = callback(self, events);
            self.event_callback = Some(callback);
        }
        //s(*callback)(self, events);
        //self.event_recur_block = false;
    }
    
    pub fn post_signal(signal: Signal) {
        unsafe {
            let cocoa_app = get_cocoa_app_global();
            if let Ok(signals) = cocoa_app.signals.lock(){
                let mut signals = signals.borrow_mut();
                // if empty, we do shit. otherwise we add
                if signals.is_empty(){
                    signals.insert(signal);
                    let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
                    //let cocoa_app = get_cocoa_app_global();
                    let post_delegate_instance: ObjcId = msg_send![get_cocoa_class_global().post_delegate, new];
                    //(*post_delegate_instance).set_ivar("cocoa_app_ptr", GLOBAL_COCOA_APP as *mut _ as *mut c_void);
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
                else{
                    signals.insert(signal);
                }
            }
            
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
                self.do_callback(vec![CocoaEvent::Timer(TimerEvent {timer_id: timer_id})]);
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
    
    pub fn send_signal_event(&mut self) {
        let signals = if let Ok(signals) = self.signals.lock(){
            let mut new_signals = HashSet::new();
            std::mem::swap(&mut *signals.borrow_mut(), &mut new_signals);
            new_signals
        }else{panic!()};
        
        self.do_callback(vec![
            CocoaEvent::Signal(SignalEvent {
                signals,
            })
        ]);
        self.do_callback(vec![CocoaEvent::Paint]);
    }
    
    pub fn send_command_event(&mut self, command: MenuCommand) {
        self.do_callback(vec![
            CocoaEvent::MenuCommand(command)
        ]);
        self.do_callback(vec![CocoaEvent::Paint]);
    }
    
    pub fn send_paint_event(&mut self) {
        self.do_callback(vec![CocoaEvent::Paint]);
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
