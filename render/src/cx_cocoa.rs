//  Life is too short for leaky abstractions.
// Gleaned/learned/templated from https://github.com/tomaka/winit/blob/master/src/platform/macos/

use std::collections::HashMap;
use crate::cx_apple::*;
use std::os::raw::c_void;
use std::sync::{Mutex};
//use core_graphics::display::CGDisplay;
//use time::precise_time_ns;

static mut GLOBAL_COCOA_APP: *mut CocoaApp = 0 as *mut _;


extern{
    pub fn mach_absolute_time()->u64;
}

use crate::cx::*; 
 
#[derive(Clone)]
pub struct CocoaWindow {
    pub window_id: usize,
    pub window_delegate: id,
    //pub layer_delegate: id,
    pub view: id,
    pub window: id,
    pub live_resize_timer: id,
    pub cocoa_app: *mut CocoaApp,
    pub last_window_geom: Option<WindowGeom>,
    pub ime_spot: Vec2,
    pub time_start: u64,
    pub is_fullscreen: bool,
    pub fingers_down: Vec<bool>,
    pub last_mouse_pos: Vec2,
}

#[derive(Clone)]
pub struct CocoaTimer {
    timer_id: u64,
    nstimer: id,
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
    pub menu_delegate_instance: id,
    pub app_delegate_instance: id,
    pub const_attributes_for_marked_text: id,
    pub const_empty_string: id,
    pub time_start: u64,
    pub timer_delegate_instance: id,
    pub timers: Vec<CocoaTimer>,
    pub cocoa_windows: Vec<(id, id)>,
    pub last_key_mod: KeyModifiers,
    pub pasteboard: id,
    pub event_callback: Option<*mut dyn FnMut(&mut CocoaApp, &mut Vec<Event>) -> bool>,
    pub event_recur_block: bool,
    pub event_loop_running: bool,
    pub loop_block: bool,
    pub init_app_after_first_window: bool,
    pub cursors: HashMap<MouseCursor, id>,
    pub current_cursor: MouseCursor,
    pub status_map: Mutex<CocoaStatusMap>
}

#[derive(Default)]
pub struct CocoaStatusMap {
    pub status_to_usize: HashMap<StatusId, usize>,
    pub usize_to_status: HashMap<usize, StatusId>,
    pub command_to_usize: HashMap<CommandId, usize>,
    pub usize_to_command: HashMap<usize, CommandId>,
}


impl CocoaApp {
    pub fn new() -> CocoaApp {
        unsafe {
            
            let timer_delegate_class = define_cocoa_timer_delegate();
            let timer_delegate_instance: id = msg_send![timer_delegate_class, new];
            let menu_delegate_class = define_menu_delegate();
            let menu_delegate_instance: id = msg_send![menu_delegate_class, new];
            let app_delegate_class = define_app_delegate();
            let app_delegate_instance: id = msg_send![app_delegate_class, new];
            
            let const_attributes = vec![
                str_to_nsstring("NSMarkedClauseSegment"),
                str_to_nsstring("NSGlyphInfo")
            ];
            
            // Construct the bits that are shared between windows
            CocoaApp {
                const_attributes_for_marked_text: msg_send![
                    class!(NSArray),
                    arrayWithObjects: const_attributes.as_ptr()
                    count: const_attributes.len()
                ],
                init_app_after_first_window: false,
                const_empty_string: str_to_nsstring(""),
                pasteboard: msg_send![class!(NSPasteboard), generalPasteboard],
                time_start: mach_absolute_time(),
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
                status_map: Mutex::new(CocoaStatusMap::default()),
                current_cursor: MouseCursor::Default,
            }
        }
    }
    
    pub fn update_app_menu(&mut self, menu: &Menu, command_settings: &HashMap<CommandId, CxCommandSetting>,) {
        unsafe fn make_menu(
            parent_menu: id,
            delegate: id,
            menu_target_class: *const Class,
            menu: &Menu,
            status_map: &Mutex<CocoaStatusMap>,
            command_settings: &HashMap<CommandId, CxCommandSetting>
        ) {
            match menu {
                Menu::Main {items} => {
                    let main_menu: id = msg_send![class!(NSMenu), new];
                    let () = msg_send![main_menu, setTitle: str_to_nsstring("MainMenu")];
                    let () = msg_send![main_menu, setAutoenablesItems: NO];
                    let () = msg_send![main_menu, setDelegate: delegate];
                    
                    for item in items {
                        make_menu(main_menu, delegate, menu_target_class, item, status_map, command_settings);
                    }
                    let ns_app: id = msg_send![class!(NSApplication), sharedApplication];
                    let () = msg_send![
                        ns_app,
                        setMainMenu: main_menu
                    ];
                },
                Menu::Sub {name, items} => {
                    let sub_menu: id = msg_send![class!(NSMenu), new];
                    let () = msg_send![sub_menu, setTitle: str_to_nsstring(name)];
                    let () = msg_send![sub_menu, setAutoenablesItems: NO];
                    let () = msg_send![sub_menu, setDelegate: delegate];
                    // append item to parebt
                    let sub_item: id = msg_send![
                        parent_menu,
                        addItemWithTitle: str_to_nsstring(name)
                        action: nil
                        keyEquivalent: str_to_nsstring("")
                    ];
                    // connect submenu
                    let () = msg_send![parent_menu, setSubmenu: sub_menu forItem: sub_item];
                    for item in items {
                        make_menu(sub_menu, delegate, menu_target_class, item, status_map, command_settings);
                    }
                },
                Menu::Item {name, command} => {
                    let settings = if let Some(settings) = command_settings.get(command) {
                        *settings
                    }
                    else {
                        CxCommandSetting::default()
                    };
                    let sub_item: id = msg_send![
                        parent_menu,
                        addItemWithTitle: str_to_nsstring(name)
                        action: sel!(menuAction:)
                        keyEquivalent: str_to_nsstring(keycode_to_menu_key(settings.key_code, settings.shift))
                    ];
                    let target: id = msg_send![menu_target_class, new];
                    let () = msg_send![sub_item, setTarget: target];
                    let () = msg_send![sub_item, setEnabled: if settings.enabled {YES}else {NO}];
                    
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
                    };
                    (*target).set_ivar("cocoa_app_ptr", GLOBAL_COCOA_APP as *mut _ as *mut c_void);
                    (*target).set_ivar("command_usize", command_usize);
                },
                Menu::Line => {
                    let sep_item: id = msg_send![class!(NSMenuItem), separatorItem];
                    let () = msg_send![
                        parent_menu,
                        addItem: sep_item
                    ];
                }
            }
        }
        unsafe {
            make_menu(nil, self.menu_delegate_instance, self.menu_target_class, menu, &self.status_map, command_settings);
        }
    }
    
    pub fn init_app_after_first_window(&mut self) {
        if self.init_app_after_first_window {
            return
        }
        self.init_app_after_first_window = true;
        unsafe {
            let ns_app: id = msg_send![class!(NSApplication), sharedApplication];
            let () = msg_send![ns_app, setActivationPolicy: NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular as i64];
            let () = msg_send![ns_app, finishLaunching];
            let current_app: id = msg_send![class!(NSRunningApplication), currentApplication];
            let () = msg_send![current_app, activateWithOptions: NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps as u64];
        }
    }
    
    pub fn init(&mut self) {
        unsafe {
            GLOBAL_COCOA_APP = self;
            let ns_app: id = msg_send![class!(NSApplication), sharedApplication];
            (*self.timer_delegate_instance).set_ivar("cocoa_app_ptr", self as *mut _ as *mut c_void);
            (*self.menu_delegate_instance).set_ivar("cocoa_app_ptr", self as *mut _ as *mut c_void);
            (*self.app_delegate_instance).set_ivar("cocoa_app_ptr", self as *mut _ as *mut c_void);
            let () = msg_send![ns_app, setDelegate: self.app_delegate_instance];
        }
    }
    
    pub fn time_now(&self) -> f64 {
        let time_now = unsafe{mach_absolute_time()};
        (time_now - self.time_start) as f64 / 1_000_000_000.0
    }
    
    unsafe fn process_ns_event(&mut self, ns_event: id) {
        if ns_event == nil {
            return;
        }
        
        let ev_type: NSEventType = msg_send![ns_event, type];
        
        if ev_type as u64 == 21 { // some missing event from cocoa-rs crate
            return;
        }
        
        let ns_app: id = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![ns_app, sendEvent: ns_event];
        
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
                            let nsstring: id = msg_send![self.pasteboard, stringForType: NSStringPboardType];
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
                                    let nsstring: id = str_to_nsstring(&response);
                                    let array: id = msg_send![class!(NSArray), arrayWithObject: NSStringPboardType];
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
                let window: id = msg_send![ns_event, window];
                if window == nil {
                    return
                }
                let window_delegate: id = msg_send![window, delegate];
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
                            window_id: cocoa_window.window_id,
                            scroll: Vec2 {x: -dx as f32, y: -dy as f32},
                            abs: cocoa_window.last_mouse_pos,
                            rel: cocoa_window.last_mouse_pos,
                            rect: Rect::default(),
                            is_wheel: false,
                            modifiers: get_event_key_modifier(ns_event),
                            handled: false,
                            time: self.time_now()
                        })
                    ]);
                } else {
                    self.do_callback(&mut vec![
                        Event::FingerScroll(FingerScrollEvent {
                            window_id: cocoa_window.window_id,
                            scroll: Vec2 {x: -dx as f32 * 32., y: -dy as f32 * 32.},
                            abs: cocoa_window.last_mouse_pos,
                            rel: cocoa_window.last_mouse_pos,
                            rect: Rect::default(),
                            is_wheel: true,
                            modifiers: get_event_key_modifier(ns_event),
                            handled: false,
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
            self.event_callback = Some(&mut event_handler as *const dyn FnMut(&mut CocoaApp, &mut Vec<Event>) -> bool as *mut dyn FnMut(&mut CocoaApp, &mut Vec<Event>) -> bool);
            
            while self.event_loop_running {
                let pool: id = msg_send![class!(NSAutoreleasePool), new];
                
                // in here the event loop state is handled
                let ns_app: id = msg_send![class!(NSApplication), sharedApplication];
                let ns_until: id = if self.loop_block {
                    msg_send![class!(NSDate), distantFuture]
                }else {
                    msg_send![class!(NSDate), distantPast]
                };
                let ns_event: id = msg_send![
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
    
    pub fn post_signal(signal_id: usize, status: StatusId) {
        unsafe {
            let pool: id = msg_send![class!(NSAutoreleasePool), new];
            
            let cocoa_app = &mut (*GLOBAL_COCOA_APP);
            let post_delegate_instance: id = msg_send![cocoa_app.post_delegate_class, new];
            
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
            };
            
            (*post_delegate_instance).set_ivar("cocoa_app_ptr", GLOBAL_COCOA_APP as *mut _ as *mut c_void);
            (*post_delegate_instance).set_ivar("signal_id", signal_id);
            (*post_delegate_instance).set_ivar("status", status_id);
            let nstimer: id = msg_send![
                class!(NSTimer),
                timerWithTimeInterval: 0.
                target: post_delegate_instance
                selector: sel!(receivedPost:)
                userInfo: nil
                repeats: false
            ];
            let nsrunloop: id = msg_send![class!(NSRunLoop), mainRunLoop];
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
            let pool: id = msg_send![class!(NSAutoreleasePool), new];
            
            let nstimer: id = msg_send![
                class!(NSTimer),
                timerWithTimeInterval: interval
                target: self.timer_delegate_instance
                selector: sel!(receivedTimer:)
                userInfo: nil
                repeats: repeats
            ];
            let nsrunloop: id = msg_send![class!(NSRunLoop), mainRunLoop];
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
    
    pub fn send_timer_received(&mut self, nstimer: id) {
        for i in 0..self.timers.len() {
            if self.timers[i].nstimer == nstimer {
                let timer_id = self.timers[i].timer_id;
                if !self.timers[i].repeats {
                    self.timers.remove(i);
                }
                self.do_callback(&mut vec![Event::Timer(TimerEvent {timer_id: timer_id})]);
                // break the eventloop if its in blocked mode
                unsafe {
                    let pool: id = msg_send![class!(NSAutoreleasePool), new];
                    let nsevent: id = msg_send![
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
                    let ns_app: id = msg_send![class!(NSApplication), sharedApplication];
                    let () = msg_send![ns_app, postEvent: nsevent atStart: 0];
                    let () = msg_send![pool, release];
                }
                return;
            }
        }
    }
    
    pub fn send_signal_event(&mut self, signal: Signal, status: StatusId) {
        let mut signals = HashMap::new();
        signals.insert(signal, vec![status]);
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
    
}

impl CocoaWindow {
    
    pub fn new(cocoa_app: &mut CocoaApp, window_id: usize) -> CocoaWindow {
        unsafe {
            let pool: id = msg_send![class!(NSAutoreleasePool), new];
            
            let window: id = msg_send![cocoa_app.window_class, alloc];
            let window_delegate: id = msg_send![cocoa_app.window_delegate_class, new];
            let view: id = msg_send![cocoa_app.view_class, alloc];
            
            let () = msg_send![pool, drain];
            cocoa_app.cocoa_windows.push((window, view));
            CocoaWindow {
                is_fullscreen: false,
                time_start: cocoa_app.time_start,
                live_resize_timer: nil,
                cocoa_app: cocoa_app,
                window_delegate: window_delegate,
                //layer_delegate:layer_delegate,
                window: window,
                window_id: window_id,
                view: view,
                last_window_geom: None,
                ime_spot: Vec2::default(),
                fingers_down: Vec::new(),
                last_mouse_pos: Vec2::default(),
            }
        }
    }
    
    // complete window initialization with pointers to self
    pub fn init(&mut self, title: &str, size: Vec2, position: Option<Vec2>) {
        unsafe {
            (*self.cocoa_app).init_app_after_first_window();
            self.fingers_down.resize(NUM_FINGERS, false);
            
            let pool: id = msg_send![class!(NSAutoreleasePool), new];
            
            // set the backpointeers
            (*self.window_delegate).set_ivar("cocoa_window_ptr", self as *mut _ as *mut c_void);
            //(*self.layer_delegate).set_ivar("cocoa_window_ptr", self as *mut _ as *mut c_void);
            let () = msg_send![self.view, initWithPtr: self as *mut _ as *mut c_void];
            
            let left_top = if let Some(position) = position {
                NSPoint {x: position.x as f64, y: position.y as f64}
            }
            else {
                NSPoint {x: 0., y: 0.}
            };
            let ns_size = NSSize {width: size.x as f64, height: size.y as f64};
            let window_frame = NSRect {origin: left_top, size: ns_size};
            let window_masks = NSWindowStyleMask::NSClosableWindowMask as u64
                | NSWindowStyleMask::NSMiniaturizableWindowMask as u64
                | NSWindowStyleMask::NSResizableWindowMask as u64
                | NSWindowStyleMask::NSTitledWindowMask as u64
                | NSWindowStyleMask::NSFullSizeContentViewWindowMask as u64;
            
            let () = msg_send![
                self.window,
                initWithContentRect: window_frame
                styleMask: window_masks as u64
                backing: NSBackingStoreType::NSBackingStoreBuffered as u64
                defer: NO
            ];
            
            let () = msg_send![self.window, setDelegate: self.window_delegate];
            
            let title = str_to_nsstring(title);
            let () = msg_send![self.window, setReleasedWhenClosed: NO];
            let () = msg_send![self.window, setTitle: title];
            let () = msg_send![self.window, setTitleVisibility: NSWindowTitleVisibility::NSWindowTitleHidden];
            let () = msg_send![self.window, setTitlebarAppearsTransparent: YES];
            
            //let subviews:id = msg_send![self.window, getSubviews];
            //println!("{}", subviews as u64);
            let () = msg_send![self.window, setAcceptsMouseMovedEvents: YES];
            
            let () = msg_send![self.view, setLayerContentsRedrawPolicy: 2]; //duringViewResize
            
            let () = msg_send![self.window, setContentView: self.view];
            let () = msg_send![self.window, makeFirstResponder: self.view];
            let () = msg_send![self.window, makeKeyAndOrderFront: nil];
            
            if position.is_none() {
                let () = msg_send![self.window, center];
            }
            let input_context: id = msg_send![self.view, inputContext];
            let () = msg_send![input_context, invalidateCharacterCoordinates];
            
            let () = msg_send![pool, drain];
            
        }
    }
    
    pub fn update_ptrs(&mut self) {
        unsafe {
            //(*self.layer_delegate).set_ivar("cocoa_window_ptr", self as *mut _ as *mut c_void);
            (*self.window_delegate).set_ivar("cocoa_window_ptr", self as *mut _ as *mut c_void);
            (*self.view).set_ivar("cocoa_window_ptr", self as *mut _ as *mut c_void);
        }
    }
    
    pub fn set_ime_spot(&mut self, spot: Vec2) {
        self.ime_spot = spot;
    }
    
    pub fn start_live_resize(&mut self) {
        unsafe {
            let pool: id = msg_send![class!(NSAutoreleasePool), new];
            let cocoa_app = &(*self.cocoa_app);
            self.live_resize_timer = msg_send![
                class!(NSTimer),
                timerWithTimeInterval: 0.01666666
                target: cocoa_app.timer_delegate_instance
                selector: sel!(receivedLiveResize:)
                userInfo: nil
                repeats: YES
            ];
            let nsrunloop: id = msg_send![class!(NSRunLoop), mainRunLoop];
            let () = msg_send![nsrunloop, addTimer: self.live_resize_timer forMode: NSRunLoopCommonModes];
            
            let () = msg_send![pool, release];
        }
    }
    
    pub fn close_window(&mut self) {
        unsafe {
            (*self.cocoa_app).event_recur_block = false;
            let () = msg_send![self.window, close];
        }
    }
    
    pub fn restore(&mut self) {
        unsafe {
            let () = msg_send![self.window, toggleFullScreen: nil];
        }
    }
    
    pub fn maximize(&mut self) {
        unsafe {
            let () = msg_send![self.window, toggleFullScreen: nil];
        }
    }
    
    pub fn minimize(&mut self) {
        unsafe {
            let () = msg_send![self.window, miniaturize: nil];
        }
    }
    
    pub fn set_topmost(&mut self, _topmost: bool) {
    }
    
    pub fn end_live_resize(&mut self) {
        unsafe {
            let () = msg_send![self.live_resize_timer, invalidate];
            self.live_resize_timer = nil;
        }
    }
    
    pub fn time_now(&self) -> f64 {
        let time_now = unsafe{mach_absolute_time()};
        (time_now - self.time_start) as f64 / 1_000_000_000.0
    }
    
    pub fn get_window_geom(&self) -> WindowGeom {
        WindowGeom {
            vr_is_presenting: false,
            is_topmost: false,
            is_fullscreen: self.is_fullscreen,
            inner_size: self.get_inner_size(),
            outer_size: self.get_outer_size(),
            dpi_factor: self.get_dpi_factor(),
            position: self.get_position()
        }
    }
    
    pub fn do_callback(&mut self, events: &mut Vec<Event>) {
        unsafe {
            (*self.cocoa_app).do_callback(events);
        }
    }
    
    pub fn set_position(&mut self, pos: Vec2) {
        let mut window_frame: NSRect = unsafe {msg_send![self.window, frame]};
        window_frame.origin.x = pos.x as f64;
        window_frame.origin.y = pos.y as f64;
        //not very nice: CGDisplay::main().pixels_high() as f64
        unsafe {let () = msg_send![self.window, setFrame: window_frame display: YES];};
    }
    
    pub fn get_position(&self) -> Vec2 {
        let window_frame: NSRect = unsafe {msg_send![self.window, frame]};
        Vec2 {x: window_frame.origin.x as f32, y: window_frame.origin.y as f32}
    }
    
    fn get_ime_origin(&self) -> Vec2 {
        let rect = NSRect {
            origin: NSPoint {x: 0.0, y: 0.0},
            //view_frame.size.height),
            size: NSSize {width: 0.0, height: 0.0},
        };
        let out: NSRect = unsafe {msg_send![self.window, convertRectToScreen: rect]};
        Vec2 {x: out.origin.x as f32, y: out.origin.y as f32}
    }
    
    pub fn get_inner_size(&self) -> Vec2 {
        let view_frame: NSRect = unsafe {msg_send![self.view, frame]};
        Vec2 {x: view_frame.size.width as f32, y: view_frame.size.height as f32}
    }
    
    pub fn get_outer_size(&self) -> Vec2 {
        let window_frame: NSRect = unsafe {msg_send![self.window, frame]};
        Vec2 {x: window_frame.size.width as f32, y: window_frame.size.height as f32}
    }
    
    pub fn set_outer_size(&self, size: Vec2) {
        let mut window_frame: NSRect = unsafe {msg_send![self.window, frame]};
        window_frame.size.width = size.x as f64;
        window_frame.size.height = size.y as f64;
        unsafe {let () = msg_send![self.window, setFrame: window_frame display: YES];};
    }
    
    pub fn get_dpi_factor(&self) -> f32 {
        let scale: f64 = unsafe {msg_send![self.window, backingScaleFactor]};
        scale as f32
    }
    
    pub fn send_change_event(&mut self) {
        //return;
        let new_geom = self.get_window_geom();
        let old_geom = if let Some(old_geom) = &self.last_window_geom {
            old_geom.clone()
        }
        else {
            new_geom.clone()
        };
        self.last_window_geom = Some(new_geom.clone());
        self.do_callback(&mut vec![
            Event::WindowGeomChange(WindowGeomChangeEvent {
                window_id: self.window_id,
                old_geom: old_geom,
                new_geom: new_geom
            }),
            Event::Paint
        ]);
        // we should schedule a timer for +16ms another Paint
        
    }
    
    pub fn send_focus_event(&mut self) {
        self.do_callback(&mut vec![Event::AppFocus]);
    }
    
    pub fn send_focus_lost_event(&mut self) {
        self.do_callback(&mut vec![Event::AppFocusLost]);
    }
    
    pub fn mouse_down_can_drag_window(&mut self) -> bool {
        let mut events = vec![
            Event::WindowDragQuery(WindowDragQueryEvent {
                window_id: self.window_id,
                abs: self.last_mouse_pos,
                response: WindowDragQueryResponse::NoAnswer
            })
        ];
        self.do_callback(&mut events);
        match &events[0] {
            Event::WindowDragQuery(wd) => match &wd.response {
                WindowDragQueryResponse::Client => (),
                WindowDragQueryResponse::Caption | WindowDragQueryResponse::SysMenu => {
                    // we start a window drag
                    return true
                },
                _ => ()
            },
            _ => ()
        }
        return false
    }
    
    pub fn send_finger_down(&mut self, digit: usize, modifiers: KeyModifiers) {
        self.fingers_down[digit] = true;
        self.do_callback(&mut vec![Event::FingerDown(FingerDownEvent {
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            rel: self.last_mouse_pos,
            rect: Rect::default(),
            digit: digit,
            handled: false,
            is_touch: false,
            modifiers: modifiers,
            tap_count: 0,
            time: self.time_now()
        })]);
    }
    
    pub fn send_finger_up(&mut self, digit: usize, modifiers: KeyModifiers) {
        self.fingers_down[digit] = false;
        self.do_callback(&mut vec![Event::FingerUp(FingerUpEvent {
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            rel: self.last_mouse_pos,
            rect: Rect::default(),
            abs_start: Vec2::default(),
            rel_start: Vec2::default(),
            digit: digit,
            is_over: false,
            is_touch: false,
            modifiers: modifiers,
            time: self.time_now()
        })]);
    }
    
    pub fn send_finger_hover_and_move(&mut self, pos: Vec2, modifiers: KeyModifiers) {
        self.last_mouse_pos = pos;
        let mut events = Vec::new();
        for (digit, down) in self.fingers_down.iter().enumerate() {
            if *down {
                events.push(Event::FingerMove(FingerMoveEvent {
                    window_id: self.window_id,
                    abs: pos,
                    rel: pos,
                    rect: Rect::default(),
                    digit: digit,
                    abs_start: Vec2::default(),
                    rel_start: Vec2::default(),
                    is_over: false,
                    is_touch: false,
                    modifiers: modifiers.clone(),
                    time: self.time_now()
                }));
            }
        };
        events.push(Event::FingerHover(FingerHoverEvent {
            window_id: self.window_id,
            abs: pos,
            rel: pos,
            any_down: false,
            rect: Rect::default(),
            handled: false,
            hover_state: HoverState::Over,
            modifiers: modifiers,
            time: self.time_now()
        }));
        self.do_callback(&mut events);
    }
    
    pub fn send_window_close_requested_event(&mut self) -> bool {
        let mut events = vec![Event::WindowCloseRequested(WindowCloseRequestedEvent {
            window_id: self.window_id,
            accept_close: true
        })];
        self.do_callback(&mut events);
        if let Event::WindowCloseRequested(cre) = &events[0] {
            return cre.accept_close
        }
        true
    }
    
    pub fn send_window_closed_event(&mut self) {
        self.do_callback(&mut vec![Event::WindowClosed(WindowClosedEvent {
            window_id: self.window_id
        })])
    }
    
    pub fn send_text_input(&mut self, input: String, replace_last: bool) {
        self.do_callback(&mut vec![Event::TextInput(TextInputEvent {
            input: input,
            was_paste: false,
            replace_last: replace_last
        })])
    }
}

fn get_event_char(event: id) -> char {
    unsafe {
        let characters: id = msg_send![event, characters];
        if characters == nil {
            return '\0'
        }
        let chars = nsstring_to_string(characters);
        
        if chars.len() == 0 {
            return '\0'
        }
        chars.chars().next().unwrap()
    }
}

fn get_event_key_modifier(event: id) -> KeyModifiers {
    let flags:u64 = unsafe {msg_send![event, modifierFlags]};
    KeyModifiers {
        shift: flags & NSEventModifierFlags::NSShiftKeyMask as u64 != 0,
        control: flags & NSEventModifierFlags::NSControlKeyMask as u64 != 0,
        alt: flags & NSEventModifierFlags::NSAlternateKeyMask as u64 != 0,
        logo: flags & NSEventModifierFlags::NSCommandKeyMask as u64!= 0,
    }
}

fn get_event_keycode(event: id) -> Option<KeyCode> {
    let scan_code: std::os::raw::c_ushort = unsafe {
        msg_send![event, keyCode]
    };
    
    Some(match scan_code {
        0x00 => KeyCode::KeyA,
        0x01 => KeyCode::KeyS,
        0x02 => KeyCode::KeyD,
        0x03 => KeyCode::KeyF,
        0x04 => KeyCode::KeyH,
        0x05 => KeyCode::KeyG,
        0x06 => KeyCode::KeyZ,
        0x07 => KeyCode::KeyX,
        0x08 => KeyCode::KeyC,
        0x09 => KeyCode::KeyV,
        //0x0a => World 1,
        0x0b => KeyCode::KeyB,
        0x0c => KeyCode::KeyQ,
        0x0d => KeyCode::KeyW,
        0x0e => KeyCode::KeyE,
        0x0f => KeyCode::KeyR,
        0x10 => KeyCode::KeyY,
        0x11 => KeyCode::KeyT,
        0x12 => KeyCode::Key1,
        0x13 => KeyCode::Key2,
        0x14 => KeyCode::Key3,
        0x15 => KeyCode::Key4,
        0x16 => KeyCode::Key6,
        0x17 => KeyCode::Key5,
        0x18 => KeyCode::Equals,
        0x19 => KeyCode::Key9,
        0x1a => KeyCode::Key7,
        0x1b => KeyCode::Minus,
        0x1c => KeyCode::Key8,
        0x1d => KeyCode::Key0,
        0x1e => KeyCode::RBracket,
        0x1f => KeyCode::KeyO,
        0x20 => KeyCode::KeyU,
        0x21 => KeyCode::LBracket,
        0x22 => KeyCode::KeyI,
        0x23 => KeyCode::KeyP,
        0x24 => KeyCode::Return,
        0x25 => KeyCode::KeyL,
        0x26 => KeyCode::KeyJ,
        0x27 => KeyCode::Backtick,
        0x28 => KeyCode::KeyK,
        0x29 => KeyCode::Semicolon,
        0x2a => KeyCode::Backslash,
        0x2b => KeyCode::Comma,
        0x2c => KeyCode::Slash,
        0x2d => KeyCode::KeyN,
        0x2e => KeyCode::KeyM,
        0x2f => KeyCode::Period,
        0x30 => KeyCode::Tab,
        0x31 => KeyCode::Space,
        0x32 => KeyCode::Backtick,
        0x33 => KeyCode::Backspace,
        //0x34 => unkown,
        0x35 => KeyCode::Escape,
        //0x36 => KeyCode::RLogo,
        //0x37 => KeyCode::LLogo,
        //0x38 => KeyCode::LShift,
        0x39 => KeyCode::Capslock,
        //0x3a => KeyCode::LAlt,
        //0x3b => KeyCode::LControl,
        //0x3c => KeyCode::RShift,
        //0x3d => KeyCode::RAlt,
        //0x3e => KeyCode::RControl,
        //0x3f => Fn key,
        //0x40 => KeyCode::F17,
        0x41 => KeyCode::NumpadDecimal,
        //0x42 -> unkown,
        0x43 => KeyCode::NumpadMultiply,
        //0x44 => unkown,
        0x45 => KeyCode::NumpadAdd,
        //0x46 => unkown,
        0x47 => KeyCode::Numlock,
        //0x48 => KeypadClear,
        //0x49 => KeyCode::VolumeUp,
        //0x4a => KeyCode::VolumeDown,
        0x4b => KeyCode::NumpadDivide,
        0x4c => KeyCode::NumpadEnter,
        0x4e => KeyCode::NumpadSubtract,
        //0x4d => unkown,
        //0x4e => KeyCode::Subtract,
        //0x4f => KeyCode::F18,
        //0x50 => KeyCode::F19,
        0x51 => KeyCode::NumpadEquals,
        0x52 => KeyCode::Numpad0,
        0x53 => KeyCode::Numpad1,
        0x54 => KeyCode::Numpad2,
        0x55 => KeyCode::Numpad3,
        0x56 => KeyCode::Numpad4,
        0x57 => KeyCode::Numpad5,
        0x58 => KeyCode::Numpad6,
        0x59 => KeyCode::Numpad7,
        //0x5a => KeyCode::F20,
        0x5b => KeyCode::Numpad8,
        0x5c => KeyCode::Numpad9,
        //0x5d => KeyCode::Yen,
        //0x5e => JIS Ro,
        //0x5f => unkown,
        0x60 => KeyCode::F5,
        0x61 => KeyCode::F6,
        0x62 => KeyCode::F7,
        0x63 => KeyCode::F3,
        0x64 => KeyCode::F8,
        0x65 => KeyCode::F9,
        //0x66 => JIS Eisuu (macOS),
        0x67 => KeyCode::F11,
        //0x68 => JIS Kana (macOS),
        0x69 => KeyCode::PrintScreen,
        //0x6a => KeyCode::F16,
        //0x6b => KeyCode::F14,
        //0x6c => unkown,
        0x6d => KeyCode::F10,
        //0x6e => unkown,
        0x6f => KeyCode::F12,
        //0x70 => unkown,
        //0x71 => KeyCode::F15,
        0x72 => KeyCode::Insert,
        0x73 => KeyCode::Home,
        0x74 => KeyCode::PageUp,
        0x75 => KeyCode::Delete,
        0x76 => KeyCode::F4,
        0x77 => KeyCode::End,
        0x78 => KeyCode::F2,
        0x79 => KeyCode::PageDown,
        0x7a => KeyCode::F1,
        0x7b => KeyCode::ArrowLeft,
        0x7c => KeyCode::ArrowRight,
        0x7d => KeyCode::ArrowDown,
        0x7e => KeyCode::ArrowUp,
        //0x7f =>  unkown,
        //0xa => KeyCode::Caret,
        _ => return None,
    })
}

fn keycode_to_menu_key(keycode: KeyCode, shift: bool) -> &'static str {
    if !shift {
        match keycode {
            KeyCode::Backtick => "`",
            KeyCode::Key0 => "0",
            KeyCode::Key1 => "1",
            KeyCode::Key2 => "2",
            KeyCode::Key3 => "3",
            KeyCode::Key4 => "4",
            KeyCode::Key5 => "5",
            KeyCode::Key6 => "6",
            KeyCode::Key7 => "7",
            KeyCode::Key8 => "8",
            KeyCode::Key9 => "9",
            KeyCode::Minus => "-",
            KeyCode::Equals => "=",
            
            KeyCode::KeyQ => "q",
            KeyCode::KeyW => "w",
            KeyCode::KeyE => "e",
            KeyCode::KeyR => "r",
            KeyCode::KeyT => "t",
            KeyCode::KeyY => "y",
            KeyCode::KeyU => "u",
            KeyCode::KeyI => "i",
            KeyCode::KeyO => "o",
            KeyCode::KeyP => "p",
            KeyCode::LBracket => "[",
            KeyCode::RBracket => "]",
            
            KeyCode::KeyA => "a",
            KeyCode::KeyS => "s",
            KeyCode::KeyD => "d",
            KeyCode::KeyF => "f",
            KeyCode::KeyG => "g",
            KeyCode::KeyH => "h",
            KeyCode::KeyJ => "j",
            KeyCode::KeyK => "l",
            KeyCode::KeyL => "l",
            KeyCode::Semicolon => ";",
            KeyCode::Quote => "'",
            KeyCode::Backslash => "\\",
            
            KeyCode::KeyZ => "z",
            KeyCode::KeyX => "x",
            KeyCode::KeyC => "c",
            KeyCode::KeyV => "v",
            KeyCode::KeyB => "b",
            KeyCode::KeyN => "n",
            KeyCode::KeyM => "m",
            KeyCode::Comma => ",",
            KeyCode::Period => ".",
            KeyCode::Slash => "/",
            _ => ""
        }
    }
    else {
        match keycode {
            KeyCode::Backtick => "~",
            KeyCode::Key0 => "!",
            KeyCode::Key1 => "@",
            KeyCode::Key2 => "#",
            KeyCode::Key3 => "$",
            KeyCode::Key4 => "%",
            KeyCode::Key5 => "^",
            KeyCode::Key6 => "&",
            KeyCode::Key7 => "*",
            KeyCode::Key8 => "(",
            KeyCode::Key9 => ")",
            KeyCode::Minus => "_",
            KeyCode::Equals => "+",
            
            KeyCode::KeyQ => "Q",
            KeyCode::KeyW => "W",
            KeyCode::KeyE => "E",
            KeyCode::KeyR => "R",
            KeyCode::KeyT => "T",
            KeyCode::KeyY => "Y",
            KeyCode::KeyU => "U",
            KeyCode::KeyI => "I",
            KeyCode::KeyO => "O",
            KeyCode::KeyP => "P",
            KeyCode::LBracket => "{",
            KeyCode::RBracket => "}",
            
            KeyCode::KeyA => "A",
            KeyCode::KeyS => "S",
            KeyCode::KeyD => "D",
            KeyCode::KeyF => "F",
            KeyCode::KeyG => "G",
            KeyCode::KeyH => "H",
            KeyCode::KeyJ => "J",
            KeyCode::KeyK => "K",
            KeyCode::KeyL => "L",
            KeyCode::Semicolon => ":",
            KeyCode::Quote => "\"",
            KeyCode::Backslash => "|",
            
            KeyCode::KeyZ => "Z",
            KeyCode::KeyX => "X",
            KeyCode::KeyC => "C",
            KeyCode::KeyV => "V",
            KeyCode::KeyB => "B",
            KeyCode::KeyN => "N",
            KeyCode::KeyM => "M",
            KeyCode::Comma => "<",
            KeyCode::Period => ">",
            KeyCode::Slash => "?",
            _ => ""
        }
    }
}

fn get_cocoa_window(this: &Object) -> &mut CocoaWindow {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar("cocoa_window_ptr");
        &mut *(ptr as *mut CocoaWindow)
    }
}

fn get_cocoa_app(this: &Object) -> &mut CocoaApp {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar("cocoa_app_ptr");
        &mut *(ptr as *mut CocoaApp)
    }
}

pub fn define_cocoa_timer_delegate() -> *const Class {
    
    extern fn received_timer(this: &Object, _: Sel, nstimer: id) {
        let ca = get_cocoa_app(this);
        ca.send_timer_received(nstimer);
    }
    
    extern fn received_live_resize(this: &Object, _: Sel, _nstimer: id) {
        let ca = get_cocoa_app(this);
        ca.send_paint_event();
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("TimerDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(receivedTimer:), received_timer as extern fn(&Object, Sel, id));
        decl.add_method(sel!(receivedLiveResize:), received_live_resize as extern fn(&Object, Sel, id));
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("cocoa_app_ptr");
    
    return decl.register();
}

pub fn define_app_delegate() -> *const Class {
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("NSAppDelegate", superclass).unwrap();
    decl.add_ivar::<*mut c_void>("cocoa_app_ptr");
    return decl.register();
}

pub fn define_menu_target_class() -> *const Class {
    
    extern fn menu_action(this: &Object, _sel: Sel, _item: id) {
        //println!("markedRange");
        let ca = get_cocoa_app(this);
        unsafe {
            let command_usize: usize = *this.get_ivar("command_usize");
            let cmd = if let Ok(status_map) = ca.status_map.lock() {
                *status_map.usize_to_command.get(&command_usize).expect("")
            }
            else {
                panic!("Cannot lock cmd_map")
            };
            ca.send_command_event(cmd);
        }
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MenuTarget", superclass).unwrap();
    unsafe {
        decl.add_method(sel!(menuAction:), menu_action as extern fn(&Object, Sel, id));
    }
    decl.add_ivar::<*mut c_void>("cocoa_app_ptr");
    decl.add_ivar::<usize>("command_usize");
    return decl.register();
}

pub fn define_menu_delegate() -> *const Class {
    // NSMenuDelegate protocol
    extern fn menu_will_open(this: &Object, _sel: Sel, _item: id) {
        //println!("markedRange");
        let _ca = get_cocoa_app(this);
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MenuDelegate", superclass).unwrap();
    unsafe {
        decl.add_method(sel!(menuWillOpen:), menu_will_open as extern fn(&Object, Sel, id));
    }
    decl.add_ivar::<*mut c_void>("cocoa_app_ptr");
    decl.add_protocol(&Protocol::get("NSMenuDelegate").unwrap());
    return decl.register();
}


struct CocoaPostInit {
    cocoa_app_ptr: *mut CocoaApp,
    signal_id: u64,
    
}

pub fn define_cocoa_post_delegate() -> *const Class {
    
    extern fn received_post(this: &Object, _: Sel, _nstimer: id) {
        let ca = get_cocoa_app(this);
        unsafe {
            let signal_id: usize = *this.get_ivar("signal_id");
            let status: usize = *this.get_ivar("status");
            let status = if let Ok(status_map) = ca.status_map.lock() {
                *status_map.usize_to_status.get(&status).expect("status invalid")
            }
            else {
                panic!("cannot lock cmd_map")
            };
            ca.send_signal_event(Signal {signal_id: signal_id}, status);
        }
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("PostDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(receivedPost:), received_post as extern fn(&Object, Sel, id));
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("cocoa_app_ptr");
    decl.add_ivar::<usize>("signal_id");
    decl.add_ivar::<usize>("status");
    
    return decl.register();
}

pub fn define_cocoa_window_delegate() -> *const Class {
    
    extern fn window_should_close(this: &Object, _: Sel, _: id) -> BOOL {
        let cw = get_cocoa_window(this);
        if cw.send_window_close_requested_event() {
            YES
        }
        else {
            NO
        }
    }
    
    extern fn window_will_close(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.send_window_closed_event();
    }
    
    extern fn window_did_resize(this: &Object, _: Sel, _: id) {
        let _cw = get_cocoa_window(this);
        //cw.send_change_event();
    }
    
    extern fn window_will_start_live_resize(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.start_live_resize();
    }
    
    extern fn window_did_end_live_resize(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.end_live_resize();
    }
    
    // This won't be triggered if the move was part of a resize.
    extern fn window_did_move(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.send_change_event();
    }
    
    extern fn window_did_change_screen(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.send_change_event();
    }
    
    // This will always be called before `window_did_change_screen`.
    extern fn window_did_change_backing_properties(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.send_change_event();
    }
    
    extern fn window_did_become_key(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.send_focus_event();
    }
    
    extern fn window_did_resign_key(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.send_focus_lost_event();
    }
    
    // Invoked when the dragged image enters destination bounds or frame
    extern fn dragging_entered(_this: &Object, _: Sel, _sender: id) -> BOOL {
        YES
    }
    
    // Invoked when the image is released
    extern fn prepare_for_drag_operation(_: &Object, _: Sel, _: id) -> BOOL {
        YES
    }
    
    // Invoked after the released image has been removed from the screen
    extern fn perform_drag_operation(_this: &Object, _: Sel, _sender: id) -> BOOL {
        YES
    }
    
    // Invoked when the dragging operation is complete
    extern fn conclude_drag_operation(_: &Object, _: Sel, _: id) {}
    
    // Invoked when the dragging operation is cancelled
    extern fn dragging_exited(this: &Object, _: Sel, _: id) {
        let _cw = get_cocoa_window(this);
        //WindowDelegate::emit_event(state, WindowEvent::HoveredFileCancelled);
    }
    
    // Invoked when entered fullscreen
    extern fn window_did_enter_fullscreen(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.is_fullscreen = true;
        cw.send_change_event();
    }
    
    // Invoked when before enter fullscreen
    extern fn window_will_enter_fullscreen(this: &Object, _: Sel, _: id) {
        let _cw = get_cocoa_window(this);
    }
    
    // Invoked when exited fullscreen
    extern fn window_did_exit_fullscreen(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.is_fullscreen = false;
        cw.send_change_event();
    }
    
    extern fn window_did_fail_to_enter_fullscreen(_this: &Object, _: Sel, _: id) {
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("RenderWindowDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(windowShouldClose:), window_should_close as extern fn(&Object, Sel, id) -> BOOL);
        decl.add_method(sel!(windowWillClose:), window_will_close as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidResize:), window_did_resize as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowWillStartLiveResize:), window_will_start_live_resize as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidEndLiveResize:), window_did_end_live_resize as extern fn(&Object, Sel, id));
        
        decl.add_method(sel!(windowDidMove:), window_did_move as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidChangeScreen:), window_did_change_screen as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidChangeBackingProperties:), window_did_change_backing_properties as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidBecomeKey:), window_did_become_key as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidResignKey:), window_did_resign_key as extern fn(&Object, Sel, id));
        
        // callbacks for drag and drop events
        decl.add_method(sel!(draggingEntered:), dragging_entered as extern fn(&Object, Sel, id) -> BOOL);
        decl.add_method(sel!(prepareForDragOperation:), prepare_for_drag_operation as extern fn(&Object, Sel, id) -> BOOL);
        decl.add_method(sel!(performDragOperation:), perform_drag_operation as extern fn(&Object, Sel, id) -> BOOL);
        decl.add_method(sel!(concludeDragOperation:), conclude_drag_operation as extern fn(&Object, Sel, id));
        decl.add_method(sel!(draggingExited:), dragging_exited as extern fn(&Object, Sel, id));
        
        // callbacks for fullscreen events
        decl.add_method(sel!(windowDidEnterFullScreen:), window_did_enter_fullscreen as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowWillEnterFullScreen:), window_will_enter_fullscreen as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidExitFullScreen:), window_did_exit_fullscreen as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidFailToEnterFullScreen:), window_did_fail_to_enter_fullscreen as extern fn(&Object, Sel, id));
        // custom timer fn
        //decl.add_method(sel!(windowReceivedTimer:), window_received_timer as extern fn(&Object, Sel, id));
        
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("cocoa_window_ptr");
    
    return decl.register();
}

pub fn define_cocoa_window_class() -> *const Class {
    extern fn yes(_: &Object, _: Sel) -> BOOL {
        YES
    }
    
    extern fn is_movable_by_window_background(_: &Object, _: Sel) -> BOOL {
        println!("CALLED!");
        YES
    }
    
    let window_superclass = class!(NSWindow);
    let mut decl = ClassDecl::new("RenderWindow", window_superclass).unwrap();
    unsafe {
        decl.add_method(sel!(canBecomeMainWindow), yes as extern fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(canBecomeKeyWindow), yes as extern fn(&Object, Sel) -> BOOL);
    }
    return decl.register();
}

pub fn define_cocoa_view_class() -> *const Class {
    
    extern fn dealloc(this: &Object, _sel: Sel) {
        unsafe {
            let marked_text: id = *this.get_ivar("markedText");
            let _: () = msg_send![marked_text, release];
        }
    }
    
    extern fn init_with_ptr(this: &Object, _sel: Sel, cx: *mut c_void) -> id {
        unsafe {
            let this: id = msg_send![this, init];
            if this != nil {
                (*this).set_ivar("cocoa_window_ptr", cx);
                let marked_text = <id as NSMutableAttributedString>::init(
                    NSMutableAttributedString::alloc(nil),
                );
                (*this).set_ivar("markedText", marked_text);
            }
            this
        }
    }
    
    extern fn mouse_down(this: &Object, _sel: Sel, event: id) {
        
        let cw = get_cocoa_window(this);
        unsafe {
            if cw.mouse_down_can_drag_window() {
                let () = msg_send![cw.window, performWindowDragWithEvent: event];
                return
            }
        }
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_down(0, modifiers);
    }
    
    extern fn mouse_up(this: &Object, _sel: Sel, event: id) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_up(0, modifiers);
    }
    
    extern fn right_mouse_down(this: &Object, _sel: Sel, event: id) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_down(1, modifiers);
    }
    
    extern fn right_mouse_up(this: &Object, _sel: Sel, event: id) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_up(1, modifiers);
    }
    
    extern fn other_mouse_down(this: &Object, _sel: Sel, event: id) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_down(2, modifiers);
    }
    
    extern fn other_mouse_up(this: &Object, _sel: Sel, event: id) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_up(2, modifiers);
    }
    
    fn mouse_pos_from_event(this: &Object, event: id) -> Vec2 {
        // We have to do this to have access to the `NSView` trait...
        unsafe {
            let view: id = this as *const _ as *mut _;
            let window_point: NSPoint = msg_send![event, locationInWindow]; //.locationInWindow();
            let view_point: NSPoint = msg_send![view, convertPoint: window_point fromView: nil]; // view.convertPoint_fromView_(window_point, nil);
            let view_rect: NSRect = msg_send![view, frame];
            Vec2 {x: view_point.x as f32, y: view_rect.size.height as f32 - view_point.y as f32}
        }
    }
    
    fn mouse_motion(this: &Object, event: id) {
        let cw = get_cocoa_window(this);
        let pos = mouse_pos_from_event(this, event);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_hover_and_move(pos, modifiers);
    }
    
    extern fn mouse_moved(this: &Object, _sel: Sel, event: id) {
        mouse_motion(this, event);
    }
    
    extern fn mouse_dragged(this: &Object, _sel: Sel, event: id) {
        mouse_motion(this, event);
    }
    
    extern fn right_mouse_dragged(this: &Object, _sel: Sel, event: id) {
        mouse_motion(this, event);
    }
    
    extern fn other_mouse_dragged(this: &Object, _sel: Sel, event: id) {
        mouse_motion(this, event);
    }
    
    extern fn draw_rect(this: &Object, _sel: Sel, rect: NSRect) {
        let _cw = get_cocoa_window(this);
        unsafe {
            let superclass = superclass(this);
            let () = msg_send![super (this, superclass), drawRect: rect];
        }
    }
    
    extern fn reset_cursor_rects(this: &Object, _sel: Sel) {
        let cw = get_cocoa_window(this);
        unsafe {
            let cocoa_app = &mut (*cw.cocoa_app);
            let current_cursor = cocoa_app.current_cursor.clone();
            let cursor_id = *cocoa_app.cursors.entry(current_cursor.clone()).or_insert_with( || {
                load_mouse_cursor(current_cursor.clone())
            });
            let bounds: NSRect = msg_send![this, bounds];
            let _: () = msg_send![
                this,
                addCursorRect: bounds
                cursor: cursor_id
            ];
        }
    }
    
    // NSTextInput protocol
    extern fn marked_range(this: &Object, _sel: Sel) -> NSRange {
        //println!("markedRange");
        unsafe {
            let marked_text: id = *this.get_ivar("markedText");
            let length = marked_text.length();
            if length >0 {
                NSRange {
                    location: 0,
                    length: length - 1
                }
            } else {
                NSRange {
                    location: i64::max_value() as u64,
                    length: 0,
                }
            }
        }
    }
    
    extern fn selected_range(_this: &Object, _sel: Sel) -> NSRange {
        NSRange {
            location: 0,
            length: 1,
        }
    }
    
    extern fn has_marked_text(this: &Object, _sel: Sel) -> BOOL {
        unsafe {
            let marked_text: id = *this.get_ivar("markedText");
            (marked_text.length() >0) as i8
        }
    }
    
    extern fn set_marked_text(this: &mut Object, _sel: Sel, string: id, _selected_range: NSRange, _replacement_range: NSRange) {
        unsafe {
            let marked_text_ref: &mut id = this.get_mut_ivar("markedText");
            let _: () = msg_send![(*marked_text_ref), release];
            let marked_text = NSMutableAttributedString::alloc(nil);
            let has_attr = msg_send![string, isKindOfClass: class!(NSAttributedString)];
            if has_attr {
                marked_text.init_with_attributed_string(string);
            } else {
                marked_text.init_with_string(string);
            };
            *marked_text_ref = marked_text;
        }
    }
    
    extern fn unmark_text(this: &Object, _sel: Sel) {
        let cw = get_cocoa_window(this);
        unsafe {
            let cocoa_app = &(*cw.cocoa_app);
            let marked_text: id = *this.get_ivar("markedText");
            let mutable_string = marked_text.mutable_string();
            let _: () = msg_send![mutable_string, setString: cocoa_app.const_empty_string];
            let input_context: id = msg_send![this, inputContext];
            let _: () = msg_send![input_context, discardMarkedText];
        }
    }
    
    extern fn valid_attributes_for_marked_text(this: &Object, _sel: Sel) -> id {
        let cw = get_cocoa_window(this);
        unsafe {
            let cocoa_app = &(*cw.cocoa_app);
            cocoa_app.const_attributes_for_marked_text
        }
    }
    
    extern fn attributed_substring_for_proposed_range(_this: &Object, _sel: Sel, _range: NSRange, _actual_range: *mut c_void) -> id {
        nil
    }
    
    extern fn character_index_for_point(_this: &Object, _sel: Sel, _point: NSPoint) -> u64 {
        // println!("character_index_for_point");
        0
    }
    
    extern fn first_rect_for_character_range(this: &Object, _sel: Sel, _range: NSRange, _actual_range: *mut c_void) -> NSRect {
        let cw = get_cocoa_window(this);
        
        let view: id = this as *const _ as *mut _;
        //let window_point = event.locationInWindow();
        //et view_point = view.convertPoint_fromView_(window_point, nil);
        let view_rect: NSRect = unsafe {msg_send![view, frame]};
        let window_rect: NSRect = unsafe {msg_send![cw.window, frame]};
        
        let origin = cw.get_ime_origin();
        let bar = (window_rect.size.height - view_rect.size.height) as f32 - 5.;
        NSRect {
            origin: NSPoint {x: (origin.x + cw.ime_spot.x) as f64, y: (origin.y + (view_rect.size.height as f32 - cw.ime_spot.y - bar)) as f64},
            // as _, y as _),
            size: NSSize {width: 0.0, height: 0.0},
        }
    }
    
    extern fn insert_text(this: &Object, _sel: Sel, string: id, replacement_range: NSRange) {
        let cw = get_cocoa_window(this);
        unsafe {
            let has_attr = msg_send![string, isKindOfClass: class!(NSAttributedString)];
            let characters = if has_attr {
                msg_send![string, string]
            } else {
                string
            };
            let string = nsstring_to_string(characters);
            cw.send_text_input(string, replacement_range.length != 0);
            let input_context: id = msg_send![this, inputContext];
            let () = msg_send![input_context, invalidateCharacterCoordinates];
            let () = msg_send![cw.view, setNeedsDisplay: YES];
            unmark_text(this, _sel);
        }
    }
    
    extern fn do_command_by_selector(this: &Object, _sel: Sel, _command: Sel) {
        let _cw = get_cocoa_window(this);
    }
    
    extern fn key_down(this: &Object, _sel: Sel, event: id) {
        let _cw = get_cocoa_window(this);
        unsafe {
            let input_context: id = msg_send![this, inputContext];
            let () = msg_send![input_context, handleEvent: event];
        }
    }
    
    extern fn key_up(_this: &Object, _sel: Sel, _event: id) {
    }
    
    extern fn insert_tab(this: &Object, _sel: Sel, _sender: id) {
        unsafe {
            let window: id = msg_send![this, window];
            let first_responder: id = msg_send![window, firstResponder];
            let this_ptr = this as *const _ as *mut _;
            if first_responder == this_ptr {
                let (): _ = msg_send![window, selectNextKeyView: this];
            }
        }
    }
    
    extern fn insert_back_tab(this: &Object, _sel: Sel, _sender: id) {
        unsafe {
            let window: id = msg_send![this, window];
            let first_responder: id = msg_send![window, firstResponder];
            let this_ptr = this as *const _ as *mut _;
            if first_responder == this_ptr {
                let (): _ = msg_send![window, selectPreviousKeyView: this];
            }
        }
    }
    
    extern fn yes_function(_this: &Object, _se: Sel, _event: id) -> BOOL {
        YES
    }
    
    extern fn display_layer(this: &Object, _: Sel, _calayer: id) {
        let cw = get_cocoa_window(this);
        cw.send_change_event();
    }
    /*
    extern fn draw(this: &Object, _: Sel, _calayer: id, _cgcontext: id) {
        println!("draw");
        //let cw = get_cocoa_window(this);
        //cw.send_change_event();
    }

    extern fn layer_will_draw(this: &Object, _: Sel, _calayer: id) {
        println!("layer_will_draw");
        //let cw = get_cocoa_window(this);
        //cw.send_change_event();
    }*/
    
    
    let superclass = class!(NSView);
    let mut decl = ClassDecl::new("RenderViewClass", superclass).unwrap();
    unsafe {
        decl.add_method(sel!(dealloc), dealloc as extern fn(&Object, Sel));
        decl.add_method(sel!(initWithPtr:), init_with_ptr as extern fn(&Object, Sel, *mut c_void) -> id);
        decl.add_method(sel!(drawRect:), draw_rect as extern fn(&Object, Sel, NSRect));
        decl.add_method(sel!(resetCursorRects), reset_cursor_rects as extern fn(&Object, Sel));
        decl.add_method(sel!(hasMarkedText), has_marked_text as extern fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(markedRange), marked_range as extern fn(&Object, Sel) -> NSRange);
        decl.add_method(sel!(selectedRange), selected_range as extern fn(&Object, Sel) -> NSRange);
        decl.add_method(sel!(setMarkedText: selectedRange: replacementRange:), set_marked_text as extern fn(&mut Object, Sel, id, NSRange, NSRange));
        decl.add_method(sel!(unmarkText), unmark_text as extern fn(&Object, Sel));
        decl.add_method(sel!(validAttributesForMarkedText), valid_attributes_for_marked_text as extern fn(&Object, Sel) -> id);
        decl.add_method(
            sel!(attributedSubstringForProposedRange: actualRange:),
            attributed_substring_for_proposed_range
            as extern fn(&Object, Sel, NSRange, *mut c_void) -> id,
        );
        decl.add_method(
            sel!(insertText: replacementRange:),
            insert_text as extern fn(&Object, Sel, id, NSRange),
        );
        decl.add_method(
            sel!(characterIndexForPoint:),
            character_index_for_point as extern fn(&Object, Sel, NSPoint) -> u64,
        );
        decl.add_method(
            sel!(firstRectForCharacterRange: actualRange:),
            first_rect_for_character_range
            as extern fn(&Object, Sel, NSRange, *mut c_void) -> NSRect,
        );
        decl.add_method(sel!(doCommandBySelector:), do_command_by_selector as extern fn(&Object, Sel, Sel));
        decl.add_method(sel!(keyDown:), key_down as extern fn(&Object, Sel, id));
        decl.add_method(sel!(keyUp:), key_up as extern fn(&Object, Sel, id));
        //decl.add_method(sel!(insertTab:), insert_tab as extern fn(&Object, Sel, id));
        //decl.add_method(sel!(insertBackTab:), insert_back_tab as extern fn(&Object, Sel, id));
        decl.add_method(sel!(mouseDown:), mouse_down as extern fn(&Object, Sel, id));
        decl.add_method(sel!(mouseUp:), mouse_up as extern fn(&Object, Sel, id));
        decl.add_method(sel!(rightMouseDown:), right_mouse_down as extern fn(&Object, Sel, id));
        decl.add_method(sel!(rightMouseUp:), right_mouse_up as extern fn(&Object, Sel, id));
        decl.add_method(sel!(otherMouseDown:), other_mouse_down as extern fn(&Object, Sel, id));
        decl.add_method(sel!(otherMouseUp:), other_mouse_up as extern fn(&Object, Sel, id));
        decl.add_method(sel!(mouseMoved:), mouse_moved as extern fn(&Object, Sel, id));
        decl.add_method(sel!(mouseDragged:), mouse_dragged as extern fn(&Object, Sel, id));
        decl.add_method(sel!(rightMouseDragged:), right_mouse_dragged as extern fn(&Object, Sel, id));
        decl.add_method(sel!(otherMouseDragged:), other_mouse_dragged as extern fn(&Object, Sel, id));
        decl.add_method(sel!(wantsKeyDownForEvent:), yes_function as extern fn(&Object, Sel, id) -> BOOL);
        decl.add_method(sel!(acceptsFirstResponder:), yes_function as extern fn(&Object, Sel, id) -> BOOL);
        decl.add_method(sel!(becomeFirstResponder:), yes_function as extern fn(&Object, Sel, id) -> BOOL);
        decl.add_method(sel!(resignFirstResponder:), yes_function as extern fn(&Object, Sel, id) -> BOOL);
        
        decl.add_method(sel!(displayLayer:), display_layer as extern fn(&Object, Sel, id));
    }
    decl.add_ivar::<*mut c_void>("cocoa_window_ptr");
    decl.add_ivar::<id>("markedText");
    decl.add_protocol(&Protocol::get("NSTextInputClient").unwrap());
    decl.add_protocol(&Protocol::get("CALayerDelegate").unwrap());
    return decl.register();
}

pub unsafe fn superclass<'a>(this: &'a Object) -> &'a Class {
    let superclass: id = msg_send![this, superclass];
    &*(superclass as *const _)
}

pub fn bottom_left_to_top_left(rect: NSRect) -> f64 {
    let height = unsafe {CGDisplayPixelsHigh(CGMainDisplayID())};
    height as f64 - (rect.origin.y + rect.size.height)
}

fn load_mouse_cursor(cursor: MouseCursor) -> id {
    match cursor {
        MouseCursor::Arrow | MouseCursor::Default | MouseCursor::Hidden => load_native_cursor("arrowCursor"),
        MouseCursor::Hand => load_native_cursor("pointingHandCursor"),
        MouseCursor::Text => load_native_cursor("IBeamCursor"),
        MouseCursor::NotAllowed /*| MouseCursor::NoDrop*/ => load_native_cursor("operationNotAllowedCursor"),
        MouseCursor::Crosshair => load_native_cursor("crosshairCursor"),
        /*
        MouseCursor::Grabbing | MouseCursor::Grab => load_native_cursor("closedHandCursor"),
        MouseCursor::VerticalText => load_native_cursor("IBeamCursorForVerticalLayout"),
        MouseCursor::Copy => load_native_cursor("dragCopyCursor"),
        MouseCursor::Alias => load_native_cursor("dragLinkCursor"),
        MouseCursor::ContextMenu => load_native_cursor("contextualMenuCursor"),
        */
        MouseCursor::EResize => load_native_cursor("resizeRightCursor"),
        MouseCursor::NResize => load_native_cursor("resizeUpCursor"),
        MouseCursor::WResize => load_native_cursor("resizeLeftCursor"),
        MouseCursor::SResize => load_native_cursor("resizeDownCursor"),
        MouseCursor::NeResize => load_undocumented_cursor("_windowResizeNorthEastCursor"),
        MouseCursor::NwResize => load_undocumented_cursor("_windowResizeNorthWestCursor"),
        MouseCursor::SeResize => load_undocumented_cursor("_windowResizeSouthEastCursor"),
        MouseCursor::SwResize => load_undocumented_cursor("_windowResizeSouthWestCursor"),
        
        MouseCursor::EwResize | MouseCursor::ColResize => load_native_cursor("resizeLeftRightCursor"),
        MouseCursor::NsResize | MouseCursor::RowResize => load_native_cursor("resizeUpDownCursor"),
        
        // Undocumented cursors: https://stackoverflow.com/a/46635398/5435443
        MouseCursor::Help => load_undocumented_cursor("_helpCursor"),
        //MouseCursor::ZoomIn => load_undocumented_cursor("_zoomInCursor"),
        //MouseCursor::ZoomOut => load_undocumented_cursor("_zoomOutCursor"),
        
        MouseCursor::NeswResize => load_undocumented_cursor("_windowResizeNorthEastSouthWestCursor"),
        MouseCursor::NwseResize => load_undocumented_cursor("_windowResizeNorthWestSouthEastCursor"),
        
        // While these are available, the former just loads a white arrow,
        // and the latter loads an ugly deflated beachball!
        // MouseCursor::Move => Cursor::Undocumented("_moveCursor"),
        // MouseCursor::Wait => Cursor::Undocumented("_waitCursor"),
        // An even more undocumented cursor...
        // https://bugs.eclipse.org/bugs/show_bug.cgi?id=522349
        // This is the wrong semantics for `Wait`, but it's the same as
        // what's used in Safari and Chrome.
        MouseCursor::Wait/* | MouseCursor::Progress*/ => load_undocumented_cursor("busyButClickableCursor"),
        
        // For the rest, we can just snatch the cursors from WebKit...
        // They fit the style of the native cursors, and will seem
        // completely standard to macOS users.
        // https://stackoverflow.com/a/21786835/5435443
        MouseCursor::Move /*| MouseCursor::AllScroll*/ => load_webkit_cursor("move"),
        // MouseCursor::Cell => load_webkit_cursor("cell"),
    }
}