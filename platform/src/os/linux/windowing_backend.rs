use super::super::shared_framebuf::PollTimers;
use super::gstreamer_sys::LibGStreamer;
use super::linux_media::CxLinuxMedia;
use super::linux_video_player::LinuxVideoPlayer;

use crate::{cx::Cx, makepad_live_id::LiveId, opengl_cx::OpenglCx, CxOsApi, OpenUrlInPlace};
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Instant};
// Import OpenglCx from x11 for the unified type

/// Scroll distance in logical pixels for one wheel detent, roughly three lines of
/// text, matching the common Windows and GTK defaults.
pub const PIXELS_PER_WHEEL_DETENT: f64 = 60.0;

fn env_var_is_nonempty(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

#[cfg(use_vulkan)]
use super::gpu_preference::{gpu_preference, GpuPreference};

fn is_stdin_loop_mode() -> bool {
    crate::app_main::should_run_stdin_loop_from_env()
}

fn forced_windowing_protocol_from_args() -> Option<WindowingProtocol> {
    for arg in std::env::args() {
        if let Some(value) = arg.strip_prefix("--linux-backend=") {
            match value {
                "x11" => return Some(WindowingProtocol::X11),
                "wayland" => return Some(WindowingProtocol::Wayland),
                _ => {}
            }
        }
    }
    None
}

// Protocol detection for windowing system
fn detect_windowing_protocol() -> WindowingProtocol {
    // stdin-loop mode renders into Studio's RunView via shared framebuffers.
    // Both X11 and Wayland backends support this path, so let normal
    // protocol detection proceed instead of forcing X11.

    if let Some(protocol) = forced_windowing_protocol_from_args() {
        return protocol;
    }

    // Check for Wayland first
    if env_var_is_nonempty("WAYLAND_DISPLAY") {
        return WindowingProtocol::Wayland;
    }

    // Check for X11
    if env_var_is_nonempty("DISPLAY") {
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
        // Hosted (`--stdin-loop`) rendering, chosen the same way as a window:
        // Vulkan when a device answers, OpenGL ES otherwise, so a machine
        // without a Vulkan driver still runs inside Studio or the wm. The
        // OpenGL side falls through to the windowed loop below, which creates
        // the EGL context before handing over to the same stdin loop.
        #[cfg(use_vulkan)]
        if is_stdin_loop_mode() && gpu_preference() != GpuPreference::OpenGl {
            match super::vulkan::CxVulkan::new_offscreen() {
                Ok(vulkan) => {
                    let mut cx = cx.borrow_mut();
                    cx.in_makepad_studio = true;
                    cx.os_type = crate::cx::OsType::LinuxWindow(crate::cx::LinuxWindowParams {
                        custom_window_chrome: false,
                    });
                    cx.os.vulkan = Some(vulkan);
                    cx.os.gpu_backend = Some(crate::cx::GpuBackend::Vulkan);
                    cx.stdin_event_loop();
                    drop(cx.os.vulkan.take());
                    return;
                }
                Err(error) if gpu_preference() == GpuPreference::Vulkan => {
                    panic!("Offscreen Vulkan initialization failed: {error}")
                }
                Err(error) => {
                    crate::log!("Vulkan unavailable ({error}); hosting with OpenGL ES");
                }
            }
        }
        let protocol = detect_windowing_protocol();
        // Vulkan windowing needs Wayland; an X11 session renders with OpenGL ES
        // (the X11 loop creates its own EGL context) unless Vulkan was insisted on.
        #[cfg(use_vulkan)]
        if protocol != WindowingProtocol::Wayland {
            if gpu_preference() == GpuPreference::Vulkan {
                panic!("Linux Vulkan windowing requires Wayland; launch inside a Wayland session or build with MAKEPAD=linux_direct+vulkan for exclusive display access");
            }
            crate::log!("{protocol:?} session: Vulkan windowing needs Wayland; rendering with OpenGL ES");
        }

        // Show environment variables
        match std::env::var("WAYLAND_DISPLAY") {
            Ok(wayland_display) if !wayland_display.is_empty() => {
                println!("WAYLAND_DISPLAY: {}", wayland_display);
            }
            Ok(_) => println!("WAYLAND_DISPLAY: <empty>"),
            Err(_) => println!("WAYLAND_DISPLAY: Not set"),
        }

        match std::env::var("DISPLAY") {
            Ok(x11_display) if !x11_display.is_empty() => {
                println!("DISPLAY: {}", x11_display);
            }
            Ok(_) => println!("DISPLAY: <empty>"),
            Err(_) => println!("DISPLAY: Not set"),
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
                if forced_windowing_protocol_from_args() == Some(WindowingProtocol::Wayland) {
                    println!("Reason: --linux-backend=wayland override");
                } else {
                    println!("Reason: WAYLAND_DISPLAY environment variable is set");
                }
            }
            WindowingProtocol::X11 => {
                println!("Selected: X11 backend");
                if is_stdin_loop_mode() {
                    println!("Reason: --stdin-loop mode uses X11 stdin backend");
                } else if forced_windowing_protocol_from_args() == Some(WindowingProtocol::X11) {
                    println!("Reason: --linux-backend=x11 override");
                } else if env_var_is_nonempty("DISPLAY") {
                    println!("Reason: DISPLAY environment variable is set");
                } else {
                    println!("Reason: Default fallback (no display variables set)");
                }
            }
        }

        // Launch appropriate backend
        match protocol {
            WindowingProtocol::Wayland => Self::wayland_event_loop(cx),
            WindowingProtocol::X11 => {
                cx.borrow_mut().os.gpu_backend = Some(crate::cx::GpuBackend::OpenGl);
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
        self.dispatch_network_runtime_events();
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.os.start_time = Some(Instant::now());
        if let Some(item) = std::option_env!("MAKEPAD_PACKAGE_DIR") {
            self.package_root = Some(item.to_string());
        }
        self.native_load_dependencies();
    }

    fn seconds_since_app_start(&self) -> f64 {
        Instant::now()
            .duration_since(self.os.start_time.unwrap())
            .as_secs_f64()
    }

    fn open_url(&mut self, _url: &str, _in_place: OpenUrlInPlace) {
        if self.script_data.std.host_io_only() { return; }
        crate::error!("open_url not implemented on this platform");
    }
}

// Unified CxOs that can handle both X11 and Wayland
#[derive(Default)]
pub struct CxOs {
    pub(crate) media: CxLinuxMedia,
    pub(crate) stdin_timers: PollTimers,
    pub(crate) start_time: Option<Instant>,
    pub opengl_cx: Option<OpenglCx>,
    #[cfg(use_vulkan)]
    pub(crate) vulkan: Option<super::vulkan::CxVulkan>,
    /// The GPU API the event loop settled on at startup (`None` until then):
    /// a Vulkan-capable build renders with OpenGL ES when no usable device
    /// answers. `Cx::gpu_backend` reports it from here.
    pub(crate) gpu_backend: Option<crate::cx::GpuBackend>,
    pub(crate) video_players: HashMap<LiveId, LinuxVideoPlayer>,
    pub(crate) gstreamer: Option<LibGStreamer>,
}

impl CxOs {
    pub fn init(&mut self) {
        self.start_time = Some(Instant::now());
    }

    /// True while this process renders with Vulkan. A Vulkan-capable build
    /// that fell back to OpenGL ES, and every OpenGL-only build, answer false.
    pub(crate) fn vulkan_active(&self) -> bool {
        #[cfg(use_vulkan)]
        {
            // The renderer is taken out of `CxOs` for the duration of a
            // present; the recorded choice keeps the answer stable then.
            self.vulkan.is_some() || matches!(self.gpu_backend, Some(crate::cx::GpuBackend::Vulkan))
        }
        #[cfg(not(use_vulkan))]
        {
            false
        }
    }

    pub(crate) fn gl(&self) -> &super::super::gl_sys::LibGl {
        if let Some(ref cx) = self.opengl_cx {
            &cx.libgl
        } else {
            panic!("No OpenGL context available");
        }
    }
}
