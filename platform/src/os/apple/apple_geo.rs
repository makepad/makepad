//! CoreLocation backend for `Cx::start_location_updates` (macOS + iOS).
//!
//! A single `CLLocationManager` is created lazily on the main thread; its
//! delegate forwards fixes/errors/authorization changes over a channel and
//! wakes the event loop via `SignalToUI`. `handle_location_signals` drains
//! the channel on the main loop and emits `Event::LocationUpdate` /
//! `Event::LocationError` / `Event::PermissionResult`.

use {
    crate::{
        apple_classes::get_apple_class_global,
        cx::Cx,
        event::{Event, LocationErrorEvent, LocationUpdateEvent},
        os::apple::apple_sys::*,
        permission::{Permission, PermissionResult, PermissionStatus},
        thread::SignalToUI,
    },
    std::{
        os::raw::c_void,
        sync::mpsc::{channel, Receiver, Sender},
    },
};

// CLAuthorizationStatus values
const CL_AUTH_NOT_DETERMINED: i32 = 0;
const CL_AUTH_RESTRICTED: i32 = 1;
const CL_AUTH_DENIED: i32 = 2;
const CL_AUTH_AUTHORIZED_ALWAYS: i32 = 3;
const CL_AUTH_AUTHORIZED_WHEN_IN_USE: i32 = 4;

// CLError codes
const CL_ERROR_LOCATION_UNKNOWN: i64 = 0;
const CL_ERROR_DENIED: i64 = 1;

pub enum AppleLocationDelegateEvent {
    Update(LocationUpdateEvent),
    Error(LocationErrorEvent),
    AuthChanged(i32),
}

#[derive(Default)]
pub struct CxAppleLocation {
    pub access: Option<AppleLocationAccess>,
    pub rx: Option<Receiver<AppleLocationDelegateEvent>>,
}

pub struct AppleLocationAccess {
    manager: RcObjcId,
    delegate: RcObjcId,
    _callback: Box<Box<dyn Fn(AppleLocationDelegateEvent) + Send + 'static>>,
    tx: Sender<AppleLocationDelegateEvent>,
    /// The app called start and expects updates (survives auth round-trips).
    pub running_wanted: bool,
    /// A `Permission::Location` request waiting on the auth dialog.
    pub pending_permission: Option<i32>,
    denied_reported: bool,
}

impl Drop for AppleLocationAccess {
    fn drop(&mut self) {
        unsafe {
            let () = msg_send![self.manager.as_id(), setDelegate: nil];
            (*self.delegate.as_id()).set_ivar("callback", 0 as *mut c_void);
        }
    }
}

impl AppleLocationAccess {
    pub fn new(tx: Sender<AppleLocationDelegateEvent>) -> Self {
        unsafe {
            let event_tx = tx.clone();
            let callback: Box<dyn Fn(AppleLocationDelegateEvent) + Send + 'static> =
                Box::new(move |event| {
                    let _ = event_tx.send(event);
                    SignalToUI::set_ui_signal();
                });
            let double_box = Box::new(callback);
            let delegate = RcObjcId::from_owned(msg_send![
                get_apple_class_global().location_manager_delegate,
                new
            ]);
            (*delegate.as_id()).set_ivar("callback", &*double_box as *const _ as *const c_void);

            let manager: RcObjcId = RcObjcId::from_owned(msg_send![class!(CLLocationManager), new]);
            let () = msg_send![manager.as_id(), setDesiredAccuracy: kCLLocationAccuracyBest];
            let () = msg_send![manager.as_id(), setDistanceFilter: 3.0f64];
            // Triggers an initial locationManagerDidChangeAuthorization callback.
            let () = msg_send![manager.as_id(), setDelegate: delegate.as_id()];

            Self {
                manager,
                delegate,
                _callback: double_box,
                tx,
                running_wanted: false,
                pending_permission: None,
                denied_reported: false,
            }
        }
    }

    pub fn auth_status(&self) -> i32 {
        unsafe { msg_send![self.manager.as_id(), authorizationStatus] }
    }

    /// Start (or keep) streaming fixes. Prompts for permission when the
    /// status is not determined yet; reports denial over the event channel.
    pub fn start(&mut self) {
        self.running_wanted = true;
        self.denied_reported = false;
        unsafe {
            match self.auth_status() {
                CL_AUTH_RESTRICTED | CL_AUTH_DENIED => {
                    self.report_denied_once();
                }
                CL_AUTH_NOT_DETERMINED => {
                    let () = msg_send![self.manager.as_id(), requestWhenInUseAuthorization];
                    // Safe pre-authorization; fixes flow once granted.
                    let () = msg_send![self.manager.as_id(), startUpdatingLocation];
                }
                _ => {
                    let () = msg_send![self.manager.as_id(), startUpdatingLocation];
                }
            }
        }
    }

    pub fn stop(&mut self) {
        self.running_wanted = false;
        unsafe {
            let () = msg_send![self.manager.as_id(), stopUpdatingLocation];
        }
    }

    pub fn start_updating_raw(&self) {
        unsafe {
            let () = msg_send![self.manager.as_id(), startUpdatingLocation];
        }
    }

    pub fn request_authorization(&self) {
        unsafe {
            let () = msg_send![self.manager.as_id(), requestWhenInUseAuthorization];
        }
    }

    pub fn report_denied_once(&mut self) {
        if !self.denied_reported {
            self.denied_reported = true;
            let _ = self
                .tx
                .send(AppleLocationDelegateEvent::Error(LocationErrorEvent::PermissionDenied));
            SignalToUI::set_ui_signal();
        }
    }
}

pub fn auth_status_to_permission_status(status: i32) -> PermissionStatus {
    match status {
        CL_AUTH_AUTHORIZED_ALWAYS | CL_AUTH_AUTHORIZED_WHEN_IN_USE => PermissionStatus::Granted,
        CL_AUTH_RESTRICTED | CL_AUTH_DENIED => PermissionStatus::DeniedPermanent,
        _ => PermissionStatus::NotDetermined,
    }
}

impl Cx {
    fn location_access(&mut self) -> &mut AppleLocationAccess {
        let location = &mut self.os.media.location;
        if location.access.is_none() {
            let (tx, rx) = channel();
            location.rx = Some(rx);
            location.access = Some(AppleLocationAccess::new(tx));
        }
        location.access.as_mut().unwrap()
    }

    pub(crate) fn apple_start_location_updates(&mut self) {
        self.location_access().start();
    }

    pub(crate) fn apple_stop_location_updates(&mut self) {
        if let Some(access) = self.os.media.location.access.as_mut() {
            access.stop();
        }
    }

    /// Current CoreLocation authorization as a cross-platform status,
    /// without instantiating a manager.
    pub(crate) fn apple_location_permission_status() -> PermissionStatus {
        let status: i32 = unsafe { msg_send![class!(CLLocationManager), authorizationStatus] };
        auth_status_to_permission_status(status)
    }

    /// Fire the system location-permission dialog; the result arrives as
    /// `Event::PermissionResult` via the delegate's authorization callback.
    pub(crate) fn apple_request_location_permission(&mut self, request_id: i32) {
        let access = self.location_access();
        access.pending_permission = Some(request_id);
        access.request_authorization();
    }

    /// Drain delegate events into app-facing events. Called from
    /// `handle_media_signals` on the main loop.
    pub(crate) fn handle_location_signals(&mut self) {
        let mut events = Vec::new();
        if let Some(rx) = &self.os.media.location.rx {
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            match event {
                AppleLocationDelegateEvent::Update(e) => {
                    self.call_event_handler(&Event::LocationUpdate(e));
                }
                AppleLocationDelegateEvent::Error(e) => {
                    self.call_event_handler(&Event::LocationError(e));
                }
                AppleLocationDelegateEvent::AuthChanged(status) => {
                    let mut permission_result = None;
                    if let Some(access) = self.os.media.location.access.as_mut() {
                        if status != CL_AUTH_NOT_DETERMINED {
                            if let Some(request_id) = access.pending_permission.take() {
                                permission_result = Some(PermissionResult {
                                    permission: Permission::Location,
                                    request_id,
                                    status: auth_status_to_permission_status(status),
                                });
                            }
                        }
                        if access.running_wanted {
                            match status {
                                CL_AUTH_AUTHORIZED_ALWAYS | CL_AUTH_AUTHORIZED_WHEN_IN_USE => {
                                    access.start_updating_raw();
                                }
                                CL_AUTH_RESTRICTED | CL_AUTH_DENIED => {
                                    access.report_denied_once();
                                }
                                _ => {}
                            }
                        }
                    }
                    if let Some(result) = permission_result {
                        self.call_event_handler(&Event::PermissionResult(result));
                    }
                }
            }
        }
    }
}

pub fn define_cl_location_manager_delegate() -> *const Class {
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
            let event = LocationUpdateEvent {
                lon: coord.longitude,
                lat: coord.latitude,
                accuracy_m,
                altitude_m: if vertical_accuracy >= 0.0 { Some(altitude) } else { None },
                speed_mps: if speed >= 0.0 { Some(speed) } else { None },
                heading_deg: if course >= 0.0 { Some(course) } else { None },
                time,
            };
            (*(ptr as *const Box<dyn Fn(AppleLocationDelegateEvent)>))(
                AppleLocationDelegateEvent::Update(event),
            );
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
                // transient "keep waiting" — not an error
                CL_ERROR_LOCATION_UNKNOWN => return,
                CL_ERROR_DENIED => LocationErrorEvent::PermissionDenied,
                _ => LocationErrorEvent::Unavailable(format!("CLError code {code}")),
            };
            (*(ptr as *const Box<dyn Fn(AppleLocationDelegateEvent)>))(
                AppleLocationDelegateEvent::Error(event),
            );
        }
    }

    extern "C" fn did_change_authorization(this: &Object, _: Sel, manager: ObjcId) {
        unsafe {
            let ptr: *const c_void = *this.get_ivar("callback");
            if ptr == 0 as *const c_void {
                return;
            }
            let status: i32 = msg_send![manager, authorizationStatus];
            (*(ptr as *const Box<dyn Fn(AppleLocationDelegateEvent)>))(
                AppleLocationDelegateEvent::AuthChanged(status),
            );
        }
    }

    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MakepadCLLocationManagerDelegate", superclass).unwrap();
    unsafe {
        decl.add_method(
            sel!(locationManager: didUpdateLocations:),
            did_update_locations as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(locationManager: didFailWithError:),
            did_fail_with_error as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(locationManagerDidChangeAuthorization:),
            did_change_authorization as extern "C" fn(&Object, Sel, ObjcId),
        );
        if let Some(protocol) = Protocol::get("CLLocationManagerDelegate") {
            decl.add_protocol(protocol);
        }
    }
    decl.add_ivar::<*mut c_void>("callback");
    decl.register()
}
