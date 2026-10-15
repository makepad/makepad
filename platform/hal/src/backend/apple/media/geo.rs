//! CoreLocation for `Cx::start_location_updates`. The `CLLocationManager` lives on the main
//! thread (its delegate callbacks follow the run loop of the thread that made it); its
//! callbacks fill a mailbox the session's step turns into `Location` and `Permission` events.
//!
//! Moved from os/apple/apple_geo.rs. Without the window system's main thread (offscreen
//! sessions) there is no location service: a start answers with an `Unavailable` fix error.

use super::Signal;
use makepad_apple_sys::*;
use std::os::raw::c_void;
use std::sync::{Arc, Mutex, OnceLock};

// CLAuthorizationStatus values
pub const CL_AUTH_NOT_DETERMINED: i32 = 0;
pub const CL_AUTH_RESTRICTED: i32 = 1;
pub const CL_AUTH_DENIED: i32 = 2;
pub const CL_AUTH_AUTHORIZED_ALWAYS: i32 = 3;
pub const CL_AUTH_AUTHORIZED_WHEN_IN_USE: i32 = 4;

// CLError codes
const CL_ERROR_LOCATION_UNKNOWN: i64 = 0;
const CL_ERROR_DENIED: i64 = 1;

/// What the location manager's delegate reported, for the session's step.
pub enum GeoMsg {
    Fix { lon: f64, lat: f64, accuracy_m: f64, altitude: Option<f64>, speed: Option<f64>, heading: Option<f64>, time: f64 },
    Denied,
    Unavailable(String),
    /// the answer to a `Permission::Location` request (the request id)
    Permission(i32, i32),
}

/// What the delegate callbacks (main thread) and the commands (any thread) share.
struct Shared {
    out: Mutex<Vec<GeoMsg>>,
    signal: Signal,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The app called start and expects updates (survives auth round-trips).
    running_wanted: bool,
    /// A `Permission::Location` request waiting on the auth dialog.
    pending_permission: Option<i32>,
    denied_reported: bool,
}

impl Shared {
    fn push(&self, m: GeoMsg) {
        self.out.lock().unwrap_or_else(|e| e.into_inner()).push(m);
        self.signal.set();
    }

    fn report_denied_once(&self) {
        let first = !std::mem::replace(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()).denied_reported, true);
        if first {
            self.push(GeoMsg::Denied);
        }
    }
}

/// The manager and its delegate (main thread objects).
struct Manager {
    manager: RcObjcId,
    delegate: RcObjcId,
    _callback: Box<Box<dyn Fn(Event) + Send + 'static>>,
}

// SAFETY: only ever used from the main thread, but stored where the session's thread owns it
unsafe impl Send for Manager {}

enum Event {
    Update(GeoMsg),
    Error(GeoMsg),
    AuthChanged(i32, ObjcId),
}

impl Drop for Manager {
    fn drop(&mut self) {
        unsafe {
            let () = msg_send![self.manager.as_id(), setDelegate: nil];
            (*self.delegate.as_id()).set_ivar("callback", 0 as *mut c_void);
        }
    }
}

pub fn auth_status_to_granted(status: i32) -> u32 {
    use crate::media::permission_status as p;
    match status {
        CL_AUTH_AUTHORIZED_ALWAYS | CL_AUTH_AUTHORIZED_WHEN_IN_USE => p::GRANTED,
        CL_AUTH_RESTRICTED | CL_AUTH_DENIED => p::DENIED_PERMANENT,
        _ => p::NOT_DETERMINED,
    }
}

/// The current CoreLocation authorization, without making a manager.
pub fn location_permission_status() -> u32 {
    let status: i32 = unsafe { msg_send![class!(CLLocationManager), authorizationStatus] };
    auth_status_to_granted(status)
}

pub struct Geo {
    shared: Arc<Shared>,
    /// made on the main thread when first needed
    manager: Arc<Mutex<Option<Manager>>>,
}

impl Geo {
    pub fn new(signal: Signal) -> Geo {
        Geo { shared: Arc::new(Shared { out: Mutex::new(Vec::new()), signal, state: Mutex::new(State::default()) }), manager: Arc::new(Mutex::new(None)) }
    }

    pub fn take(&self) -> Vec<GeoMsg> {
        std::mem::take(&mut *self.shared.out.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Runs `f` with the manager on the main thread (made first when it is not there yet).
    fn with_manager(&self, f: impl FnOnce(&Shared, &Manager) + Send + 'static) {
        let (shared, slot) = (self.shared.clone(), self.manager.clone());
        let queued = crate::backend::apple::shell::on_main(move || {
            let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
            let m = slot.get_or_insert_with(|| Manager::new(shared.clone()));
            f(&shared, m);
        });
        if !queued {
            self.shared.push(GeoMsg::Unavailable("no location service without the window system".into()));
        }
    }

    /// Start (or keep) streaming fixes. Prompts for permission when the status is not
    /// determined yet; reports denial.
    pub fn start(&self) {
        self.with_manager(|shared, m| {
            {
                let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                st.running_wanted = true;
                st.denied_reported = false;
            }
            unsafe {
                let status: i32 = msg_send![m.manager.as_id(), authorizationStatus];
                match status {
                    CL_AUTH_RESTRICTED | CL_AUTH_DENIED => shared.report_denied_once(),
                    CL_AUTH_NOT_DETERMINED => {
                        let () = msg_send![m.manager.as_id(), requestWhenInUseAuthorization];
                        // Safe pre-authorization; fixes flow once granted.
                        let () = msg_send![m.manager.as_id(), startUpdatingLocation];
                    }
                    _ => {
                        let () = msg_send![m.manager.as_id(), startUpdatingLocation];
                    }
                }
            }
        });
    }

    pub fn stop(&self) {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).running_wanted = false;
        // a manager that was never made has nothing to stop
        let slot = self.manager.clone();
        crate::backend::apple::shell::on_main(move || {
            if let Some(m) = &*slot.lock().unwrap_or_else(|e| e.into_inner()) {
                unsafe {
                    let () = msg_send![m.manager.as_id(), stopUpdatingLocation];
                }
            }
        });
    }

    /// Fires the system location-permission dialog; the answer is a `GeoMsg::Permission`.
    pub fn request_permission(&self, request_id: i32) {
        self.with_manager(move |shared, m| {
            shared.state.lock().unwrap_or_else(|e| e.into_inner()).pending_permission = Some(request_id);
            unsafe {
                let () = msg_send![m.manager.as_id(), requestWhenInUseAuthorization];
            }
        });
    }
}

impl Manager {
    /// Main thread.
    fn new(shared: Arc<Shared>) -> Manager {
        unsafe {
            let callback: Box<dyn Fn(Event) + Send + 'static> = Box::new(move |event| match event {
                Event::Update(m) | Event::Error(m) => shared.push(m),
                Event::AuthChanged(status, manager) => {
                    let (request, running) = {
                        let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                        let request = if status != CL_AUTH_NOT_DETERMINED { st.pending_permission.take() } else { None };
                        (request, st.running_wanted)
                    };
                    if let Some(request_id) = request {
                        shared.push(GeoMsg::Permission(request_id, status));
                    }
                    if running {
                        match status {
                            CL_AUTH_AUTHORIZED_ALWAYS | CL_AUTH_AUTHORIZED_WHEN_IN_USE => {
                                let () = msg_send![manager, startUpdatingLocation];
                            }
                            CL_AUTH_RESTRICTED | CL_AUTH_DENIED => shared.report_denied_once(),
                            _ => {}
                        }
                    }
                }
            });
            let double_box = Box::new(callback);
            let delegate = RcObjcId::from_owned(msg_send![delegate_class(), new]);
            (*delegate.as_id()).set_ivar("callback", &*double_box as *const _ as *const c_void);

            let manager: RcObjcId = RcObjcId::from_owned(msg_send![class!(CLLocationManager), new]);
            let () = msg_send![manager.as_id(), setDesiredAccuracy: kCLLocationAccuracyBest];
            let () = msg_send![manager.as_id(), setDistanceFilter: 3.0f64];
            // Triggers an initial locationManagerDidChangeAuthorization callback.
            let () = msg_send![manager.as_id(), setDelegate: delegate.as_id()];
            Manager { manager, delegate, _callback: double_box }
        }
    }
}

fn delegate_class() -> *const Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| define_cl_location_manager_delegate() as usize) as *const Class
}

fn define_cl_location_manager_delegate() -> *const Class {
    extern "C" fn did_update_locations(this: &Object, _: Sel, _manager: ObjcId, locations: ObjcId) {
        unsafe {
            let ptr: *const c_void = *this.get_ivar("callback");
            if ptr == 0 as *const c_void {
                return;
            }
            let count: u64 = msg_send![locations, count];
            if count == 0 {
                return;
            }
            let location: ObjcId = msg_send![locations, objectAtIndex: count - 1];
            let coord: CLLocationCoordinate2D = msg_send![location, coordinate];
            let accuracy_m: f64 = msg_send![location, horizontalAccuracy];
            if accuracy_m < 0.0 {
                // invalid fix
                return;
            }
            let vertical_accuracy: f64 = msg_send![location, verticalAccuracy];
            let altitude: f64 = msg_send![location, altitude];
            let speed: f64 = msg_send![location, speed];
            let course: f64 = msg_send![location, course];
            let timestamp: ObjcId = msg_send![location, timestamp];
            let time: f64 = msg_send![timestamp, timeIntervalSince1970];
            let event = GeoMsg::Fix {
                lon: coord.longitude,
                lat: coord.latitude,
                accuracy_m,
                altitude: if vertical_accuracy >= 0.0 { Some(altitude) } else { None },
                speed: if speed >= 0.0 { Some(speed) } else { None },
                heading: if course >= 0.0 { Some(course) } else { None },
                time,
            };
            (*(ptr as *const Box<dyn Fn(Event)>))(Event::Update(event));
        }
    }

    extern "C" fn did_fail_with_error(this: &Object, _: Sel, _manager: ObjcId, error: ObjcId) {
        unsafe {
            let ptr: *const c_void = *this.get_ivar("callback");
            if ptr == 0 as *const c_void {
                return;
            }
            let code: i64 = msg_send![error, code];
            let event = match code {
                // transient "keep waiting", not an error
                CL_ERROR_LOCATION_UNKNOWN => return,
                CL_ERROR_DENIED => GeoMsg::Denied,
                _ => GeoMsg::Unavailable(format!("CLError code {code}")),
            };
            (*(ptr as *const Box<dyn Fn(Event)>))(Event::Error(event));
        }
    }

    extern "C" fn did_change_authorization(this: &Object, _: Sel, manager: ObjcId) {
        unsafe {
            let ptr: *const c_void = *this.get_ivar("callback");
            if ptr == 0 as *const c_void {
                return;
            }
            let status: i32 = msg_send![manager, authorizationStatus];
            (*(ptr as *const Box<dyn Fn(Event)>))(Event::AuthChanged(status, manager));
        }
    }

    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MakepadCLLocationManagerDelegate", superclass).unwrap();
    unsafe {
        decl.add_method(sel!(locationManager: didUpdateLocations:), did_update_locations as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
        decl.add_method(sel!(locationManager: didFailWithError:), did_fail_with_error as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
        decl.add_method(sel!(locationManagerDidChangeAuthorization:), did_change_authorization as extern "C" fn(&Object, Sel, ObjcId));
        if let Some(protocol) = Protocol::get("CLLocationManagerDelegate") {
            decl.add_protocol(protocol);
        }
    }
    decl.add_ivar::<*mut c_void>("callback");
    decl.register()
}
