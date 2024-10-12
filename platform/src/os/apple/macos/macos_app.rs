use crate::file_dialogs::FileDialog;

use {
    std::{
        rc::Rc,
        cell::{RefCell},
        time::Instant,
        collections::{HashMap},
        os::raw::{c_void},
    },
    crate::{
        makepad_live_id::*,
        makepad_math::{
            DVec2,
        },
        os::{
            apple::apple_sys::*,
            macos::{
                macos_delegates::*,
                macos_event::*,
                macos_window::MacosWindow,
            },
            apple_util::{
                nsstring_to_string,
                str_to_nsstring,
                keycode_to_menu_key,
                get_event_keycode,
                get_event_key_modifier
            },
            cx_native::EventFlow,
        },
        //macos_menu::{
        //    CxCommandSetting
        //},
        //turtle::{
        //    Rect
        //},
        event::{
            KeyCode,
            KeyEvent,
            TextInputEvent,
            TextClipboardEvent,
            TimerEvent,
            KeyModifiers,
        },
        cursor::MouseCursor,
        macos_menu::{
            MacosMenu,
        },
    }
};

// this is unsafe, however we don't have much choice since the system calls into
// the objective C entrypoints we need to enter our eventloop
// So wherever we put this boundary, it will be unsafe

// this value will be fetched from multiple threads (post signal uses it)
pub static mut MACOS_CLASSES: *const MacosClasses = 0 as *const _;
// this value should not. Todo: guard this somehow proper

pub static mut MACOS_APP: Option<RefCell<MacosApp>> = None;

pub fn get_macos_app_global() -> std::cell::RefMut<'static, MacosApp> {
    unsafe {
        MACOS_APP.as_mut().unwrap().borrow_mut()
    }
}

pub fn init_macos_app_global(event_callback: Box<dyn FnMut(MacosEvent) -> EventFlow>) {
    unsafe {
        MACOS_CLASSES = Box::into_raw(Box::new(MacosClasses::new()));
        MACOS_APP = Some(RefCell::new(MacosApp::new(event_callback)));
    }
}

pub fn get_macos_class_global() -> &'static MacosClasses {
    unsafe {
        &*(MACOS_CLASSES)
    }
}

#[derive(Clone)]
pub struct CocoaTimer {
    timer_id: u64,
    nstimer: ObjcId,
    repeats: bool
}

pub struct MacosClasses {
    pub window: *const Class,
    pub window_delegate: *const Class,
    pub menu_delegate: *const Class,
    pub app_delegate: *const Class,
    pub menu_target: *const Class,
    pub view: *const Class,
    pub timer_delegate: *const Class,
}

impl MacosClasses {
    pub fn new() -> Self {
        /*let const_attributes = vec![
            RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSMarkedClauseSegment")).unwrap()).forget(),
            RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSGlyphInfo")).unwrap()).forget(),
        ];*/
        Self {
            timer_delegate: define_macos_timer_delegate(),
            window: define_macos_window_class(),
            window_delegate: define_macos_window_delegate(),
            //post_delegate: define_cocoa_post_delegate(),
            menu_delegate: define_menu_delegate(),
            app_delegate: define_app_delegate(),
            menu_target: define_menu_target_class(),
            view: define_cocoa_view_class(),
        }
    }
}

pub struct MacosApp {
    menu_delegate_instance: ObjcId,
    //app_delegate_instance: ObjcId,
    pub time_start: Instant,
    pub timer_delegate_instance: ObjcId,
    timers: Vec<CocoaTimer>,
    //pub signals: Mutex<RefCell<HashSet<Signal>>>,
    pub cocoa_windows: Vec<(ObjcId, ObjcId)>,
    last_key_mod: KeyModifiers,
    #[allow(unused)]
    pasteboard: ObjcId,
    startup_focus_hack_ran: bool,
    event_callback: Option<Box<dyn FnMut(MacosEvent) -> EventFlow >>,
    event_flow: EventFlow,
    
    pub cursors: HashMap<MouseCursor, ObjcId>,
    pub current_cursor: MouseCursor,
    //current_ns_event: Option<ObjcId>,
}

impl MacosApp {
    pub fn new(event_callback: Box<dyn FnMut(MacosEvent) -> EventFlow>) -> MacosApp {
        unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let app_delegate_instance: ObjcId = msg_send![get_macos_class_global().app_delegate, new];
            
            let () = msg_send![ns_app, setDelegate: app_delegate_instance];
            let () = msg_send![ns_app, setActivationPolicy: NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular as i64];
            
            
            // Construct the bits that are shared between windows
            MacosApp {
                startup_focus_hack_ran: false,
                pasteboard: msg_send![class!(NSPasteboard), generalPasteboard],
                time_start: Instant::now(),
                timer_delegate_instance: msg_send![get_macos_class_global().timer_delegate, new],
                menu_delegate_instance: msg_send![get_macos_class_global().menu_delegate, new],
                //app_delegate_instance,
                //signals: Mutex::new(RefCell::new(HashSet::new())),
                timers: Vec::new(),
                cocoa_windows: Vec::new(),
                event_flow: EventFlow::Poll,
                last_key_mod: KeyModifiers {..Default::default()},
                event_callback: Some(event_callback),
                cursors: HashMap::new(),
                current_cursor: MouseCursor::Default,
                //current_ns_event: None,
            }
        }
    }
    
    pub fn init_quit_menu(&mut self){
        self.update_macos_menu(
            &MacosMenu::Main{items:vec![MacosMenu::Sub{
                name:"Makepad".to_string(),
                items:vec![MacosMenu::Item{
                    command:live_id!(quit),
                    key:KeyCode::KeyQ,
                    shift: false,
                    enabled: true,
                    name:"Quit Example".to_string()
                }]
            }]}
        );
    }
    
    
    pub fn update_macos_menu(&mut self, menu: &MacosMenu) {
        unsafe fn make_menu(
            parent_menu: ObjcId,
            delegate: ObjcId,
            menu_target_class: *const Class,
            menu: &MacosMenu,
        ) {
            
            match menu {
                MacosMenu::Main {items} => {
                    let main_menu: ObjcId = msg_send![class!(NSMenu), new];
                    let () = msg_send![main_menu, setTitle: str_to_nsstring("MainMenu")];
                    let () = msg_send![main_menu, setAutoenablesItems: NO];
                    let () = msg_send![main_menu, setDelegate: delegate];
                    
                    for item in items {
                        make_menu(main_menu, delegate, menu_target_class, item);
                    }
                    let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                    let () = msg_send![
                        ns_app,
                        setMainMenu: main_menu
                    ];
                },
                MacosMenu::Sub {name, items} => {
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
                        make_menu(sub_menu, delegate, menu_target_class, item);
                    }
                },
                MacosMenu::Item {name, command, shift, key, enabled} => {
                    
                    let sub_item: ObjcId = msg_send![
                        parent_menu,
                        addItemWithTitle: str_to_nsstring(name)
                        action: sel!(menuAction:)
                        keyEquivalent: str_to_nsstring(keycode_to_menu_key(*key, *shift))
                    ];
                    let target: ObjcId = msg_send![menu_target_class, new];
                    let () = msg_send![sub_item, setTarget: target];
                    let () = msg_send![sub_item, setEnabled: if *enabled {YES}else {NO}];
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
                    
                    //(*target).set_ivar("macos_app_ptr", GLOBAL_COCOA_APP as *mut _ as *mut c_void);
                    (*target).set_ivar("command_u64", command.0);
                },
                MacosMenu::Line => {
                    let sep_item: ObjcId = msg_send![class!(NSMenuItem), separatorItem];
                    let () = msg_send![
                        parent_menu,
                        addItem: sep_item
                    ];
                }
            }
        }
        unsafe {
            make_menu(nil, self.menu_delegate_instance, get_macos_class_global().menu_target, menu);
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
    pub fn startup_focus_hack(&mut self) {
        
        unsafe {
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
        (time_now.duration_since(self.time_start)).as_secs_f64() 
    }
    
    unsafe fn process_ns_event(ns_event: ObjcId) {
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
                    let time = get_macos_app_global().time_now();
                    MacosApp::do_callback(
                        MacosEvent::KeyUp(KeyEvent {
                            key_code: key_code,
                            //key_char: key_char,
                            is_repeat: is_repeat,
                            modifiers: modifiers,
                            time
                        })
                    );
                }
            },
            NSEventType::NSKeyDown => {
                if let Some(key_code) = get_event_keycode(ns_event) {
                    let modifiers = get_event_key_modifier(ns_event);
                    //let key_char = get_event_char(ns_event);
                    let is_repeat: bool = msg_send![ns_event, isARepeat];
                    //let is_return = if let KeyCode::Return = key_code{true} else{false};
                    
                    
                    #[cfg(target_os = "macos")]
                    match key_code {
                        KeyCode::KeyV => if modifiers.logo || modifiers.control {
                            // was a paste
                            let pasteboard: ObjcId = get_macos_app_global().pasteboard;
                            let nsstring: ObjcId = msg_send![pasteboard, stringForType: NSStringPboardType];
                            if nsstring != std::ptr::null_mut() {
                                let string = nsstring_to_string(nsstring);
                                MacosApp::do_callback(
                                    MacosEvent::TextInput(TextInputEvent {
                                        input: string,
                                        was_paste: true,
                                        replace_last: false
                                    })
                                );
                            }
                        },
                        KeyCode::KeyC => if modifiers.logo || modifiers.control {
                            let pasteboard: ObjcId = get_macos_app_global().pasteboard;
                            let response = Rc::new(RefCell::new(None));
                            MacosApp::do_callback(
                                MacosEvent::TextCopy(TextClipboardEvent {
                                    response: response.clone()
                                })
                            );
                            let response = response.borrow();
                            if let Some(response) = response.as_ref() {
                                let nsstring = str_to_nsstring(&response);
                                let array: ObjcId = msg_send![class!(NSArray), arrayWithObject: NSStringPboardType];
                                let () = msg_send![pasteboard, declareTypes: array owner: nil];
                                let () = msg_send![pasteboard, setString: nsstring forType: NSStringPboardType];
                            }
                        },
                        KeyCode::KeyX => if modifiers.logo || modifiers.control {
                            let pasteboard: ObjcId = get_macos_app_global().pasteboard;
                            let response = Rc::new(RefCell::new(None));
                            MacosApp::do_callback(
                                MacosEvent::TextCut(TextClipboardEvent {
                                    response: response.clone()
                                })
                            );
                            let response = response.borrow();
                            if let Some(response) = response.as_ref() {
                                let nsstring = str_to_nsstring(&response);
                                let array: ObjcId = msg_send![class!(NSArray), arrayWithObject: NSStringPboardType];
                                let () = msg_send![pasteboard, declareTypes: array owner: nil];
                                let () = msg_send![pasteboard, setString: nsstring forType: NSStringPboardType];
                            }
                        },
                        _ => {}
                    }
                    let time = get_macos_app_global().time_now();
                    // lets check if we have marked text
                    if KeyCode::Backspace == key_code {
                        // we have to check if we dont have any marked text in our windows
                        for (_,view) in &get_macos_app_global().cocoa_windows{
                            let marked = unsafe{msg_send![*view, hasMarkedText]};
                            if marked{
                                return
                            }
                        }
                    }
                    MacosApp::do_callback(
                        MacosEvent::KeyDown(KeyEvent {
                            key_code: key_code,
                            is_repeat: is_repeat,
                            modifiers: modifiers,
                            time
                        })
                    );
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
                let last_key_mod = get_macos_app_global().last_key_mod.clone();
                get_macos_app_global().last_key_mod = modifiers.clone();
                let mut events = Vec::new();
                fn add_event(time: f64, old: bool, new: bool, modifiers: KeyModifiers, events: &mut Vec<MacosEvent>, key_code: KeyCode) {
                    if old != new {
                        let event = KeyEvent {
                            key_code: key_code,
                            //key_char: '\0',
                            is_repeat: false,
                            modifiers: modifiers,
                            time: time
                        };
                        if new {
                            events.push(MacosEvent::KeyDown(event));
                        }
                        else {
                            events.push(MacosEvent::KeyUp(event));
                        }
                    }
                }
                let time = get_macos_app_global().time_now();
                add_event(time, last_key_mod.shift, modifiers.shift, modifiers.clone(), &mut events, KeyCode::Shift);
                add_event(time, last_key_mod.alt, modifiers.alt, modifiers.clone(), &mut events, KeyCode::Alt);
                add_event(time, last_key_mod.logo, modifiers.logo, modifiers.clone(), &mut events, KeyCode::Logo);
                add_event(time, last_key_mod.control, modifiers.control, modifiers.clone(), &mut events, KeyCode::Control);
                if events.len() >0 {
                    for event in events {
                        MacosApp::do_callback(event);
                    }
                }
            },
            NSEventType::NSMouseEntered => {},
            NSEventType::NSMouseExited => {},
            NSEventType::NSScrollWheel => {
                let window: ObjcId = msg_send![ns_event, window];
                if window == nil {
                    return
                }
                let window_delegate: ObjcId = msg_send![window, delegate];
                if window_delegate == nil {
                    return
                }
                let ptr: *mut c_void = *(*window_delegate).get_ivar("macos_window_ptr");
                let cocoa_window = &mut *(ptr as *mut MacosWindow);
                let dx: f64 = msg_send![ns_event, scrollingDeltaX];
                let dy: f64 = msg_send![ns_event, scrollingDeltaY];
                let has_prec: BOOL = msg_send![ns_event, hasPreciseScrollingDeltas];
                return if has_prec == YES {
                    cocoa_window.send_scroll(DVec2 {x: -dx, y: -dy}, get_event_key_modifier(ns_event), false);
                } else {
                    cocoa_window.send_scroll(DVec2 {x: -dx * 32., y: -dy * 32.}, get_event_key_modifier(ns_event), true);
                }
            },
            NSEventType::NSEventTypePressure => {
            },
            _ => (),
        }
    }
    
    pub fn event_loop() {
        unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            //let () = msg_send![ns_app, activateIgnoringOtherApps:YES];
            let () = msg_send![ns_app, finishLaunching];
           // get_macos_app_global().init_quit_menu();
           // get_macos_app_global().startup_focus_hack();
           
            loop {
                let event_flow = get_macos_app_global().event_flow;
                match event_flow {
                    EventFlow::Exit => {
                        break;
                    }
                    EventFlow::Poll | EventFlow::Wait => {
                        let event_wait = if let EventFlow::Wait = get_macos_app_global().event_flow {true}else {false};
                        let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
                        
                        let ns_until: ObjcId = if event_wait {
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
                        //self.current_ns_event = Some(ns_event);
                        if ns_event != nil {
                            MacosApp::process_ns_event(ns_event);
                        }
                        
                        if ns_event == nil || event_wait {
                            MacosApp::do_callback(MacosEvent::Paint);
                        }
                        //self.current_ns_event = None;
                        
                        let () = msg_send![pool, release];
                    }
                }
            }
        }
    }
    
    pub fn do_callback(event: MacosEvent) {
        let cb = get_macos_app_global().event_callback.take();
        if let Some(mut callback) = cb {
            let event_flow = callback(event);
            get_macos_app_global().event_flow = event_flow;
            if let EventFlow::Exit = event_flow {
                unsafe {
                    let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                    let () = msg_send![ns_app, terminate: nil];
                }
            }
            get_macos_app_global().event_callback = Some(callback);
        }
    }
    /*
    pub fn post_signal(signal: Signal) {
        unsafe {
            let cocoa_app = get_macos_app_global();
            if let Ok(signals) = cocoa_app.signals.lock(){
                let mut signals = signals.borrow_mut();
                // if empty, we do shit. otherwise we add
                if signals.is_empty(){
                    signals.insert(signal);
                    let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
                    //let cocoa_app = get_macos_app_global();
                    let post_delegate_instance: ObjcId = msg_send![get_macos_class_global().post_delegate, new];
                    //(*post_delegate_instance).set_ivar("macos_app_ptr", GLOBAL_COCOA_APP as *mut _ as *mut c_void);
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
    }*/
    
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
    
    pub fn send_timer_received(nstimer: ObjcId) {
        
        let len = get_macos_app_global().timers.len() ;
        for i in 0..len {
            let time = get_macos_app_global().time_now();
            if get_macos_app_global().timers[i].nstimer == nstimer {
                let timer_id = get_macos_app_global().timers[i].timer_id;
                if !get_macos_app_global().timers[i].repeats {
                    get_macos_app_global().timers.remove(i);
                }
                
                MacosApp::do_callback(MacosEvent::Timer(TimerEvent {time:Some(time), timer_id: timer_id}));
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
    /*
    pub fn send_signal_event(&mut self) {
        let signals = if let Ok(signals) = self.signals.lock(){
            let mut new_signals = HashSet::new();
            std::mem::swap(&mut *signals.borrow_mut(), &mut new_signals);
            new_signals
        }else{panic!()};
        
        self.do_callback(vec![
            MacosEvent::Signal(SignalEvent {
                signals,
            })
        ]);
        self.do_callback(vec![MacosEvent::Paint]);
    }*/
    
    pub fn send_command_event(command: LiveId) {
        MacosApp::do_callback(
            MacosEvent::MacosMenuCommand(command)
        );
        MacosApp::do_callback(MacosEvent::Paint);
    }
    
    pub fn send_paint_event() {
        MacosApp::do_callback(MacosEvent::Paint);
    }
    /*
    #[cfg(target_os = "macos")]
    pub fn start_dragging(&mut self, items: Vec<DragItem>) {
       unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let ns_event: ObjcId = msg_send![ns_app, currentEvent];
            
            let window: ObjcId = msg_send![ns_event, window];
            let window_delegate: ObjcId = msg_send![window, delegate];
            if window == nil {
                crate::error!("start_dragging: Cocoa window nil on event");
                return
            }
            let cocoa_window: *mut c_void = *(*window_delegate).get_ivar("macos_window_ptr");
            let cocoa_window = &mut *(cocoa_window as *mut MacosWindow);
            cocoa_window.start_dragging(ns_event, items);
        };
    }*/

    pub fn copy_to_clipboard(&mut self, content: &str) {
        unsafe {
            let pasteboard: ObjcId = self.pasteboard;
            let nsstring = str_to_nsstring(content);
            let array: ObjcId = msg_send![class!(NSArray), arrayWithObject: NSStringPboardType];
            let () = msg_send![pasteboard, declareTypes: array owner: nil];
            let () = msg_send![pasteboard, setString: nsstring forType: NSStringPboardType];
        }
    }

    pub fn open_save_file_dialog(&mut self, _settings: FileDialog)
    {
        println!("open save file dialog!");
    }

    pub fn open_select_file_dialog(&mut self, _settings: FileDialog )
    {
        println!("open select file dialog!");
    }

    pub fn open_save_folder_dialog(&mut self,  _settings: FileDialog)
    {
        println!("open save folder dialog!");
    }

    pub fn open_select_folder_dialog(&mut self, _settings: FileDialog)
    {
        println!("open select folder dialog!");
    }

}
