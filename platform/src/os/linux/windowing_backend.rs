use super::linux_media::CxLinuxMedia;
use super::super::cx_stdin::PollTimers;

use std::{time::Instant, rc::Rc, cell::RefCell};
use crate::{cx::Cx, opengl_cx::OpenglCx, x11::xlib_app::get_xlib_app_global, CxOsApi, OpenUrlInPlace};
// Import OpenglCx from x11 for the unified type

// Protocol detection for windowing system
fn detect_windowing_protocol() -> WindowingProtocol {
    // Check for Wayland first
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        return WindowingProtocol::Wayland;
    }

    // Check for X11
    if std::env::var("DISPLAY").is_ok() {
        return WindowingProtocol::X11;
    }

    // Default to X11 if neither is detected
    WindowingProtocol::X11
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowingProtocol {
    X11,
    Wayland,
}


impl Cx {
    pub fn event_loop(cx: Rc<RefCell<Cx>>) {
        let protocol = detect_windowing_protocol();

        // Show environment variables
        if let Ok(wayland_display) = std::env::var("WAYLAND_DISPLAY") {
            println!("WAYLAND_DISPLAY: {}", wayland_display);
        } else {
            println!("WAYLAND_DISPLAY: Not set");
        }

        if let Ok(x11_display) = std::env::var("DISPLAY") {
            println!("DISPLAY: {}", x11_display);
        } else {
            println!("DISPLAY: Not set");
        }

        // Show additional environment info
        if let Ok(session_type) = std::env::var("XDG_SESSION_TYPE") {
            println!("XDG_SESSION_TYPE: {}", session_type);
        }

        if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
            println!("XDG_CURRENT_DESKTOP: {}", desktop);
        }

        // Show the decision
        match protocol {
            WindowingProtocol::Wayland => {
                println!("Selected: Wayland backend");
                println!("Reason: WAYLAND_DISPLAY environment variable is set");
            }
            WindowingProtocol::X11 => {
                println!("Selected: X11 backend");
                if std::env::var("DISPLAY").is_ok() {
                    println!("Reason: DISPLAY environment variable is set");
                } else {
                    println!("Reason: Default fallback (no display variables set)");
                }
            }
        }

        // Launch appropriate backend
        match protocol {
            WindowingProtocol::Wayland => {
                Self::wayland_event_loop(cx)
            }
            WindowingProtocol::X11 => {
                Self::x11_event_loop(cx)
            }
        }
    }

    fn wayland_event_loop(cx: Rc<RefCell<Cx>>) {
        super::wayland::linux_wayland::wayland_event_loop(cx)
    }

    fn x11_event_loop(cx: Rc<RefCell<Cx>>) {
        super::x11::linux_x11::x11_event_loop(cx)
    }

    pub(crate) fn handle_networking_events(&mut self) {

    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.os.start_time = Some(Instant::now());
        if let Some(item) = std::option_env!("MAKEPAD_PACKAGE_DIR"){
            self.live_registry.borrow_mut().package_root = Some(item.to_string());
        }
        self.live_expand();
        if !Self::has_studio_web_socket() {
            self.start_disk_live_file_watcher(100);
        }
        self.live_scan_dependencies();
        self.native_load_dependencies();
    }

    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        std::thread::spawn(f);
    }

    fn seconds_since_app_start(&self)->f64{
        Instant::now().duration_since(self.os.start_time.unwrap()).as_secs_f64()
    }

    fn open_url(&mut self, _url:&str, _in_place:OpenUrlInPlace){
        crate::error!("open_url not implemented on this platform");
    }
}

// Unified CxOs that can handle both X11 and Wayland
#[derive(Default)]
pub struct CxOs {
    pub(crate) media: CxLinuxMedia,
    pub(crate) stdin_timers: PollTimers,
    pub(crate) start_time: Option<Instant>,
    pub(super) opengl_cx: Option<OpenglCx>,
}

impl CxOs {
    pub fn init(&mut self) {
        self.start_time = Some(Instant::now());
    }

    pub(crate) fn gl(&self) -> &super::super::gl_sys::LibGl {
        if let Some(ref cx) = self.opengl_cx {
            &cx.libgl
        } else {
            panic!("No OpenGL context available");
        }
    }
}
